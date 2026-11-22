use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use calamine::{Data, Reader, open_workbook_auto, open_workbook_auto_from_rs};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use medical_core::crypto::{hash_api_key, hash_patient_id};
use medical_core::models::{Gender, L2Profile, L3VisitEvent, PatientRecord};
use medical_core::privacy::scrub_pii;
use medical_core::validation::Validate;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::{PipelineError, PipelineResult};

/// Configuration for patient record ingestion & PII masking.
#[derive(Debug, Clone)]
pub struct PatientMaskingConfig {
    /// Hospital-scoped salt used for deterministic SHA-256 `patient_id` derivation.
    pub salt: Vec<u8>,
    /// Default consent flag when source records omit explicit consent.
    pub default_consent: bool,
}

impl Default for PatientMaskingConfig {
    fn default() -> Self {
        Self {
            salt: b"medical-server-hospital-salt-v1".to_vec(),
            default_consent: false,
        }
    }
}

/// Summary of auto-detected PII columns in a tabular patient dataset (used by Studio preview).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiColumnDetection {
    pub detected_pii_columns: Vec<String>,
    pub all_columns: Vec<String>,
}

/// Result of importing and masking patient records from CSV / XLSX / JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatientImportReport {
    pub records: Vec<PatientRecord>,
    pub detected_pii_columns: Vec<String>,
    pub failed_rows: Vec<String>,
    pub total_input_rows: usize,
}

/// Intermediate raw row before parallel privacy masking.
#[derive(Debug, Clone, Default)]
struct RawPatientRow {
    row_index: usize,
    name: Option<String>,
    id_card: Option<String>,
    phone: Option<String>,
    address: Option<String>,
    age_raw: Option<String>,
    gender_raw: Option<String>,
    is_pregnant: bool,
    consent: Option<bool>,
    chronic_diseases: Vec<String>,
    allergies: Vec<String>,
    surgical_history: Vec<String>,
    long_term_medications: Vec<String>,
    event_id: Option<String>,
    occurred_at: Option<String>,
    chief_complaint: Option<String>,
    extracted_symptoms: Vec<String>,
    recommended_department: Option<String>,
    urgency_level: Option<u8>,
    summary: Option<String>,
}

/// JSON export format for raw hospital patient files (e.g. `patients_raw.json`).
#[derive(Debug, Clone, Deserialize)]
struct RawPatientJsonItem {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    id_card: Option<String>,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    age: Option<serde_json::Value>,
    #[serde(default)]
    gender: Option<String>,
    #[serde(default)]
    is_pregnant: Option<bool>,
    #[serde(default)]
    consent_long_term_memory: Option<bool>,
    #[serde(default)]
    chronic_diseases: Vec<String>,
    #[serde(default)]
    allergies: Vec<String>,
    #[serde(default)]
    surgical_history: Vec<String>,
    #[serde(default)]
    long_term_medications: Vec<String>,
    #[serde(default)]
    visit_event: Option<RawVisitEventJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawVisitEventJson {
    #[serde(default)]
    event_id: Option<String>,
    #[serde(default)]
    occurred_at: Option<String>,
    #[serde(default)]
    chief_complaint: Option<String>,
    #[serde(default)]
    extracted_symptoms: Vec<String>,
    #[serde(default)]
    recommended_department: Option<String>,
    #[serde(default)]
    urgency_level: Option<u8>,
    #[serde(default)]
    summary: Option<String>,
}

/// Auto-detects columns that contain sensitive PII (name, ID card, phone, address).
pub fn detect_pii_columns(headers: &[String]) -> PiiColumnDetection {
    let mut detected = Vec::new();
    for h in headers {
        let lower = h.trim().to_ascii_lowercase();
        let is_pii = lower.contains("姓名")
            || lower.contains("name")
            || lower.contains("身份证")
            || lower.contains("id_card")
            || lower.contains("idcard")
            || lower.contains("手机")
            || lower.contains("电话")
            || lower.contains("phone")
            || lower.contains("mobile")
            || lower.contains("住址")
            || lower.contains("地址")
            || lower.contains("address")
            || lower.contains("住院号")
            || lower.contains("病案号")
            || lower.contains("门诊号")
            || lower.contains("医保")
            || lower.contains("床号")
            || lower.contains("邮箱")
            || lower.contains("email")
            || lower.contains("联系人")
            || lower.contains("微信")
            || lower.contains("出生日期");
        if is_pii {
            detected.push(h.clone());
        }
    }
    PiiColumnDetection {
        detected_pii_columns: detected,
        all_columns: headers.to_vec(),
    }
}

/// Imports and masks patient records from a `.csv`, `.xlsx`, `.xls`, or `.json` file.
pub fn import_patients_file(
    path: impl AsRef<Path>,
    config: &PatientMaskingConfig,
) -> PipelineResult<PatientImportReport> {
    let path = path.as_ref();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "csv" => import_patients_csv(path, config),
        "xlsx" | "xls" => import_patients_xlsx(path, config),
        "json" => import_patients_json(path, config),
        other => Err(PipelineError::UnsupportedFormat(format!(
            "unsupported patient file extension: .{other}"
        ))),
    }
}

/// Reads a CSV file via `csv` crate and masks all rows in parallel via `rayon`.
pub fn import_patients_csv(
    path: impl AsRef<Path>,
    config: &PatientMaskingConfig,
) -> PipelineResult<PatientImportReport> {
    let raw = std::fs::read_to_string(path.as_ref())?;
    import_patients_csv_str(&raw, config)
}

/// Parses CSV text directly and masks all rows in parallel via `rayon`.
pub fn import_patients_csv_str(
    csv_text: &str,
    config: &PatientMaskingConfig,
) -> PipelineResult<PatientImportReport> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(csv_text.as_bytes());

    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| PipelineError::Parse(format!("csv header error: {e}")))?
        .iter()
        .map(|h| h.trim().trim_start_matches('\u{feff}').to_string())
        .collect();
    let detection = detect_pii_columns(&headers);

    let mut raw_rows = Vec::new();
    let mut failed_rows = Vec::new();
    for (idx, rec) in reader.records().enumerate() {
        let row_num = idx + 2;
        match rec {
            Ok(record) => {
                let mut map = HashMap::new();
                for (col_idx, header) in headers.iter().enumerate() {
                    if let Some(val) = record.get(col_idx) {
                        map.insert(header.as_str(), val.trim().to_string());
                    }
                }
                raw_rows.push(raw_row_from_map(row_num, &map));
            }
            Err(e) => failed_rows.push(format!("row {row_num}: csv parse error: {e}")),
        }
    }

    mask_raw_rows_parallel(raw_rows, detection.detected_pii_columns, failed_rows, config)
}

/// Reads an Excel `.xlsx`/`.xls` file via `calamine` and masks all rows in parallel via `rayon`.
pub fn import_patients_xlsx(
    path: impl AsRef<Path>,
    config: &PatientMaskingConfig,
) -> PipelineResult<PatientImportReport> {
    let mut workbook = open_workbook_auto(path.as_ref())
        .map_err(|e| PipelineError::Parse(format!("calamine workbook open error: {e}")))?;
    import_patients_workbook(&mut workbook, config)
}

/// Parses Excel `.xlsx`/`.xls` bytes via `calamine` and masks all rows in parallel via `rayon`.
pub fn import_patients_xlsx_bytes(
    bytes: &[u8],
    config: &PatientMaskingConfig,
) -> PipelineResult<PatientImportReport> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|e| PipelineError::Parse(format!("calamine workbook bytes open error: {e}")))?;
    import_patients_workbook(&mut workbook, config)
}

fn import_patients_workbook<RS: std::io::Read + std::io::Seek>(
    workbook: &mut calamine::Sheets<RS>,
    config: &PatientMaskingConfig,
) -> PipelineResult<PatientImportReport> {
    let sheet_names = workbook.sheet_names().to_vec();
    let first_sheet = sheet_names
        .first()
        .ok_or_else(|| PipelineError::Parse("xlsx workbook has no worksheets".to_string()))?
        .clone();

    let range = workbook
        .worksheet_range(&first_sheet)
        .map_err(|e| PipelineError::Parse(format!("calamine worksheet error: {e}")))?;

    let mut rows_iter = range.rows();
    let header_row = rows_iter
        .next()
        .ok_or_else(|| PipelineError::Parse("xlsx worksheet is empty".to_string()))?;

    let headers: Vec<String> = header_row
        .iter()
        .map(|cell| cell_to_string(cell).trim().to_string())
        .collect();
    let detection = detect_pii_columns(&headers);

    let mut raw_rows = Vec::new();
    for (idx, row) in rows_iter.enumerate() {
        let row_num = idx + 2;
        let mut map = HashMap::new();
        let mut non_empty = false;
        for (col_idx, header) in headers.iter().enumerate() {
            if let Some(cell) = row.get(col_idx) {
                let s = cell_to_string(cell).trim().to_string();
                if !s.is_empty() {
                    non_empty = true;
                }
                map.insert(header.as_str(), s);
            }
        }
        if non_empty {
            raw_rows.push(raw_row_from_map(row_num, &map));
        }
    }

    mask_raw_rows_parallel(raw_rows, detection.detected_pii_columns, Vec::new(), config)
}

/// Extracts all sheets of an Excel `.xlsx`/`.xls` byte buffer into human-readable TSV text for AI normalization.
pub fn extract_xlsx_as_text(bytes: &[u8]) -> PipelineResult<String> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|e| PipelineError::Parse(format!("calamine workbook bytes open error: {e}")))?;
    let sheet_names = workbook.sheet_names().to_vec();
    let mut out = String::new();
    for name in sheet_names {
        let Ok(range) = workbook.worksheet_range(&name) else {
            continue;
        };
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("=== Sheet: {name} ===\n"));
        for row in range.rows() {
            let vals: Vec<String> = row
                .iter()
                .map(|cell| cell_to_string(cell).trim().replace('\t', " "))
                .collect();
            if vals.iter().any(|v| !v.is_empty()) {
                out.push_str(&vals.join("\t"));
                out.push('\n');
            }
        }
    }
    Ok(out)
}

/// Reads a raw JSON array of patient records via `sonic-rs` and masks them in parallel via `rayon`.
pub fn import_patients_json(
    path: impl AsRef<Path>,
    config: &PatientMaskingConfig,
) -> PipelineResult<PatientImportReport> {
    let raw = std::fs::read_to_string(path.as_ref())?;
    import_patients_json_str(&raw, config)
}

/// Parses a raw JSON array of patient records from a string and masks them in parallel via `rayon`.
pub fn import_patients_json_str(
    raw: &str,
    config: &PatientMaskingConfig,
) -> PipelineResult<PatientImportReport> {
    let cleaned = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let items: Vec<RawPatientJsonItem> = sonic_rs::from_str(cleaned)
        .map_err(|e| PipelineError::Parse(format!("sonic-rs patient json error: {e}")))?;

    let raw_rows: Vec<RawPatientRow> = items
        .into_iter()
        .enumerate()
        .map(|(idx, item)| {
            let age_raw = item.age.map(|v| match v {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            });
            let (event_id, occurred_at, chief_complaint, extracted_symptoms, recommended_department, urgency_level, summary) =
                match item.visit_event {
                    Some(ev) => (
                        ev.event_id,
                        ev.occurred_at,
                        ev.chief_complaint,
                        ev.extracted_symptoms,
                        ev.recommended_department,
                        ev.urgency_level,
                        ev.summary,
                    ),
                    None => (None, None, None, Vec::new(), None, None, None),
                };
            RawPatientRow {
                row_index: idx + 1,
                name: item.name,
                id_card: item.id_card,
                phone: item.phone,
                address: item.address,
                age_raw,
                gender_raw: item.gender,
                is_pregnant: item.is_pregnant.unwrap_or(false),
                consent: item.consent_long_term_memory,
                chronic_diseases: clean_list(item.chronic_diseases),
                allergies: clean_list(item.allergies),
                surgical_history: clean_list(item.surgical_history),
                long_term_medications: clean_list(item.long_term_medications),
                event_id,
                occurred_at,
                chief_complaint,
                extracted_symptoms: clean_list(extracted_symptoms),
                recommended_department,
                urgency_level,
                summary,
            }
        })
        .collect();

    let detected_pii = vec![
        "name".to_string(),
        "id_card".to_string(),
        "phone".to_string(),
        "address".to_string(),
    ];
    mask_raw_rows_parallel(raw_rows, detected_pii, Vec::new(), config)
}

fn mask_raw_rows_parallel(
    raw_rows: Vec<RawPatientRow>,
    detected_pii_columns: Vec<String>,
    mut failed_rows: Vec<String>,
    config: &PatientMaskingConfig,
) -> PipelineResult<PatientImportReport> {
    let total_input_rows = raw_rows.len() + failed_rows.len();

    let results: Vec<Result<PatientRecord, String>> = raw_rows
        .par_iter()
        .map(|row| mask_single_row(row, config))
        .collect();

    let mut merged: HashMap<String, PatientRecord> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for res in results {
        match res {
            Ok(rec) => {
                if let Some(existing) = merged.get_mut(&rec.patient_id) {
                    // Merge L2 lists safely (union) instead of overwriting allergies/chronic history
                    existing.consent_long_term_memory =
                        existing.consent_long_term_memory && rec.consent_long_term_memory;
                    existing.l2_profile.is_pregnant |= rec.l2_profile.is_pregnant;
                    for item in rec.l2_profile.chronic_diseases {
                        if !existing.l2_profile.chronic_diseases.contains(&item) {
                            existing.l2_profile.chronic_diseases.push(item);
                        }
                    }
                    for item in rec.l2_profile.allergies {
                        if !existing.l2_profile.allergies.contains(&item) {
                            existing.l2_profile.allergies.push(item);
                        }
                    }
                    for item in rec.l2_profile.surgical_history {
                        if !existing.l2_profile.surgical_history.contains(&item) {
                            existing.l2_profile.surgical_history.push(item);
                        }
                    }
                    for item in rec.l2_profile.long_term_medications {
                        if !existing.l2_profile.long_term_medications.contains(&item) {
                            existing.l2_profile.long_term_medications.push(item);
                        }
                    }
                    for ev in rec.l3_visit_events {
                        if !existing
                            .l3_visit_events
                            .iter()
                            .any(|e| e.event_id == ev.event_id)
                        {
                            existing.l3_visit_events.push(ev);
                        }
                    }
                } else {
                    order.push(rec.patient_id.clone());
                    merged.insert(rec.patient_id.clone(), rec);
                }
            }
            Err(err) => failed_rows.push(err),
        }
    }
    let records: Vec<PatientRecord> = order
        .into_iter()
        .filter_map(|id| merged.remove(&id))
        .collect();

    Ok(PatientImportReport {
        records,
        detected_pii_columns,
        failed_rows,
        total_input_rows,
    })
}

fn mask_single_row(row: &RawPatientRow, config: &PatientMaskingConfig) -> Result<PatientRecord, String> {
    // Derive deterministic HMAC-SHA256 patient_id from id_card (preferred) or name+phone.
    // Never derive an ID from a bare phone number with empty name to avoid family phone collisions.
    let id_source = row
        .id_card
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            let phone = row.phone.as_deref().map(str::trim).unwrap_or("");
            let name = row.name.as_deref().map(str::trim).unwrap_or("");
            if !name.is_empty() {
                Some(format!("{name}|{phone}"))
            } else {
                None
            }
        })
        .ok_or_else(|| {
            format!(
                "row {}: missing identifier (require id_card, or non-empty name with optional phone)",
                row.row_index
            )
        })?;

    let patient_id = hash_patient_id(&id_source, &config.salt);

    let age_group = bucket_age_group(row.age_raw.as_deref().unwrap_or(""))
        .ok_or_else(|| format!("row {}: missing or invalid age/age_group", row.row_index))?;

    let gender = parse_gender(row.gender_raw.as_deref().unwrap_or(""));

    let l2_profile = L2Profile {
        age_group,
        gender,
        is_pregnant: row.is_pregnant,
        chronic_diseases: row
            .chronic_diseases
            .iter()
            .map(|s| scrub_row_text(s, row))
            .filter(|s| !s.is_empty())
            .collect(),
        allergies: row
            .allergies
            .iter()
            .map(|s| scrub_row_text(s, row))
            .filter(|s| !s.is_empty())
            .collect(),
        surgical_history: row
            .surgical_history
            .iter()
            .map(|s| scrub_row_text(s, row))
            .filter(|s| !s.is_empty())
            .collect(),
        long_term_medications: row
            .long_term_medications
            .iter()
            .map(|s| scrub_row_text(s, row))
            .filter(|s| !s.is_empty())
            .collect(),
    };

    let mut l3_visit_events = Vec::new();
    if let Some(raw_cc) = row.chief_complaint.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let occurred_at = match row.occurred_at.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw_dt) => parse_clinical_datetime(raw_dt).ok_or_else(|| {
                format!(
                    "row {}: invalid occurred_at datetime format '{}'",
                    row.row_index, raw_dt
                )
            })?,
            None => Utc::now(),
        };

        let chief_complaint = scrub_row_text(raw_cc, row);
        let summary = row
            .summary
            .as_deref()
            .map(|s| scrub_row_text(s, row))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| chief_complaint.clone());
        let recommended_department = row
            .recommended_department
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("全科医学科")
            .to_string();
        let urgency_level = row.urgency_level.unwrap_or(2).clamp(1, 4);

        let event_id = row
            .event_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                let digest = hash_api_key(&format!(
                    "{}|{}|{}",
                    patient_id,
                    occurred_at.to_rfc3339(),
                    chief_complaint
                ));
                format!("evt_{}", &digest[..12])
            });

        l3_visit_events.push(L3VisitEvent {
            event_id,
            occurred_at,
            chief_complaint,
            extracted_symptoms: row
                .extracted_symptoms
                .iter()
                .map(|s| scrub_row_text(s, row))
                .filter(|s| !s.is_empty())
                .collect(),
            recommended_department,
            urgency_level,
            summary,
        });
    }

    let record = PatientRecord {
        patient_id,
        consent_long_term_memory: row.consent.unwrap_or(config.default_consent),
        l2_profile,
        l3_visit_events,
    };

    record
        .validate()
        .map_err(|e| format!("row {}: validation failed: {e}", row.row_index))?;

    Ok(record)
}

fn parse_clinical_datetime(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in [
        "%Y-%m-%d %H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M",
    ] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(raw, fmt) {
            return Some(DateTime::from_naive_utc_and_offset(ndt, Utc));
        }
    }
    for fmt in ["%Y-%m-%d", "%Y/%m/%d", "%Y.%m.%d"] {
        if let Ok(nd) = NaiveDate::parse_from_str(raw, fmt) {
            if let Some(ndt) = nd.and_hms_opt(0, 0, 0) {
                return Some(DateTime::from_naive_utc_and_offset(ndt, Utc));
            }
        }
    }
    None
}

/// Protected clinical vocabulary that MUST NEVER be destroyed by blind `name`/`address` string replacement.
const PROTECTED_CLINICAL_TERMS: &[&str] = &[
    "高热", "低热", "发热", "发烧", "胸痛", "腹痛", "头痛", "咳嗽", "呼吸困难", "心悸", "大汗",
    "医院", "急诊", "门诊", "病房", "内科", "外科", "儿科", "妇产科", "高血压", "糖尿病",
    "冠心病", "青霉素", "阿司匹林", "华法林", "二甲双胍", "头孢", "胰岛素",
];

/// Scrubs general PII via `medical_core::privacy::scrub_pii` and safely strips
/// the row's own name/id_card/phone/address from free-text clinical notes without corrupting
/// clinical terminology or single-character drug prefixes (e.g. "华法林" / "高热").
fn scrub_row_text(input: &str, row: &RawPatientRow) -> String {
    let mut text = scrub_pii(input).scrubbed_text;
    for (opt, placeholder, min_chars, check_clinical) in [
        (&row.id_card, "[ID_REDACTED]", 6usize, false),
        (&row.phone, "[PHONE_REDACTED]", 7usize, false),
        (&row.address, "[ADDRESS_REDACTED]", 4usize, true),
        (&row.name, "[NAME_REDACTED]", 2usize, true),
    ] {
        let Some(val) = opt.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        if val.chars().count() < min_chars {
            continue;
        }
        if check_clinical
            && PROTECTED_CLINICAL_TERMS
                .iter()
                .any(|term| *term == val || term.contains(val))
        {
            continue;
        }
        if text.contains(val) {
            text = text.replace(val, placeholder);
        }
    }
    text.trim().to_string()
}

fn raw_row_from_map(row_index: usize, map: &HashMap<&str, String>) -> RawPatientRow {
    let get_any = |keys: &[&str]| -> Option<String> {
        for k in keys {
            if let Some(v) = map.get(k) {
                let t = v.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        None
    };

    let split_list = |opt: Option<String>| -> Vec<String> {
        let Some(s) = opt else {
            return Vec::new();
        };
        clean_list(
            s.split([';', '；', '、', '|'])
                .map(str::trim)
                .map(ToString::to_string)
                .collect(),
        )
    };

    let is_pregnant = get_any(&["孕产状态", "is_pregnant", "pregnant"])
        .map(|s| matches!(s.to_ascii_lowercase().as_str(), "true" | "1" | "是" | "yes"))
        .unwrap_or(false);

    let consent = get_any(&["同意长期记忆", "consent_long_term_memory", "consent"])
        .map(|s| matches!(s.to_ascii_lowercase().as_str(), "true" | "1" | "是" | "yes"));

    let urgency_level = get_any(&["紧急等级", "urgency_level", "urgency"])
        .and_then(|s| s.parse::<u8>().ok());

    RawPatientRow {
        row_index,
        name: get_any(&["姓名", "患者姓名", "name"]),
        id_card: get_any(&["身份证号", "身份证", "id_card", "idcard"]),
        phone: get_any(&["手机号", "联系电话", "phone", "mobile"]),
        address: get_any(&["详细住址", "家庭住址", "住址", "address"]),
        age_raw: get_any(&["年龄", "年龄段", "age", "age_group"]),
        gender_raw: get_any(&["性别", "gender"]),
        is_pregnant,
        consent,
        chronic_diseases: split_list(get_any(&["慢病史", "chronic_diseases"])),
        allergies: split_list(get_any(&["过敏史", "allergies"])),
        surgical_history: split_list(get_any(&["手术史", "surgical_history"])),
        long_term_medications: split_list(get_any(&["长期用药", "long_term_medications"])),
        event_id: get_any(&["事件ID", "event_id"]),
        occurred_at: get_any(&["就诊时间", "occurred_at"]),
        chief_complaint: get_any(&["主诉", "chief_complaint"]),
        extracted_symptoms: split_list(get_any(&["提取症状", "extracted_symptoms"])),
        recommended_department: get_any(&["推荐科室", "recommended_department"]),
        urgency_level,
        summary: get_any(&["摘要", "summary"]),
    }
}

fn clean_list(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "无" && !s.eq_ignore_ascii_case("none"))
        .collect()
}

pub fn bucket_age_group(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Validate pre-bucketed forms strictly: "<lo>-<hi>" (0 <= lo <= hi <= 130) or "<n>+" (0 <= n <= 120)
    if let Some((lo_s, hi_s)) = trimmed.split_once('-') {
        if !lo_s.is_empty()
            && !hi_s.is_empty()
            && lo_s.chars().all(|c| c.is_ascii_digit())
            && hi_s.chars().all(|c| c.is_ascii_digit())
            && let (Ok(lo), Ok(hi)) = (lo_s.parse::<u32>(), hi_s.parse::<u32>())
            && lo <= hi
            && hi <= 130
        {
            return Some(format!("{lo}-{hi}"));
        }
        return None;
    }
    if let Some(num_s) = trimmed.strip_suffix('+') {
        if !num_s.is_empty()
            && num_s.chars().all(|c| c.is_ascii_digit())
            && let Ok(n) = num_s.parse::<u32>()
            && n <= 120
        {
            return Some(format!("{n}+"));
        }
        return None;
    }
    // Infant age in months/days (e.g. "6个月", "10月", "28天")
    if trimmed.contains("个月") || trimmed.ends_with('月') || trimmed.ends_with('天') {
        let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && digits.parse::<u32>().is_ok() {
            return Some("0-17".to_string());
        }
    }
    // Extract leading numeric age (supports "40", "40岁", "40岁男性", "1岁3个月")
    let num_part: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if num_part.is_empty() {
        return None;
    }
    let val = num_part.parse::<f64>().ok()?;
    if !val.is_finite() || val < 0.0 || val > 130.0 {
        return None;
    }
    let age_num = val.round() as u32;
    let bucket = match age_num {
        0..=17 => "0-17",
        18..=29 => "18-29",
        30..=39 => "30-39",
        40..=49 => "40-49",
        50..=59 => "50-59",
        60..=69 => "60-69",
        _ => "70+",
    };
    Some(bucket.to_string())
}

fn parse_gender(raw: &str) -> Gender {
    match raw.trim().to_ascii_lowercase().as_str() {
        "male" | "m" | "男" => Gender::Male,
        "female" | "f" | "女" => Gender::Female,
        "other" | "其他" => Gender::Other,
        _ => Gender::Unknown,
    }
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            if f.fract().abs() < 1e-9 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("{e:?}"),
    }
}

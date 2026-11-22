use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::clinical::{
    AbnormalFlag, ClinicalExamBundle, ClinicalMediaAsset, ClinicalMediaCategory, ImagingExamRecord,
    ImagingModality, LabBiochemItem, LabBiochemPanel, LabPanelCategory, TriggerCategory,
    TriggerSubstanceRecord, VitalSignsRecord,
};

/// Type of external hospital / regional health information system connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HospitalConnectorKind {
    /// 标准 HL7 FHIR R4 接口（国际/国内互联互通标准）
    #[default]
    Hl7FhirR4,
    /// 医院检验科 LIS 接口（血常规、生化、免疫、微生物报告）
    HospitalLis,
    /// 医院影像科 PACS / 云胶片接口（MRI、CT、X线、超声、心电图及云影像链接）
    HospitalPacs,
    /// 医院微信公众号 / 互联网医院小程序电子报告查询接口
    WechatHospitalMiniapp,
    /// 国家/省市电子健康卡 / 区域卫生信息平台接口
    NationalHealthCard,
    /// 院内集成平台 / ESB 自定义 HTTP Webhook 接口
    CustomWebhook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HospitalAuthMode {
    #[default]
    Bearer,
    ApiKey,
    OAuth2ClientCredentials,
    HmacSha256,
    None,
}

/// Configuration for an external hospital HIS/LIS/PACS/FHIR data connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HospitalConnectorConfig {
    pub connector_id: String,
    pub hospital_code: String,
    pub hospital_name: String,
    #[serde(default)]
    pub kind: HospitalConnectorKind,
    pub base_url: String,
    #[serde(default)]
    pub auth_mode: HospitalAuthMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Secret / API key / Bearer token (never returned in plaintext by public listing if masked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_secret: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_true")]
    pub supports_lab_reports: bool,
    #[serde(default = "default_true")]
    pub supports_imaging_reports: bool,
    #[serde(default = "default_true")]
    pub supports_cloud_film: bool,
    #[serde(default = "default_true")]
    pub supports_qr_pull: bool,
}

fn default_true() -> bool {
    true
}

fn default_timeout_ms() -> u64 {
    8000
}

/// Request to pull electronic medical reports from a hospital HIS/LIS/PACS or mobile QR code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HospitalReportPullRequest {
    pub patient_id: String,
    #[serde(default)]
    pub connector_id: Option<String>,
    #[serde(default)]
    pub hospital_code: Option<String>,
    /// 检查/检验报告单号（如 LIS202611220088）
    #[serde(default)]
    pub report_no: Option<String>,
    /// 患者手机扫码获取的医院报告二维码内容、电子报告链接或云胶片 URL
    #[serde(default)]
    pub qr_code_payload: Option<String>,
    /// 院内门诊号/住院号/就诊流水号（仅用于实时向医院查询，绝不落盘明文）
    #[serde(default)]
    pub visit_no: Option<String>,
    /// 期望拉取的报告类型：["lab", "imaging", "vitals", "pathology"]
    #[serde(default)]
    pub report_types: Vec<String>,
    #[serde(default)]
    pub date_from: Option<String>,
    #[serde(default)]
    pub date_to: Option<String>,
    /// 拉取成功后是否自动脱敏并写入患者 L2/L3 加密病历
    #[serde(default = "default_true")]
    pub auto_attach_to_patient: bool,
}

/// Standardized external hospital report (supports lab, imaging, cloud film URL & PDF link).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HospitalExternalReport {
    pub report_no: String,
    pub hospital_name: String,
    #[serde(default)]
    pub department: String,
    /// "lab" | "imaging" | "vitals" | "pathology" | "comprehensive"
    pub report_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_at: Option<DateTime<Utc>>,
    /// 现代医院手机可查的「电子云胶片」查看链接（DICOM Web / H5）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_film_url: Option<String>,
    /// 电子报告 PDF 下载/查看链接
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_pdf_url: Option<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub clinical_exams: ClinicalExamBundle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HospitalReportPullResponse {
    pub source_hospital: String,
    pub connector_id: String,
    /// "live_hospital_api" | "fhir_bundle_import" | "qr_sandbox_preview"
    pub pull_mode: String,
    pub imported_reports: Vec<HospitalExternalReport>,
    pub merged_clinical_exams: ClinicalExamBundle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached_event_id: Option<String>,
    pub pii_scrubbed: bool,
}

/// Webhook push payload sent by hospital LIS/PACS/ESB once a patient's report is finalized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HospitalWebhookPushPayload {
    pub hospital_code: String,
    pub hospital_name: String,
    /// 目标匿名患者 ID（若医院仅传脱敏前院内标识，需由院内网关先加盐哈希）
    pub patient_id: String,
    pub report_no: String,
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub cloud_film_url: Option<String>,
    #[serde(default)]
    pub report_pdf_url: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    /// 结构化医学检查包（与 `fhir_bundle` 二选一或同时提供）
    #[serde(default)]
    pub clinical_exams: ClinicalExamBundle,
    /// 可选：直接推送标准 HL7 FHIR R4 Bundle JSON
    #[serde(default)]
    pub fhir_bundle: Option<Value>,
}

/// Parsed hint from a modern hospital mobile report QR code or cloud-film URL.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedQrReportHint {
    pub hospital_code: Option<String>,
    pub report_no: Option<String>,
    pub report_type: Option<String>,
    pub cloud_film_url: Option<String>,
}

/// Parses hospital report QR code payloads (URL query params, JSON, or bare report ID).
pub fn parse_mobile_report_qr_payload(raw: &str) -> ParsedQrReportHint {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ParsedQrReportHint::default();
    }

    // 1. JSON payload encoded inside QR code
    if trimmed.starts_with('{')
        && let Ok(val) = serde_json::from_str::<Value>(trimmed)
    {
        return ParsedQrReportHint {
            hospital_code: val
                .get("hospital_code")
                .or_else(|| val.get("hosp"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            report_no: val
                .get("report_no")
                .or_else(|| val.get("reportId"))
                .or_else(|| val.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            report_type: val
                .get("report_type")
                .or_else(|| val.get("type"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            cloud_film_url: val
                .get("cloud_film_url")
                .or_else(|| val.get("filmUrl"))
                .or_else(|| val.get("url"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
        };
    }

    // 2. URL payload (e.g. https://cloudfilm.hospital.cn/report?hosp=BJ001&report_no=MRI2026001)
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let mut hint = ParsedQrReportHint {
            cloud_film_url: Some(trimmed.to_string()),
            ..Default::default()
        };
        if let Some((_, query)) = trimmed.split_once('?') {
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    let key = k.to_ascii_lowercase();
                    match key.as_str() {
                        "hosp" | "hospital" | "hospital_code" | "org" => {
                            hint.hospital_code = Some(v.to_string());
                        }
                        "report_no" | "reportid" | "accession" | "exam_no" | "id" => {
                            hint.report_no = Some(v.to_string());
                        }
                        "type" | "report_type" | "modality" => {
                            hint.report_type = Some(v.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
        return hint;
    }

    // 3. Bare report number
    ParsedQrReportHint {
        report_no: Some(trimmed.to_string()),
        ..Default::default()
    }
}

/// Converts a standard HL7 FHIR R4 `Bundle` (containing `Observation`, `DiagnosticReport`,
/// `AllergyIntolerance`, and `Media` / `DocumentReference` resources) into `ClinicalExamBundle`.
pub fn convert_fhir_bundle_to_clinical_exams(bundle: &Value) -> ClinicalExamBundle {
    let mut out = ClinicalExamBundle::default();
    let entries = bundle
        .get("entry")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut vitals = VitalSignsRecord {
        record_id: format!("vs_fhir_{}", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let mut lab_items: Vec<LabBiochemItem> = Vec::new();

    for entry in entries {
        let resource = entry.get("resource").unwrap_or(&entry);
        let rtype = resource
            .get("resourceType")
            .and_then(Value::as_str)
            .unwrap_or("");
        match rtype {
            "Observation" => {
                let code_obj = resource.get("code");
                let loinc = extract_fhir_code(code_obj);
                let display = extract_fhir_display(code_obj).unwrap_or_else(|| "检验项目".to_string());

                // Blood pressure panel (LOINC 85354-9) with components
                if let Some(components) = resource.get("component").and_then(Value::as_array) {
                    for comp in components {
                        let c_code = extract_fhir_code(comp.get("code")).unwrap_or_default();
                        let c_val = comp
                            .pointer("/valueQuantity/value")
                            .and_then(Value::as_f64)
                            .map(|v| v as f32);
                        match c_code.as_str() {
                            "8480-6" => vitals.systolic_bp_mmhg = c_val,
                            "8462-4" => vitals.diastolic_bp_mmhg = c_val,
                            _ => {}
                        }
                    }
                    continue;
                }

                let num_val = resource
                    .pointer("/valueQuantity/value")
                    .and_then(Value::as_f64);
                let unit = resource
                    .pointer("/valueQuantity/unit")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let text_val = resource
                    .get("valueString")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);

                match loinc.as_deref() {
                    Some("8480-6") => vitals.systolic_bp_mmhg = num_val.map(|v| v as f32),
                    Some("8462-4") => vitals.diastolic_bp_mmhg = num_val.map(|v| v as f32),
                    Some("8867-4") => vitals.heart_rate_bpm = num_val.map(|v| v as f32),
                    Some("9279-1") => vitals.respiratory_rate_rpm = num_val.map(|v| v as f32),
                    Some("8310-5") => vitals.temperature_c = num_val.map(|v| v as f32),
                    Some("2708-6") => vitals.spo2_percent = num_val.map(|v| v as f32),
                    Some("2339-0") => vitals.blood_glucose_mmol_l = num_val.map(|v| v as f32),
                    _ => {
                        let ref_range = resource
                            .pointer("/referenceRange/0/text")
                            .and_then(Value::as_str)
                            .map(ToString::to_string);
                        let interp_code = resource
                            .pointer("/interpretation/0/coding/0/code")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let abnormal_flag = match interp_code.to_ascii_uppercase().as_str() {
                            "H" => Some(AbnormalFlag::High),
                            "L" => Some(AbnormalFlag::Low),
                            "HH" => Some(AbnormalFlag::CriticalHigh),
                            "LL" => Some(AbnormalFlag::CriticalLow),
                            "N" => Some(AbnormalFlag::Normal),
                            "POS" => Some(AbnormalFlag::Positive),
                            "NEG" => Some(AbnormalFlag::Negative),
                            "A" => Some(AbnormalFlag::Abnormal),
                            _ => None,
                        };
                        lab_items.push(LabBiochemItem {
                            loinc_code: loinc,
                            item_code: None,
                            item_name: display,
                            value_numeric: num_val,
                            value_text: text_val,
                            unit,
                            reference_range: ref_range,
                            abnormal_flag,
                        });
                    }
                }
            }
            "DiagnosticReport" => {
                let exam_name = extract_fhir_display(resource.get("code"))
                    .unwrap_or_else(|| "医院影像/检查报告".to_string());
                let conclusion = resource
                    .get("conclusion")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let lower = exam_name.to_ascii_lowercase();
                let modality = if lower.contains("mri") || exam_name.contains("核磁") || exam_name.contains("磁共振") {
                    ImagingModality::Mri
                } else if lower.contains("ct") {
                    ImagingModality::Ct
                } else if exam_name.contains("超声") || lower.contains("ultrasound") {
                    ImagingModality::Ultrasound
                } else if exam_name.contains("心电") || lower.contains("ecg") {
                    ImagingModality::Ecg
                } else if exam_name.contains("X线") || lower.contains("xray") {
                    ImagingModality::Xray
                } else {
                    ImagingModality::Other
                };
                if !conclusion.is_empty() {
                    out.imaging_exams.push(ImagingExamRecord {
                        exam_id: resource
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                            .unwrap_or_else(|| format!("fhir_dr_{}", uuid::Uuid::new_v4().simple())),
                        modality,
                        exam_name,
                        body_part: String::new(),
                        examined_at: None,
                        findings: conclusion.clone(),
                        impression: conclusion,
                        contrast_used: None,
                        images: Vec::new(),
                    });
                }
            }
            "AllergyIntolerance" => {
                let name = extract_fhir_display(resource.get("code"))
                    .unwrap_or_else(|| "医院记录过敏原".to_string());
                out.trigger_substances.push(TriggerSubstanceRecord {
                    trigger_id: resource
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                        .unwrap_or_else(|| format!("fhir_alg_{}", uuid::Uuid::new_v4().simple())),
                    category: TriggerCategory::Other,
                    substance_name: name,
                    exposure_route: None,
                    reaction_symptoms: Vec::new(),
                    severity: None,
                    exposure_time: None,
                    notes: None,
                    images: Vec::new(),
                });
            }
            "Media" | "DocumentReference" => {
                let content = resource
                    .get("content")
                    .or_else(|| resource.pointer("/content/0/attachment"));
                let url = content
                    .and_then(|c| c.get("url"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let data = content
                    .and_then(|c| c.get("data"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let title = content
                    .and_then(|c| c.get("title"))
                    .and_then(Value::as_str)
                    .unwrap_or("医院影像/附件")
                    .to_string();
                let mime = content
                    .and_then(|c| c.get("contentType"))
                    .and_then(Value::as_str)
                    .unwrap_or("image/jpeg")
                    .to_string();
                if url.is_some() || data.is_some() {
                    out.media_assets.push(ClinicalMediaAsset {
                        asset_id: resource
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                            .unwrap_or_else(|| {
                                format!("fhir_media_{}", uuid::Uuid::new_v4().simple())
                            }),
                        category: ClinicalMediaCategory::Other,
                        title,
                        mime_type: mime,
                        uri_or_path: url,
                        data_base64: data,
                        sha256: None,
                        body_site: None,
                        captured_at: None,
                        caption: None,
                        tags: vec!["FHIR".to_string()],
                    });
                }
            }
            _ => {}
        }
    }

    if vitals.has_any_measurement() {
        out.vital_signs.push(vitals);
    }
    if !lab_items.is_empty() {
        out.lab_panels.push(LabBiochemPanel {
            panel_id: format!("lab_fhir_{}", uuid::Uuid::new_v4().simple()),
            category: LabPanelCategory::Other,
            panel_name: "医院 FHIR 检验生化报告".to_string(),
            specimen: Some("静脉血".to_string()),
            collected_at: None,
            reported_at: Some(Utc::now()),
            items: lab_items,
            summary: None,
            attachments: Vec::new(),
        });
    }

    out
}

fn extract_fhir_code(code_obj: Option<&Value>) -> Option<String> {
    code_obj?
        .pointer("/coding/0/code")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn extract_fhir_display(code_obj: Option<&Value>) -> Option<String> {
    let obj = code_obj?;
    obj.get("text")
        .and_then(Value::as_str)
        .or_else(|| obj.pointer("/coding/0/display").and_then(Value::as_str))
        .map(ToString::to_string)
}

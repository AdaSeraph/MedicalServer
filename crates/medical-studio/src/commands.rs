use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use chrono::Utc;
use medical_core::crypto::hash_patient_id;
use medical_core::ecs::{
    ExtractedSymptoms, RagEvidence, RedFlagAlert, TriageExecutionResult, TriageRequest,
    TriageSlots,
};
use medical_core::llm::{ProbeResult, ProviderConfig, RouterConfig};
use medical_core::models::{
    DataCollectionPackage, EpidemicContext, KnowledgeDocument, L2Profile, L3VisitEvent,
    MedicalDictionaries, PatientRecord, RedFlagLevel, RedFlagRule, SymptomSynonym,
    TargetPopulation, TriageStatus,
};
use medical_core::privacy::scrub_pii;
use medical_core::storage::{
    ApiKeyRecord, GatewayMetricsSummary, IngestionCheckpoint, IssueApiKeyRequest, IssuedApiKey,
};
use medical_core::validation::Validate;
use medical_pipeline::{
    BookMetadataDefaults, CustomChunkRule, JobProgress, PatientImportReport, PatientMaskingConfig,
    export_error_samples_jsonl, import_patients_csv_str, import_patients_file,
    import_patients_json_str, import_patients_xlsx_bytes, parse_collection_bundle_file,
    parse_collection_bundle_str, parse_docx_book, parse_markdown_book, parse_medical_book_file,
    parse_pdf_book, parse_plain_text_book, stream_knowledge_jsonl,
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::ai_normalize::{
    AiImportPrompts, AiKnowledgeInspectionResponse, AiKnowledgeNormalizeResponse,
    AiPatientInspectionResponse, AiPatientNormalizeResponse, BatchImportItem, CategoryCount,
    ai_inspect_knowledge_batch_with_state, ai_inspect_patient_batch_with_state,
    ai_normalize_knowledge_batch_with_state, ai_normalize_patient_batch_with_state,
    default_ai_import_prompts,
};
use crate::state::StudioState;

// ===========================================================================
// Module 1: 医学知识库与书籍导入工坊
// ===========================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct KnowledgePreviewRequest {
    pub file_path: String,
    #[serde(default)]
    pub defaults: BookMetadataDefaults,
    #[serde(default)]
    pub custom_rule: Option<CustomChunkRule>,
    #[serde(default = "default_preview_limit")]
    pub limit: usize,
}

fn default_preview_limit() -> usize {
    20
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgePreviewResponse {
    pub total_chunks: usize,
    pub preview_chunks: Vec<KnowledgeDocument>,
    pub errors: Vec<String>,
    pub metadata_fields: Vec<&'static str>,
    pub regex_applied: bool,
}

pub async fn preview_knowledge_chunks_impl(
    req: KnowledgePreviewRequest,
) -> Result<KnowledgePreviewResponse, String> {
    let path = Path::new(&req.file_path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let (docs, errors) = match ext.as_str() {
        "json" => {
            let pkg = parse_collection_bundle_file(path).map_err(|e| e.to_string())?;
            (pkg.knowledge_documents, Vec::new())
        }
        "jsonl" => {
            let rep = stream_knowledge_jsonl(path, 0, Some(req.limit.max(1)))
                .map_err(|e| e.to_string())?;
            let errs = rep
                .errors
                .into_iter()
                .map(|e| format!("line {}: {}", e.line_number, e.message))
                .collect();
            (rep.records, errs)
        }
        _ => {
            let chunks =
                parse_medical_book_file(path, &req.defaults, req.custom_rule.as_ref(), None)
                    .map_err(|e| e.to_string())?;
            (chunks, Vec::new())
        }
    };

    let total_chunks = docs.len();
    let preview_chunks = docs.into_iter().take(req.limit.max(1)).collect();
    Ok(KnowledgePreviewResponse {
        total_chunks,
        preview_chunks,
        errors,
        metadata_fields: vec![
            "target_population",
            "department",
            "authority_level",
            "seasonal_tags",
        ],
        regex_applied: req.custom_rule.is_some(),
    })
}

#[tauri::command]
pub async fn preview_knowledge_chunks(
    req: KnowledgePreviewRequest,
) -> Result<KnowledgePreviewResponse, String> {
    preview_knowledge_chunks_impl(req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct StartKnowledgeImportRequest {
    pub job_id: String,
    pub file_path: String,
    #[serde(default = "default_ns")]
    pub tenant_namespace: String,
    #[serde(default)]
    pub defaults: BookMetadataDefaults,
    #[serde(default)]
    pub custom_rule: Option<CustomChunkRule>,
}

fn default_ns() -> String {
    "default".to_string()
}

pub async fn start_knowledge_import_impl(
    state: &StudioState,
    req: StartKnowledgeImportRequest,
) -> Result<JobProgress, String> {
    let path = Path::new(&req.file_path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if ext == "jsonl" {
        return state
            .indexer
            .ingest_knowledge_jsonl_resumable(&req.job_id, path, &req.tenant_namespace, None)
            .await
            .map_err(|e| e.to_string());
    }

    let started = Instant::now();
    let docs = if ext == "json" {
        parse_collection_bundle_file(path)
            .map_err(|e| e.to_string())?
            .knowledge_documents
    } else {
        parse_medical_book_file(path, &req.defaults, req.custom_rule.as_ref(), None)
            .map_err(|e| e.to_string())?
    };

    let mut cp = state
        .checkpoints
        .load_or_init(&req.job_id, &req.file_path, "knowledge_book", docs.len() as u64)
        .await
        .map_err(|e| e.to_string())?;
    let written = state
        .indexer
        .upsert_documents(&docs, &req.tenant_namespace)
        .await
        .map_err(|e| e.to_string())?;
    cp.processed_items = written;
    cp.last_doc_id = docs.last().map(|d| d.doc_id.clone());
    state
        .checkpoints
        .save_progress(&mut cp, medical_core::storage::CheckpointStatus::Completed)
        .await
        .map_err(|e| e.to_string())?;
    Ok(medical_pipeline::CheckpointManager::build_progress(&cp, started))
}

#[tauri::command]
pub async fn start_knowledge_import(
    state: State<'_, StudioState>,
    req: StartKnowledgeImportRequest,
) -> Result<JobProgress, String> {
    start_knowledge_import_impl(&state, req).await
}

pub async fn resume_knowledge_import_impl(
    state: &StudioState,
    job_id: String,
    tenant_namespace: Option<String>,
) -> Result<JobProgress, String> {
    let cp = state
        .checkpoints
        .get(&job_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("checkpoint not found for job_id={job_id}"))?;
    let ns = tenant_namespace.unwrap_or_else(|| "default".to_string());
    state
        .indexer
        .ingest_knowledge_jsonl_resumable(&job_id, &cp.source_path, &ns, None)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn resume_knowledge_import(
    state: State<'_, StudioState>,
    job_id: String,
    tenant_namespace: Option<String>,
) -> Result<JobProgress, String> {
    resume_knowledge_import_impl(&state, job_id, tenant_namespace).await
}

pub async fn get_import_job_status_impl(
    state: &StudioState,
    job_id: String,
) -> Result<Option<IngestionCheckpoint>, String> {
    let cp = state
        .checkpoints
        .get(&job_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(ref item) = cp {
        let _completed = item.status == medical_core::storage::CheckpointStatus::Completed;
        let _failed = item.failed_items;
    }
    Ok(cp)
}

#[tauri::command]
pub async fn get_import_job_status(
    state: State<'_, StudioState>,
    job_id: String,
) -> Result<Option<IngestionCheckpoint>, String> {
    get_import_job_status_impl(&state, job_id).await
}

pub async fn export_import_errors_impl(
    state: &StudioState,
    job_id: String,
    output_path: String,
) -> Result<usize, String> {
    let cp = state
        .checkpoints
        .get(&job_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("checkpoint not found for job_id={job_id}"))?;
    export_error_samples_jsonl(&cp.error_samples, &output_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn export_import_errors(
    state: State<'_, StudioState>,
    job_id: String,
    output_path: String,
) -> Result<usize, String> {
    export_import_errors_impl(&state, job_id, output_path).await
}

#[tauri::command]
pub fn get_ai_import_prompts() -> AiImportPrompts {
    default_ai_import_prompts()
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiNormalizeKnowledgeRequest {
    pub items: Vec<BatchImportItem>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub custom_prompt: Option<String>,
    #[serde(default)]
    pub defaults: BookMetadataDefaults,
}

pub async fn ai_normalize_knowledge_batch_impl(
    state: &StudioState,
    req: AiNormalizeKnowledgeRequest,
) -> Result<AiKnowledgeNormalizeResponse, String> {
    ai_normalize_knowledge_batch_with_state(
        state,
        &req.items,
        req.provider_id.as_deref(),
        req.custom_prompt.as_deref(),
        &req.defaults,
    )
    .await
}

#[tauri::command]
pub async fn ai_normalize_knowledge_batch(
    state: State<'_, StudioState>,
    req: AiNormalizeKnowledgeRequest,
) -> Result<AiKnowledgeNormalizeResponse, String> {
    ai_normalize_knowledge_batch_impl(&state, req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct BatchKnowledgePreviewRequest {
    pub items: Vec<BatchImportItem>,
    #[serde(default)]
    pub defaults: BookMetadataDefaults,
    #[serde(default)]
    pub custom_rule: Option<CustomChunkRule>,
}

pub async fn batch_preview_knowledge_items_impl(
    req: BatchKnowledgePreviewRequest,
) -> Result<KnowledgePreviewResponse, String> {
    if req.items.is_empty() {
        return Err("请至少添加一个文件或文本进行批量解析".to_string());
    }
    let mut all_docs = Vec::new();
    let mut errors = Vec::new();

    for item in &req.items {
        let ext = Path::new(&item.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();

        let parsed: Result<Vec<KnowledgeDocument>, String> = if let Some(fp) = &item.file_path {
            let p = Path::new(fp);
            let fext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or(ext.clone());
            match fext.as_str() {
                "json" => parse_collection_bundle_file(p)
                    .map(|pkg| pkg.knowledge_documents)
                    .map_err(|e| e.to_string()),
                "jsonl" => stream_knowledge_jsonl(p, 0, None)
                    .map(|rep| rep.records)
                    .map_err(|e| e.to_string()),
                _ => parse_medical_book_file(p, &req.defaults, req.custom_rule.as_ref(), None)
                    .map_err(|e| e.to_string()),
            }
        } else if let Some(b64) = &item.content_base64 {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| format!("base64 decode failed: {e}"));
            match bytes {
                Ok(b) => match ext.as_str() {
                    "docx" => parse_docx_book(&b, &req.defaults, req.custom_rule.as_ref())
                        .map_err(|e| e.to_string()),
                    "pdf" => parse_pdf_book(&b, &req.defaults, req.custom_rule.as_ref(), None)
                        .map_err(|e| e.to_string()),
                    _ => {
                        let txt = String::from_utf8_lossy(&b);
                        parse_knowledge_text_by_ext(&ext, &txt, &req.defaults, req.custom_rule.as_ref())
                    }
                },
                Err(e) => Err(e),
            }
        } else if let Some(txt) = &item.content_text {
            parse_knowledge_text_by_ext(&ext, txt, &req.defaults, req.custom_rule.as_ref())
        } else {
            Err("导入项内容为空".to_string())
        };

        match parsed {
            Ok(docs) => all_docs.extend(docs),
            Err(e) => errors.push(format!("[{}] {e}", item.name)),
        }
    }

    let total_chunks = all_docs.len();
    Ok(KnowledgePreviewResponse {
        total_chunks,
        preview_chunks: all_docs,
        errors,
        metadata_fields: vec![
            "target_population",
            "department",
            "authority_level",
            "seasonal_tags",
        ],
        regex_applied: req.custom_rule.is_some(),
    })
}

fn parse_knowledge_text_by_ext(
    ext: &str,
    text: &str,
    defaults: &BookMetadataDefaults,
    custom_rule: Option<&CustomChunkRule>,
) -> Result<Vec<KnowledgeDocument>, String> {
    match ext {
        "json" => parse_collection_bundle_str(text)
            .map(|pkg| pkg.knowledge_documents)
            .map_err(|e| e.to_string()),
        "jsonl" => {
            let mut docs = Vec::new();
            for (idx, line) in text.lines().enumerate() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                let doc: KnowledgeDocument = serde_json::from_str(t)
                    .map_err(|e| format!("line {}: {e}", idx + 1))?;
                doc.validate()
                    .map_err(|e| format!("line {}: {e}", idx + 1))?;
                docs.push(doc);
            }
            Ok(docs)
        }
        "md" | "markdown" => parse_markdown_book(text, defaults).map_err(|e| e.to_string()),
        _ => parse_plain_text_book(text, defaults, custom_rule).map_err(|e| e.to_string()),
    }
}

#[tauri::command]
pub async fn batch_preview_knowledge_items(
    req: BatchKnowledgePreviewRequest,
) -> Result<KnowledgePreviewResponse, String> {
    batch_preview_knowledge_items_impl(req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitKnowledgeDocumentsRequest {
    pub job_id: String,
    #[serde(default = "default_ns")]
    pub tenant_namespace: String,
    pub documents: Vec<KnowledgeDocument>,
}

pub async fn commit_knowledge_documents_impl(
    state: &StudioState,
    req: CommitKnowledgeDocumentsRequest,
) -> Result<JobProgress, String> {
    if req.documents.is_empty() {
        return Err("没有可写入数据库的规范化知识条目".to_string());
    }
    let started = Instant::now();
    let mut valid_docs = Vec::with_capacity(req.documents.len());
    let mut failed_count = 0u64;
    for doc in req.documents {
        if doc.validate().is_ok() {
            valid_docs.push(doc);
        } else {
            failed_count += 1;
        }
    }
    if valid_docs.is_empty() {
        return Err("所有知识条目校验均未通过，无法写入数据库".to_string());
    }

    let job_id = if req.job_id.trim().is_empty() {
        format!("job_ai_kb_{}", Utc::now().timestamp())
    } else {
        req.job_id.trim().to_string()
    };

    let mut cp = state
        .checkpoints
        .load_or_init(
            &job_id,
            "batch_ai_import",
            "knowledge_batch",
            (valid_docs.len() as u64) + failed_count,
        )
        .await
        .map_err(|e| e.to_string())?;
    let written = state
        .indexer
        .upsert_documents(&valid_docs, &req.tenant_namespace)
        .await
        .map_err(|e| e.to_string())?;
    cp.processed_items = written;
    cp.failed_items = failed_count;
    cp.last_doc_id = valid_docs.last().map(|d| d.doc_id.clone());
    state
        .checkpoints
        .save_progress(&mut cp, medical_core::storage::CheckpointStatus::Completed)
        .await
        .map_err(|e| e.to_string())?;
    Ok(medical_pipeline::CheckpointManager::build_progress(&cp, started))
}

#[tauri::command]
pub async fn commit_knowledge_documents(
    state: State<'_, StudioState>,
    req: CommitKnowledgeDocumentsRequest,
) -> Result<JobProgress, String> {
    commit_knowledge_documents_impl(&state, req).await
}

// ===========================================================================
// Module 2: 病人历史数据批量导入与脱敏中心
// ===========================================================================

pub async fn preview_patient_import_impl(
    file_path: String,
    salt: Option<String>,
) -> Result<PatientImportReport, String> {
    let mut cfg = PatientMaskingConfig::default();
    if let Some(s) = salt.filter(|v| !v.trim().is_empty()) {
        cfg.salt = s.into_bytes();
    }
    import_patients_file(&file_path, &cfg).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_patient_import(
    file_path: String,
    salt: Option<String>,
) -> Result<PatientImportReport, String> {
    preview_patient_import_impl(file_path, salt).await
}

#[derive(Debug, Clone, Serialize)]
pub struct PatientImportCommitResult {
    pub imported_patients: usize,
    pub imported_l3_events: usize,
    pub failed_rows: Vec<String>,
    pub detected_pii_columns: Vec<String>,
}

pub async fn apply_masking_and_import_patients_impl(
    state: &StudioState,
    file_path: String,
    salt: Option<String>,
) -> Result<PatientImportCommitResult, String> {
    let report = preview_patient_import_impl(file_path, salt).await?;
    let mut imported_patients = 0usize;
    let mut imported_l3_events = 0usize;

    for rec in &report.records {
        state
            .stores
            .patient_memory
            .upsert_l2_profile(
                &rec.patient_id,
                rec.consent_long_term_memory,
                &rec.l2_profile,
            )
            .await
            .map_err(|e| e.to_string())?;
        imported_patients += 1;

        for ev in &rec.l3_visit_events {
            state
                .stores
                .patient_memory
                .insert_l3_event(&rec.patient_id, ev)
                .await
                .map_err(|e| e.to_string())?;
            if let Ok(vec) = state
                .embedder
                .embed_one(&format!("{} {}", ev.chief_complaint, ev.summary))
                .await
            {
                let _ = state
                    .stores
                    .patient_l3_vectors
                    .upsert_event_vector(&rec.patient_id, ev, vec)
                    .await;
            }
            imported_l3_events += 1;
        }
    }

    Ok(PatientImportCommitResult {
        imported_patients,
        imported_l3_events,
        failed_rows: report.failed_rows,
        detected_pii_columns: report.detected_pii_columns,
    })
}

#[tauri::command]
pub async fn apply_masking_and_import_patients(
    state: State<'_, StudioState>,
    file_path: String,
    salt: Option<String>,
) -> Result<PatientImportCommitResult, String> {
    apply_masking_and_import_patients_impl(&state, file_path, salt).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiNormalizePatientRequest {
    pub items: Vec<BatchImportItem>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub custom_prompt: Option<String>,
    #[serde(default)]
    pub salt: Option<String>,
    #[serde(default = "default_true")]
    pub pre_scrub_pii: bool,
}

fn default_true() -> bool {
    true
}

pub async fn ai_normalize_patient_batch_impl(
    state: &StudioState,
    req: AiNormalizePatientRequest,
) -> Result<AiPatientNormalizeResponse, String> {
    let salt_bytes = req
        .salt
        .filter(|v| !v.trim().is_empty())
        .map(String::into_bytes)
        .unwrap_or_else(|| PatientMaskingConfig::default().salt);

    ai_normalize_patient_batch_with_state(
        state,
        &req.items,
        req.provider_id.as_deref(),
        req.custom_prompt.as_deref(),
        &salt_bytes,
        req.pre_scrub_pii,
    )
    .await
}

#[tauri::command]
pub async fn ai_normalize_patient_batch(
    state: State<'_, StudioState>,
    req: AiNormalizePatientRequest,
) -> Result<AiPatientNormalizeResponse, String> {
    ai_normalize_patient_batch_impl(&state, req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct BatchPatientPreviewRequest {
    pub items: Vec<BatchImportItem>,
    #[serde(default)]
    pub salt: Option<String>,
}

pub async fn batch_preview_patient_items_impl(
    req: BatchPatientPreviewRequest,
) -> Result<PatientImportReport, String> {
    if req.items.is_empty() {
        return Err("请至少添加一个病例文件进行批量预检".to_string());
    }
    let mut cfg = PatientMaskingConfig::default();
    if let Some(s) = req.salt.filter(|v| !v.trim().is_empty()) {
        cfg.salt = s.into_bytes();
    }

    let mut records = Vec::new();
    let mut detected_pii_columns = Vec::new();
    let mut failed_rows = Vec::new();
    let mut total_input_rows = 0usize;

    for item in &req.items {
        let ext = Path::new(&item.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();

        let report_res = if let Some(fp) = &item.file_path {
            import_patients_file(fp, &cfg).map_err(|e| e.to_string())
        } else if let Some(b64) = &item.content_base64 {
            base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| format!("base64 decode failed: {e}"))
                .and_then(|bytes| match ext.as_str() {
                    "xlsx" | "xls" => import_patients_xlsx_bytes(&bytes, &cfg)
                        .map_err(|e| e.to_string()),
                    "json" => {
                        let txt = String::from_utf8_lossy(&bytes);
                        import_patients_json_str(&txt, &cfg).map_err(|e| e.to_string())
                    }
                    "csv" => {
                        let txt = String::from_utf8_lossy(&bytes);
                        import_patients_csv_str(&txt, &cfg).map_err(|e| e.to_string())
                    }
                    other => Err(format!(
                        "规则预检不支持 .{other} 格式，请使用「AI 批量规范化」处理非结构化病例"
                    )),
                })
        } else if let Some(txt) = &item.content_text {
            match ext.as_str() {
                "json" => import_patients_json_str(txt, &cfg).map_err(|e| e.to_string()),
                "csv" => import_patients_csv_str(txt, &cfg).map_err(|e| e.to_string()),
                other => Err(format!(
                    "规则预检不支持 .{other} 格式，请使用「AI 批量规范化」处理非结构化病例"
                )),
            }
        } else {
            Err("导入项内容为空".to_string())
        };

        match report_res {
            Ok(rep) => {
                total_input_rows += rep.total_input_rows;
                records.extend(rep.records);
                for col in rep.detected_pii_columns {
                    if !detected_pii_columns.contains(&col) {
                        detected_pii_columns.push(col);
                    }
                }
                for f in rep.failed_rows {
                    failed_rows.push(format!("[{}] {f}", item.name));
                }
            }
            Err(e) => failed_rows.push(format!("[{}] {e}", item.name)),
        }
    }

    Ok(PatientImportReport {
        records,
        detected_pii_columns,
        failed_rows,
        total_input_rows,
    })
}

#[tauri::command]
pub async fn batch_preview_patient_items(
    req: BatchPatientPreviewRequest,
) -> Result<PatientImportReport, String> {
    batch_preview_patient_items_impl(req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitPatientRecordsRequest {
    pub records: Vec<PatientRecord>,
    #[serde(default)]
    pub detected_pii_columns: Vec<String>,
}

pub async fn commit_patient_records_impl(
    state: &StudioState,
    req: CommitPatientRecordsRequest,
) -> Result<PatientImportCommitResult, String> {
    if req.records.is_empty() {
        return Err("没有可写入数据库的规范化病例记录".to_string());
    }

    let mut imported_patients = 0usize;
    let mut imported_l3_events = 0usize;
    let mut failed_rows = Vec::new();

    for (idx, rec) in req.records.iter().enumerate() {
        if let Err(e) = rec.validate() {
            failed_rows.push(format!("record {}: 隐私/结构校验失败: {e}", idx + 1));
            continue;
        }
        state
            .stores
            .patient_memory
            .upsert_l2_profile(
                &rec.patient_id,
                rec.consent_long_term_memory,
                &rec.l2_profile,
            )
            .await
            .map_err(|e| e.to_string())?;
        imported_patients += 1;

        for ev in &rec.l3_visit_events {
            state
                .stores
                .patient_memory
                .insert_l3_event(&rec.patient_id, ev)
                .await
                .map_err(|e| e.to_string())?;
            if let Ok(vec) = state
                .embedder
                .embed_one(&format!("{} {}", ev.chief_complaint, ev.summary))
                .await
            {
                let _ = state
                    .stores
                    .patient_l3_vectors
                    .upsert_event_vector(&rec.patient_id, ev, vec)
                    .await;
            }
            imported_l3_events += 1;
        }
    }

    Ok(PatientImportCommitResult {
        imported_patients,
        imported_l3_events,
        failed_rows,
        detected_pii_columns: req.detected_pii_columns,
    })
}

#[tauri::command]
pub async fn commit_patient_records(
    state: State<'_, StudioState>,
    req: CommitPatientRecordsRequest,
) -> Result<PatientImportCommitResult, String> {
    commit_patient_records_impl(&state, req).await
}

// ===========================================================================
// Module 3: 季节与流行病上下文控制台
// ===========================================================================

pub async fn list_epidemic_contexts_impl(
    state: &StudioState,
) -> Result<Vec<EpidemicContext>, String> {
    state
        .stores
        .epidemics
        .list_all()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_epidemic_contexts(
    state: State<'_, StudioState>,
) -> Result<Vec<EpidemicContext>, String> {
    list_epidemic_contexts_impl(&state).await
}

pub async fn upsert_epidemic_context_impl(
    state: &StudioState,
    ctx: EpidemicContext,
) -> Result<EpidemicContext, String> {
    ctx.validate().map_err(|e| e.to_string())?;
    state
        .stores
        .epidemics
        .upsert(&ctx)
        .await
        .map_err(|e| e.to_string())?;
    state.sync_ecs_resources().await?;
    Ok(ctx)
}

#[tauri::command]
pub async fn upsert_epidemic_context(
    state: State<'_, StudioState>,
    ctx: EpidemicContext,
) -> Result<EpidemicContext, String> {
    upsert_epidemic_context_impl(&state, ctx).await
}

pub async fn delete_epidemic_context_impl(
    state: &StudioState,
    context_id: String,
) -> Result<bool, String> {
    let deleted = state
        .stores
        .epidemics
        .delete(&context_id)
        .await
        .map_err(|e| e.to_string())?;
    state.sync_ecs_resources().await?;
    Ok(deleted)
}

#[tauri::command]
pub async fn delete_epidemic_context(
    state: State<'_, StudioState>,
    context_id: String,
) -> Result<bool, String> {
    delete_epidemic_context_impl(&state, context_id).await
}

#[derive(Debug, Clone, Serialize)]
pub struct HotReloadNotifyResult {
    pub local_reloaded: bool,
    pub remote_notified: bool,
    pub remote_status: Option<u16>,
    pub message: String,
}

pub async fn notify_server_hot_reload_impl(
    state: &StudioState,
    server_url: Option<String>,
    admin_token: Option<String>,
) -> Result<HotReloadNotifyResult, String> {
    state.sync_ecs_resources().await?;
    let Some(base) = server_url.filter(|s| !s.trim().is_empty()) else {
        return Ok(HotReloadNotifyResult {
            local_reloaded: true,
            remote_notified: false,
            remote_status: None,
            message: "本地 ECS 资源已热更新（未配置远程 medical-server 地址）".to_string(),
        });
    };

    let base_trimmed = base.trim_end_matches('/');
    // Push local epidemic contexts to remote medical-server BEFORE triggering reload
    // so remote production nodes actually receive Studio changes (no fake hot-reload).
    let local_epidemics = state
        .stores
        .epidemics
        .list_all()
        .await
        .unwrap_or_default();
    let mut synced_epidemics = 0usize;
    let epi_url = format!("{base_trimmed}/v1/admin/epidemics");
    for ctx in &local_epidemics {
        let mut epi_req = state.http_client.post(&epi_url).json(ctx);
        if let Some(ref tok) = admin_token
            && !tok.trim().is_empty()
        {
            epi_req = epi_req.bearer_auth(tok.trim());
        }
        if let Ok(resp) = epi_req.send().await
            && resp.status().is_success()
        {
            synced_epidemics += 1;
        }
    }

    let url = format!("{base_trimmed}/v1/admin/reload");
    let mut req = state.http_client.post(&url).json(&serde_json::json!({
        "reload_epidemics": true,
        "reload_dictionaries": true,
        "reload_router": true
    }));
    if let Some(tok) = admin_token.filter(|s| !s.trim().is_empty()) {
        req = req.bearer_auth(tok);
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            Ok(HotReloadNotifyResult {
                local_reloaded: true,
                remote_notified: resp.status().is_success(),
                remote_status: Some(status),
                message: format!(
                    "已同步 {synced_epidemics}/{} 条流行病规则并通知远程服务 {url}，HTTP {status}",
                    local_epidemics.len()
                ),
            })
        }
        Err(e) => Ok(HotReloadNotifyResult {
            local_reloaded: true,
            remote_notified: false,
            remote_status: None,
            message: format!("本地已热更新，远程同步/通知失败: {e}"),
        }),
    }
}

#[tauri::command]
pub async fn notify_server_hot_reload(
    state: State<'_, StudioState>,
    server_url: Option<String>,
    admin_token: Option<String>,
) -> Result<HotReloadNotifyResult, String> {
    let _reload_path = "/v1/admin/reload";
    notify_server_hot_reload_impl(&state, server_url, admin_token).await
}

// ===========================================================================
// Module 4: 关键词与急危重症词典管理器
// ===========================================================================

pub async fn get_medical_dictionaries_impl(
    state: &StudioState,
) -> Result<MedicalDictionaries, String> {
    state
        .stores
        .dictionaries
        .load_dictionaries()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_medical_dictionaries(
    state: State<'_, StudioState>,
) -> Result<MedicalDictionaries, String> {
    get_medical_dictionaries_impl(&state).await
}

pub async fn save_symptom_synonym_impl(
    state: &StudioState,
    synonym: SymptomSynonym,
) -> Result<SymptomSynonym, String> {
    let _ = (&synonym.standard_term, &synonym.colloquial_terms);
    synonym.validate().map_err(|e| e.to_string())?;
    state
        .stores
        .dictionaries
        .save_symptom_synonym(&synonym)
        .await
        .map_err(|e| e.to_string())?;
    state.sync_ecs_resources().await?;
    Ok(synonym)
}

#[tauri::command]
pub async fn save_symptom_synonym(
    state: State<'_, StudioState>,
    synonym: SymptomSynonym,
) -> Result<SymptomSynonym, String> {
    save_symptom_synonym_impl(&state, synonym).await
}

pub async fn delete_symptom_synonym_impl(
    state: &StudioState,
    standard_term: String,
) -> Result<bool, String> {
    let deleted = state
        .stores
        .dictionaries
        .delete_symptom_synonym(&standard_term)
        .await
        .map_err(|e| e.to_string())?;
    state.sync_ecs_resources().await?;
    Ok(deleted)
}

#[tauri::command]
pub async fn delete_symptom_synonym(
    state: State<'_, StudioState>,
    standard_term: String,
) -> Result<bool, String> {
    delete_symptom_synonym_impl(&state, standard_term).await
}

pub async fn save_red_flag_rule_impl(
    state: &StudioState,
    rule: RedFlagRule,
) -> Result<RedFlagRule, String> {
    let _ = (
        &rule.rule_id,
        &rule.level,
        &rule.trigger_all_of,
        &rule.trigger_any_of,
        &rule.alert_message,
        RedFlagLevel::Emergency120,
    );
    rule.validate().map_err(|e| e.to_string())?;
    state
        .stores
        .dictionaries
        .save_red_flag_rule(&rule)
        .await
        .map_err(|e| e.to_string())?;
    state.sync_ecs_resources().await?;
    Ok(rule)
}

#[tauri::command]
pub async fn save_red_flag_rule(
    state: State<'_, StudioState>,
    rule: RedFlagRule,
) -> Result<RedFlagRule, String> {
    save_red_flag_rule_impl(&state, rule).await
}

pub async fn delete_red_flag_rule_impl(
    state: &StudioState,
    rule_id: String,
) -> Result<bool, String> {
    let deleted = state
        .stores
        .dictionaries
        .delete_red_flag_rule(&rule_id)
        .await
        .map_err(|e| e.to_string())?;
    state.sync_ecs_resources().await?;
    Ok(deleted)
}

#[tauri::command]
pub async fn delete_red_flag_rule(
    state: State<'_, StudioState>,
    rule_id: String,
) -> Result<bool, String> {
    delete_red_flag_rule_impl(&state, rule_id).await
}

#[derive(Debug, Clone, Serialize)]
pub struct DictionarySandboxResult {
    pub utterance: String,
    pub standardized_terms: Vec<String>,
    pub red_flag_level: RedFlagLevel,
    pub matched_rule_ids: Vec<String>,
    pub alert_message: Option<String>,
}

pub async fn test_dictionary_sandbox_impl(
    state: &StudioState,
    utterance: String,
) -> Result<DictionarySandboxResult, String> {
    let exec = state
        .triage_engine
        .run_triage(TriageRequest::simple(
            "studio_dict_sandbox",
            "pat_sha256_sandbox_000000000000000000000000000000000000000000000000",
            utterance.clone(),
        ))
        .await
        .map_err(|e| e.to_string())?;
    let _ = state
        .stores
        .session_cache
        .delete_session("studio_dict_sandbox")
        .await;

    Ok(DictionarySandboxResult {
        utterance,
        standardized_terms: exec.trace.extracted_symptoms.normalized_terms,
        red_flag_level: exec.trace.red_flag_alert.level,
        matched_rule_ids: exec
            .trace
            .red_flag_alert
            .matched_rule_id
            .into_iter()
            .collect(),
        alert_message: exec.trace.red_flag_alert.alert_message,
    })
}

#[tauri::command]
pub async fn test_dictionary_sandbox(
    state: State<'_, StudioState>,
    utterance: String,
) -> Result<DictionarySandboxResult, String> {
    test_dictionary_sandbox_impl(&state, utterance).await
}

// ===========================================================================
// Module 5: ECS 分诊白盒调试台（Sandbox）
// ===========================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct WhiteboxTriageRequest {
    pub session_id: String,
    pub patient_id: String,
    pub utterance: String,
    #[serde(default)]
    pub region_code: Option<String>,
    #[serde(default)]
    pub target_population: Vec<TargetPopulation>,
}

pub async fn run_ecs_whitebox_triage_impl(
    state: &StudioState,
    req: WhiteboxTriageRequest,
) -> Result<TriageExecutionResult, String> {
    let prior_session = state
        .stores
        .session_cache
        .get_session(&req.session_id)
        .await
        .ok()
        .flatten();

    let triage_req = TriageRequest {
        session_id: req.session_id,
        patient_id: req.patient_id,
        tenant_id: "default".to_string(),
        region_code: req.region_code,
        utterance: req.utterance,
        target_population: if req.target_population.is_empty() {
            vec![TargetPopulation::Adult]
        } else {
            req.target_population
        },
        consent_long_term_memory: Some(true),
        prior_session,
        stream_tx: None,
    };

    let exec = state
        .triage_engine
        .run_triage(triage_req)
        .await
        .map_err(|e| e.to_string())?;
    let _ecs_components: (
        &ExtractedSymptoms,
        &RedFlagAlert,
        &RagEvidence,
        &TriageSlots,
    ) = (
        &exec.trace.extracted_symptoms,
        &exec.trace.red_flag_alert,
        &exec.trace.rag_evidence,
        &exec.trace.triage_slots,
    );
    Ok(exec)
}

#[tauri::command]
pub async fn run_ecs_whitebox_triage(
    state: State<'_, StudioState>,
    req: WhiteboxTriageRequest,
) -> Result<TriageExecutionResult, String> {
    run_ecs_whitebox_triage_impl(&state, req).await
}

pub async fn reset_ecs_debug_session_impl(
    state: &StudioState,
    session_id: String,
) -> Result<bool, String> {
    state
        .stores
        .session_cache
        .delete_session(&session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reset_ecs_debug_session(
    state: State<'_, StudioState>,
    session_id: String,
) -> Result<bool, String> {
    reset_ecs_debug_session_impl(&state, session_id).await
}

pub async fn list_debug_patients_impl(state: &StudioState) -> Result<Vec<String>, String> {
    state
        .stores
        .patient_memory
        .list_patient_ids(50, 0)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_debug_patients(state: State<'_, StudioState>) -> Result<Vec<String>, String> {
    list_debug_patients_impl(&state).await
}

// ===========================================================================
// Module 6: AI 大模型与开放 API 网关配置中心
// ===========================================================================

fn mask_api_key_for_ui(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = t.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let prefix: String = chars[..3].iter().collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}****{suffix}")
}

pub async fn list_llm_providers_impl(state: &StudioState) -> Result<Vec<ProviderConfig>, String> {
    let mut list = state
        .stores
        .llm_providers
        .list_providers()
        .await
        .map_err(|e| e.to_string())?;
    for p in &mut list {
        p.api_key = mask_api_key_for_ui(&p.api_key);
    }
    Ok(list)
}

#[tauri::command]
pub async fn list_llm_providers(
    state: State<'_, StudioState>,
) -> Result<Vec<ProviderConfig>, String> {
    list_llm_providers_impl(&state).await
}

pub async fn upsert_llm_provider_impl(
    state: &StudioState,
    mut config: ProviderConfig,
) -> Result<ProviderConfig, String> {
    config.id = config.id.trim().to_string();
    config.name = config.name.trim().to_string();
    config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
    config.api_key = config.api_key.trim().to_string();
    config.model = config.model.trim().to_string();

    // Preserve existing secret if frontend submitted the masked placeholder back unchanged
    if config.api_key.contains("****")
        && let Ok(existing_list) = state.stores.llm_providers.list_providers().await
        && let Some(existing) = existing_list.into_iter().find(|p| p.id == config.id)
    {
        config.api_key = existing.api_key;
    }

    if config.id.is_empty() {
        return Err("Provider ID 不能为空".to_string());
    }
    if config.base_url.is_empty() {
        return Err("API Base URL 不能为空".to_string());
    }
    if !config.base_url.starts_with("http://") && !config.base_url.starts_with("https://") {
        return Err("API Base URL 必须以 http:// 或 https:// 开头".to_string());
    }
    if config.model.is_empty() {
        return Err("模型名称 (model) 不能为空".to_string());
    }
    if config.name.is_empty() {
        config.name = config.id.clone();
    }
    if config.timeout_ms == 0 {
        config.timeout_ms = 8000;
    }

    state
        .stores
        .llm_providers
        .upsert_provider(&config)
        .await
        .map_err(|e| e.to_string())?;
    let _ = state.sync_ecs_resources().await;
    Ok(config)
}

#[tauri::command]
pub async fn upsert_llm_provider(
    state: State<'_, StudioState>,
    config: ProviderConfig,
) -> Result<ProviderConfig, String> {
    upsert_llm_provider_impl(&state, config).await
}

pub async fn delete_llm_provider_impl(
    state: &StudioState,
    provider_id: String,
) -> Result<bool, String> {
    let deleted = state
        .stores
        .llm_providers
        .delete_provider(&provider_id)
        .await
        .map_err(|e| e.to_string())?;
    let _ = state.sync_ecs_resources().await;
    Ok(deleted)
}

#[tauri::command]
pub async fn delete_llm_provider(
    state: State<'_, StudioState>,
    provider_id: String,
) -> Result<bool, String> {
    delete_llm_provider_impl(&state, provider_id).await
}

fn derive_probe_urls(base_url: &str) -> (String, String) {
    let trimmed = base_url.trim().trim_end_matches('/');
    if let Some(prefix) = trimmed.strip_suffix("/chat/completions") {
        (format!("{prefix}/models"), trimmed.to_string())
    } else {
        (
            format!("{trimmed}/models"),
            format!("{trimmed}/chat/completions"),
        )
    }
}

async fn format_http_error(resp: reqwest::Response) -> String {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let compact: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let snippet: String = compact.chars().take(180).collect();
    if snippet.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {snippet}")
    }
}

pub async fn test_llm_provider_connectivity_impl(
    state: &StudioState,
    provider_id: String,
) -> Result<ProbeResult, String> {
    let started = Instant::now();
    let cfg = state
        .stores
        .llm_providers
        .get_provider(&provider_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("provider not found: {provider_id}"))?;

    let (models_url, chat_url) = derive_probe_urls(&cfg.base_url);
    let timeout = Duration::from_millis(cfg.timeout_ms.clamp(2_000, 30_000));
    let api_key = cfg.api_key.trim();

    let mut models_req = state.http_client.get(&models_url).timeout(timeout);
    if !api_key.is_empty() {
        models_req = models_req.bearer_auth(api_key);
    }

    match models_req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let ms = started.elapsed().as_millis() as u64;
            Ok(ProbeResult {
                provider_id,
                ok: true,
                ttft_ms: Some(ms),
                total_ms: ms,
                error: None,
            })
        }
        Ok(resp) if matches!(resp.status().as_u16(), 404 | 405) => {
            // Some OpenAI-compatible gateways do not expose GET /models; fallback to minimal chat completion probe.
            let mut chat_req = state
                .http_client
                .post(&chat_url)
                .timeout(timeout)
                .json(&serde_json::json!({
                    "model": cfg.model,
                    "messages": [{ "role": "user", "content": "ping" }],
                    "max_tokens": 1,
                    "stream": false
                }));
            if !api_key.is_empty() {
                chat_req = chat_req.bearer_auth(api_key);
            }
            match chat_req.send().await {
                Ok(chat_resp) => {
                    let ms = started.elapsed().as_millis() as u64;
                    if chat_resp.status().is_success() {
                        Ok(ProbeResult {
                            provider_id,
                            ok: true,
                            ttft_ms: Some(ms),
                            total_ms: ms,
                            error: None,
                        })
                    } else {
                        let err = format_http_error(chat_resp).await;
                        Ok(ProbeResult {
                            provider_id,
                            ok: false,
                            ttft_ms: Some(ms),
                            total_ms: ms,
                            error: Some(err),
                        })
                    }
                }
                Err(e) => Ok(ProbeResult {
                    provider_id,
                    ok: false,
                    ttft_ms: None,
                    total_ms: started.elapsed().as_millis() as u64,
                    error: Some(e.to_string()),
                }),
            }
        }
        Ok(resp) => {
            let ms = started.elapsed().as_millis() as u64;
            let err = format_http_error(resp).await;
            Ok(ProbeResult {
                provider_id,
                ok: false,
                ttft_ms: Some(ms),
                total_ms: ms,
                error: Some(err),
            })
        }
        Err(e) => Ok(ProbeResult {
            provider_id,
            ok: false,
            ttft_ms: None,
            total_ms: started.elapsed().as_millis() as u64,
            error: Some(e.to_string()),
        }),
    }
}

#[tauri::command]
pub async fn test_llm_provider_connectivity(
    state: State<'_, StudioState>,
    provider_id: String,
) -> Result<ProbeResult, String> {
    test_llm_provider_connectivity_impl(&state, provider_id).await
}

pub async fn get_router_config_impl(state: &StudioState) -> Result<RouterConfig, String> {
    state
        .stores
        .llm_providers
        .get_router_config()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_router_config(state: State<'_, StudioState>) -> Result<RouterConfig, String> {
    get_router_config_impl(&state).await
}

pub async fn save_router_config_impl(
    state: &StudioState,
    config: RouterConfig,
) -> Result<RouterConfig, String> {
    state
        .stores
        .llm_providers
        .save_router_config(&config)
        .await
        .map_err(|e| e.to_string())?;
    let _ = state.sync_ecs_resources().await;
    Ok(config)
}

#[tauri::command]
pub async fn save_router_config(
    state: State<'_, StudioState>,
    config: RouterConfig,
) -> Result<RouterConfig, String> {
    save_router_config_impl(&state, config).await
}

pub async fn list_api_keys_impl(state: &StudioState) -> Result<Vec<ApiKeyRecord>, String> {
    state
        .stores
        .api_keys
        .list_keys()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_api_keys(state: State<'_, StudioState>) -> Result<Vec<ApiKeyRecord>, String> {
    list_api_keys_impl(&state).await
}

pub async fn issue_api_key_impl(
    state: &StudioState,
    req: IssueApiKeyRequest,
) -> Result<IssuedApiKey, String> {
    state
        .stores
        .api_keys
        .issue_key(req)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn issue_api_key(
    state: State<'_, StudioState>,
    req: IssueApiKeyRequest,
) -> Result<IssuedApiKey, String> {
    issue_api_key_impl(&state, req).await
}

pub async fn revoke_api_key_impl(state: &StudioState, key_id: String) -> Result<bool, String> {
    state
        .stores
        .api_keys
        .revoke_key(&key_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn revoke_api_key(
    state: State<'_, StudioState>,
    key_id: String,
) -> Result<bool, String> {
    revoke_api_key_impl(&state, key_id).await
}

pub async fn get_gateway_metrics_impl(
    state: &StudioState,
    window_hours: Option<u32>,
) -> Result<GatewayMetricsSummary, String> {
    state
        .stores
        .audit_logs
        .get_metrics_summary(window_hours.unwrap_or(24))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_gateway_metrics(
    state: State<'_, StudioState>,
    window_hours: Option<u32>,
) -> Result<GatewayMetricsSummary, String> {
    get_gateway_metrics_impl(&state, window_hours).await
}

// ===========================================================================
// Module 7: 已有知识库分类、AI 巡查分类、数据导出与交流共享、ECS 运行状态
// ===========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDepartmentCategory {
    pub department: String,
    pub doc_count: usize,
    pub diseases: Vec<String>,
    pub section_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeCategoriesResponse {
    pub total_documents: usize,
    pub departments: Vec<KnowledgeDepartmentCategory>,
    pub section_types: Vec<CategoryCount>,
    pub source_types: Vec<CategoryCount>,
    pub seasonal_tags: Vec<CategoryCount>,
    pub populations: Vec<CategoryCount>,
}

pub async fn get_knowledge_categories_impl(
    state: &StudioState,
    tenant_namespace: Option<String>,
) -> Result<KnowledgeCategoriesResponse, String> {
    let docs = state
        .tantivy
        .list_all_documents(tenant_namespace.as_deref())
        .map_err(|e| e.to_string())?;

    let mut dept_map: HashMap<String, (usize, HashSet<String>, HashSet<String>)> = HashMap::new();
    let mut section_map: HashMap<String, usize> = HashMap::new();
    let mut source_map: HashMap<String, usize> = HashMap::new();
    let mut tag_map: HashMap<String, usize> = HashMap::new();
    let mut pop_map: HashMap<String, usize> = HashMap::new();

    for d in &docs {
        let entry = dept_map
            .entry(d.department.clone())
            .or_insert_with(|| (0, HashSet::new(), HashSet::new()));
        entry.0 += 1;
        entry.1.insert(d.disease_name.clone());
        entry.2.insert(d.section_type.as_str().to_string());

        *section_map
            .entry(d.section_type.as_str().to_string())
            .or_insert(0) += 1;
        let src_str = serde_json::to_value(d.source_type)
            .ok()
            .and_then(|v| v.as_str().map(ToString::to_string))
            .unwrap_or_else(|| "textbook".to_string());
        *source_map.entry(src_str).or_insert(0) += 1;
        for tag in &d.seasonal_tags {
            if !tag.trim().is_empty() {
                *tag_map.entry(tag.trim().to_string()).or_insert(0) += 1;
            }
        }
        for pop in &d.target_population {
            *pop_map.entry(pop.as_str().to_string()).or_insert(0) += 1;
        }
    }

    let mut departments: Vec<KnowledgeDepartmentCategory> = dept_map
        .into_iter()
        .map(|(department, (doc_count, diseases_set, sections_set))| {
            let mut diseases: Vec<String> = diseases_set.into_iter().collect();
            diseases.sort();
            let mut section_types: Vec<String> = sections_set.into_iter().collect();
            section_types.sort();
            KnowledgeDepartmentCategory {
                department,
                doc_count,
                diseases,
                section_types,
            }
        })
        .collect();
    departments.sort_by(|a, b| {
        b.doc_count
            .cmp(&a.doc_count)
            .then_with(|| a.department.cmp(&b.department))
    });

    let to_sorted_counts = |map: HashMap<String, usize>| -> Vec<CategoryCount> {
        let mut v: Vec<CategoryCount> = map
            .into_iter()
            .map(|(name, count)| CategoryCount { name, count })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        v
    };

    Ok(KnowledgeCategoriesResponse {
        total_documents: docs.len(),
        departments,
        section_types: to_sorted_counts(section_map),
        source_types: to_sorted_counts(source_map),
        seasonal_tags: to_sorted_counts(tag_map),
        populations: to_sorted_counts(pop_map),
    })
}

#[tauri::command]
pub async fn get_knowledge_categories(
    state: State<'_, StudioState>,
    tenant_namespace: Option<String>,
) -> Result<KnowledgeCategoriesResponse, String> {
    get_knowledge_categories_impl(&state, tenant_namespace).await
}

pub async fn list_knowledge_documents_impl(
    state: &StudioState,
    department: Option<String>,
    tenant_namespace: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<KnowledgeDocument>, String> {
    let mut docs = state
        .tantivy
        .list_all_documents(tenant_namespace.as_deref())
        .map_err(|e| e.to_string())?;
    if let Some(dept) = department.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        docs.retain(|d| d.department == dept);
    }
    if let Some(lim) = limit {
        docs.truncate(lim.max(1));
    }
    Ok(docs)
}

#[tauri::command]
pub async fn list_knowledge_documents(
    state: State<'_, StudioState>,
    department: Option<String>,
    tenant_namespace: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<KnowledgeDocument>, String> {
    list_knowledge_documents_impl(&state, department, tenant_namespace, limit).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiInspectKnowledgeRequest {
    pub documents: Vec<KnowledgeDocument>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub custom_prompt: Option<String>,
    #[serde(default)]
    pub tenant_namespace: Option<String>,
}

pub async fn ai_inspect_knowledge_batch_impl(
    state: &StudioState,
    req: AiInspectKnowledgeRequest,
) -> Result<AiKnowledgeInspectionResponse, String> {
    ai_inspect_knowledge_batch_with_state(
        state,
        &req.documents,
        req.provider_id.as_deref(),
        req.custom_prompt.as_deref(),
        req.tenant_namespace.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn ai_inspect_knowledge_batch(
    state: State<'_, StudioState>,
    req: AiInspectKnowledgeRequest,
) -> Result<AiKnowledgeInspectionResponse, String> {
    ai_inspect_knowledge_batch_impl(&state, req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiInspectPatientRequest {
    pub records: Vec<PatientRecord>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub custom_prompt: Option<String>,
}

pub async fn ai_inspect_patient_batch_impl(
    state: &StudioState,
    req: AiInspectPatientRequest,
) -> Result<AiPatientInspectionResponse, String> {
    ai_inspect_patient_batch_with_state(
        state,
        &req.records,
        req.provider_id.as_deref(),
        req.custom_prompt.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn ai_inspect_patient_batch(
    state: State<'_, StudioState>,
    req: AiInspectPatientRequest,
) -> Result<AiPatientInspectionResponse, String> {
    ai_inspect_patient_batch_impl(&state, req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExportExchangePackageRequest {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub include_knowledge: bool,
    #[serde(default)]
    pub knowledge_departments: Vec<String>,
    #[serde(default)]
    pub tenant_namespace: Option<String>,
    #[serde(default = "default_true")]
    pub include_patients: bool,
    #[serde(default = "default_true")]
    pub include_epidemics: bool,
    #[serde(default = "default_true")]
    pub include_dictionaries: bool,
    /// "json" (full DataCollectionPackage exchange bundle) or "knowledge_jsonl"
    #[serde(default = "default_export_format")]
    pub format: String,
    /// Optional local file path to also save directly to disk
    #[serde(default)]
    pub output_path: Option<String>,
}

fn default_export_format() -> String {
    "json".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangePackageStats {
    pub knowledge_count: usize,
    pub patient_count: usize,
    pub l3_event_count: usize,
    pub epidemic_count: usize,
    pub synonym_count: usize,
    pub red_flag_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportExchangePackageResponse {
    pub filename: String,
    pub format: String,
    pub content: String,
    pub saved_path: Option<String>,
    pub stats: ExchangePackageStats,
}

pub async fn export_exchange_package_impl(
    state: &StudioState,
    req: ExportExchangePackageRequest,
) -> Result<ExportExchangePackageResponse, String> {
    let mut knowledge_documents = if req.include_knowledge {
        let mut docs = state
            .tantivy
            .list_all_documents(req.tenant_namespace.as_deref())
            .map_err(|e| e.to_string())?;
        if !req.knowledge_departments.is_empty() {
            let allowed: HashSet<String> = req
                .knowledge_departments
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !allowed.is_empty() {
                docs.retain(|d| allowed.contains(&d.department));
            }
        }
        docs
    } else {
        Vec::new()
    };
    knowledge_documents.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));

    let mut patient_records = Vec::new();
    let mut l3_event_count = 0usize;
    if req.include_patients {
        let ids = state
            .stores
            .patient_memory
            .list_patient_ids(10_000, 0)
            .await
            .map_err(|e| e.to_string())?;
        for pid in ids {
            if let Ok(Some(rec)) = state.stores.patient_memory.get_full_memory(&pid, 200).await {
                l3_event_count += rec.l3_visit_events.len();
                patient_records.push(rec);
            }
        }
    }

    let epidemic_contexts = if req.include_epidemics {
        state
            .stores
            .epidemics
            .list_all()
            .await
            .map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };

    let medical_dictionaries = if req.include_dictionaries {
        Some(
            state
                .stores
                .dictionaries
                .load_dictionaries()
                .await
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };

    let synonym_count = medical_dictionaries
        .as_ref()
        .map(|d| d.symptom_synonyms.len())
        .unwrap_or(0);
    let red_flag_count = medical_dictionaries
        .as_ref()
        .map(|d| d.red_flag_rules.len())
        .unwrap_or(0);

    let stats = ExchangePackageStats {
        knowledge_count: knowledge_documents.len(),
        patient_count: patient_records.len(),
        l3_event_count,
        epidemic_count: epidemic_contexts.len(),
        synonym_count,
        red_flag_count,
    };

    let ts = Utc::now().format("%Y%m%d_%H%M%S");
    let fmt = req.format.trim().to_ascii_lowercase();
    let (filename, content) = if fmt == "knowledge_jsonl" || fmt == "jsonl" {
        let mut lines = Vec::with_capacity(knowledge_documents.len());
        for doc in &knowledge_documents {
            let line = serde_json::to_string(doc).map_err(|e| format!("序列化知识文档失败: {e}"))?;
            lines.push(line);
        }
        (
            format!("medical_knowledge_{ts}.jsonl"),
            lines.join("\n"),
        )
    } else {
        let pkg = DataCollectionPackage {
            schema_version: "1.0.0".to_string(),
            description: req.description.filter(|s| !s.trim().is_empty()).or_else(|| {
                Some(format!(
                    "MedicalStudio 学习交流共享包 (导出时间 {})",
                    Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
                ))
            }),
            knowledge_documents,
            patient_records,
            epidemic_contexts,
            medical_dictionaries,
        };
        let json = serde_json::to_string_pretty(&pkg)
            .map_err(|e| format!("序列化共享包 JSON 失败: {e}"))?;
        (format!("medical_exchange_bundle_{ts}.json"), json)
    };

    let mut saved_path = None;
    if let Some(out_path) = req
        .output_path
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        if let Some(parent) = Path::new(&out_path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建导出目录失败 ({out_path}): {e}"))?;
        }
        std::fs::write(&out_path, content.as_bytes())
            .map_err(|e| format!("写入导出文件失败 ({out_path}): {e}"))?;
        saved_path = Some(out_path);
    }

    Ok(ExportExchangePackageResponse {
        filename,
        format: if fmt == "knowledge_jsonl" || fmt == "jsonl" {
            "knowledge_jsonl".to_string()
        } else {
            "json".to_string()
        },
        content,
        saved_path,
        stats,
    })
}

#[tauri::command]
pub async fn export_exchange_package(
    state: State<'_, StudioState>,
    req: ExportExchangePackageRequest,
) -> Result<ExportExchangePackageResponse, String> {
    export_exchange_package_impl(&state, req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct PreviewExchangePackageRequest {
    pub content: String,
    #[serde(default)]
    pub tenant_namespace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangePackagePreview {
    pub schema_version: String,
    pub description: Option<String>,
    pub stats: ExchangePackageStats,
    pub knowledge_departments: Vec<CategoryCount>,
    pub duplicate_knowledge_count: usize,
    pub package: DataCollectionPackage,
}

pub async fn preview_exchange_package_impl(
    state: &StudioState,
    req: PreviewExchangePackageRequest,
) -> Result<ExchangePackagePreview, String> {
    let pkg = parse_collection_bundle_str(&req.content).map_err(|e| format!("共享包解析校验失败: {e}"))?;

    let existing_docs = state
        .tantivy
        .list_all_documents(req.tenant_namespace.as_deref())
        .unwrap_or_default();
    let existing_ids: HashSet<String> = existing_docs.iter().map(|d| d.doc_id.clone()).collect();
    let existing_keys: HashSet<(String, String)> = existing_docs
        .iter()
        .map(|d| {
            (
                d.disease_name.trim().to_ascii_lowercase(),
                d.section_type.as_str().to_string(),
            )
        })
        .collect();

    let mut dept_map: HashMap<String, usize> = HashMap::new();
    let mut dup_count = 0usize;
    for d in &pkg.knowledge_documents {
        *dept_map.entry(d.department.clone()).or_insert(0) += 1;
        if existing_ids.contains(&d.doc_id)
            || existing_keys.contains(&(
                d.disease_name.trim().to_ascii_lowercase(),
                d.section_type.as_str().to_string(),
            ))
        {
            dup_count += 1;
        }
    }
    let mut knowledge_departments: Vec<CategoryCount> = dept_map
        .into_iter()
        .map(|(name, count)| CategoryCount { name, count })
        .collect();
    knowledge_departments.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));

    let l3_event_count: usize = pkg
        .patient_records
        .iter()
        .map(|p| p.l3_visit_events.len())
        .sum();
    let stats = ExchangePackageStats {
        knowledge_count: pkg.knowledge_documents.len(),
        patient_count: pkg.patient_records.len(),
        l3_event_count,
        epidemic_count: pkg.epidemic_contexts.len(),
        synonym_count: pkg
            .medical_dictionaries
            .as_ref()
            .map(|d| d.symptom_synonyms.len())
            .unwrap_or(0),
        red_flag_count: pkg
            .medical_dictionaries
            .as_ref()
            .map(|d| d.red_flag_rules.len())
            .unwrap_or(0),
    };

    Ok(ExchangePackagePreview {
        schema_version: pkg.schema_version.clone(),
        description: pkg.description.clone(),
        stats,
        knowledge_departments,
        duplicate_knowledge_count: dup_count,
        package: pkg,
    })
}

#[tauri::command]
pub async fn preview_exchange_package(
    state: State<'_, StudioState>,
    req: PreviewExchangePackageRequest,
) -> Result<ExchangePackagePreview, String> {
    preview_exchange_package_impl(&state, req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportExchangePackageRequest {
    pub package: DataCollectionPackage,
    #[serde(default = "default_ns")]
    pub tenant_namespace: String,
    #[serde(default = "default_true")]
    pub import_knowledge: bool,
    #[serde(default = "default_true")]
    pub import_patients: bool,
    #[serde(default = "default_true")]
    pub import_epidemics: bool,
    #[serde(default = "default_true")]
    pub import_dictionaries: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangePackageImportResult {
    pub imported_knowledge: u64,
    pub imported_patients: usize,
    pub imported_l3_events: usize,
    pub imported_epidemics: usize,
    pub imported_synonyms: usize,
    pub imported_red_flags: usize,
}

pub async fn import_exchange_package_impl(
    state: &StudioState,
    req: ImportExchangePackageRequest,
) -> Result<ExchangePackageImportResult, String> {
    req.package
        .validate()
        .map_err(|e| format!("共享包校验失败: {e}"))?;

    let mut imported_knowledge = 0u64;
    if req.import_knowledge && !req.package.knowledge_documents.is_empty() {
        imported_knowledge = state
            .indexer
            .upsert_documents(&req.package.knowledge_documents, &req.tenant_namespace)
            .await
            .map_err(|e| e.to_string())?;
    }

    let mut imported_patients = 0usize;
    let mut imported_l3_events = 0usize;
    if req.import_patients {
        for rec in &req.package.patient_records {
            state
                .stores
                .patient_memory
                .upsert_l2_profile(
                    &rec.patient_id,
                    rec.consent_long_term_memory,
                    &rec.l2_profile,
                )
                .await
                .map_err(|e| e.to_string())?;
            imported_patients += 1;

            for ev in &rec.l3_visit_events {
                state
                    .stores
                    .patient_memory
                    .insert_l3_event(&rec.patient_id, ev)
                    .await
                    .map_err(|e| e.to_string())?;
                if let Ok(vec) = state
                    .embedder
                    .embed_one(&format!("{} {}", ev.chief_complaint, ev.summary))
                    .await
                {
                    let _ = state
                        .stores
                        .patient_l3_vectors
                        .upsert_event_vector(&rec.patient_id, ev, vec)
                        .await;
                }
                imported_l3_events += 1;
            }
        }
    }

    let mut imported_epidemics = 0usize;
    if req.import_epidemics {
        for ctx in &req.package.epidemic_contexts {
            state
                .stores
                .epidemics
                .upsert(ctx)
                .await
                .map_err(|e| e.to_string())?;
            imported_epidemics += 1;
        }
    }

    let mut imported_synonyms = 0usize;
    let mut imported_red_flags = 0usize;
    if req.import_dictionaries
        && let Some(dicts) = &req.package.medical_dictionaries
    {
        for syn in &dicts.symptom_synonyms {
            state
                .stores
                .dictionaries
                .save_symptom_synonym(syn)
                .await
                .map_err(|e| e.to_string())?;
            imported_synonyms += 1;
        }
        for rule in &dicts.red_flag_rules {
            state
                .stores
                .dictionaries
                .save_red_flag_rule(rule)
                .await
                .map_err(|e| e.to_string())?;
            imported_red_flags += 1;
        }
    }

    state.sync_ecs_resources().await?;

    Ok(ExchangePackageImportResult {
        imported_knowledge,
        imported_patients,
        imported_l3_events,
        imported_epidemics,
        imported_synonyms,
        imported_red_flags,
    })
}

#[tauri::command]
pub async fn import_exchange_package(
    state: State<'_, StudioState>,
    req: ImportExchangePackageRequest,
) -> Result<ExchangePackageImportResult, String> {
    import_exchange_package_impl(&state, req).await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcsRuntimeStatus {
    pub using_real_llm: bool,
    pub active_providers: Vec<String>,
    pub knowledge_doc_count: u64,
    pub patient_count: usize,
    pub epidemic_count: usize,
    pub synonym_count: usize,
    pub red_flag_count: usize,
}

pub async fn get_ecs_runtime_status_impl(
    state: &StudioState,
) -> Result<EcsRuntimeStatus, String> {
    let (_, using_real_llm, active_providers) = state.build_active_model_router().await;
    let knowledge_doc_count = state.tantivy.num_docs();
    let patient_count = state
        .stores
        .patient_memory
        .list_patient_ids(10_000, 0)
        .await
        .map(|v| v.len())
        .unwrap_or(0);
    let epidemic_count = state
        .stores
        .epidemics
        .list_all()
        .await
        .map(|v| v.len())
        .unwrap_or(0);
    let dicts = state
        .stores
        .dictionaries
        .load_dictionaries()
        .await
        .unwrap_or_default();

    Ok(EcsRuntimeStatus {
        using_real_llm,
        active_providers,
        knowledge_doc_count,
        patient_count,
        epidemic_count,
        synonym_count: dicts.symptom_synonyms.len(),
        red_flag_count: dicts.red_flag_rules.len(),
    })
}

#[tauri::command]
pub async fn get_ecs_runtime_status(
    state: State<'_, StudioState>,
) -> Result<EcsRuntimeStatus, String> {
    get_ecs_runtime_status_impl(&state).await
}

// ===========================================================================
// Module 8: 病人复诊反馈与新病人建档问诊（按序列号查询、实时更新病历）
// ===========================================================================

fn resolve_canonical_patient_id(serial_or_id: &str, salt: Option<&str>) -> Result<String, String> {
    let trimmed = serial_or_id.trim();
    if trimmed.is_empty() {
        return Err("病人序列号不能为空".to_string());
    }
    if trimmed.starts_with("pat_sha256_") && trimmed.len() == 75 {
        return Ok(trimmed.to_string());
    }
    let salt_bytes = salt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.as_bytes().to_vec())
        .unwrap_or_else(|| PatientMaskingConfig::default().salt);
    Ok(hash_patient_id(trimmed, &salt_bytes))
}

fn sanitize_l2_profile(mut profile: L2Profile) -> L2Profile {
    if profile.age_group.trim().is_empty() {
        profile.age_group = "unknown".to_string();
    } else {
        profile.age_group = profile.age_group.trim().to_string();
    }
    let clean_vec = |items: Vec<String>| -> Vec<String> {
        let mut out = Vec::new();
        for item in items {
            let s = scrub_pii(item.trim()).scrubbed_text;
            let t = s.trim();
            if !t.is_empty() && !out.iter().any(|x: &String| x == t) {
                out.push(t.to_string());
            }
        }
        out
    };
    profile.chronic_diseases = clean_vec(profile.chronic_diseases);
    profile.allergies = clean_vec(profile.allergies);
    profile.surgical_history = clean_vec(profile.surgical_history);
    profile.long_term_medications = clean_vec(profile.long_term_medications);
    profile
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatientLookupResponse {
    pub found: bool,
    pub input_serial: String,
    pub resolved_patient_id: String,
    pub record: Option<PatientRecord>,
}

pub async fn lookup_patient_by_serial_impl(
    state: &StudioState,
    serial_or_id: String,
    salt: Option<String>,
) -> Result<PatientLookupResponse, String> {
    let trimmed = serial_or_id.trim().to_string();
    if trimmed.is_empty() {
        return Err("请输入病人序列号或匿名 Patient ID".to_string());
    }

    // 1. Direct match (e.g., user pasted exact pat_sha256_...)
    if let Ok(Some(rec)) = state.stores.patient_memory.get_full_memory(&trimmed, 50).await {
        return Ok(PatientLookupResponse {
            found: true,
            input_serial: trimmed.clone(),
            resolved_patient_id: rec.patient_id.clone(),
            record: Some(rec),
        });
    }

    // 2. Safe prefix/suffix match on stored patient_ids:
    // Strip "pat_sha256_" prefix first and require >= 12 hex chars so short prefixes like
    // "pat_sha256_" can NEVER collide with a random patient. Reject ambiguous multiple hits.
    let hex_part = trimmed.strip_prefix("pat_sha256_").unwrap_or(&trimmed);
    if hex_part.len() >= 12 && hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Ok(all_ids) = state.stores.patient_memory.list_patient_ids(5000, 0).await {
            let matches: Vec<String> = all_ids
                .into_iter()
                .filter(|id| {
                    let id_hex = id.strip_prefix("pat_sha256_").unwrap_or(id);
                    id_hex.starts_with(hex_part) || id_hex.ends_with(hex_part)
                })
                .collect();
            if matches.len() > 1 {
                return Err(format!(
                    "临床串档拦截：序列号片段「{trimmed}」命中了 {} 位患者，请输入更完整的序列号以精确定位。",
                    matches.len()
                ));
            }
            if let Some(matched_id) = matches.into_iter().next()
                && let Ok(Some(rec)) = state
                    .stores
                    .patient_memory
                    .get_full_memory(&matched_id, 50)
                    .await
            {
                return Ok(PatientLookupResponse {
                    found: true,
                    input_serial: trimmed,
                    resolved_patient_id: matched_id,
                    record: Some(rec),
                });
            }
        }
    }

    // 3. Deterministic salted hash match for raw clinic serial numbers
    let canonical_id = resolve_canonical_patient_id(&trimmed, salt.as_deref())?;
    let record = state
        .stores
        .patient_memory
        .get_full_memory(&canonical_id, 50)
        .await
        .map_err(|e| e.to_string())?;

    Ok(PatientLookupResponse {
        found: record.is_some(),
        input_serial: trimmed,
        resolved_patient_id: canonical_id,
        record,
    })
}

#[tauri::command]
pub async fn lookup_patient_by_serial(
    state: State<'_, StudioState>,
    serial_or_id: String,
    salt: Option<String>,
) -> Result<PatientLookupResponse, String> {
    lookup_patient_by_serial_impl(&state, serial_or_id, salt).await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatientSummaryItem {
    pub patient_id: String,
    pub age_group: String,
    pub gender: medical_core::models::Gender,
    pub chronic_diseases: Vec<String>,
    pub visit_count: usize,
    pub latest_complaint: Option<String>,
    pub latest_department: Option<String>,
    pub latest_visit_at: Option<chrono::DateTime<Utc>>,
}

pub async fn list_patient_summaries_impl(
    state: &StudioState,
    limit: Option<usize>,
) -> Result<Vec<PatientSummaryItem>, String> {
    let ids = state
        .stores
        .patient_memory
        .list_patient_ids(limit.unwrap_or(200).clamp(1, 1000), 0)
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(ids.len());
    for pid in ids {
        if let Ok(Some(rec)) = state.stores.patient_memory.get_full_memory(&pid, 50).await {
            let latest = rec.l3_visit_events.first();
            out.push(PatientSummaryItem {
                patient_id: rec.patient_id,
                age_group: rec.l2_profile.age_group,
                gender: rec.l2_profile.gender,
                chronic_diseases: rec.l2_profile.chronic_diseases,
                visit_count: rec.l3_visit_events.len(),
                latest_complaint: latest.map(|e| e.chief_complaint.clone()),
                latest_department: latest.map(|e| e.recommended_department.clone()),
                latest_visit_at: latest.map(|e| e.occurred_at),
            });
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn list_patient_summaries(
    state: State<'_, StudioState>,
    limit: Option<usize>,
) -> Result<Vec<PatientSummaryItem>, String> {
    list_patient_summaries_impl(&state, limit).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertPatientProfileRequest {
    pub serial_or_id: String,
    #[serde(default)]
    pub salt: Option<String>,
    #[serde(default = "default_true")]
    pub consent_long_term_memory: bool,
    pub l2_profile: L2Profile,
}

pub async fn create_or_update_patient_profile_impl(
    state: &StudioState,
    req: UpsertPatientProfileRequest,
) -> Result<PatientRecord, String> {
    let patient_id = resolve_canonical_patient_id(&req.serial_or_id, req.salt.as_deref())?;
    let clean_l2 = sanitize_l2_profile(req.l2_profile);

    state
        .stores
        .patient_memory
        .upsert_l2_profile(&patient_id, req.consent_long_term_memory, &clean_l2)
        .await
        .map_err(|e| e.to_string())?;

    state
        .stores
        .patient_memory
        .get_full_memory(&patient_id, 50)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "保存患者档案后读取失败".to_string())
}

#[tauri::command]
pub async fn create_or_update_patient_profile(
    state: State<'_, StudioState>,
    req: UpsertPatientProfileRequest,
) -> Result<PatientRecord, String> {
    create_or_update_patient_profile_impl(&state, req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubmitRevisitFeedbackRequest {
    pub serial_or_id: String,
    #[serde(default)]
    pub salt: Option<String>,
    /// e.g. "明显好转" | "部分好转" | "症状持续" | "症状加重" | "出现新症状" | "初诊建档"
    #[serde(default)]
    pub outcome_status: String,
    pub chief_complaint: String,
    #[serde(default)]
    pub extracted_symptoms: Vec<String>,
    pub actual_department: String,
    #[serde(default = "default_urgency")]
    pub urgency_level: u8,
    pub feedback_summary: String,
    #[serde(default)]
    pub updated_l2_profile: Option<L2Profile>,
}

fn default_urgency() -> u8 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisitFeedbackResult {
    pub resolved_patient_id: String,
    pub created_event: L3VisitEvent,
    pub patient_record: PatientRecord,
}

pub async fn submit_patient_revisit_feedback_impl(
    state: &StudioState,
    req: SubmitRevisitFeedbackRequest,
) -> Result<RevisitFeedbackResult, String> {
    let patient_id = resolve_canonical_patient_id(&req.serial_or_id, req.salt.as_deref())?;
    let chief_raw = req.chief_complaint.trim();
    if chief_raw.is_empty() {
        return Err("请填写本次复诊主诉或反馈主诉".to_string());
    }

    // Ensure L2 exists or update it with latest profile
    if let Some(l2) = req.updated_l2_profile {
        let clean_l2 = sanitize_l2_profile(l2);
        state
            .stores
            .patient_memory
            .upsert_l2_profile(&patient_id, true, &clean_l2)
            .await
            .map_err(|e| e.to_string())?;
    } else if state
        .stores
        .patient_memory
        .get_l2_profile(&patient_id)
        .await
        .map_err(|e| e.to_string())?
        .is_none()
    {
        state
            .stores
            .patient_memory
            .upsert_l2_profile(
                &patient_id,
                true,
                &L2Profile {
                    age_group: "unknown".to_string(),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| e.to_string())?;
    }

    let outcome_prefix = req.outcome_status.trim();
    let scrubbed_chief = scrub_pii(chief_raw).scrubbed_text;
    let combined_summary_raw = if outcome_prefix.is_empty() {
        req.feedback_summary.trim().to_string()
    } else if req.feedback_summary.trim().is_empty() {
        format!("[复诊转归: {outcome_prefix}] {scrubbed_chief}")
    } else {
        format!("[复诊转归: {outcome_prefix}] {}", req.feedback_summary.trim())
    };
    let scrubbed_summary = scrub_pii(&combined_summary_raw).scrubbed_text;

    let department = if req.actual_department.trim().is_empty() {
        "全科医学科".to_string()
    } else {
        req.actual_department.trim().to_string()
    };

    let symptoms: Vec<String> = req
        .extracted_symptoms
        .into_iter()
        .map(|s| scrub_pii(s.trim()).scrubbed_text)
        .filter(|s| !s.trim().is_empty())
        .collect();

    let event = L3VisitEvent {
        event_id: format!("evt_revisit_{}", uuid::Uuid::new_v4().simple()),
        occurred_at: Utc::now(),
        chief_complaint: scrubbed_chief,
        extracted_symptoms: symptoms,
        recommended_department: department,
        urgency_level: req.urgency_level.clamp(1, 4),
        summary: scrubbed_summary,
    };

    state
        .stores
        .patient_memory
        .insert_l3_event(&patient_id, &event)
        .await
        .map_err(|e| e.to_string())?;

    if let Ok(vec) = state
        .embedder
        .embed_one(&format!("{} {}", event.chief_complaint, event.summary))
        .await
    {
        let _ = state
            .stores
            .patient_l3_vectors
            .upsert_event_vector(&patient_id, &event, vec)
            .await;
    }

    let patient_record = state
        .stores
        .patient_memory
        .get_full_memory(&patient_id, 50)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "写入复诊记录后读取患者档案失败".to_string())?;

    Ok(RevisitFeedbackResult {
        resolved_patient_id: patient_id,
        created_event: event,
        patient_record,
    })
}

#[tauri::command]
pub async fn submit_patient_revisit_feedback(
    state: State<'_, StudioState>,
    req: SubmitRevisitFeedbackRequest,
) -> Result<RevisitFeedbackResult, String> {
    submit_patient_revisit_feedback_impl(&state, req).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConsultationTriageRequest {
    pub session_id: String,
    pub serial_or_id: String,
    #[serde(default)]
    pub salt: Option<String>,
    /// True = 新病人初诊（不携带任何旧记忆，新建档案并在问诊中更新）；False = 老病人复诊（携带完整 L2/L3 记忆）
    #[serde(default)]
    pub is_new_patient: bool,
    pub utterance: String,
    #[serde(default)]
    pub region_code: Option<String>,
    #[serde(default)]
    pub target_population: Vec<TargetPopulation>,
    /// Optional live L2 profile update provided by clinician/patient during consultation
    #[serde(default)]
    pub live_l2_update: Option<L2Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsultationTriageResponse {
    pub resolved_patient_id: String,
    pub is_new_patient: bool,
    pub triage: TriageExecutionResult,
    pub updated_patient_record: Option<PatientRecord>,
    pub memory_updated: bool,
}

pub async fn run_consultation_triage_impl(
    state: &StudioState,
    req: ConsultationTriageRequest,
) -> Result<ConsultationTriageResponse, String> {
    let patient_id = resolve_canonical_patient_id(&req.serial_or_id, req.salt.as_deref())?;

    // Guard against clinical record collision: if clinician selected "新病人初诊建档",
    // block execution when a patient archive already exists for this ID (unless continuing same session).
    let prior_session = state
        .stores
        .session_cache
        .get_session(&req.session_id)
        .await
        .ok()
        .flatten();
    if let Some(ref ps) = prior_session
        && ps.patient_id != patient_id
    {
        return Err("会话归属校验失败：当前 session_id 属于其他患者，请开启新会话。".to_string());
    }
    if req.is_new_patient
        && prior_session.is_none()
        && state
            .stores
            .patient_memory
            .get_l2_profile(&patient_id)
            .await
            .ok()
            .flatten()
            .is_some()
    {
        return Err(
            "临床串档拦截：该序列号在库中已存在既有患者档案！请切换至「老病人复诊」模式，或更换全新的患者序列号。"
                .to_string(),
        );
    }

    // If this is a brand-new patient on turn 1, ensure no stale prior memory is recalled
    // (we only create their fresh L2 record once they fill live_l2_update or when triage completes)
    let mut memory_updated = false;
    if let Some(l2) = req.live_l2_update {
        let clean_l2 = sanitize_l2_profile(l2);
        state
            .stores
            .patient_memory
            .upsert_l2_profile(&patient_id, true, &clean_l2)
            .await
            .map_err(|e| e.to_string())?;
        memory_updated = true;
    }

    // For a brand-new patient with no prior record yet:
    // - Stage 2 will naturally find no L2/L3 history (zero prior memory!)
    // - Stage 5 will automatically create & save the new patient's L2 + L3 record when triage completes
    let triage_req = TriageRequest {
        session_id: req.session_id,
        patient_id: patient_id.clone(),
        tenant_id: "default".to_string(),
        region_code: req.region_code,
        utterance: req.utterance,
        target_population: if req.target_population.is_empty() {
            vec![TargetPopulation::Adult]
        } else {
            req.target_population
        },
        consent_long_term_memory: Some(true),
        prior_session,
        stream_tx: None,
    };

    let triage = state
        .triage_engine
        .run_triage(triage_req)
        .await
        .map_err(|e| e.to_string())?;

    if triage.response.status == TriageStatus::Completed && triage.trace.consolidation_completed {
        memory_updated = true;
    }

    let updated_patient_record = state
        .stores
        .patient_memory
        .get_full_memory(&patient_id, 50)
        .await
        .ok()
        .flatten();

    Ok(ConsultationTriageResponse {
        resolved_patient_id: patient_id,
        is_new_patient: req.is_new_patient,
        triage,
        updated_patient_record,
        memory_updated,
    })
}

#[tauri::command]
pub async fn run_consultation_triage(
    state: State<'_, StudioState>,
    req: ConsultationTriageRequest,
) -> Result<ConsultationTriageResponse, String> {
    run_consultation_triage_impl(&state, req).await
}

// ===========================================================================
// Module 9: 临床治理、审计日志与安全删除闭环 (STD-02)
// ===========================================================================

pub async fn list_triage_audit_logs_impl(
    state: &StudioState,
    limit: Option<usize>,
    tenant_id: Option<String>,
) -> Result<Vec<medical_core::storage::TriageAuditLog>, String> {
    state
        .stores
        .audit_logs
        .query_recent(limit.unwrap_or(100).clamp(1, 500), tenant_id.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_triage_audit_logs(
    state: State<'_, StudioState>,
    limit: Option<usize>,
    tenant_id: Option<String>,
) -> Result<Vec<medical_core::storage::TriageAuditLog>, String> {
    list_triage_audit_logs_impl(&state, limit, tenant_id).await
}

pub async fn delete_knowledge_documents_impl(
    state: &StudioState,
    doc_ids: Vec<String>,
    tenant_namespace: Option<String>,
) -> Result<u64, String> {
    if doc_ids.is_empty() {
        return Err("请至少指定一条要删除的知识库文档 ID".to_string());
    }
    let ns = tenant_namespace
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    let deleted_bm25 = state
        .tantivy
        .delete_by_doc_ids_in_namespace(&doc_ids, ns)
        .map_err(|e| e.to_string())?;
    let _ = state
        .stores
        .knowledge_vectors
        .delete_by_doc_ids(&doc_ids)
        .await;
    Ok(deleted_bm25)
}

#[tauri::command]
pub async fn delete_knowledge_documents(
    state: State<'_, StudioState>,
    doc_ids: Vec<String>,
    tenant_namespace: Option<String>,
) -> Result<u64, String> {
    delete_knowledge_documents_impl(&state, doc_ids, tenant_namespace).await
}

pub async fn delete_patient_record_impl(
    state: &StudioState,
    patient_id: String,
) -> Result<medical_core::storage::WipedMemoryStats, String> {
    let pid = patient_id.trim();
    if pid.is_empty() {
        return Err("patient_id 不能为空".to_string());
    }
    let mut stats = state
        .stores
        .patient_memory
        .delete_patient_memory(pid)
        .await
        .map_err(|e| e.to_string())?;
    stats.qdrant_points_deleted = state
        .stores
        .patient_l3_vectors
        .delete_patient_events(pid)
        .await
        .map_err(|e| e.to_string())?;
    stats.l1_sessions_deleted = state
        .stores
        .session_cache
        .delete_patient_sessions(pid)
        .await
        .map_err(|e| e.to_string())?;
    Ok(stats)
}

#[tauri::command]
pub async fn delete_patient_record(
    state: State<'_, StudioState>,
    patient_id: String,
) -> Result<medical_core::storage::WipedMemoryStats, String> {
    delete_patient_record_impl(&state, patient_id).await
}

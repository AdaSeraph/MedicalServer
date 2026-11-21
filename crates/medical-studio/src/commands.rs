use std::path::Path;
use std::time::Instant;

use medical_core::ecs::{TriageExecutionResult, TriageRequest};
use medical_core::llm::{ProbeResult, ProviderConfig, RouterConfig};
use medical_core::models::{
    EpidemicContext, KnowledgeDocument, MedicalDictionaries, RedFlagLevel, RedFlagRule,
    SymptomSynonym, TargetPopulation,
};
use medical_core::storage::{
    ApiKeyRecord, GatewayMetricsSummary, IngestionCheckpoint, IssueApiKeyRequest, IssuedApiKey,
};
use medical_core::validation::Validate;
use medical_pipeline::{
    BookMetadataDefaults, CustomChunkRule, JobProgress, PatientImportReport, PatientMaskingConfig,
    export_error_samples_jsonl, import_patients_file, parse_collection_bundle_file,
    parse_medical_book_file, stream_knowledge_jsonl,
};
use serde::{Deserialize, Serialize};
use tauri::State;

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
    state
        .checkpoints
        .get(&job_id)
        .await
        .map_err(|e| e.to_string())
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

    let url = format!("{}/v1/admin/reload", base.trim_end_matches('/'));
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
                message: format!("已通知远程服务 {url}，HTTP {status}"),
            })
        }
        Err(e) => Ok(HotReloadNotifyResult {
            local_reloaded: true,
            remote_notified: false,
            remote_status: None,
            message: format!("本地已热更新，远程通知失败: {e}"),
        }),
    }
}

#[tauri::command]
pub async fn notify_server_hot_reload(
    state: State<'_, StudioState>,
    server_url: Option<String>,
    admin_token: Option<String>,
) -> Result<HotReloadNotifyResult, String> {
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

    state
        .triage_engine
        .run_triage(triage_req)
        .await
        .map_err(|e| e.to_string())
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

pub async fn list_llm_providers_impl(state: &StudioState) -> Result<Vec<ProviderConfig>, String> {
    state
        .stores
        .llm_providers
        .list_providers()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_llm_providers(
    state: State<'_, StudioState>,
) -> Result<Vec<ProviderConfig>, String> {
    list_llm_providers_impl(&state).await
}

pub async fn upsert_llm_provider_impl(
    state: &StudioState,
    config: ProviderConfig,
) -> Result<ProviderConfig, String> {
    state
        .stores
        .llm_providers
        .upsert_provider(&config)
        .await
        .map_err(|e| e.to_string())?;
    Ok(config)
}

#[tauri::command]
pub async fn upsert_llm_provider(
    state: State<'_, StudioState>,
    config: ProviderConfig,
) -> Result<ProviderConfig, String> {
    upsert_llm_provider_impl(&state, config).await
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

    let url = format!("{}/models", cfg.base_url.trim_end_matches('/'));
    match state
        .http_client
        .get(&url)
        .bearer_auth(&cfg.api_key)
        .send()
        .await
    {
        Ok(resp) => {
            let ms = started.elapsed().as_millis() as u64;
            Ok(ProbeResult {
                provider_id,
                ok: resp.status().is_success(),
                ttft_ms: Some(ms),
                total_ms: ms,
                error: if resp.status().is_success() {
                    None
                } else {
                    Some(format!("HTTP {}", resp.status().as_u16()))
                },
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

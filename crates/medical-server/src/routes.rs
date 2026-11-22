use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures::stream;
use medical_core::crypto::hash_patient_id;
use medical_core::ecs::{TriageRequest, TriageStreamEvent};
use medical_core::llm::{LlmMessage, LlmRequest, LlmStreamChunk, MessageRole, ModelTier};
use medical_core::models::{
    ClinicalExamBundle, ClinicalMediaAsset, ClinicalMediaCategory, EpidemicContext,
    HospitalAuthMode, HospitalConnectorConfig, HospitalExternalReport, HospitalReportPullRequest,
    HospitalReportPullResponse, HospitalWebhookPushPayload, ImagingExamRecord, ImagingModality,
    KnowledgeDocument, L2Profile, L3VisitEvent, PatientRecord, RedFlagLevel, RedFlagRule,
    SymptomSynonym, TargetPopulation, TriggerSubstanceRecord, VitalSignsRecord,
    convert_fhir_bundle_to_clinical_exams, default_clinical_data_catalog,
    parse_mobile_report_qr_payload,
};
use medical_core::privacy::{MEDICAL_DISCLAIMER, sanitize_medical_output, scrub_pii};
use medical_core::storage::KnowledgeSearchQuery;
use medical_core::validation::{Validate, validate_patient_id};
use medical_pipeline::{
    PatientMaskingConfig, import_patients_csv_str, import_patients_json_str,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::auth::{AuthContext, AuthError, authenticate_request, issue_wechat_jwt};
use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = if !state.config.cors_allowed_origins.is_empty() {
        let origins: Vec<HeaderValue> = state
            .config
            .cors_allowed_origins
            .iter()
            .filter_map(|o| HeaderValue::from_str(o).ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    };

    Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .route("/v1/clinical/types", get(clinical_types_catalog_handler))
        .route("/v1/auth/wechat", post(wechat_auth_handler))
        .route("/v1/triage/chat", post(triage_chat_handler))
        .route("/v1/knowledge/search", post(knowledge_search_handler))
        .route("/v1/knowledge/import", post(knowledge_import_handler))
        .route(
            "/v1/knowledge/documents",
            get(knowledge_list_handler).delete(knowledge_delete_handler),
        )
        .route(
            "/v1/openai/chat/completions",
            post(openai_proxy_handler),
        )
        .route("/v1/epidemic/current", get(epidemic_current_handler))
        .route("/v1/patients", get(list_patients_handler))
        .route("/v1/patients/import", post(import_patients_batch_handler))
        .route(
            "/v1/patients/{patient_id}/memory",
            get(get_patient_memory_handler)
                .put(put_patient_memory_handler)
                .delete(delete_patient_memory_handler),
        )
        .route(
            "/v1/patients/{patient_id}/clinical-exams",
            post(attach_patient_clinical_exams_handler),
        )
        .route("/v1/dictionaries", get(get_dictionaries_handler))
        .route("/v1/dictionaries/synonyms", post(upsert_synonym_handler))
        .route("/v1/dictionaries/red-flags", post(upsert_red_flag_handler))
        .route("/v1/admin/reload", post(admin_reload_handler))
        .route("/v1/admin/save", post(admin_save_snapshot_handler))
        .route("/v1/admin/epidemics", post(admin_upsert_epidemic_handler))
        // Modern Hospital HIS / LIS / PACS / Mobile Cloud-Film / HL7 FHIR R4 Integration Interfaces
        .route(
            "/v1/hospital/connectors",
            get(list_hospital_connectors_handler).post(upsert_hospital_connector_handler),
        )
        .route(
            "/v1/hospital/connectors/{connector_id}",
            delete(delete_hospital_connector_handler),
        )
        .route("/v1/hospital/reports/pull", post(pull_hospital_report_handler))
        .route(
            "/v1/hospital/reports/webhook",
            post(hospital_report_webhook_handler),
        )
        .route("/v1/hospital/fhir/import", post(import_fhir_bundle_handler))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(ConcurrencyLimitLayer::new(512))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn auth_err_response(err: AuthError) -> Response {
    let body = serde_json::json!({
        "error": err.message,
        "code": err.status.as_u16(),
    });
    let mut resp = (err.status, Json(body)).into_response();
    if let Some(retry) = err.retry_after_secs
        && let Ok(val) = HeaderValue::from_str(&retry.to_string())
    {
        resp.headers_mut().insert(header::RETRY_AFTER, val);
    }
    resp
}

fn json_error(status: StatusCode, msg: impl Into<String>) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": msg.into(),
            "code": status.as_u16(),
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /health & GET /metrics
// ---------------------------------------------------------------------------

async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state
        .metrics
        .requests_total
        .with_label_values(&["/health", "GET", "200"])
        .inc();

    let router_status = state.model_router.router_health_summary();
    let overall_status = if router_status == "circuit_open" {
        "degraded"
    } else {
        "ok"
    };

    Json(serde_json::json!({
        "status": overall_status,
        "version": env!("CARGO_PKG_VERSION"),
        "checks": {
            "storage": if state.config.offline_mode { "in_memory_ok" } else { "pg_ok" },
            "knowledge_index": format!("tantivy_docs:{}", state.tantivy.num_docs()),
            "llm_router": router_status,
            "offline_mode": state.config.offline_mode
        }
    }))
}

async fn metrics_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if state.config.protect_metrics {
        match authenticate_request(&state, &headers, 1).await {
            Ok(auth) if auth.is_admin => {}
            Ok(_) => return auth_err_response(AuthError::forbidden("admin authorization required")),
            Err(e) => return auth_err_response(e),
        }
    }
    let body = state.metrics.render_text();
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /v1/auth/wechat
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct WeChatAuthRequest {
    code: String,
}

#[derive(Debug, Deserialize)]
struct WeChatCode2SessionResp {
    openid: Option<String>,
    errcode: Option<i32>,
    errmsg: Option<String>,
}

async fn wechat_auth_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WeChatAuthRequest>,
) -> Response {
    let code = req.code.trim();
    if code.is_empty() {
        state
            .metrics
            .requests_total
            .with_label_values(&["/v1/auth/wechat", "POST", "400"])
            .inc();
        return json_error(StatusCode::BAD_REQUEST, "wechat code must not be empty");
    }

    // Call real WeChat jscode2session if credentials configured and not in offline mode
    let openid = if !state.config.offline_mode
        && let (Some(app_id), Some(secret)) =
            (&state.config.wechat_app_id, &state.config.wechat_app_secret)
    {
        let url = format!(
            "{}/sns/jscode2session?appid={}&secret={}&js_code={}&grant_type=authorization_code",
            state.config.wechat_api_base.trim_end_matches('/'),
            app_id,
            secret,
            code
        );
        match state.http_client.get(&url).send().await {
            Ok(resp) => match resp.json::<WeChatCode2SessionResp>().await {
                Ok(parsed) if parsed.errcode.unwrap_or(0) == 0 && parsed.openid.is_some() => {
                    parsed.openid.unwrap()
                }
                Ok(parsed) => {
                    return json_error(
                        StatusCode::UNAUTHORIZED,
                        format!(
                            "wechat jscode2session rejected: {}",
                            parsed.errmsg.unwrap_or_default()
                        ),
                    );
                }
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_GATEWAY,
                        format!("wechat response parse error: {e}"),
                    );
                }
            },
            Err(e) => {
                return json_error(
                    StatusCode::BAD_GATEWAY,
                    format!("wechat upstream error: {e}"),
                );
            }
        }
    } else if !state.config.offline_mode {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "WeChat authentication is not configured for production (missing WECHAT_APP_ID / WECHAT_APP_SECRET)",
        );
    } else {
        // Offline deterministic openid derived from code (never echoes raw code in patient_id)
        format!("wx_openid_{}", medical_core::crypto::hash_api_key(code))
    };

    let patient_id = hash_patient_id(&openid, &state.config.patient_id_salt);
    let (token, expires_in) =
        match issue_wechat_jwt(&state.config.jwt_secret, &patient_id, state.config.jwt_ttl_secs) {
            Ok(v) => v,
            Err(e) => return auth_err_response(e),
        };

    state
        .metrics
        .requests_total
        .with_label_values(&["/v1/auth/wechat", "POST", "200"])
        .inc();

    Json(serde_json::json!({
        "token": token,
        "expires_in": expires_in,
        "patient_id": patient_id,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// POST /v1/triage/chat (JSON + SSE)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TriageChatRequest {
    session_id: String,
    #[allow(dead_code)]
    #[serde(default)]
    patient_id: Option<String>,
    utterance: String,
    #[serde(default)]
    region_code: Option<String>,
    #[serde(default)]
    target_population: Vec<TargetPopulation>,
    #[serde(default)]
    consent_long_term_memory: Option<bool>,
    #[serde(default)]
    stream: bool,
}

async fn triage_chat_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TriageChatRequest>,
) -> Response {
    let est_tokens = (req.utterance.chars().count() as u32).max(16) * 2;
    let auth = match authenticate_request(&state, &headers, est_tokens).await {
        Ok(a) => a,
        Err(e) => {
            state
                .metrics
                .requests_total
                .with_label_values(&["/v1/triage/chat", "POST", &e.status.as_u16().to_string()])
                .inc();
            return auth_err_response(e);
        }
    };

    if req.utterance.trim().is_empty() || req.session_id.trim().is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "session_id and utterance must not be empty",
        );
    }

    let patient_id = resolve_patient_id(&auth, req.patient_id.as_deref());
    if let Err(verr) = validate_patient_id(&patient_id) {
        return json_error(StatusCode::BAD_REQUEST, verr.message);
    }

    let prior_session = state
        .stores
        .session_cache
        .get_session(&req.session_id)
        .await
        .ok()
        .flatten();
    if let Some(ref prior) = prior_session
        && prior.patient_id != patient_id
    {
        return auth_err_response(AuthError::forbidden(
            "session_id belongs to a different patient",
        ));
    }

    let mut triage_req = TriageRequest {
        session_id: req.session_id.clone(),
        patient_id,
        tenant_id: auth.namespace.clone(),
        region_code: req.region_code.clone(),
        utterance: req.utterance.clone(),
        target_population: req.target_population.clone(),
        consent_long_term_memory: req.consent_long_term_memory,
        prior_session,
        stream_tx: None,
    };

    if req.stream {
        let (tx, rx) = mpsc::channel::<TriageStreamEvent>(32);
        triage_req.stream_tx = Some(tx.clone());

        let state_bg = state.clone();
        let api_key_rec = auth.api_key.clone();
        state.metrics.sse_active_connections.inc();
        let started = Instant::now();

        tokio::spawn(async move {
            // Cancel in-flight LLM/RAG work immediately if the SSE client disconnects
            tokio::select! {
                _ = tx.closed() => {}
                outcome = state_bg.triage_engine.run_triage(triage_req) => {
                    state_bg
                        .metrics
                        .triage_duration_seconds
                        .observe(started.elapsed().as_secs_f64());

                    match outcome {
                        Ok(exec) => {
                            if exec.response.is_fallback {
                                state_bg.metrics.llm_errors_total.inc();
                            }
                            if let Some(ref k) = api_key_rec {
                                let est_out = exec
                                    .response
                                    .question
                                    .as_deref()
                                    .map(|s| s.chars().count() as u32)
                                    .or_else(|| {
                                        exec.response
                                            .triage_result
                                            .as_ref()
                                            .map(|c| c.summary.chars().count() as u32)
                                    })
                                    .unwrap_or(64);
                                state_bg.rate_limiter.record_actual_tokens(
                                    &k.key_id,
                                    k.tpm_limit,
                                    est_tokens.saturating_add(est_out.saturating_mul(2)),
                                );
                            }
                            // Note: `TriageEngine::run_triage` already emitted `TriageStreamEvent::Result`
                            // once on `tx`; do NOT re-send a duplicate `event: result`.
                        }
                        Err(e) => {
                            state_bg.metrics.llm_errors_total.inc();
                            let _ = tx
                                .send(TriageStreamEvent::Error {
                                    message: e.to_string(),
                                })
                                .await;
                        }
                    }
                }
            }
            state_bg.metrics.sse_active_connections.dec();
        });

        state
            .metrics
            .requests_total
            .with_label_values(&["/v1/triage/chat", "POST", "200"])
            .inc();

        let stream = ReceiverStream::new(rx);
        let sse_stream = futures::StreamExt::map(stream, |ev| -> Result<Event, Infallible> {
            match ev {
                TriageStreamEvent::Delta { delta, provider_id } => {
                    let payload = serde_json::json!({
                        "delta": delta,
                        "provider_id": provider_id,
                    });
                    Ok(Event::default().event("delta").data(payload.to_string()))
                }
                TriageStreamEvent::Result(resp) => {
                    let data = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string());
                    Ok(Event::default().event("result").data(data))
                }
                TriageStreamEvent::Error { message } => {
                    let payload = serde_json::json!({ "error": message });
                    Ok(Event::default().event("error").data(payload.to_string()))
                }
            }
        });

        return Sse::new(sse_stream)
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    // Non-streaming JSON mode
    let started = Instant::now();
    let failures_before = state.model_router.total_failures();
    match state.triage_engine.run_triage(triage_req).await {
        Ok(exec) => {
            let failures_after = state.model_router.total_failures();
            let new_failures = failures_after.saturating_sub(failures_before);
            if new_failures > 0 {
                state.metrics.llm_errors_total.inc_by(new_failures);
            } else if exec.response.is_fallback {
                state.metrics.llm_errors_total.inc();
            }
            state
                .metrics
                .triage_duration_seconds
                .observe(started.elapsed().as_secs_f64());
            state
                .metrics
                .requests_total
                .with_label_values(&["/v1/triage/chat", "POST", "200"])
                .inc();
            if let Some(ref k) = auth.api_key {
                state.rate_limiter.record_actual_tokens(
                    &k.key_id,
                    k.tpm_limit,
                    exec.trace.llm_calls_made.saturating_mul(128),
                );
            }
            Json(exec.response).into_response()
        }
        Err(e) => {
            state.metrics.llm_errors_total.inc();
            state
                .metrics
                .requests_total
                .with_label_values(&["/v1/triage/chat", "POST", "500"])
                .inc();
            json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

fn resolve_patient_id(auth: &AuthContext, req_pid: Option<&str>) -> String {
    if let Some(bound) = &auth.patient_id {
        return bound.clone();
    }
    req_pid
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("pat_sha256_{}", Uuid::new_v4().simple()))
}

// ---------------------------------------------------------------------------
// POST /v1/knowledge/search
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct KnowledgeSearchRequest {
    query: String,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    target_population: Option<Vec<TargetPopulation>>,
    #[serde(default)]
    department: Option<String>,
    #[serde(default)]
    region_code: Option<String>,
    #[serde(default)]
    season: Option<String>,
    #[serde(default = "default_true")]
    seasonal_boost: bool,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_true() -> bool {
    true
}

fn default_top_k() -> usize {
    5
}

#[derive(Debug, Serialize, Deserialize)]
struct KnowledgeSearchResponse {
    results: Vec<medical_core::storage::KnowledgeSearchHit>,
    total: usize,
    seasonal_context_applied: Vec<String>,
    cached: bool,
}

async fn knowledge_search_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<KnowledgeSearchRequest>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 20).await {
        Ok(a) => a,
        Err(e) => {
            state
                .metrics
                .requests_total
                .with_label_values(&[
                    "/v1/knowledge/search",
                    "POST",
                    &e.status.as_u16().to_string(),
                ])
                .inc();
            return auth_err_response(e);
        }
    };

    if req.query.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "query must not be empty");
    }

    let cache_key = format!(
        "ksearch:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        auth.namespace,
        req.query.trim(),
        req.keywords.join(","),
        req.department.as_deref().unwrap_or(""),
        req.region_code.as_deref().unwrap_or(""),
        req.season.as_deref().unwrap_or(""),
        req.seasonal_boost,
        req.top_k,
        serde_json::to_string(&req.target_population).unwrap_or_default()
    );

    if let Ok(Some(cached_json)) = state.stores.session_cache.get_cached_response(&cache_key).await
        && let Ok(mut parsed) = serde_json::from_str::<KnowledgeSearchResponse>(&cached_json)
    {
        state.metrics.cache_hits_total.inc();
        state
            .metrics
            .requests_total
            .with_label_values(&["/v1/knowledge/search", "POST", "200"])
            .inc();
        parsed.cached = true;
        return Json(parsed).into_response();
    }

    let started = Instant::now();
    let active_epidemics = if req.seasonal_boost {
        state
            .stores
            .epidemics
            .list_active(
                req.region_code.as_deref(),
                req.season.as_deref(),
                Utc::now().date_naive(),
            )
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let search_q = KnowledgeSearchQuery {
        query_text: req.query.clone(),
        query_vector: None,
        keywords: req.keywords.clone(),
        target_population: req.target_population.clone(),
        department: req.department.clone(),
        tenant_namespace: Some(auth.namespace.clone()),
        top_k: req.top_k.clamp(1, 50),
    };

    match state
        .hybrid_search
        .search_with_seasonal_boost(&search_q, &active_epidemics, None)
        .await
    {
        Ok((hits, applied)) => {
            state
                .metrics
                .rag_duration_seconds
                .observe(started.elapsed().as_secs_f64());
            state
                .metrics
                .requests_total
                .with_label_values(&["/v1/knowledge/search", "POST", "200"])
                .inc();
            let resp = KnowledgeSearchResponse {
                total: hits.len(),
                results: hits,
                seasonal_context_applied: applied,
                cached: false,
            };
            if let Ok(serialized) = serde_json::to_string(&resp) {
                let _ = state
                    .stores
                    .session_cache
                    .set_cached_response(&cache_key, &serialized, 300)
                    .await;
            }
            Json(resp).into_response()
        }
        Err(e) => {
            state
                .metrics
                .requests_total
                .with_label_values(&["/v1/knowledge/search", "POST", "500"])
                .inc();
            json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// POST /v1/openai/chat/completions (Enhanced OpenAI-Compatible Proxy)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OpenAiProxyMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiProxyRequest {
    #[serde(default = "default_proxy_model")]
    model: String,
    messages: Vec<OpenAiProxyMessage>,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    stream: bool,
    #[allow(dead_code)]
    #[serde(default)]
    patient_id: Option<String>,
}

fn default_proxy_model() -> String {
    "medical-triage".to_string()
}

async fn openai_proxy_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<OpenAiProxyRequest>,
) -> Response {
    let total_chars: usize = req.messages.iter().map(|m| m.content.chars().count()).sum();
    let auth = match authenticate_request(&state, &headers, (total_chars as u32).max(16)).await {
        Ok(a) => a,
        Err(e) => {
            state
                .metrics
                .requests_total
                .with_label_values(&[
                    "/v1/openai/chat/completions",
                    "POST",
                    &e.status.as_u16().to_string(),
                ])
                .inc();
            return auth_err_response(e);
        }
    };

    if req.messages.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "messages must not be empty");
    }

    let combined_user_text: String = req
        .messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if combined_user_text.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "at least one non-empty user message is required",
        );
    }

    // 1. Run lightweight Stage-1-only CPU red-flag check across ALL user turns (< 1ms)
    let red_flag_alert = state.triage_engine.check_red_flag_fast(&combined_user_text);

    let completion_id = format!("chatcmpl-{}", Uuid::new_v4().simple());
    let created = Utc::now().timestamp();
    let model = req.model.clone();

    if red_flag_alert.level == RedFlagLevel::Emergency120 {
        let alert = red_flag_alert
            .alert_message
            .unwrap_or_else(|| "检测到高危急症指征，请立即拨打 120 急救电话！".to_string());
        let final_content = format!("[急诊红线预警 Emergency120] {alert}\n\n{MEDICAL_DISCLAIMER}");

        state
            .metrics
            .requests_total
            .with_label_values(&["/v1/openai/chat/completions", "POST", "200"])
            .inc();

        if req.stream {
            let chunk1 = serde_json::json!({
                "id": completion_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": { "role": "assistant", "content": final_content },
                    "finish_reason": serde_json::Value::Null
                }]
            });
            let chunk2 = serde_json::json!({
                "id": completion_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }]
            });
            let events: Vec<Result<Event, Infallible>> = vec![
                Ok(Event::default().data(chunk1.to_string())),
                Ok(Event::default().data(chunk2.to_string())),
                Ok(Event::default().data("[DONE]")),
            ];
            return Sse::new(stream::iter(events)).into_response();
        }

        return Json(serde_json::json!({
            "id": completion_id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": final_content },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": (total_chars as u32).max(1),
                "completion_tokens": (final_content.chars().count() as u32).max(1),
                "total_tokens": (total_chars + final_content.chars().count()) as u32
            }
        }))
        .into_response();
    }

    // 2. Prepend unoverridable medical safety system prompt + scrub outbound PII
    let mut scrubbed_msgs: Vec<LlmMessage> = Vec::with_capacity(req.messages.len() + 1);
    scrubbed_msgs.push(LlmMessage {
        role: MessageRole::System,
        content: "你是医院智能导诊与健康咨询助手。严禁下达确诊结论，严禁开具具体处方药物剂量或指导患者自行停药/购药；若识别到胸痛大汗、脑卒中、大出血、呼吸困难或自伤等急危重症，必须首要提示立即拨打120急诊就医。".to_string(),
    });
    for m in &req.messages {
        let role = match m.role.as_str() {
            "system" => MessageRole::System,
            "assistant" => MessageRole::Assistant,
            _ => MessageRole::User,
        };
        scrubbed_msgs.push(LlmMessage {
            role,
            content: scrub_pii(&m.content).scrubbed_text,
        });
    }

    let llm_req = LlmRequest {
        messages: scrubbed_msgs,
        tier: ModelTier::Reasoning,
        temperature: req.temperature.or(Some(0.2)),
        max_tokens: req.max_tokens.or(Some(1024)),
        response_format_json: false,
        json_schema: None,
        timeout_ms: Some(10_000),
    };

    state
        .metrics
        .requests_total
        .with_label_values(&["/v1/openai/chat/completions", "POST", "200"])
        .inc();

    if req.stream {
        let (out_tx, out_rx) = mpsc::channel::<Result<Event, Infallible>>(64);
        let state_bg = state.clone();
        let api_key_rec = auth.api_key.clone();
        let cid = completion_id.clone();
        let mdl = model.clone();
        state.metrics.sse_active_connections.inc();

        tokio::spawn(async move {
            let (llm_tx, mut llm_rx) = mpsc::channel::<LlmStreamChunk>(64);
            let fwd_tx = out_tx.clone();
            let cid_fwd = cid.clone();
            let mdl_fwd = mdl.clone();

            let fwd_handle = tokio::spawn(async move {
                let mut sent_any = false;
                let mut sentence_buf = String::new();
                while let Some(chunk) = llm_rx.recv().await {
                    if chunk.delta.is_empty() {
                        continue;
                    }
                    sentence_buf.push_str(&chunk.delta);
                    let should_flush = sentence_buf.ends_with(['。', '！', '？', '\n', '.', '!', '?'])
                        || sentence_buf.chars().count() >= 64;
                    if !should_flush {
                        continue;
                    }
                    let safe_delta = sanitize_medical_output(&sentence_buf, false).sanitized_text;
                    sentence_buf.clear();
                    if safe_delta.is_empty() {
                        continue;
                    }
                    sent_any = true;
                    let payload = serde_json::json!({
                        "id": cid_fwd,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": mdl_fwd,
                        "choices": [{
                            "index": 0,
                            "delta": { "role": "assistant", "content": safe_delta },
                            "finish_reason": serde_json::Value::Null
                        }]
                    });
                    if fwd_tx
                        .send(Ok(Event::default().data(payload.to_string())))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                if !sentence_buf.is_empty() {
                    let safe_delta = sanitize_medical_output(&sentence_buf, false).sanitized_text;
                    if !safe_delta.is_empty() {
                        sent_any = true;
                        let payload = serde_json::json!({
                            "id": cid_fwd,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": mdl_fwd,
                            "choices": [{
                                "index": 0,
                                "delta": { "role": "assistant", "content": safe_delta },
                                "finish_reason": serde_json::Value::Null
                            }]
                        });
                        let _ = fwd_tx
                            .send(Ok(Event::default().data(payload.to_string())))
                            .await;
                    }
                }
                sent_any
            });

            tokio::select! {
                _ = out_tx.closed() => {}
                res = state_bg.model_router.chat_stream(&llm_req, llm_tx) => {
                    let sent_any = fwd_handle.await.unwrap_or(false);
                    match res {
                        Ok(resp) => {
                            if let Some(ref k) = api_key_rec {
                                state_bg.rate_limiter.record_actual_tokens(
                                    &k.key_id,
                                    k.tpm_limit,
                                    resp.usage.completion_tokens,
                                );
                            }
                            if !sent_any && !resp.content.is_empty() {
                                let safe_text = sanitize_medical_output(&resp.content, true).sanitized_text;
                                let chunk1 = serde_json::json!({
                                    "id": cid,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": mdl,
                                    "choices": [{
                                        "index": 0,
                                        "delta": { "role": "assistant", "content": safe_text },
                                        "finish_reason": serde_json::Value::Null
                                    }]
                                });
                                let _ = out_tx.send(Ok(Event::default().data(chunk1.to_string()))).await;
                            } else {
                                let disclaimer_chunk = serde_json::json!({
                                    "id": cid,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": mdl,
                                    "choices": [{
                                        "index": 0,
                                        "delta": { "content": format!("\n\n{MEDICAL_DISCLAIMER}") },
                                        "finish_reason": serde_json::Value::Null
                                    }]
                                });
                                let _ = out_tx.send(Ok(Event::default().data(disclaimer_chunk.to_string()))).await;
                            }
                        }
                        Err(_) => {
                            state_bg.metrics.llm_errors_total.inc();
                            let fallback = sanitize_medical_output(
                                "当前智能推理服务繁忙，建议前往医院全科医学科或对应专科现场评估。",
                                true,
                            )
                            .sanitized_text;
                            let fb_chunk = serde_json::json!({
                                "id": cid,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": mdl,
                                "choices": [{
                                    "index": 0,
                                    "delta": { "role": "assistant", "content": fallback },
                                    "finish_reason": serde_json::Value::Null
                                }]
                            });
                            let _ = out_tx.send(Ok(Event::default().data(fb_chunk.to_string()))).await;
                        }
                    }
                    let stop_chunk = serde_json::json!({
                        "id": cid,
                        "object": "chat.completion.chunk",
                        "created": created,
                        "model": mdl,
                        "choices": [{
                            "index": 0,
                            "delta": {},
                            "finish_reason": "stop"
                        }]
                    });
                    let _ = out_tx.send(Ok(Event::default().data(stop_chunk.to_string()))).await;
                    let _ = out_tx.send(Ok(Event::default().data("[DONE]"))).await;
                }
            }
            state_bg.metrics.sse_active_connections.dec();
        });

        return Sse::new(ReceiverStream::new(out_rx))
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    let raw_answer = match state.model_router.complete_json(&llm_req).await {
        Ok(resp) => {
            if let Some(ref k) = auth.api_key {
                state.rate_limiter.record_actual_tokens(
                    &k.key_id,
                    k.tpm_limit,
                    resp.usage.completion_tokens,
                );
            }
            resp.content
        }
        Err(_) => {
            state.metrics.llm_errors_total.inc();
            "当前智能推理服务繁忙，建议前往医院全科医学科或对应专科现场评估。".to_string()
        }
    };

    let final_content = sanitize_medical_output(&raw_answer, true).sanitized_text;

    Json(serde_json::json!({
        "id": completion_id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": final_content
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": (total_chars as u32).max(1),
            "completion_tokens": (final_content.chars().count() as u32).max(1),
            "total_tokens": (total_chars + final_content.chars().count()) as u32
        }
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// GET /v1/epidemic/current
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EpidemicQueryParams {
    #[serde(default)]
    region_code: Option<String>,
    #[serde(default)]
    season: Option<String>,
}

async fn epidemic_current_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<EpidemicQueryParams>,
) -> Response {
    match state
        .stores
        .epidemics
        .list_active(
            q.region_code.as_deref(),
            q.season.as_deref(),
            Utc::now().date_naive(),
        )
        .await
    {
        Ok(contexts) => {
            state
                .metrics
                .requests_total
                .with_label_values(&["/v1/epidemic/current", "GET", "200"])
                .inc();
            Json(serde_json::json!({ "contexts": contexts })).into_response()
        }
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// GET / PUT / DELETE /v1/patients/{patient_id}/memory
// ---------------------------------------------------------------------------

fn check_patient_access(
    _state: &AppState,
    auth: &AuthContext,
    target_patient_id: &str,
) -> Result<(), AuthError> {
    if auth.is_admin {
        return Ok(());
    }
    if let Some(ref key) = auth.api_key {
        // Tenant API keys are only allowed when explicitly scoped (non-empty tenant_id)
        if !key.tenant_id.trim().is_empty() {
            return Ok(());
        }
        return Err(AuthError::forbidden("API key lacks valid tenant scope"));
    }
    if let Some(bound) = &auth.patient_id {
        if bound == target_patient_id {
            return Ok(());
        }
    }
    Err(AuthError::forbidden(
        "patient JWT may only access its own memory record",
    ))
}

async fn get_patient_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(patient_id): AxumPath<String>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 5).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if let Err(e) = check_patient_access(&state, &auth, &patient_id) {
        return auth_err_response(e);
    }

    match state
        .stores
        .patient_memory
        .get_full_memory(&patient_id, 50)
        .await
    {
        Ok(Some(record)) => {
            state
                .metrics
                .requests_total
                .with_label_values(&["/v1/patients/memory", "GET", "200"])
                .inc();
            Json(record).into_response()
        }
        Ok(None) => json_error(StatusCode::NOT_FOUND, "patient memory not found"),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn put_patient_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(patient_id): AxumPath<String>,
    Json(mut record): Json<PatientRecord>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 10).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if let Err(e) = check_patient_access(&state, &auth, &patient_id) {
        return auth_err_response(e);
    }

    if record.patient_id.trim().is_empty() {
        record.patient_id = patient_id.clone();
    } else if record.patient_id != patient_id {
        return json_error(
            StatusCode::BAD_REQUEST,
            "body patient_id does not match URL path patient_id",
        );
    }

    if let Err(verr) = record.validate() {
        return json_error(StatusCode::BAD_REQUEST, verr.to_string());
    }

    if let Err(e) = state
        .stores
        .patient_memory
        .upsert_l2_profile(
            &patient_id,
            record.consent_long_term_memory,
            &record.l2_profile,
        )
        .await
    {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    for event in &record.l3_visit_events {
        if let Err(e) = state
            .stores
            .patient_memory
            .insert_l3_event(&patient_id, event)
            .await
        {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to persist L3 visit event {}: {e}", event.event_id),
            );
        }
        if let Ok(vec) = state
            .embedder
            .embed_one(&format!("{} {}", event.chief_complaint, event.summary))
            .await
        {
            if let Err(e) = state
                .stores
                .patient_l3_vectors
                .upsert_event_vector(&patient_id, event, vec)
                .await
            {
                return json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "failed to index L3 vector for event {}: {e}",
                        event.event_id
                    ),
                );
            }
        }
    }

    state
        .metrics
        .requests_total
        .with_label_values(&["/v1/patients/memory", "PUT", "200"])
        .inc();

    Json(record).into_response()
}

async fn delete_patient_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(patient_id): AxumPath<String>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 5).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if let Err(e) = check_patient_access(&state, &auth, &patient_id) {
        return auth_err_response(e);
    }

    let mut stats = match state
        .stores
        .patient_memory
        .delete_patient_memory(&patient_id)
        .await
    {
        Ok(s) => s,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    match state
        .stores
        .patient_l3_vectors
        .delete_patient_events(&patient_id)
        .await
    {
        Ok(q_del) => stats.qdrant_points_deleted = q_del,
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to delete patient vector memory: {e}"),
            );
        }
    }
    match state
        .stores
        .session_cache
        .delete_patient_sessions(&patient_id)
        .await
    {
        Ok(l1_del) => stats.l1_sessions_deleted = l1_del,
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to delete patient L1 sessions: {e}"),
            );
        }
    }

    state
        .metrics
        .requests_total
        .with_label_values(&["/v1/patients/memory", "DELETE", "200"])
        .inc();

    Json(serde_json::json!({
        "deleted": true,
        "patient_id": patient_id,
        "stats": stats,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// POST /v1/admin/reload & POST /v1/admin/epidemics
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AdminReloadRequest {
    #[serde(default = "default_true")]
    reload_epidemics: bool,
    #[serde(default = "default_true")]
    reload_dictionaries: bool,
    #[serde(default = "default_true")]
    reload_router: bool,
}

async fn admin_reload_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 1).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }
    let req: AdminReloadRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")),
    };

    match state
        .reload_hot_resources(
            req.reload_epidemics,
            req.reload_dictionaries,
            req.reload_router,
        )
        .await
    {
        Ok(()) => {
            state
                .metrics
                .requests_total
                .with_label_values(&["/v1/admin/reload", "POST", "200"])
                .inc();
            Json(serde_json::json!({
                "reloaded": true,
                "reload_epidemics": req.reload_epidemics,
                "reload_dictionaries": req.reload_dictionaries,
                "reload_router": req.reload_router,
            }))
            .into_response()
        }
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn admin_upsert_epidemic_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 1).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }
    let ctx: EpidemicContext = match serde_json::from_slice(&body) {
        Ok(c) => c,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")),
    };
    if let Err(verr) = ctx.validate() {
        return json_error(StatusCode::BAD_REQUEST, verr.to_string());
    }
    if let Err(e) = state.stores.epidemics.upsert(&ctx).await {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let _ = state.reload_hot_resources(true, false, false).await;
    Json(serde_json::json!({ "saved": true, "context_id": ctx.context_id })).into_response()
}

// ---------------------------------------------------------------------------
// GET /v1/clinical/types (Standardized Clinical Data Type Catalog)
// ---------------------------------------------------------------------------

async fn clinical_types_catalog_handler() -> impl IntoResponse {
    Json(default_clinical_data_catalog())
}

// ---------------------------------------------------------------------------
// POST /v1/knowledge/import, GET & DELETE /v1/knowledge/documents
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct KnowledgeImportRequest {
    documents: Vec<KnowledgeDocument>,
    #[serde(default)]
    tenant_namespace: Option<String>,
}

async fn knowledge_import_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<KnowledgeImportRequest>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 20).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin && auth.api_key.is_none() {
        return auth_err_response(AuthError::forbidden(
            "admin or tenant API key required for knowledge import",
        ));
    }
    if req.documents.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "documents must not be empty");
    }
    for (idx, doc) in req.documents.iter().enumerate() {
        if let Err(verr) = doc.validate() {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("documents[{idx}] validation failed: {verr}"),
            );
        }
    }
    let ns = req
        .tenant_namespace
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&auth.namespace)
        .to_string();

    match state.indexer.upsert_documents(&req.documents, &ns).await {
        Ok(written) => Json(serde_json::json!({
            "imported": written,
            "tenant_namespace": ns,
            "total_docs_in_index": state.tantivy.num_docs(),
        }))
        .into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct KnowledgeListQuery {
    #[serde(default)]
    department: Option<String>,
    #[serde(default)]
    keyword: Option<String>,
    #[serde(default = "default_list_limit")]
    limit: usize,
}

fn default_list_limit() -> usize {
    50
}

async fn knowledge_list_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<KnowledgeListQuery>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 5).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    let ns = if auth.is_admin {
        None
    } else {
        Some(auth.namespace.as_str())
    };
    match state.tantivy.list_all_documents(ns) {
        Ok(mut docs) => {
            if let Some(dept) = q.department.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                docs.retain(|d| d.department == dept || d.secondary_departments.iter().any(|s| s == dept));
            }
            if let Some(kw) = q.keyword.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                let kw_lower = kw.to_ascii_lowercase();
                docs.retain(|d| {
                    d.disease_name.to_ascii_lowercase().contains(&kw_lower)
                        || d.content.to_ascii_lowercase().contains(&kw_lower)
                        || d.doc_id.to_ascii_lowercase().contains(&kw_lower)
                });
            }
            let total = docs.len();
            docs.truncate(q.limit.clamp(1, 500));
            Json(serde_json::json!({
                "total": total,
                "documents": docs,
            }))
            .into_response()
        }
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct KnowledgeDeleteRequest {
    doc_ids: Vec<String>,
    #[serde(default)]
    tenant_namespace: Option<String>,
}

async fn knowledge_delete_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<KnowledgeDeleteRequest>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 5).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }
    if req.doc_ids.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "doc_ids must not be empty");
    }
    let ns = req
        .tenant_namespace
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    if let Err(e) = state
        .tantivy
        .delete_by_doc_ids_in_namespace(&req.doc_ids, ns)
    {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let _ = state
        .stores
        .knowledge_vectors
        .delete_by_doc_ids(&req.doc_ids)
        .await;
    Json(serde_json::json!({
        "deleted": req.doc_ids.len(),
        "doc_ids": req.doc_ids,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// GET /v1/patients, POST /v1/patients/import, POST /v1/patients/{id}/clinical-exams
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListPatientsQuery {
    #[serde(default = "default_list_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

async fn list_patients_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListPatientsQuery>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 5).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin && auth.api_key.is_none() {
        return auth_err_response(AuthError::forbidden(
            "admin or tenant API key required to list patients",
        ));
    }
    match state
        .stores
        .patient_memory
        .list_patient_ids(q.limit.clamp(1, 500), q.offset)
        .await
    {
        Ok(ids) => Json(serde_json::json!({
            "total": ids.len(),
            "patient_ids": ids,
        }))
        .into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct AttachClinicalExamsRequest {
    #[serde(default)]
    event_id: Option<String>,
    #[serde(default)]
    chief_complaint: Option<String>,
    #[serde(default)]
    recommended_department: Option<String>,
    #[serde(default)]
    urgency_level: Option<u8>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    baseline_vitals: Option<VitalSignsRecord>,
    #[serde(default)]
    l2_trigger_substances: Vec<TriggerSubstanceRecord>,
    #[serde(default)]
    clinical_exams: ClinicalExamBundle,
}

async fn attach_patient_clinical_exams_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(patient_id): AxumPath<String>,
    Json(req): Json<AttachClinicalExamsRequest>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 10).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if let Err(e) = check_patient_access(&state, &auth, &patient_id) {
        return auth_err_response(e);
    }
    if let Err(verr) = validate_patient_id(&patient_id) {
        return json_error(StatusCode::BAD_REQUEST, verr.message);
    }
    if let Err(verr) = req.clinical_exams.validate() {
        return json_error(StatusCode::BAD_REQUEST, verr.to_string());
    }
    if let Some(ref bv) = req.baseline_vitals
        && let Err(verr) = bv.validate()
    {
        return json_error(StatusCode::BAD_REQUEST, verr.to_string());
    }
    for (idx, t) in req.l2_trigger_substances.iter().enumerate() {
        if let Err(verr) = t.validate() {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("l2_trigger_substances[{idx}]: {verr}"),
            );
        }
    }

    // Ensure L2 profile exists and update baseline vitals / trigger substances if provided
    let existing_l2 = state
        .stores
        .patient_memory
        .get_l2_profile(&patient_id)
        .await
        .ok()
        .flatten();
    let (consent, mut profile) = match existing_l2 {
        Some(rec) => (rec.consent_long_term_memory, rec.l2_profile),
        None => (
            true,
            L2Profile {
                age_group: "unknown".to_string(),
                ..Default::default()
            },
        ),
    };
    if let Some(bv) = req.baseline_vitals {
        profile.baseline_vitals = Some(bv);
    } else if profile.baseline_vitals.is_none()
        && let Some(first_vs) = req.clinical_exams.vital_signs.first()
    {
        profile.baseline_vitals = Some(first_vs.clone());
    }
    for t in req.l2_trigger_substances {
        if let Some(existing) = profile
            .trigger_substances
            .iter_mut()
            .find(|x| x.trigger_id == t.trigger_id)
        {
            *existing = t;
        } else {
            profile.trigger_substances.push(t);
        }
    }
    for t in &req.clinical_exams.trigger_substances {
        if !profile
            .trigger_substances
            .iter()
            .any(|x| x.trigger_id == t.trigger_id)
        {
            profile.trigger_substances.push(t.clone());
        }
    }
    if let Err(e) = state
        .stores
        .patient_memory
        .upsert_l2_profile(&patient_id, consent, &profile)
        .await
    {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    // Attach to existing event_id or create a new L3 visit event
    let target_event = if let Some(ref eid) = req.event_id
        && let Ok(events) = state
            .stores
            .patient_memory
            .list_l3_events(&patient_id, 100)
            .await
        && let Some(mut found) = events.into_iter().find(|e| &e.event_id == eid)
    {
        found.clinical_exams.merge(req.clinical_exams);
        found
    } else {
        let exam_summary = req.clinical_exams.to_searchable_text();
        let cc = req
            .chief_complaint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| scrub_pii(s).scrubbed_text)
            .unwrap_or_else(|| "医学检查与生化/影像/创面记录".to_string());
        let summary = req
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| scrub_pii(s).scrubbed_text)
            .unwrap_or_else(|| {
                if exam_summary.is_empty() {
                    cc.clone()
                } else {
                    format!("{cc}：{exam_summary}")
                }
            });
        L3VisitEvent {
            event_id: req
                .event_id
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("evt_exam_{}", Uuid::new_v4().simple())),
            occurred_at: Utc::now(),
            chief_complaint: cc,
            extracted_symptoms: Vec::new(),
            recommended_department: req
                .recommended_department
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "全科医学科".to_string()),
            urgency_level: req.urgency_level.unwrap_or(2).clamp(1, 4),
            summary,
            clinical_exams: req.clinical_exams,
        }
    };

    if let Err(e) = state
        .stores
        .patient_memory
        .insert_l3_event(&patient_id, &target_event)
        .await
    {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let embed_text = format!(
        "{} {} {}",
        target_event.chief_complaint,
        target_event.summary,
        target_event.clinical_exams.to_searchable_text()
    );
    if let Ok(vec) = state.embedder.embed_one(&embed_text).await {
        let _ = state
            .stores
            .patient_l3_vectors
            .upsert_event_vector(&patient_id, &target_event, vec)
            .await;
    }

    Json(serde_json::json!({
        "saved": true,
        "patient_id": patient_id,
        "event": target_event,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct BatchPatientImportRequest {
    /// "json" or "csv"
    format: String,
    content: String,
    #[serde(default)]
    default_consent: bool,
}

async fn import_patients_batch_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<BatchPatientImportRequest>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 20).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin && auth.api_key.is_none() {
        return auth_err_response(AuthError::forbidden(
            "admin or tenant API key required for batch patient import",
        ));
    }
    let cfg = PatientMaskingConfig {
        salt: state.config.patient_id_salt.clone(),
        default_consent: req.default_consent,
    };
    let report = match req.format.trim().to_ascii_lowercase().as_str() {
        "json" => import_patients_json_str(&req.content, &cfg),
        "csv" => import_patients_csv_str(&req.content, &cfg),
        other => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("unsupported format '{other}', expected 'json' or 'csv'"),
            );
        }
    };
    let report = match report {
        Ok(r) => r,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, e.to_string()),
    };

    let mut persisted_patients = 0usize;
    let mut persisted_events = 0usize;
    for rec in &report.records {
        if state
            .stores
            .patient_memory
            .upsert_l2_profile(
                &rec.patient_id,
                rec.consent_long_term_memory,
                &rec.l2_profile,
            )
            .await
            .is_ok()
        {
            persisted_patients += 1;
        }
        for ev in &rec.l3_visit_events {
            if state
                .stores
                .patient_memory
                .insert_l3_event(&rec.patient_id, ev)
                .await
                .is_ok()
            {
                persisted_events += 1;
            }
        }
    }

    Json(serde_json::json!({
        "persisted_patients": persisted_patients,
        "persisted_events": persisted_events,
        "total_input_rows": report.total_input_rows,
        "detected_pii_columns": report.detected_pii_columns,
        "failed_rows": report.failed_rows,
        "patient_ids": report.records.iter().map(|r| &r.patient_id).collect::<Vec<_>>(),
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// GET /v1/dictionaries, POST /v1/dictionaries/synonyms & /red-flags
// ---------------------------------------------------------------------------

async fn get_dictionaries_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = authenticate_request(&state, &headers, 5).await {
        return auth_err_response(e);
    }
    match state.stores.dictionaries.load_dictionaries().await {
        Ok(dict) => Json(dict).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn upsert_synonym_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(syn): Json<SymptomSynonym>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 5).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }
    if let Err(verr) = syn.validate() {
        return json_error(StatusCode::BAD_REQUEST, verr.to_string());
    }
    if let Err(e) = state.stores.dictionaries.save_symptom_synonym(&syn).await {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let _ = state.reload_hot_resources(false, true, false).await;
    Json(serde_json::json!({ "saved": true, "standard_term": syn.standard_term })).into_response()
}

async fn upsert_red_flag_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(rule): Json<RedFlagRule>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 5).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }
    if let Err(verr) = rule.validate() {
        return json_error(StatusCode::BAD_REQUEST, verr.to_string());
    }
    if let Err(e) = state.stores.dictionaries.save_red_flag_rule(&rule).await {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let _ = state.reload_hot_resources(false, true, false).await;
    Json(serde_json::json!({ "saved": true, "rule_id": rule.rule_id })).into_response()
}

async fn admin_save_snapshot_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 1).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }
    match state.save_snapshot_now().await {
        Ok(Some(count)) => Json(serde_json::json!({
            "saved": true,
            "mode": "local_encrypted_snapshot",
            "records_saved": count,
            "path": state.config.local_snapshot_path.as_ref().map(|p| p.display().to_string()),
        }))
        .into_response(),
        Ok(None) => Json(serde_json::json!({
            "saved": true,
            "mode": "external_database_or_ephemeral",
            "records_saved": 0,
        }))
        .into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

// ---------------------------------------------------------------------------
// Modern Hospital HIS / LIS / PACS / Mobile Cloud-Film / HL7 FHIR R4 Integration
// ---------------------------------------------------------------------------

async fn list_hospital_connectors_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = authenticate_request(&state, &headers, 5).await {
        return auth_err_response(e);
    }
    let connectors: Vec<HospitalConnectorConfig> = state
        .hospital_connectors
        .iter()
        .map(|entry| {
            let mut c = entry.value().clone();
            if c.api_secret.is_some() {
                c.api_secret = Some("***REDACTED***".to_string());
            }
            c
        })
        .collect();
    Json(serde_json::json!({
        "total": connectors.len(),
        "connectors": connectors,
    }))
    .into_response()
}

async fn upsert_hospital_connector_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(cfg): Json<HospitalConnectorConfig>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 5).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }
    if cfg.connector_id.trim().is_empty() || cfg.hospital_name.trim().is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "connector_id and hospital_name must not be empty",
        );
    }
    let cid = cfg.connector_id.clone();
    state.hospital_connectors.insert(cid.clone(), cfg);
    Json(serde_json::json!({ "saved": true, "connector_id": cid })).into_response()
}

async fn delete_hospital_connector_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(connector_id): AxumPath<String>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 5).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }
    let deleted = state.hospital_connectors.remove(&connector_id).is_some();
    Json(serde_json::json!({ "deleted": deleted, "connector_id": connector_id })).into_response()
}

async fn persist_hospital_bundle_to_patient(
    state: &AppState,
    patient_id: &str,
    hospital_name: &str,
    department: &str,
    report_no: &str,
    summary_raw: &str,
    bundle: ClinicalExamBundle,
) -> Result<String, String> {
    let existing_l2 = state
        .stores
        .patient_memory
        .get_l2_profile(patient_id)
        .await
        .ok()
        .flatten();
    let (consent, mut profile) = match existing_l2 {
        Some(rec) => (rec.consent_long_term_memory, rec.l2_profile),
        None => (
            true,
            L2Profile {
                age_group: "unknown".to_string(),
                ..Default::default()
            },
        ),
    };
    if profile.baseline_vitals.is_none()
        && let Some(vs) = bundle.vital_signs.first()
    {
        profile.baseline_vitals = Some(vs.clone());
    }
    for t in &bundle.trigger_substances {
        if !profile
            .trigger_substances
            .iter()
            .any(|x| x.trigger_id == t.trigger_id)
        {
            profile.trigger_substances.push(t.clone());
        }
    }
    state
        .stores
        .patient_memory
        .upsert_l2_profile(patient_id, consent, &profile)
        .await
        .map_err(|e| e.to_string())?;

    let event_id = format!("evt_hosp_{report_no}");
    let exam_text = bundle.to_searchable_text();
    let scrubbed_summary = scrub_pii(if summary_raw.trim().is_empty() {
        &exam_text
    } else {
        summary_raw
    })
    .scrubbed_text;
    let event = L3VisitEvent {
        event_id: event_id.clone(),
        occurred_at: Utc::now(),
        chief_complaint: format!("[医院报告同步·{hospital_name}] 单号 {report_no}"),
        extracted_symptoms: Vec::new(),
        recommended_department: if department.trim().is_empty() {
            "全科医学科".to_string()
        } else {
            department.to_string()
        },
        urgency_level: 3,
        summary: if scrubbed_summary.trim().is_empty() {
            format!("来自{hospital_name}的电子检查/检验报告（单号 {report_no}）")
        } else {
            scrubbed_summary
        },
        clinical_exams: bundle,
    };
    state
        .stores
        .patient_memory
        .insert_l3_event(patient_id, &event)
        .await
        .map_err(|e| e.to_string())?;
    let embed_text = format!("{} {}", event.chief_complaint, event.summary);
    if let Ok(vec) = state.embedder.embed_one(&embed_text).await {
        let _ = state
            .stores
            .patient_l3_vectors
            .upsert_event_vector(patient_id, &event, vec)
            .await;
    }
    Ok(event_id)
}

async fn pull_hospital_report_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<HospitalReportPullRequest>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 10).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if let Err(e) = check_patient_access(&state, &auth, &req.patient_id) {
        return auth_err_response(e);
    }
    if let Err(verr) = validate_patient_id(&req.patient_id) {
        return json_error(StatusCode::BAD_REQUEST, verr.message);
    }

    let qr_hint = req
        .qr_code_payload
        .as_deref()
        .map(parse_mobile_report_qr_payload)
        .unwrap_or_default();

    let effective_report_no = req
        .report_no
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or(qr_hint.report_no.clone())
        .unwrap_or_else(|| format!("RPT{}", Uuid::new_v4().simple().to_string()[..8].to_uppercase()));

    let effective_hosp_code = req
        .hospital_code
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or(qr_hint.hospital_code.clone());

    // Look up matching hospital connector if registered
    let connector = req
        .connector_id
        .as_deref()
        .and_then(|cid| state.hospital_connectors.get(cid).map(|r| r.value().clone()))
        .or_else(|| {
            effective_hosp_code.as_deref().and_then(|hc| {
                state
                    .hospital_connectors
                    .iter()
                    .find(|e| e.value().enabled && e.value().hospital_code == hc)
                    .map(|e| e.value().clone())
            })
        });

    let mut imported_reports = Vec::new();
    let mut merged_exams = ClinicalExamBundle::default();
    let mut pull_mode = "qr_sandbox_preview".to_string();
    let mut source_hospital = connector
        .as_ref()
        .map(|c| c.hospital_name.clone())
        .unwrap_or_else(|| "医院电子报告对接接口(预留适配器)".to_string());
    let connector_id = connector
        .as_ref()
        .map(|c| c.connector_id.clone())
        .unwrap_or_else(|| "builtin_qr_fhir_adapter".to_string());

    // 1. Live hospital HTTP / FHIR pull when an enabled connector with a real base_url is configured
    if let Some(ref conn) = connector
        && conn.enabled
        && !conn.base_url.trim().is_empty()
        && !conn.base_url.starts_with("mock://")
    {
        let endpoint = format!(
            "{}/reports/pull",
            conn.base_url.trim_end_matches('/')
        );
        let mut http_req = state
            .http_client
            .post(&endpoint)
            .timeout(std::time::Duration::from_millis(conn.timeout_ms.max(1000)))
            .json(&serde_json::json!({
                "report_no": effective_report_no,
                "qr_code_payload": req.qr_code_payload,
                "visit_no": req.visit_no,
                "report_types": req.report_types,
                "date_from": req.date_from,
                "date_to": req.date_to,
            }));
        if let Some(ref secret) = conn.api_secret {
            match conn.auth_mode {
                HospitalAuthMode::Bearer => {
                    http_req = http_req.header(header::AUTHORIZATION, format!("Bearer {secret}"));
                }
                HospitalAuthMode::ApiKey => {
                    http_req = http_req.header("x-api-key", secret);
                }
                _ => {}
            }
        }
        match http_req.send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(val) = resp.json::<serde_json::Value>().await {
                    if val.get("resourceType").and_then(|v| v.as_str()) == Some("Bundle") {
                        let bundle = convert_fhir_bundle_to_clinical_exams(&val);
                        merged_exams.merge(bundle.clone());
                        imported_reports.push(HospitalExternalReport {
                            report_no: effective_report_no.clone(),
                            hospital_name: conn.hospital_name.clone(),
                            department: "医院检验/影像科".to_string(),
                            report_type: "comprehensive".to_string(),
                            reported_at: Some(Utc::now()),
                            cloud_film_url: qr_hint.cloud_film_url.clone(),
                            report_pdf_url: None,
                            summary: bundle.to_searchable_text(),
                            clinical_exams: bundle,
                        });
                        pull_mode = "fhir_bundle_import".to_string();
                    } else if let Ok(ext_report) =
                        serde_json::from_value::<HospitalExternalReport>(val.clone())
                    {
                        merged_exams.merge(ext_report.clinical_exams.clone());
                        source_hospital = ext_report.hospital_name.clone();
                        imported_reports.push(ext_report);
                        pull_mode = "live_hospital_api".to_string();
                    }
                }
            }
            Ok(resp) => {
                return json_error(
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "hospital connector {} returned HTTP {}",
                        conn.connector_id,
                        resp.status().as_u16()
                    ),
                );
            }
            Err(e) => {
                return json_error(
                    StatusCode::BAD_GATEWAY,
                    format!("failed to reach hospital connector {}: {e}", conn.connector_id),
                );
            }
        }
    }

    // 2. Fallback / QR cloud-film link parser when no live hospital server is wired yet
    if imported_reports.is_empty() {
        let mut bundle = ClinicalExamBundle::default();
        if let Some(ref url) = qr_hint.cloud_film_url {
            let modality = match qr_hint
                .report_type
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "mri" => ImagingModality::Mri,
                "ct" => ImagingModality::Ct,
                "us" | "ultrasound" => ImagingModality::Ultrasound,
                "ecg" => ImagingModality::Ecg,
                "xray" | "dr" => ImagingModality::Xray,
                _ => ImagingModality::Other,
            };
            let media = ClinicalMediaAsset {
                asset_id: format!("cloudfilm_{effective_report_no}"),
                category: if modality == ImagingModality::Mri {
                    ClinicalMediaCategory::MriImage
                } else {
                    ClinicalMediaCategory::LabReportImage
                },
                title: format!("医院手机电子报告/云胶片 ({effective_report_no})"),
                mime_type: "text/uri-list".to_string(),
                uri_or_path: Some(url.clone()),
                data_base64: None,
                sha256: None,
                body_site: None,
                captured_at: Some(Utc::now()),
                caption: Some(format!("来源链接: {url}")),
                tags: vec!["医院电子报告".to_string(), "云胶片".to_string()],
            };
            bundle.media_assets.push(media.clone());
            bundle.imaging_exams.push(ImagingExamRecord {
                exam_id: format!("exam_qr_{effective_report_no}"),
                modality,
                exam_name: format!("手机扫码关联报告 ({effective_report_no})"),
                body_part: String::new(),
                examined_at: Some(Utc::now()),
                findings: format!("已关联医院电子报告/云胶片链接：{url}"),
                impression: "待院内 LIS/PACS 正式授权连接器拉取完整结构化正文".to_string(),
                contrast_used: None,
                images: vec![media],
            });
        }
        merged_exams.merge(bundle.clone());
        imported_reports.push(HospitalExternalReport {
            report_no: effective_report_no.clone(),
            hospital_name: source_hospital.clone(),
            department: "检验/影像科".to_string(),
            report_type: qr_hint
                .report_type
                .unwrap_or_else(|| "comprehensive".to_string()),
            reported_at: Some(Utc::now()),
            cloud_film_url: qr_hint.cloud_film_url,
            report_pdf_url: None,
            summary: if bundle.is_empty() {
                format!("已登记医院报告单号 {effective_report_no}，等待配置院内 HIS/LIS/PACS 连接器后自动同步")
            } else {
                bundle.to_searchable_text()
            },
            clinical_exams: bundle,
        });
    }

    let attached_event_id = if req.auto_attach_to_patient {
        let summary_text = imported_reports
            .first()
            .map(|r| r.summary.as_str())
            .unwrap_or("");
        let dept = imported_reports
            .first()
            .map(|r| r.department.as_str())
            .unwrap_or("全科医学科");
        match persist_hospital_bundle_to_patient(
            &state,
            &req.patient_id,
            &source_hospital,
            dept,
            &effective_report_no,
            summary_text,
            merged_exams.clone(),
        )
        .await
        {
            Ok(eid) => Some(eid),
            Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
        }
    } else {
        None
    };

    Json(HospitalReportPullResponse {
        source_hospital,
        connector_id,
        pull_mode,
        imported_reports,
        merged_clinical_exams: merged_exams,
        attached_event_id,
        pii_scrubbed: true,
    })
    .into_response()
}

async fn hospital_report_webhook_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<HospitalWebhookPushPayload>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 10).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin && auth.api_key.is_none() {
        return auth_err_response(AuthError::forbidden(
            "admin or hospital API key required for webhook push",
        ));
    }
    if let Err(verr) = validate_patient_id(&payload.patient_id) {
        return json_error(StatusCode::BAD_REQUEST, verr.message);
    }

    let mut bundle = payload.clinical_exams;
    if let Some(ref fhir) = payload.fhir_bundle {
        bundle.merge(convert_fhir_bundle_to_clinical_exams(fhir));
    }
    if let Some(ref url) = payload.cloud_film_url {
        bundle.media_assets.push(ClinicalMediaAsset {
            asset_id: format!("webhook_film_{}", payload.report_no),
            category: ClinicalMediaCategory::MriImage,
            title: format!("{} 云胶片 ({})", payload.hospital_name, payload.report_no),
            mime_type: "text/uri-list".to_string(),
            uri_or_path: Some(url.clone()),
            data_base64: None,
            sha256: None,
            body_site: None,
            captured_at: Some(Utc::now()),
            caption: payload.summary.clone(),
            tags: vec!["云胶片".to_string(), payload.hospital_code.clone()],
        });
    }

    match persist_hospital_bundle_to_patient(
        &state,
        &payload.patient_id,
        &payload.hospital_name,
        payload.department.as_deref().unwrap_or("检验/影像科"),
        &payload.report_no,
        payload.summary.as_deref().unwrap_or(""),
        bundle,
    )
    .await
    {
        Ok(event_id) => Json(serde_json::json!({
            "accepted": true,
            "patient_id": payload.patient_id,
            "report_no": payload.report_no,
            "event_id": event_id,
            "pii_scrubbed": true,
        }))
        .into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Debug, Deserialize)]
struct FhirImportRequest {
    patient_id: String,
    #[serde(default = "default_fhir_hospital")]
    hospital_name: String,
    #[serde(default)]
    report_no: Option<String>,
    #[serde(default)]
    department: Option<String>,
    fhir_bundle: serde_json::Value,
}

fn default_fhir_hospital() -> String {
    "HL7 FHIR 医院电子报告".to_string()
}

async fn import_fhir_bundle_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<FhirImportRequest>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 10).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if let Err(e) = check_patient_access(&state, &auth, &req.patient_id) {
        return auth_err_response(e);
    }
    if let Err(verr) = validate_patient_id(&req.patient_id) {
        return json_error(StatusCode::BAD_REQUEST, verr.message);
    }

    let bundle = convert_fhir_bundle_to_clinical_exams(&req.fhir_bundle);
    let report_no = req
        .report_no
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("FHIR{}", Uuid::new_v4().simple().to_string()[..8].to_uppercase()));

    match persist_hospital_bundle_to_patient(
        &state,
        &req.patient_id,
        &req.hospital_name,
        req.department.as_deref().unwrap_or("检验/影像科"),
        &report_no,
        &bundle.to_searchable_text(),
        bundle.clone(),
    )
    .await
    {
        Ok(event_id) => Json(serde_json::json!({
            "imported": true,
            "patient_id": req.patient_id,
            "report_no": report_no,
            "event_id": event_id,
            "clinical_exams": bundle,
        }))
        .into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

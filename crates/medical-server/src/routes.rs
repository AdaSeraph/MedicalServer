use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{DefaultBodyLimit, Path as AxumPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures::stream;
use medical_core::crypto::hash_patient_id;
use medical_core::ecs::{TriageRequest, TriageStreamEvent};
use medical_core::llm::{LlmMessage, LlmRequest, LlmStreamChunk, MessageRole, ModelTier};
use medical_core::models::{
    EpidemicContext, PatientRecord, RedFlagLevel, TargetPopulation,
};
use medical_core::privacy::{MEDICAL_DISCLAIMER, sanitize_medical_output, scrub_pii};
use medical_core::storage::KnowledgeSearchQuery;
use medical_core::validation::Validate;
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
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .route("/v1/auth/wechat", post(wechat_auth_handler))
        .route("/v1/triage/chat", post(triage_chat_handler))
        .route("/v1/knowledge/search", post(knowledge_search_handler))
        .route(
            "/v1/openai/chat/completions",
            post(openai_proxy_handler),
        )
        .route("/v1/epidemic/current", get(epidemic_current_handler))
        .route(
            "/v1/patients/{patient_id}/memory",
            get(get_patient_memory_handler)
                .put(put_patient_memory_handler)
                .delete(delete_patient_memory_handler),
        )
        .route("/v1/admin/reload", post(admin_reload_handler))
        .route("/v1/admin/epidemics", post(admin_upsert_epidemic_handler))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
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

    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "checks": {
            "storage": if state.config.offline_mode { "in_memory_ok" } else { "pg_ok" },
            "knowledge_index": format!("tantivy_docs:{}", state.tantivy.num_docs()),
            "llm_router": "ready",
            "offline_mode": state.config.offline_mode
        }
    }))
}

async fn metrics_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let body = state.metrics.render_text();
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
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
    let prior_session = state
        .stores
        .session_cache
        .get_session(&req.session_id)
        .await
        .ok()
        .flatten();

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
                                state_bg.rate_limiter.record_actual_tokens(
                                    &k.key_id,
                                    k.tpm_limit,
                                    exec.trace.llm_calls_made.saturating_mul(128),
                                );
                            }
                            let _ = tx.send(TriageStreamEvent::Result(exec.response)).await;
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
        "ksearch:{}:{}:{}:{}:{}:{}:{}",
        auth.namespace,
        req.query.trim(),
        req.department.as_deref().unwrap_or(""),
        req.region_code.as_deref().unwrap_or(""),
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

    let last_user_text = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or(&req.messages.last().unwrap().content);

    // 1. Run lightweight Stage-1-only CPU red-flag check (< 1ms, zero I/O / LLM overhead)
    let red_flag_alert = state.triage_engine.check_red_flag_fast(last_user_text);

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

    // 2. Scrubbed outbound LLM call
    let scrubbed_msgs: Vec<LlmMessage> = req
        .messages
        .iter()
        .map(|m| {
            let role = match m.role.as_str() {
                "system" => MessageRole::System,
                "assistant" => MessageRole::Assistant,
                _ => MessageRole::User,
            };
            LlmMessage {
                role,
                content: scrub_pii(&m.content).scrubbed_text,
            }
        })
        .collect();

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
                while let Some(chunk) = llm_rx.recv().await {
                    if chunk.delta.is_empty() {
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
                            "delta": { "role": "assistant", "content": chunk.delta },
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
    state: &AppState,
    auth: &AuthContext,
    target_patient_id: &str,
) -> Result<(), AuthError> {
    if auth.is_admin || auth.api_key.is_some() {
        return Ok(());
    }
    if let Some(bound) = &auth.patient_id {
        if bound == target_patient_id {
            return Ok(());
        }
        if state.config.offline_mode && target_patient_id.starts_with("pat_sha256_") {
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
        let _ = state
            .stores
            .patient_memory
            .insert_l3_event(&patient_id, event)
            .await;
        if let Ok(vec) = state
            .embedder
            .embed_one(&format!("{} {}", event.chief_complaint, event.summary))
            .await
        {
            let _ = state
                .stores
                .patient_l3_vectors
                .upsert_event_vector(&patient_id, event, vec)
                .await;
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

    if let Ok(q_del) = state
        .stores
        .patient_l3_vectors
        .delete_patient_events(&patient_id)
        .await
    {
        stats.qdrant_points_deleted = q_del;
    }
    if let Ok(l1_del) = state
        .stores
        .session_cache
        .delete_patient_sessions(&patient_id)
        .await
    {
        stats.l1_sessions_deleted = l1_del;
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
    Json(req): Json<AdminReloadRequest>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 1).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin && auth.api_key.is_none() {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }

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
    Json(ctx): Json<EpidemicContext>,
) -> Response {
    let auth = match authenticate_request(&state, &headers, 1).await {
        Ok(a) => a,
        Err(e) => return auth_err_response(e),
    };
    if !auth.is_admin && auth.api_key.is_none() {
        return auth_err_response(AuthError::forbidden("admin authorization required"));
    }
    if let Err(verr) = ctx.validate() {
        return json_error(StatusCode::BAD_REQUEST, verr.to_string());
    }
    if let Err(e) = state.stores.epidemics.upsert(&ctx).await {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let _ = state.reload_hot_resources(true, false, false).await;
    Json(serde_json::json!({ "saved": true, "context_id": ctx.context_id })).into_response()
}

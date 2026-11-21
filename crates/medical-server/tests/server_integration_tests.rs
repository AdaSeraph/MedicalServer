use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use medical_core::storage::IssueApiKeyRequest;
use medical_server::{AppState, ServerConfig, build_router};
use tower::ServiceExt;

async fn test_app() -> (axum::Router, std::sync::Arc<AppState>) {
    let cfg = ServerConfig::default();
    let state = AppState::build(cfg).await.expect("build offline AppState");
    let router = build_router(state.clone());
    (router, state)
}

async fn send_json(
    app: &axum::Router,
    method: Method,
    uri: &str,
    bearer: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, String, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(tok) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {tok}"));
    }
    let req = if let Some(json) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    (status, content_type, text)
}

#[tokio::test]
async fn test_health_metrics_and_wechat_auth_flow() {
    let (app, _) = test_app().await;

    let (st, _, body) = send_json(&app, Method::GET, "/health", None, None).await;
    assert_eq!(st, StatusCode::OK);
    let h: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(h["status"], "ok");
    assert!(h["checks"].is_object());

    let (st, _, metrics) = send_json(&app, Method::GET, "/metrics", None, None).await;
    assert_eq!(st, StatusCode::OK);
    for required in [
        "medical_server_requests_total",
        "medical_server_sse_active_connections",
        "medical_server_triage_duration_seconds",
        "medical_server_llm_errors_total",
        "medical_server_rag_duration_seconds",
        "medical_server_cache_hits_total",
        "medical_server_pg_pool_active",
    ] {
        assert!(metrics.contains(required), "missing metric {required}");
    }

    let (st, _, auth_body) = send_json(
        &app,
        Method::POST,
        "/v1/auth/wechat",
        None,
        Some(serde_json::json!({ "code": "wx_test_code_001" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let auth: serde_json::Value = serde_json::from_str(&auth_body).unwrap();
    let token = auth["token"].as_str().unwrap();
    let pid = auth["patient_id"].as_str().unwrap();
    assert_eq!(token.split('.').count(), 3);
    assert!(pid.starts_with("pat_sha256_"));
    assert!(!pid.contains("wx_test_code_001"));
}

#[tokio::test]
async fn test_triage_chat_json_sse_and_emergency120() {
    let (app, _) = test_app().await;

    let (_, _, auth_body) = send_json(
        &app,
        Method::POST,
        "/v1/auth/wechat",
        None,
        Some(serde_json::json!({ "code": "wx_triage_001" })),
    )
    .await;
    let auth: serde_json::Value = serde_json::from_str(&auth_body).unwrap();
    let token = auth["token"].as_str().unwrap();
    let pid = auth["patient_id"].as_str().unwrap();

    // 1. Emergency120 short-circuit
    let (st, _, emg_body) = send_json(
        &app,
        Method::POST,
        "/v1/triage/chat",
        Some(token),
        Some(serde_json::json!({
            "session_id": "sess_emg_01",
            "patient_id": pid,
            "utterance": "突然胸痛，伴随大汗和左臂放射痛",
            "stream": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let emg: serde_json::Value = serde_json::from_str(&emg_body).unwrap();
    assert_eq!(emg["status"], "emergency");
    assert_eq!(emg["red_flag"], "Emergency120");

    // 2. Non-streaming JSON triage with disclaimer
    let (st, _, ok_body) = send_json(
        &app,
        Method::POST,
        "/v1/triage/chat",
        Some(token),
        Some(serde_json::json!({
            "session_id": "sess_normal_01",
            "patient_id": pid,
            "utterance": "高热39.5度咽痛干咳两天，身边同事发烧",
            "region_code": "CN-110000",
            "stream": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let resp: serde_json::Value = serde_json::from_str(&ok_body).unwrap();
    assert!(matches!(
        resp["status"].as_str(),
        Some("completed") | Some("follow_up")
    ));
    if resp["status"] == "completed" {
        assert!(
            resp["triage_result"]["disclaimer"]
                .as_str()
                .unwrap_or("")
                .contains("不替代执业医师诊断")
        );
    }

    // 3. SSE streaming triage
    let (st, ct, sse_body) = send_json(
        &app,
        Method::POST,
        "/v1/triage/chat",
        Some(token),
        Some(serde_json::json!({
            "session_id": "sess_sse_01",
            "patient_id": pid,
            "utterance": "发热咳嗽两天",
            "stream": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(ct.contains("text/event-stream"), "expected SSE content-type, got {ct}");
    assert!(sse_body.contains("event: result"), "expected SSE result event: {sse_body}");
}

#[tokio::test]
async fn test_knowledge_search_cache_openai_proxy_epidemic_and_memory_wipe() {
    let (app, state) = test_app().await;

    let (_, _, auth_body) = send_json(
        &app,
        Method::POST,
        "/v1/auth/wechat",
        None,
        Some(serde_json::json!({ "code": "wx_full_flow_001" })),
    )
    .await;
    let auth: serde_json::Value = serde_json::from_str(&auth_body).unwrap();
    let token = auth["token"].as_str().unwrap();
    let pid = auth["patient_id"].as_str().unwrap();

    // 1. Knowledge search (miss -> hit cache)
    let q = serde_json::json!({
        "query": "高热 肌肉酸痛 咽痛",
        "region_code": "CN-110000",
        "seasonal_boost": true,
        "top_k": 3
    });
    let (st1, _, b1) = send_json(&app, Method::POST, "/v1/knowledge/search", Some(token), Some(q.clone())).await;
    let (st2, _, b2) = send_json(&app, Method::POST, "/v1/knowledge/search", Some(token), Some(q)).await;
    assert_eq!(st1, StatusCode::OK);
    assert_eq!(st2, StatusCode::OK);
    let r1: serde_json::Value = serde_json::from_str(&b1).unwrap();
    let r2: serde_json::Value = serde_json::from_str(&b2).unwrap();
    assert!(!r1["results"].as_array().unwrap().is_empty());
    assert_eq!(r1["cached"], false);
    assert_eq!(r2["cached"], true);

    // 2. OpenAI-compatible enhanced proxy (JSON + SSE)
    let (st, _, ob) = send_json(
        &app,
        Method::POST,
        "/v1/openai/chat/completions",
        Some(token),
        Some(serde_json::json!({
            "model": "medical-triage",
            "messages": [{"role": "user", "content": "咳嗽咽痛两天怎么办"}],
            "stream": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let ojson: serde_json::Value = serde_json::from_str(&ob).unwrap();
    assert_eq!(ojson["object"], "chat.completion");
    assert!(
        ojson["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .contains("不替代执业医师诊断")
    );

    let (st, ct, sse_ob) = send_json(
        &app,
        Method::POST,
        "/v1/openai/chat/completions",
        Some(token),
        Some(serde_json::json!({
            "model": "medical-triage",
            "messages": [{"role": "user", "content": "发烧两天"}],
            "stream": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(ct.contains("text/event-stream"));
    assert!(sse_ob.contains("[DONE]"));

    // 3. Epidemic current & admin hot-reload
    let (st, _, eb) = send_json(
        &app,
        Method::GET,
        "/v1/epidemic/current?region_code=CN-110000&season=winter",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let ejson: serde_json::Value = serde_json::from_str(&eb).unwrap();
    assert!(!ejson["contexts"].as_array().unwrap().is_empty());

    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/v1/admin/reload",
        Some(&state.config.admin_token),
        Some(serde_json::json!({
            "reload_epidemics": true,
            "reload_dictionaries": true,
            "reload_router": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 4. Patient memory GET / PUT / DELETE physical wipe
    let mem_url = format!("/v1/patients/{pid}/memory");
    let record = serde_json::json!({
        "patient_id": pid,
        "consent_long_term_memory": true,
        "l2_profile": {
            "age_group": "30-39",
            "gender": "female",
            "is_pregnant": false,
            "chronic_diseases": ["高血压"],
            "allergies": ["青霉素"],
            "surgical_history": [],
            "long_term_medications": ["氨氯地平"]
        },
        "l3_visit_events": []
    });
    let (st, _, _) = send_json(&app, Method::PUT, &mem_url, Some(token), Some(record)).await;
    assert_eq!(st, StatusCode::OK);

    let (st, _, gb) = send_json(&app, Method::GET, &mem_url, Some(token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(gb.contains("青霉素"));

    let (st, _, db) = send_json(&app, Method::DELETE, &mem_url, Some(token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(db.contains("\"deleted\":true"));

    let (st, _, _) = send_json(&app, Method::GET, &mem_url, Some(token), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_api_key_qpm_rate_limiting_and_revocation() {
    let (app, state) = test_app().await;

    let low_key = "mk_live_unit_qpm_2";
    state
        .stores
        .api_keys
        .seed_key(
            low_key,
            IssueApiKeyRequest {
                tenant_id: "unit_qpm".to_string(),
                name: "Unit QPM".to_string(),
                qpm_limit: 2,
                tpm_limit: 50_000,
                namespace: "default".to_string(),
            },
            false,
        )
        .await
        .unwrap();

    let revoked_key = "mk_live_unit_revoked";
    state
        .stores
        .api_keys
        .seed_key(
            revoked_key,
            IssueApiKeyRequest {
                tenant_id: "unit_revoked".to_string(),
                name: "Revoked".to_string(),
                qpm_limit: 60,
                tpm_limit: 50_000,
                namespace: "default".to_string(),
            },
            true,
        )
        .await
        .unwrap();

    // Revoked key returns 401
    let (st, _, _) = send_json(
        &app,
        Method::POST,
        "/v1/knowledge/search",
        Some(revoked_key),
        Some(serde_json::json!({ "query": "感冒", "top_k": 1 })),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // QPM=2 key succeeds twice, then 3rd request hits 429
    let mut statuses = Vec::new();
    for _ in 0..4 {
        let (st, _, _) = send_json(
            &app,
            Method::POST,
            "/v1/knowledge/search",
            Some(low_key),
            Some(serde_json::json!({ "query": "感冒", "top_k": 1 })),
        )
        .await;
        statuses.push(st);
    }
    assert_eq!(statuses[0], StatusCode::OK);
    assert_eq!(statuses[1], StatusCode::OK);
    assert!(statuses.contains(&StatusCode::TOO_MANY_REQUESTS));
}

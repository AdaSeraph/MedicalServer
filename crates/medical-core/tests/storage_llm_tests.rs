use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use chrono::Utc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use medical_core::crypto::FieldCipher;
use medical_core::llm::{
    LlmError, LlmMessage, LlmProvider, LlmRequest, LlmStreamChunk, MockFailure, MockLlmProvider,
    MockStep, ModelRouter, ModelTier, OpenAiCompatibleLlmProvider, ProviderConfig, ProviderKind,
    RouterConfig,
};
use medical_core::models::{
    Gender, KnowledgeDocument, L2Profile, L3VisitEvent, SectionType, SourceType, TargetPopulation,
};
use medical_core::storage::{
    ApiKeyStore, CoreStores, InMemoryApiKeyStore, InMemoryKnowledgeStore,
    InMemoryPatientMemoryStore, InMemorySessionCache, IssueApiKeyRequest, KnowledgeSearchEngine,
    KnowledgeSearchQuery, KnowledgeVectorFilter, KnowledgeVectorStore, PatientMemoryStore,
    RedisSessionCache, SessionCache, SessionSlotsSnapshot, SessionState, SessionTurn,
    VectorizedKnowledgeChunk, connect_pg_lazy, qdrant::connect_qdrant,
};

#[tokio::test]
async fn test_in_memory_patient_encryption_and_physical_wipe() {
    let cipher = Arc::new(FieldCipher::from_single_key([0x55_u8; 32]));
    let store = InMemoryPatientMemoryStore::new(cipher);

    let profile = L2Profile {
        age_group: "40-49".into(),
        gender: Gender::Male,
        is_pregnant: false,
        chronic_diseases: vec!["冠心病".into()],
        allergies: vec!["阿司匹林".into()],
        surgical_history: vec!["胆囊切除".into()],
        long_term_medications: vec!["氯吡格雷".into()],
    };
    store
        .upsert_l2_profile("pat_sha256_test01", true, &profile)
        .await
        .unwrap();

    // Assert ciphertext at rest does not expose plaintext health terms
    let raw = store.raw_encrypted_l2("pat_sha256_test01").unwrap();
    let raw_lossy = String::from_utf8_lossy(&raw.encrypted_payload.ciphertext);
    assert!(!raw_lossy.contains("冠心病"));
    assert!(!raw_lossy.contains("阿司匹林"));

    let event = L3VisitEvent {
        event_id: "evt_001".into(),
        occurred_at: Utc::now(),
        chief_complaint: "胸闷气短".into(),
        extracted_symptoms: vec!["胸闷".into()],
        recommended_department: "心血管内科".into(),
        urgency_level: 2,
        summary: "建议心血管内科进一步评估".into(),
    };
    store
        .insert_l3_event("pat_sha256_test01", &event)
        .await
        .unwrap();

    let full = store
        .get_full_memory("pat_sha256_test01", 10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(full.l2_profile.chronic_diseases, vec!["冠心病"]);
    assert_eq!(full.l3_visit_events.len(), 1);

    let stats = store
        .delete_patient_memory("pat_sha256_test01")
        .await
        .unwrap();
    assert_eq!(stats.l2_profiles_deleted, 1);
    assert_eq!(stats.l3_events_deleted, 1);
    assert!(store
        .get_full_memory("pat_sha256_test01", 10)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_session_cache_ttl_and_api_key_lifecycle() {
    let cache = InMemorySessionCache::new();
    let session = SessionState {
        session_id: "sess_1".into(),
        patient_id: "pat_1".into(),
        turns: vec![SessionTurn {
            role: "user".into(),
            content: "咳嗽".into(),
            timestamp: Utc::now(),
        }],
        slots: SessionSlotsSnapshot::default(),
        updated_at: Utc::now(),
    };
    cache.save_session(&session, 60).await.unwrap();
    assert!(cache.get_session("sess_1").await.unwrap().is_some());
    assert_eq!(cache.delete_patient_sessions("pat_1").await.unwrap(), 1);
    assert!(cache.get_session("sess_1").await.unwrap().is_none());

    let keys = InMemoryApiKeyStore::new();
    let issued = keys
        .issue_key(IssueApiKeyRequest {
            tenant_id: "hosp_a".into(),
            name: "HIS".into(),
            qpm_limit: 120,
            tpm_limit: 100_000,
            namespace: "hosp_a_ns".into(),
        })
        .await
        .unwrap();
    assert!(issued.raw_key.starts_with("mk_live_"));
    assert!(keys.verify_key(&issued.raw_key).await.unwrap().is_some());
    assert!(keys.revoke_key(&issued.record.key_id).await.unwrap());
    assert!(keys.verify_key(&issued.raw_key).await.unwrap().is_none());
}

#[tokio::test]
async fn test_offline_prod_client_constructors_do_not_require_live_services() {
    let _pg = connect_pg_lazy("postgres://user:pass@127.0.0.1:5432/medical", 8)
        .expect("connect_lazy builds without live PG");
    let _redis = RedisSessionCache::from_url("redis://127.0.0.1:6379")
        .expect("RedisSessionCache::from_url builds without live Redis");
    let _qdrant = connect_qdrant("http://127.0.0.1:6334", None)
        .expect("connect_qdrant builds without live Qdrant");
    let _stores = CoreStores::in_memory(Arc::new(FieldCipher::from_single_key([1; 32])));
}

#[tokio::test]
async fn test_model_router_failover_circuit_breaker_and_pii_scrubbing() {
    let cloud_a = Arc::new(MockLlmProvider::new(
        "cloud_a",
        "deepseek-chat",
        ModelTier::Reasoning,
    ));
    let cloud_b = Arc::new(MockLlmProvider::new(
        "cloud_b",
        "qwen-max",
        ModelTier::Reasoning,
    ));
    let intranet = Arc::new(MockLlmProvider::new(
        "intranet",
        "qwen-med-32b",
        ModelTier::Reasoning,
    ));

    // cloud_a trips circuit breaker (threshold=2): 429 then 500
    cloud_a.push_step(MockStep::Fail(MockFailure::RateLimit429));
    cloud_a.push_step(MockStep::Fail(MockFailure::ServerError500));
    // cloud_b fails once with timeout
    cloud_b.push_step(MockStep::Fail(MockFailure::Timeout { sleep_ms: 250 }));
    // intranet succeeds
    intranet.push_step(MockStep::Json(r#"{"action":"completed"}"#.into()));

    let router = ModelRouter::new(
        vec![cloud_a.clone(), cloud_b.clone(), intranet.clone()],
        RouterConfig {
            fast_chain: vec![],
            reasoning_chain: vec![
                "cloud_a".into(),
                "cloud_b".into(),
                "intranet".into(),
            ],
            default_timeout_ms: 100,
            circuit_breaker_failure_threshold: 2,
            circuit_breaker_cooldown_secs: 60,
        },
    );

    let req = LlmRequest::new(
        ModelTier::Reasoning,
        vec![LlmMessage::user(
            "患者姓名：李四，手机号13900139000，身份证110101198802021234，发热两天。",
        )],
    );

    // Call 1: cloud_a (429) -> cloud_b (timeout) -> intranet (success)
    let resp1 = router.complete_json(&req).await.unwrap();
    assert_eq!(resp1.provider_id, "intranet");

    // Verify PII was scrubbed BEFORE reaching any provider
    let recorded = intranet.recorded_requests();
    assert_eq!(recorded.len(), 1);
    let sent = &recorded[0].messages[0].content;
    assert!(!sent.contains("李四"));
    assert!(!sent.contains("13900139000"));
    assert!(!sent.contains("110101198802021234"));
    assert!(sent.contains("发热两天"));

    // Call 2: cloud_a fails 2nd time (500) -> circuit opens; cloud_b succeeds
    cloud_b.push_step(MockStep::Json(r#"{"action":"follow_up"}"#.into()));
    let resp2 = router.complete_json(&req).await.unwrap();
    assert_eq!(resp2.provider_id, "cloud_b");

    // Call 3: cloud_a is circuit-open (call_count stays 2!), cloud_b is hit directly
    let calls_before = cloud_a.call_count();
    cloud_b.push_step(MockStep::Json(r#"{"action":"completed"}"#.into()));
    let resp3 = router.complete_json(&req).await.unwrap();
    assert_eq!(resp3.provider_id, "cloud_b");
    assert_eq!(cloud_a.call_count(), calls_before);

    // All providers failing returns AllProvidersFailed
    cloud_b.push_step(MockStep::Fail(MockFailure::ServerError500));
    intranet.push_step(MockStep::Fail(MockFailure::NetworkError));
    let err = router.complete_json(&req).await.unwrap_err();
    assert!(matches!(err, LlmError::AllProvidersFailed { .. }));
}

#[tokio::test]
async fn test_openai_compatible_sse_utf8_split_chunk_streaming() {
    async fn mock_sse_handler() -> Response<Body> {
        // Split a multi-byte Chinese SSE payload intentionally mid-UTF8-sequence across TCP chunks
        let sse_full = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"请尽快前往\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"呼吸内科\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"就诊评估！\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":8,\"total_tokens\":18}}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes()
        .to_vec();

        // Break at odd byte offsets (including mid-UTF-8 multi-byte character!)
        let chunk_sizes = [11_usize, 17, 9, 23, 13, 19, 1024];
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        let mut idx = 0;
        let mut cs_idx = 0;
        while idx < sse_full.len() {
            let sz = chunk_sizes[cs_idx % chunk_sizes.len()];
            let end = (idx + sz).min(sse_full.len());
            chunks.push(sse_full[idx..end].to_vec());
            idx = end;
            cs_idx += 1;
        }

        let stream = tokio_stream::iter(chunks.into_iter().map(|c| {
            Ok::<axum::body::Bytes, std::convert::Infallible>(axum::body::Bytes::from(c))
        }));
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let app = Router::new().route("/v1/chat/completions", post(mock_sse_handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let provider = OpenAiCompatibleLlmProvider::new(ProviderConfig {
        id: "local_sse_mock".into(),
        name: "Local SSE Mock".into(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: format!("http://{addr}/v1"),
        api_key: "test-key".into(),
        model: "test-model".into(),
        tier: ModelTier::Reasoning,
        timeout_ms: 5000,
        is_intranet: true,
        enabled: true,
        priority: 1,
    })
    .unwrap();

    let (tx, mut rx) = mpsc::channel::<LlmStreamChunk>(32);
    let req = LlmRequest::new(ModelTier::Reasoning, vec![LlmMessage::user("高热咳嗽")]);
    let resp = tokio::time::timeout(Duration::from_secs(5), provider.chat_stream(&req, tx))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(resp.content, "请尽快前往呼吸内科就诊评估！");
    assert_eq!(resp.usage.total_tokens, 18);

    let mut deltas = String::new();
    let mut saw_done = false;
    while let Some(c) = rx.recv().await {
        deltas.push_str(&c.delta);
        if c.done {
            saw_done = true;
            break;
        }
    }
    assert!(saw_done);
    assert_eq!(deltas, "请尽快前往呼吸内科就诊评估！");

    server.abort();
}

#[tokio::test]
async fn test_list_l3_events_limit_zero_and_api_key_seed_key() {
    let cipher = Arc::new(FieldCipher::from_single_key([0x55_u8; 32]));
    let mem = InMemoryPatientMemoryStore::new(cipher);
    mem.upsert_l2_profile(
        "pat_sha256_zero",
        true,
        &L2Profile {
            age_group: "30-39".into(),
            gender: Gender::Male,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    mem.insert_l3_event(
        "pat_sha256_zero",
        &L3VisitEvent {
            event_id: "evt_zero_1".into(),
            occurred_at: Utc::now(),
            chief_complaint: "发热".into(),
            extracted_symptoms: vec!["发热".into()],
            recommended_department: "呼吸内科".into(),
            urgency_level: 3,
            summary: "建议呼吸内科就诊".into(),
        },
    )
    .await
    .unwrap();

    assert!(mem.list_l3_events("pat_sha256_zero", 0).await.unwrap().is_empty());
    let full0 = mem.get_full_memory("pat_sha256_zero", 0).await.unwrap().unwrap();
    assert!(full0.l3_visit_events.is_empty());
    assert_eq!(mem.list_l3_events("pat_sha256_zero", 5).await.unwrap().len(), 1);

    // ApiKeyStore::seed_key upsert & revoked semantics
    let keys = InMemoryApiKeyStore::new();
    let rec_active = keys
        .seed_key(
            "mk_live_seed_hosp_a",
            IssueApiKeyRequest {
                tenant_id: "hosp_a".into(),
                name: "Seed Active".into(),
                qpm_limit: 100,
                tpm_limit: 50_000,
                namespace: "hosp_a".into(),
            },
            false,
        )
        .await
        .unwrap();
    assert!(!rec_active.revoked);
    let verified = keys.verify_key("mk_live_seed_hosp_a").await.unwrap().unwrap();
    assert_eq!(verified.tenant_id, "hosp_a");
    assert_eq!(verified.namespace, "hosp_a");

    let rec_revoked = keys
        .seed_key(
            "mk_live_seed_hosp_b_revoked",
            IssueApiKeyRequest {
                tenant_id: "hosp_b".into(),
                name: "Seed Revoked".into(),
                qpm_limit: 60,
                tpm_limit: 10_000,
                namespace: "hosp_b".into(),
            },
            true,
        )
        .await
        .unwrap();
    assert!(rec_revoked.revoked);
    assert!(keys.verify_key("mk_live_seed_hosp_b_revoked").await.unwrap().is_none());

    // Idempotent upsert by raw_key hash updates existing key instead of duplicating
    let count_before = keys.list_keys().await.unwrap().len();
    let updated = keys
        .seed_key(
            "mk_live_seed_hosp_a",
            IssueApiKeyRequest {
                tenant_id: "hosp_a".into(),
                name: "Seed Active Updated".into(),
                qpm_limit: 200,
                tpm_limit: 80_000,
                namespace: "hosp_a".into(),
            },
            true,
        )
        .await
        .unwrap();
    assert_eq!(updated.key_id, rec_active.key_id);
    assert!(updated.revoked);
    assert_eq!(keys.list_keys().await.unwrap().len(), count_before);
    assert!(keys.verify_key("mk_live_seed_hosp_a").await.unwrap().is_none());
}

#[tokio::test]
async fn test_knowledge_tenant_namespace_visibility_and_hit_field() {
    let store = InMemoryKnowledgeStore::new();
    let make_doc = |id: &str, disease: &str| KnowledgeDocument {
        doc_id: id.into(),
        source_title: "医学知识库".into(),
        source_type: SourceType::Guideline,
        authority_level: 0.9,
        disease_name: disease.into(),
        aliases: vec![],
        department: "呼吸内科".into(),
        secondary_departments: vec![],
        target_population: vec![TargetPopulation::Adult],
        section_type: SectionType::ClinicalManifestation,
        content: "咳嗽 发热 胸闷".into(),
        key_symptoms: vec!["咳嗽".into(), "发热".into()],
        differential_points: vec![],
        red_flag_signs: vec![],
        seasonal_tags: vec![],
        suggested_follow_up_slots: vec![],
        updated_at: None,
    };

    store
        .upsert_documents(&[
            VectorizedKnowledgeChunk {
                document: make_doc("doc_default", "公共感冒"),
                vector: vec![1.0, 0.0],
                tenant_namespace: "default".into(),
            },
            VectorizedKnowledgeChunk {
                document: make_doc("doc_a", "医院A专病"),
                vector: vec![1.0, 0.0],
                tenant_namespace: "hosp_a".into(),
            },
            VectorizedKnowledgeChunk {
                document: make_doc("doc_b", "医院B隐私专病"),
                vector: vec![1.0, 0.0],
                tenant_namespace: "hosp_b".into(),
            },
        ])
        .await
        .unwrap();

    // Querying as hosp_a sees hosp_a + default, strictly excludes hosp_b
    let hits_a = store
        .search(&KnowledgeSearchQuery {
            query_text: "咳嗽 发热".into(),
            query_vector: Some(vec![1.0, 0.0]),
            keywords: vec!["咳嗽".into()],
            target_population: None,
            department: None,
            tenant_namespace: Some("hosp_a".into()),
            top_k: 10,
        })
        .await
        .unwrap();
    let ids_a: Vec<&str> = hits_a.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(ids_a.contains(&"doc_default"));
    assert!(ids_a.contains(&"doc_a"));
    assert!(!ids_a.contains(&"doc_b"));
    assert!(hits_a.iter().all(|h| h.tenant_namespace.is_some()));

    // Vector search enforces the same visibility
    let vec_hits_a = store
        .search_vectors(
            &[1.0, 0.0],
            &KnowledgeVectorFilter {
                tenant_namespace: Some("hosp_a".into()),
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    let vec_ids_a: Vec<&str> = vec_hits_a.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(vec_ids_a.contains(&"doc_default"));
    assert!(vec_ids_a.contains(&"doc_a"));
    assert!(!vec_ids_a.contains(&"doc_b"));

    // Querying as default sees only default
    let hits_def = store
        .search(&KnowledgeSearchQuery {
            query_text: "咳嗽".into(),
            query_vector: None,
            keywords: vec!["咳嗽".into()],
            target_population: None,
            department: None,
            tenant_namespace: Some("default".into()),
            top_k: 10,
        })
        .await
        .unwrap();
    let ids_def: Vec<&str> = hits_def.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(ids_def, vec!["doc_default"]);
    assert_eq!(hits_def[0].tenant_namespace.as_deref(), Some("default"));

    // Blank Some("") / Some("  ") normalizes to "default" instead of bypassing isolation
    let hits_blank = store
        .search(&KnowledgeSearchQuery {
            query_text: "咳嗽".into(),
            query_vector: None,
            keywords: vec!["咳嗽".into()],
            target_population: None,
            department: None,
            tenant_namespace: Some("  ".into()),
            top_k: 10,
        })
        .await
        .unwrap();
    let ids_blank: Vec<&str> = hits_blank.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(ids_blank, vec!["doc_default"]);

    // None remains unfiltered
    let hits_unfiltered = store
        .search(&KnowledgeSearchQuery {
            query_text: "咳嗽".into(),
            query_vector: None,
            keywords: vec!["咳嗽".into()],
            target_population: None,
            department: None,
            tenant_namespace: None,
            top_k: 10,
        })
        .await
        .unwrap();
    assert_eq!(hits_unfiltered.len(), 3);
}

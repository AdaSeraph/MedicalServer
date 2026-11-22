use std::path::PathBuf;

use chrono::NaiveDate;
use medical_core::llm::{ModelTier, ProviderConfig, ProviderKind, RouterConfig};
use medical_core::models::{EpidemicContext, RedFlagLevel, RedFlagRule, Season, SymptomSynonym};
use medical_core::storage::IssueApiKeyRequest;
use medical_pipeline::BookMetadataDefaults;
use medical_studio::commands::{
    KnowledgePreviewRequest, StartKnowledgeImportRequest, WhiteboxTriageRequest,
    apply_masking_and_import_patients_impl, delete_llm_provider_impl, delete_red_flag_rule_impl,
    delete_symptom_synonym_impl, export_import_errors_impl, get_gateway_metrics_impl,
    get_import_job_status_impl, get_medical_dictionaries_impl, get_router_config_impl,
    issue_api_key_impl, list_api_keys_impl, list_debug_patients_impl, list_epidemic_contexts_impl,
    list_llm_providers_impl, notify_server_hot_reload_impl, preview_knowledge_chunks_impl,
    preview_patient_import_impl, reset_ecs_debug_session_impl, resume_knowledge_import_impl,
    revoke_api_key_impl, run_ecs_whitebox_triage_impl, save_red_flag_rule_impl,
    save_router_config_impl, save_symptom_synonym_impl, start_knowledge_import_impl,
    test_dictionary_sandbox_impl, test_llm_provider_connectivity_impl, upsert_epidemic_context_impl,
    upsert_llm_provider_impl,
};
use medical_studio::StudioState;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[tokio::test]
async fn test_studio_modules_1_and_2_knowledge_and_patient_import() {
    let state = StudioState::new_offline().await.unwrap();
    let root = workspace_root();

    // Module 1: Preview + Import + Checkpoint Status + Error Export
    let md_path = root.join("e2e/fixtures/knowledge/books_sample.md");
    let preview = preview_knowledge_chunks_impl(KnowledgePreviewRequest {
        file_path: md_path.to_string_lossy().to_string(),
        defaults: BookMetadataDefaults::default(),
        custom_rule: None,
        limit: 10,
    })
    .await
    .unwrap();
    assert!(preview.total_chunks >= 6);

    let jsonl_path = root.join("e2e/fixtures/knowledge/knowledge_100.jsonl");
    let prog = start_knowledge_import_impl(
        &state,
        StartKnowledgeImportRequest {
            job_id: "studio_job_01".to_string(),
            file_path: jsonl_path.to_string_lossy().to_string(),
            tenant_namespace: "default".to_string(),
            defaults: BookMetadataDefaults::default(),
            custom_rule: None,
        },
    )
    .await
    .unwrap();
    assert!(prog.completed);
    assert_eq!(prog.processed_items, 100);

    let status = get_import_job_status_impl(&state, "studio_job_01".to_string())
        .await
        .unwrap();
    assert!(status.is_some());

    let resumed = resume_knowledge_import_impl(&state, "studio_job_01".to_string(), None)
        .await
        .unwrap();
    assert!(resumed.completed);

    let err_out = std::env::temp_dir().join("studio_job_01_errors.jsonl");
    let _ = export_import_errors_impl(
        &state,
        "studio_job_01".to_string(),
        err_out.to_string_lossy().to_string(),
    )
    .await
    .unwrap();
    let _ = std::fs::remove_file(&err_out);

    // Module 2: Patient Preview & Masked Commit
    let csv_path = root.join("e2e/fixtures/patients/raw_pii_patients.csv");
    let pat_preview = preview_patient_import_impl(csv_path.to_string_lossy().to_string(), None)
        .await
        .unwrap();
    assert_eq!(pat_preview.records.len(), 3);

    let committed =
        apply_masking_and_import_patients_impl(&state, csv_path.to_string_lossy().to_string(), None)
            .await
            .unwrap();
    assert_eq!(committed.imported_patients, 3);
    assert_eq!(committed.imported_l3_events, 3);
}

#[tokio::test]
async fn test_studio_modules_3_4_5_6_end_to_end() {
    let state = StudioState::new_offline().await.unwrap();

    // Module 3: Epidemic Console & Hot Reload
    let epis = list_epidemic_contexts_impl(&state).await.unwrap();
    assert!(!epis.is_empty());

    let new_epi = EpidemicContext {
        context_id: "epi_studio_sh_norovirus".to_string(),
        region_code: "CN-310000".to_string(),
        region_name: "上海市".to_string(),
        season: Season::Spring,
        disease_name: "诺如病毒胃肠炎".to_string(),
        seasonal_tag: "gastroenteritis".to_string(),
        intensity: 0.75,
        boost_symptoms: vec!["呕吐".to_string(), "水样便".to_string()],
        priority_question_hint: "近期有进食生冷食物或集体发病吗？".to_string(),
        active: true,
        valid_from: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        valid_to: NaiveDate::from_ymd_opt(2027, 12, 31).unwrap(),
    };
    upsert_epidemic_context_impl(&state, new_epi).await.unwrap();
    let reload = notify_server_hot_reload_impl(&state, None, None).await.unwrap();
    assert!(reload.local_reloaded);

    // Module 4: Dictionary Manager & Sandbox
    let dicts = get_medical_dictionaries_impl(&state).await.unwrap();
    assert!(!dicts.red_flag_rules.is_empty());

    save_symptom_synonym_impl(
        &state,
        SymptomSynonym {
            standard_term: "水样便".to_string(),
            colloquial_terms: vec!["拉水".to_string(), "拉稀水".to_string()],
        },
    )
    .await
    .unwrap();
    save_red_flag_rule_impl(
        &state,
        RedFlagRule {
            rule_id: "rf_studio_custom_01".to_string(),
            level: RedFlagLevel::Emergency120,
            trigger_all_of: vec!["剧烈胸痛".to_string()],
            trigger_any_of: vec!["大汗".to_string()],
            alert_message: "疑似急性冠脉综合征，请立即拨打120！".to_string(),
        },
    )
    .await
    .unwrap();

    let sandbox = test_dictionary_sandbox_impl(&state, "突然剧烈胸痛伴大汗".to_string())
        .await
        .unwrap();
    assert_eq!(sandbox.red_flag_level, RedFlagLevel::Emergency120);

    assert!(delete_symptom_synonym_impl(&state, "水样便".to_string()).await.unwrap());
    assert!(delete_red_flag_rule_impl(&state, "rf_studio_custom_01".to_string()).await.unwrap());

    // Module 5: ECS Whitebox Debugger
    let patients = list_debug_patients_impl(&state).await.unwrap();
    assert!(!patients.is_empty());
    let trace_res = run_ecs_whitebox_triage_impl(
        &state,
        WhiteboxTriageRequest {
            session_id: "studio_dbg_sess_01".to_string(),
            patient_id: patients[0].clone(),
            utterance: "高热咳嗽咽痛两天".to_string(),
            region_code: Some("CN-110000".to_string()),
            target_population: vec![],
        },
    )
    .await
    .unwrap();
    assert!(!trace_res.trace.stage_timings.is_empty());
    let _ = reset_ecs_debug_session_impl(&state, "studio_dbg_sess_01".to_string())
        .await
        .unwrap();

    // Module 6: LLM Providers, Router, API Keys & Metrics
    let providers = list_llm_providers_impl(&state).await.unwrap();
    assert!(providers.len() >= 2);
    upsert_llm_provider_impl(
        &state,
        ProviderConfig {
            id: "ollama_local".to_string(),
            name: "Local Ollama".to_string(),
            kind: ProviderKind::Ollama,
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            api_key: "ollama".to_string(),
            model: "qwen2.5:14b".to_string(),
            tier: ModelTier::Fast,
            timeout_ms: 4000,
            is_intranet: true,
            enabled: true,
            priority: 2,
        },
    )
    .await
    .unwrap();

    let mut rcfg: RouterConfig = get_router_config_impl(&state).await.unwrap();
    rcfg.fast_chain.push("ollama_local".to_string());
    save_router_config_impl(&state, rcfg).await.unwrap();

    let issued = issue_api_key_impl(
        &state,
        IssueApiKeyRequest {
            tenant_id: "clinic_b".to_string(),
            name: "门诊自助机".to_string(),
            qpm_limit: 60,
            tpm_limit: 50_000,
            namespace: "clinic_b".to_string(),
        },
    )
    .await
    .unwrap();
    assert!(issued.raw_key.starts_with("mk_live_"));

    let keys = list_api_keys_impl(&state).await.unwrap();
    assert!(keys.iter().any(|k| k.key_id == issued.record.key_id));
    assert!(revoke_api_key_impl(&state, issued.record.key_id).await.unwrap());

    let metrics = get_gateway_metrics_impl(&state, Some(24)).await.unwrap();
    assert!(metrics.error_rate >= 0.0);
}

#[tokio::test]
async fn test_llm_provider_config_and_connectivity_probe() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let state = StudioState::new_offline().await.unwrap();

    // Reject invalid URL
    let bad = upsert_llm_provider_impl(
        &state,
        ProviderConfig {
            id: "bad_provider".to_string(),
            name: "Bad".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: "not-a-url".to_string(),
            api_key: "sk-test".to_string(),
            model: "gpt-4o-mini".to_string(),
            tier: ModelTier::Fast,
            timeout_ms: 3000,
            is_intranet: false,
            enabled: true,
            priority: 1,
        },
    )
    .await;
    assert!(bad.is_err());

    // Start a tiny local HTTP server simulating OpenAI-compatible /models and /chat/completions
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let first_line = req.lines().next().unwrap_or("");
            let has_valid_key = req.contains("Authorization: Bearer sk-valid-key")
                || req.contains("authorization: Bearer sk-valid-key");

            let (status, body) = if first_line.starts_with("GET /v1/models ") {
                if has_valid_key {
                    ("200 OK", r#"{"data":[]}"#)
                } else {
                    ("401 Unauthorized", r#"{"error":"invalid_api_key"}"#)
                }
            } else if first_line.starts_with("GET /no-models/models ") {
                ("404 Not Found", "not found")
            } else if first_line.starts_with("POST /no-models/chat/completions ") && has_valid_key {
                ("200 OK", r#"{"choices":[]}"#)
            } else {
                ("400 Bad Request", "bad request")
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });

    // 1. Valid API key against /v1/models
    upsert_llm_provider_impl(
        &state,
        ProviderConfig {
            id: "custom_ok".to_string(),
            name: "Custom OK".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: format!("http://{addr}/v1/"),
            api_key: "sk-valid-key".to_string(),
            model: "deepseek-chat".to_string(),
            tier: ModelTier::Fast,
            timeout_ms: 4000,
            is_intranet: true,
            enabled: true,
            priority: 1,
        },
    )
    .await
    .unwrap();
    let probe_ok = test_llm_provider_connectivity_impl(&state, "custom_ok".to_string())
        .await
        .unwrap();
    assert!(probe_ok.ok, "expected ok probe, got: {:?}", probe_ok.error);
    assert!(probe_ok.ttft_ms.is_some());

    // 2. Wrong API key returns 401 with body details
    upsert_llm_provider_impl(
        &state,
        ProviderConfig {
            id: "custom_ok".to_string(),
            name: "Custom Bad Key".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: format!("http://{addr}/v1"),
            api_key: "sk-wrong-key".to_string(),
            model: "deepseek-chat".to_string(),
            tier: ModelTier::Fast,
            timeout_ms: 4000,
            is_intranet: true,
            enabled: true,
            priority: 1,
        },
    )
    .await
    .unwrap();
    let probe_401 = test_llm_provider_connectivity_impl(&state, "custom_ok".to_string())
        .await
        .unwrap();
    assert!(!probe_401.ok);
    let err_msg = probe_401.error.unwrap_or_default();
    assert!(err_msg.contains("HTTP 401") && err_msg.contains("invalid_api_key"));

    // 3. Fallback to POST /chat/completions when GET /models returns 404 (and base_url ends with /chat/completions)
    upsert_llm_provider_impl(
        &state,
        ProviderConfig {
            id: "custom_fallback".to_string(),
            name: "Fallback Provider".to_string(),
            kind: ProviderKind::CustomHttp,
            base_url: format!("http://{addr}/no-models/chat/completions"),
            api_key: "sk-valid-key".to_string(),
            model: "qwen-plus".to_string(),
            tier: ModelTier::Reasoning,
            timeout_ms: 4000,
            is_intranet: true,
            enabled: true,
            priority: 2,
        },
    )
    .await
    .unwrap();
    let probe_fallback = test_llm_provider_connectivity_impl(&state, "custom_fallback".to_string())
        .await
        .unwrap();
    assert!(
        probe_fallback.ok,
        "expected fallback probe to succeed, got: {:?}",
        probe_fallback.error
    );

    // 4. Delete provider
    assert!(delete_llm_provider_impl(&state, "custom_fallback".to_string()).await.unwrap());
    let remaining = list_llm_providers_impl(&state).await.unwrap();
    assert!(!remaining.iter().any(|p| p.id == "custom_fallback"));

    let _ = server.await;
}

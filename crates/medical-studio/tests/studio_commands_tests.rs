use std::path::PathBuf;

use chrono::NaiveDate;
use medical_core::llm::{MockLlmProvider, ModelTier, ProviderConfig, ProviderKind, RouterConfig};
use medical_core::models::{
    EpidemicContext, Gender, L2Profile, RedFlagLevel, RedFlagRule, Season, SymptomSynonym,
};
use medical_core::storage::IssueApiKeyRequest;
use medical_pipeline::BookMetadataDefaults;
use medical_studio::ai_normalize::{
    BatchImportItem, InspectionStatus, ai_inspect_knowledge_with_provider,
    ai_inspect_patient_with_provider, ai_normalize_knowledge_with_provider,
    ai_normalize_patient_with_provider,
};
use medical_studio::commands::{
    BatchKnowledgePreviewRequest, BatchPatientPreviewRequest, CommitKnowledgeDocumentsRequest,
    CommitPatientRecordsRequest, ConsultationTriageRequest, ExportExchangePackageRequest,
    ImportExchangePackageRequest, KnowledgePreviewRequest, PreviewExchangePackageRequest,
    StartKnowledgeImportRequest, SubmitRevisitFeedbackRequest, UpsertPatientProfileRequest,
    WhiteboxTriageRequest, apply_masking_and_import_patients_impl,
    batch_preview_knowledge_items_impl, batch_preview_patient_items_impl,
    commit_knowledge_documents_impl, commit_patient_records_impl,
    create_or_update_patient_profile_impl, delete_epidemic_context_impl, delete_llm_provider_impl,
    delete_red_flag_rule_impl, delete_symptom_synonym_impl, export_exchange_package_impl,
    export_import_errors_impl, get_ecs_runtime_status_impl, get_gateway_metrics_impl,
    get_import_job_status_impl, get_knowledge_categories_impl, get_medical_dictionaries_impl,
    get_router_config_impl, import_exchange_package_impl, issue_api_key_impl, list_api_keys_impl,
    list_debug_patients_impl, list_epidemic_contexts_impl, list_knowledge_documents_impl,
    list_llm_providers_impl, list_patient_summaries_impl, lookup_patient_by_serial_impl,
    notify_server_hot_reload_impl, preview_exchange_package_impl, preview_knowledge_chunks_impl,
    preview_patient_import_impl, reset_ecs_debug_session_impl, resume_knowledge_import_impl,
    revoke_api_key_impl, run_consultation_triage_impl, run_ecs_whitebox_triage_impl,
    save_red_flag_rule_impl, save_router_config_impl, save_symptom_synonym_impl,
    start_knowledge_import_impl, submit_patient_revisit_feedback_impl,
    test_dictionary_sandbox_impl, test_llm_provider_connectivity_impl,
    upsert_epidemic_context_impl, upsert_llm_provider_impl,
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
    let epis_after_add = list_epidemic_contexts_impl(&state).await.unwrap();
    assert!(epis_after_add.iter().any(|e| e.context_id == "epi_studio_sh_norovirus"));
    assert!(delete_epidemic_context_impl(&state, "epi_studio_sh_norovirus".to_string()).await.unwrap());
    let epis_after_del = list_epidemic_contexts_impl(&state).await.unwrap();
    assert!(!epis_after_del.iter().any(|e| e.context_id == "epi_studio_sh_norovirus"));
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

#[tokio::test]
async fn test_ai_batch_normalize_and_commit_knowledge_and_patients() {
    let state = StudioState::new_offline().await.unwrap();

    // 1. AI Knowledge Batch Normalization (including <think> block, markdown fence & Chinese enums) -> DB commit
    let kb_mock = MockLlmProvider::new("mock_norm_kb", "qwen-norm", ModelTier::Reasoning)
        .with_default_json(
            r#"<think>分析输入文本并规范化为标准知识块</think>
```json
{
  "documents": [
    {
      "disease_name": "急性支气管炎",
      "aliases": ["气管炎"],
      "source_type": "临床指南",
      "authority_level": 0.92,
      "department": "呼吸内科",
      "target_population": ["成人", "老人"],
      "section_type": "临床表现",
      "content": "急性支气管炎多由病毒感染引起，主要表现为持续干咳或伴白色黏痰、低热及胸骨后不适。",
      "key_symptoms": ["咳嗽", "低热", "胸骨后不适"],
      "red_flag_signs": ["呼吸困难", "高热持续超过3天"],
      "seasonal_tags": ["winter"]
    }
  ]
}
```"#,
        );

    let kb_items = vec![
        BatchImportItem {
            name: "notes_1.txt".to_string(),
            content_text: Some("急性支气管炎：干咳、低热、胸骨后不适，建议挂呼吸内科。".to_string()),
            content_base64: None,
            file_path: None,
        },
        BatchImportItem {
            name: "notes_2.md".to_string(),
            content_text: Some("# 急性支气管炎\n冬季高发，如呼吸困难需急诊。".to_string()),
            content_base64: None,
            file_path: None,
        },
    ];

    let kb_norm = ai_normalize_knowledge_with_provider(
        &kb_mock,
        &kb_items,
        None,
        &BookMetadataDefaults::default(),
        10_000,
    )
    .await
    .unwrap();
    assert_eq!(kb_norm.documents.len(), 2);
    assert_eq!(kb_norm.item_results.len(), 2);
    assert!(kb_norm.errors.is_empty());

    let kb_prog = commit_knowledge_documents_impl(
        &state,
        CommitKnowledgeDocumentsRequest {
            job_id: "studio_ai_kb_batch_01".to_string(),
            tenant_namespace: "default".to_string(),
            documents: kb_norm.documents,
        },
    )
    .await
    .unwrap();
    assert!(kb_prog.completed);
    assert_eq!(kb_prog.processed_items, 2);

    // 2. Rule-based batch preview for knowledge items
    let rule_kb = batch_preview_knowledge_items_impl(BatchKnowledgePreviewRequest {
        items: vec![BatchImportItem {
            name: "sample.md".to_string(),
            content_text: Some("# 偏头痛\n## 临床表现\n单侧搏动性头痛伴畏光恶心。\n## 推荐科室\n神经内科".to_string()),
            content_base64: None,
            file_path: None,
        }],
        defaults: BookMetadataDefaults::default(),
        custom_rule: None,
    })
    .await
    .unwrap();
    assert!(rule_kb.total_chunks >= 1);

    // 3. AI Patient Case Batch Normalization (with plaintext PII scrubbing + salted SHA-256 hashing) -> DB commit
    let pat_mock = MockLlmProvider::new("mock_norm_pat", "qwen-norm", ModelTier::Reasoning)
        .with_default_json(
            r#"{
  "patients": [
    {
      "patient_identifier": "110101198505121234",
      "age": 41,
      "gender": "女",
      "is_pregnant": false,
      "chronic_diseases": ["高血压"],
      "allergies": ["青霉素"],
      "visit_events": [
        {
          "chief_complaint": "患者姓名：张三，手机号13800138000，反复头晕伴心悸2天",
          "extracted_symptoms": ["头晕", "心悸"],
          "recommended_department": "心血管内科",
          "urgency_level": 2,
          "summary": "中年女性既往高血压，近期头晕心悸，建议心血管内科评估。"
        }
      ]
    }
  ]
}"#,
        );

    let pat_items = vec![BatchImportItem {
        name: "emr_freeform.txt".to_string(),
        content_text: Some(
            "患者姓名：张三，身份证110101198505121234，电话13800138000，女，41岁，既往高血压，青霉素过敏，主诉头晕心悸2天。"
                .to_string(),
        ),
        content_base64: None,
        file_path: None,
    }];

    let pat_norm = ai_normalize_patient_with_provider(
        &pat_mock,
        &pat_items,
        None,
        b"hospital-test-salt",
        true,
        10_000,
    )
    .await
    .unwrap();
    assert_eq!(pat_norm.records.len(), 1);
    let rec = &pat_norm.records[0];
    assert!(rec.patient_id.starts_with("pat_sha256_"));
    assert_eq!(rec.l2_profile.age_group, "40-49");
    assert!(!rec.l3_visit_events[0].chief_complaint.contains("13800138000"));
    assert!(!rec.l3_visit_events[0].chief_complaint.contains("张三"));

    let pat_commit = commit_patient_records_impl(
        &state,
        CommitPatientRecordsRequest {
            records: pat_norm.records,
            detected_pii_columns: pat_norm.detected_pii_types,
        },
    )
    .await
    .unwrap();
    assert_eq!(pat_commit.imported_patients, 1);
    assert_eq!(pat_commit.imported_l3_events, 1);

    // 4. Rule-based batch preview for patient CSV text
    let rule_pat = batch_preview_patient_items_impl(BatchPatientPreviewRequest {
        items: vec![BatchImportItem {
            name: "batch.csv".to_string(),
            content_text: Some(
                "姓名,身份证号,年龄,性别,慢病史,主诉,推荐科室\n李四,110101199001011234,36,男,无,发热咳嗽两天,呼吸内科\n"
                    .to_string(),
            ),
            content_base64: None,
            file_path: None,
        }],
        salt: Some("hospital-test-salt".to_string()),
    })
    .await
    .unwrap();
    assert_eq!(rule_pat.records.len(), 1);
    assert!(rule_pat.records[0].patient_id.starts_with("pat_sha256_"));
}

#[tokio::test]
async fn test_knowledge_categories_ai_inspection_exchange_and_ecs_linkage() {
    let state_a = StudioState::new_offline().await.unwrap();
    let root = workspace_root();

    // 1. Seed knowledge into User A's studio and verify existing categories
    let jsonl_path = root.join("e2e/fixtures/knowledge/knowledge_100.jsonl");
    start_knowledge_import_impl(
        &state_a,
        StartKnowledgeImportRequest {
            job_id: "job_cat_01".to_string(),
            file_path: jsonl_path.to_string_lossy().to_string(),
            tenant_namespace: "default".to_string(),
            defaults: BookMetadataDefaults::default(),
            custom_rule: None,
        },
    )
    .await
    .unwrap();

    let cats = get_knowledge_categories_impl(&state_a, None).await.unwrap();
    assert_eq!(cats.total_documents, 100);
    assert!(!cats.departments.is_empty());
    let first_dept = cats.departments[0].department.clone();
    let dept_docs = list_knowledge_documents_impl(&state_a, Some(first_dept.clone()), None, Some(10))
        .await
        .unwrap();
    assert!(!dept_docs.is_empty());
    assert!(dept_docs.iter().all(|d| d.department == first_dept));

    // 2. AI Inspection & Classification for Knowledge (with duplicate + category alignment)
    let mut candidate = dept_docs[0].clone();
    candidate.department = "心内科".to_string(); // simulate misaligned department name
    let inspect_mock = MockLlmProvider::new("mock_inspect", "mock-inspect-v1", ModelTier::Reasoning)
        .with_default_json(format!(
            r#"{{
              "inspections": [
                {{
                  "doc_id": "{}",
                  "status": "warning",
                  "suggested_department": "{}",
                  "suggested_section_type": "clinical_manifestation",
                  "category_tags": ["重点科室", "复核对齐"],
                  "is_duplicate": true,
                  "duplicate_of": "{}",
                  "issues": ["科室名称与已有分类不一致，建议统一"],
                  "recommendation": "建议对齐已有科室并去重"
                }}
              ]
            }}"#,
            candidate.doc_id, first_dept, candidate.doc_id
        ));
    let inspect_res = ai_inspect_knowledge_with_provider(
        &inspect_mock,
        &[candidate.clone()],
        &dept_docs,
        None,
        10_000,
    )
    .await
    .unwrap();
    assert_eq!(inspect_res.summary.total, 1);
    assert_eq!(inspect_res.summary.warning_count, 1);
    assert_eq!(inspect_res.summary.duplicate_count, 1);
    assert_eq!(inspect_res.items[0].suggested_department, first_dept);
    assert_eq!(inspect_res.items[0].status, InspectionStatus::Warning);

    // 3. AI Inspection & Classification for Patients
    let csv_path = root.join("e2e/fixtures/patients/raw_pii_patients.csv");
    let pat_preview = preview_patient_import_impl(csv_path.to_string_lossy().to_string(), None)
        .await
        .unwrap();
    let pat_inspect_mock =
        MockLlmProvider::new("mock_pat_ins", "mock-pat-ins-v1", ModelTier::Reasoning)
            .with_default_json(format!(
                r#"{{
                  "inspections": [
                    {{
                      "patient_id": "{}",
                      "status": "pass",
                      "suggested_department": "呼吸内科",
                      "risk_level": "medium",
                      "category_tags": ["呼吸系统"],
                      "pii_risk": false,
                      "issues": [],
                      "recommendation": "脱敏完整，分类准确"
                    }}
                  ]
                }}"#,
                pat_preview.records[0].patient_id
            ));
    let pat_ins = ai_inspect_patient_with_provider(
        &pat_inspect_mock,
        &pat_preview.records[..1],
        &[first_dept.clone()],
        None,
        10_000,
    )
    .await
    .unwrap();
    assert_eq!(pat_ins.summary.pass_count, 1);
    assert_eq!(pat_ins.items[0].suggested_department, "呼吸内科");

    // 4. Peer Exchange Export from User A -> Preview & Import into User B
    let exported = export_exchange_package_impl(
        &state_a,
        ExportExchangePackageRequest {
            description: Some("用户A分享的强化学习包".to_string()),
            include_knowledge: true,
            knowledge_departments: vec![first_dept.clone()],
            tenant_namespace: None,
            include_patients: true,
            include_epidemics: true,
            include_dictionaries: true,
            format: "json".to_string(),
            output_path: None,
        },
    )
    .await
    .unwrap();
    assert!(exported.stats.knowledge_count > 0);
    assert!(exported.stats.patient_count >= 1);
    assert!(exported.stats.epidemic_count >= 1);
    assert!(exported.filename.ends_with(".json"));

    let state_b = StudioState::new_offline().await.unwrap();
    let preview_b = preview_exchange_package_impl(
        &state_b,
        PreviewExchangePackageRequest {
            content: exported.content.clone(),
            tenant_namespace: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(preview_b.stats.knowledge_count, exported.stats.knowledge_count);

    let import_b = import_exchange_package_impl(
        &state_b,
        ImportExchangePackageRequest {
            package: preview_b.package,
            tenant_namespace: "default".to_string(),
            import_knowledge: true,
            import_patients: true,
            import_epidemics: true,
            import_dictionaries: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(import_b.imported_knowledge as usize, exported.stats.knowledge_count);

    // 5. ECS Runtime Status: initially mock, switches to real LLM when a real provider is configured
    let st_before = get_ecs_runtime_status_impl(&state_b).await.unwrap();
    assert!(!st_before.using_real_llm);
    assert_eq!(st_before.knowledge_doc_count as usize, exported.stats.knowledge_count);

    upsert_llm_provider_impl(
        &state_b,
        ProviderConfig {
            id: "real_deepseek".to_string(),
            name: "Real DeepSeek".to_string(),
            kind: ProviderKind::DeepSeek,
            base_url: "https://api.deepseek.com/v1".to_string(),
            api_key: "sk-real-user-key-12345".to_string(),
            model: "deepseek-chat".to_string(),
            tier: ModelTier::Reasoning,
            timeout_ms: 8000,
            is_intranet: false,
            enabled: true,
            priority: 1,
        },
    )
    .await
    .unwrap();

    let st_after = get_ecs_runtime_status_impl(&state_b).await.unwrap();
    assert!(st_after.using_real_llm);
    assert!(st_after.active_providers.iter().any(|p| p.contains("Real DeepSeek")));

    // 6. Verify ECS Whitebox Triage recalls both L2 profile + L3 visit events + RAG knowledge
    let demo_patients = list_debug_patients_impl(&state_a).await.unwrap();
    let triage_res = run_ecs_whitebox_triage_impl(
        &state_a,
        WhiteboxTriageRequest {
            session_id: "sess_verify_linkage".to_string(),
            patient_id: demo_patients[0].clone(),
            utterance: "夜间干咳胸闷伴发热两天".to_string(),
            region_code: Some("CN-110000".to_string()),
            target_population: vec![],
        },
    )
    .await
    .unwrap();
    assert!(triage_res.trace.patient_profile.loaded);
    assert!(!triage_res.trace.memory_recall.l3_events.is_empty());
    assert!(!triage_res.trace.rag_evidence.items.is_empty());
}

#[tokio::test]
async fn test_patient_revisit_feedback_and_new_patient_consultation() {
    let state = StudioState::new_offline().await.unwrap();
    let new_serial = "PAT-20261122-NEW01".to_string();

    // 1. Lookup brand-new serial -> not found
    let lookup_before = lookup_patient_by_serial_impl(&state, new_serial.clone(), None)
        .await
        .unwrap();
    assert!(!lookup_before.found);
    assert!(lookup_before.resolved_patient_id.starts_with("pat_sha256_"));

    // 2. New patient consultation -> zero prior memory on turn 1, auto-creates L2+L3 on completion
    let first_visit = run_consultation_triage_impl(
        &state,
        ConsultationTriageRequest {
            session_id: "sess_new_pat_01".to_string(),
            serial_or_id: new_serial.clone(),
            salt: None,
            is_new_patient: true,
            utterance: "发热39度咳嗽咽痛两天".to_string(),
            region_code: Some("CN-110000".to_string()),
            target_population: vec![],
            live_l2_update: None,
        },
    )
    .await
    .unwrap();
    assert!(!first_visit.triage.trace.patient_profile.loaded);
    assert!(first_visit.triage.trace.memory_recall.l3_events.is_empty());
    assert!(first_visit.memory_updated);
    let created = first_visit
        .updated_patient_record
        .expect("new patient record should be created automatically");
    assert_eq!(created.l3_visit_events.len(), 1);

    // 3. Lookup same serial -> now found with 1 visit
    let lookup_after = lookup_patient_by_serial_impl(&state, new_serial.clone(), None)
        .await
        .unwrap();
    assert!(lookup_after.found);
    assert_eq!(lookup_after.record.unwrap().l3_visit_events.len(), 1);

    // 4. Update L2 profile during consultation & submit revisit feedback
    let updated_l2 = create_or_update_patient_profile_impl(
        &state,
        UpsertPatientProfileRequest {
            serial_or_id: new_serial.clone(),
            salt: None,
            consent_long_term_memory: true,
            l2_profile: L2Profile {
                age_group: "30-39".to_string(),
                gender: Gender::Male,
                is_pregnant: false,
                chronic_diseases: vec!["过敏性鼻炎".to_string()],
                allergies: vec!["头孢".to_string()],
                surgical_history: vec![],
                long_term_medications: vec![],
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(updated_l2.l2_profile.allergies, vec!["头孢".to_string()]);

    let fb = submit_patient_revisit_feedback_impl(
        &state,
        SubmitRevisitFeedbackRequest {
            serial_or_id: new_serial.clone(),
            salt: None,
            outcome_status: "部分好转".to_string(),
            chief_complaint: "体温降至37.3度但仍有阵发性干咳".to_string(),
            extracted_symptoms: vec!["干咳".to_string()],
            actual_department: "呼吸内科".to_string(),
            urgency_level: 3,
            feedback_summary: "按呼吸内科方案用药3天后退烧，复诊评估咳嗽恢复情况".to_string(),
            updated_l2_profile: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(fb.patient_record.l3_visit_events.len(), 2);

    // 5. Revisit consultation recalls BOTH updated L2 profile and L3 history
    let revisit = run_consultation_triage_impl(
        &state,
        ConsultationTriageRequest {
            session_id: "sess_revisit_pat_02".to_string(),
            serial_or_id: new_serial.clone(),
            salt: None,
            is_new_patient: false,
            utterance: "今天复诊还是有点干咳".to_string(),
            region_code: Some("CN-110000".to_string()),
            target_population: vec![],
            live_l2_update: None,
        },
    )
    .await
    .unwrap();
    assert!(revisit.triage.trace.patient_profile.loaded);
    assert!(!revisit.triage.trace.memory_recall.l3_events.is_empty());

    let summaries = list_patient_summaries_impl(&state, Some(50)).await.unwrap();
    assert!(summaries.iter().any(|s| s.patient_id == revisit.resolved_patient_id && s.visit_count >= 2));
}

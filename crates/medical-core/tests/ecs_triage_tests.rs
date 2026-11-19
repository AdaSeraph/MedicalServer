use std::sync::Arc;

use arc_swap::ArcSwap;
use bevy_ecs::prelude::{IntoSystemConfigs, Schedule, World};
use chrono::Utc;

use medical_core::crypto::FieldCipher;
use medical_core::ecs::{
    CompiledMedicalDictionary, ExtractedSymptoms, InfraClients, RagEvidence, RawUtterance,
    RedFlagAlert, SharedTriageResources, TriageEngine, TriageEngineConfig, TriageRequest,
    TriageResources, TriageResponse, TriageSlots, TriageStreamEvent, not_emergency_120,
    systems::stage1::{
        emergency_short_circuit_response_system, keyword_extraction_system, red_flag_guard_system,
    },
};
use medical_core::llm::{
    MockEmbeddingProvider, MockFailure, MockLlmProvider, MockRerankProvider, MockStep,
    ModelRouter, ModelTier, RouterConfig,
};
use medical_core::models::{
    EpidemicContext, Gender, KnowledgeDocument, L2Profile, MedicalDictionaries, RedFlagLevel,
    RedFlagRule, Season, SectionType, SourceType, SuggestedFollowUpSlot, SymptomSynonym,
    TargetPopulation, TriageStatus,
};
use medical_core::privacy::{MEDICAL_DISCLAIMER, check_medical_output_violations};
use medical_core::storage::{
    CoreStores, DictionaryStore, InMemoryDictionaryStore, InMemoryKnowledgeStore,
    InMemoryPatientMemoryStore, PatientMemoryStore,
};

fn default_test_dictionary() -> MedicalDictionaries {
    MedicalDictionaries {
        symptom_synonyms: vec![
            SymptomSynonym {
                standard_term: "上腹痛".into(),
                colloquial_terms: vec!["心口窝疼".into(), "胃口疼".into()],
            },
            SymptomSynonym {
                standard_term: "胸痛".into(),
                colloquial_terms: vec!["胸口撕裂样疼".into(), "胸口疼".into()],
            },
            SymptomSynonym {
                standard_term: "大汗".into(),
                colloquial_terms: vec!["出冷汗".into(), "大汗淋漓".into()],
            },
            SymptomSynonym {
                standard_term: "呼吸困难".into(),
                colloquial_terms: vec!["喘不上气".into(), "气短".into()],
            },
        ],
        red_flag_rules: vec![RedFlagRule {
            rule_id: "rf_acute_chest_pain".into(),
            level: RedFlagLevel::Emergency120,
            trigger_all_of: vec!["胸痛".into()],
            trigger_any_of: vec!["大汗".into(), "呼吸困难".into(), "左臂放射痛".into()],
            alert_message: "存在急性心肺危重症风险，请立即拨打 120 或前往急诊科！".into(),
        }],
    }
}

struct TestHarness {
    engine: TriageEngine,
    stores: CoreStores,
    knowledge: Arc<InMemoryKnowledgeStore>,
    patient_mem: Arc<InMemoryPatientMemoryStore>,
    fast_llm: Arc<MockLlmProvider>,
    reasoning_llm: Arc<MockLlmProvider>,
    cipher: Arc<FieldCipher>,
}

fn build_harness(epidemics: Vec<EpidemicContext>) -> TestHarness {
    let cipher = Arc::new(FieldCipher::from_single_key([0x77_u8; 32]));
    let mut stores = CoreStores::in_memory(cipher.clone());
    let knowledge = Arc::new(InMemoryKnowledgeStore::new());
    let patient_mem = Arc::new(InMemoryPatientMemoryStore::new(cipher.clone()));
    stores.knowledge_vectors = knowledge.clone();
    stores.knowledge_search = knowledge.clone();
    stores.patient_memory = patient_mem.clone();

    let fast_llm = Arc::new(
        MockLlmProvider::new("fast_mock", "fast-model", ModelTier::Fast)
            .with_default_json(r#"{"rewritten_query":"高热 咳嗽 咽痛"}"#),
    );
    let reasoning_llm = Arc::new(
        MockLlmProvider::new("reason_mock", "reason-model", ModelTier::Reasoning)
            .with_default_json(
                r#"{"action":"follow_up","question":"发烧持续多久了？","quick_options":["今天刚开始","2~3天","超过3天"]}"#,
            ),
    );

    let router = Arc::new(ModelRouter::new(
        vec![fast_llm.clone(), reasoning_llm.clone()],
        RouterConfig {
            fast_chain: vec!["fast_mock".into()],
            reasoning_chain: vec!["reason_mock".into()],
            default_timeout_ms: 5000,
            circuit_breaker_failure_threshold: 3,
            circuit_breaker_cooldown_secs: 30,
        },
    ));

    let dict = Arc::new(CompiledMedicalDictionary::compile(default_test_dictionary()).unwrap());
    let clients = Arc::new(InfraClients {
        stores: stores.clone(),
        embedding: Arc::new(MockEmbeddingProvider::new(64)),
        rerank: Arc::new(MockRerankProvider::new()),
        cipher: cipher.clone(),
    });

    let resources = Arc::new(ArcSwap::from_pointee(TriageResources {
        dictionary: dict,
        epidemics: Arc::new(epidemics),
        clients,
        model_router: router,
        config: TriageEngineConfig {
            await_consolidation: true,
            ..Default::default()
        },
    }));

    TestHarness {
        engine: TriageEngine::new(resources),
        stores,
        knowledge,
        patient_mem,
        fast_llm,
        reasoning_llm,
        cipher,
    }
}

#[tokio::test]
async fn test_emergency_120_short_circuit_zero_llm_calls() {
    let h = build_harness(vec![]);

    // Direct bevy_ecs RunCondition test: confirm `not_emergency_120` skips downstream systems
    let mut world = World::new();
    world.insert_resource(SharedTriageResources(h.engine.resources()));
    let e = world
        .spawn((
            RawUtterance {
                text: "老人突然胸口撕裂样疼出冷汗喘不上气".into(),
                scrubbed_text: "老人突然胸口撕裂样疼出冷汗喘不上气".into(),
                received_at: Utc::now(),
            },
            ExtractedSymptoms::default(),
            RedFlagAlert::default(),
            RagEvidence::default(),
            TriageSlots::default(),
            TriageResponse {
                status: TriageStatus::FollowUp,
                session_id: "s".into(),
                red_flag: RedFlagLevel::None,
                turn: 1,
                max_turns: 4,
                question: None,
                quick_options: vec![],
                seasonal_context_applied: vec![],
                triage_result: None,
                citations: vec![],
                is_fallback: false,
            },
        ))
        .id();

    let mut sched = Schedule::default();
    sched.add_systems(
        (
            keyword_extraction_system,
            red_flag_guard_system,
            emergency_short_circuit_response_system,
            (|mut slots: bevy_ecs::prelude::Query<&mut TriageSlots>| {
                if let Ok(mut s) = slots.get_single_mut() {
                    s.turn = 99; // MUST NOT RUN when Emergency120 triggers
                }
            })
            .run_if(not_emergency_120),
        )
            .chain(),
    );
    sched.run(&mut world);
    assert_eq!(
        world.get::<RedFlagAlert>(e).unwrap().level,
        RedFlagLevel::Emergency120
    );
    // `TriageSlots::default().turn` is 1; the `run_if(not_emergency_120)` system must NOT set it to 99.
    assert_eq!(world.get::<TriageSlots>(e).unwrap().turn, 1);

    // Full TriageEngine test
    let res = h
        .engine
        .run_triage(TriageRequest::simple(
            "sess_emg_1",
            "pat_sha256_emg",
            "老人突然胸口撕裂样疼出冷汗喘不上气",
        ))
        .await
        .unwrap();

    assert_eq!(res.response.status, TriageStatus::Emergency);
    assert_eq!(res.response.red_flag, RedFlagLevel::Emergency120);
    assert_eq!(
        res.response.triage_result.as_ref().unwrap().primary_department,
        "急诊科"
    );
    assert_eq!(res.trace.llm_calls_made, 0);
    assert_eq!(h.fast_llm.call_count(), 0);
    assert_eq!(h.reasoning_llm.call_count(), 0);
    assert!(
        res.trace
            .stage_timings
            .iter()
            .filter(|s| s.stage != "stage1_red_flag_guard")
            .all(|s| s.skipped)
    );
}

#[tokio::test]
async fn test_cross_turn_emergency_120_short_circuit() {
    let h = build_harness(vec![]);

    // Turn 1: chest pain alone -> does not trigger Emergency120 yet
    let t1 = h
        .engine
        .run_triage(TriageRequest::simple("sess_cross", "pat_cross", "刚才开始胸口疼"))
        .await
        .unwrap();
    assert_ne!(t1.response.red_flag, RedFlagLevel::Emergency120);
    let calls_after_t1 = h.reasoning_llm.call_count();

    // Turn 2: sweating -> merged with Turn 1 "胸痛" in Stage 2 -> triggers Emergency120 with 0 Stage 3/4 LLM calls!
    let t2 = h
        .engine
        .run_triage(TriageRequest::simple("sess_cross", "pat_cross", "现在大汗淋漓"))
        .await
        .unwrap();
    assert_eq!(t2.response.status, TriageStatus::Emergency);
    assert_eq!(t2.response.red_flag, RedFlagLevel::Emergency120);
    assert_eq!(h.reasoning_llm.call_count(), calls_after_t1);
    assert_eq!(t2.trace.llm_calls_made, 0);
}

#[tokio::test]
async fn test_follow_up_constraints_and_max_turns_convergence() {
    let h = build_harness(vec![]);
    // Turn 1: Reasoning LLM emits an oversized >30-char question and only 1 quick option -> system clamps!
    h.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"follow_up","question":"请问您目前出现持续高热和全身乏力肌肉酸痛的症状大概已经持续多少天了呢？","quick_options":["今天刚开始"]}"#.into(),
    ));

    let r1 = h
        .engine
        .run_triage(TriageRequest::simple("sess_turns", "pat_turns", "高热咽痛"))
        .await
        .unwrap();
    assert_eq!(r1.response.status, TriageStatus::FollowUp);
    assert_eq!(r1.response.turn, 1);
    assert_eq!(r1.response.max_turns, 4);
    let q1 = r1.response.question.as_ref().unwrap();
    assert!(q1.chars().count() <= 30, "question length {} > 30: {q1}", q1.chars().count());
    assert!((2..=4).contains(&r1.response.quick_options.len()));

    // Turns 2..=4: even if LLM keeps asking "follow_up", Turn 4 MUST force Completed!
    for expected_turn in 2..=4_u8 {
        h.reasoning_llm.push_step(MockStep::Json(
            r#"{"action":"follow_up","question":"还有别的不舒服吗？","quick_options":["咳嗽","流涕","头痛","胸闷","腹泻"]}"#.into(),
        ));
        let r = h
            .engine
            .run_triage(TriageRequest::simple("sess_turns", "pat_turns", "有点咳嗽"))
            .await
            .unwrap();
        assert_eq!(r.response.turn, expected_turn);
        if expected_turn < 4 {
            assert_eq!(r.response.status, TriageStatus::FollowUp);
            assert!((2..=4).contains(&r.response.quick_options.len()));
        } else {
            assert_eq!(r.response.status, TriageStatus::Completed);
            let card = r.response.triage_result.as_ref().unwrap();
            assert_eq!(card.disclaimer, MEDICAL_DISCLAIMER);
        }
    }
}

#[tokio::test]
async fn test_l2_aes256_gcm_roundtrip_and_outbound_pii_scrubbing() {
    let h = build_harness(vec![]);
    let l2 = L2Profile {
        age_group: "30-39".into(),
        gender: Gender::Female,
        is_pregnant: false,
        chronic_diseases: vec!["轻度哮喘".into()],
        allergies: vec!["青霉素".into()],
        surgical_history: vec![],
        long_term_medications: vec![],
    };
    h.patient_mem
        .upsert_l2_profile("pat_sha256_l2", true, &l2)
        .await
        .unwrap();

    // Confirm encrypted at rest
    let raw = h.patient_mem.raw_encrypted_l2("pat_sha256_l2").unwrap();
    assert!(!String::from_utf8_lossy(&raw.encrypted_payload.ciphertext).contains("轻度哮喘"));
    assert!(h.cipher.decrypt_bytes(&raw.encrypted_payload).is_ok());

    h.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"completed","primary_department":"呼吸内科","urgency_level":2,"summary":"建议呼吸内科就诊"}"#.into(),
    ));

    let res = h
        .engine
        .run_triage(TriageRequest::simple(
            "sess_pii",
            "pat_sha256_l2",
            "患者姓名：王五，手机13800138000，身份证110101199001011234，今天咳嗽喘息。",
        ))
        .await
        .unwrap();

    assert_eq!(res.response.status, TriageStatus::Completed);
    assert!(res.trace.patient_profile.loaded);

    // Check outbound LLM prompts were scrubbed
    let reqs = h.reasoning_llm.recorded_requests();
    assert!(!reqs.is_empty());
    let prompt_blob: String = reqs[0]
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!prompt_blob.contains("王五"));
    assert!(!prompt_blob.contains("13800138000"));
    assert!(!prompt_blob.contains("110101199001011234"));
    assert!(prompt_blob.contains("轻度哮喘"));
}

#[tokio::test]
async fn test_seasonal_epidemic_boost_changes_ranking() {
    let today = Utc::now().date_naive();
    let active_epi = EpidemicContext {
        context_id: "epi_flu".into(),
        region_code: "CN-110000".into(),
        region_name: "北京市".into(),
        season: Season::Winter,
        disease_name: "甲型流行性感冒".into(),
        seasonal_tag: "influenza_season".into(),
        intensity: 0.85,
        boost_symptoms: vec!["高热".into(), "干咳".into()],
        priority_question_hint: "近期是否有发热人员接触史".into(),
        active: true,
        valid_from: today - chrono::Duration::days(10),
        valid_to: today + chrono::Duration::days(10),
    };

    let h = build_harness(vec![active_epi]);

    // Seed two docs with equal authority_level=1.0:
    // doc_flu matches seasonal_tag "influenza_season"; doc_cold does not.
    // Give doc_cold slightly more symptom overlap, while doc_flu gets boosted by epidemic_intensity=0.85!
    h.knowledge.seed_documents(vec![
        KnowledgeDocument {
            doc_id: "doc_cold".into(),
            source_title: "普通感冒诊疗".into(),
            source_type: SourceType::Textbook,
            authority_level: 1.0,
            disease_name: "普通感冒".into(),
            aliases: vec![],
            department: "全科医学科".into(),
            secondary_departments: vec![],
            target_population: vec![TargetPopulation::Adult],
            section_type: SectionType::ClinicalManifestation,
            content: "发烧 发热 咽痛 干咳 流涕 鼻塞".into(),
            key_symptoms: vec!["发烧".into(), "咽痛".into(), "干咳".into()],
            differential_points: vec![],
            red_flag_signs: vec![],
            seasonal_tags: vec![],
            suggested_follow_up_slots: vec![],
            updated_at: None,
        },
        KnowledgeDocument {
            doc_id: "doc_flu".into(),
            source_title: "内科学（第9版）".into(),
            source_type: SourceType::Textbook,
            authority_level: 1.0,
            disease_name: "流行性感冒".into(),
            aliases: vec!["甲流".into()],
            department: "呼吸内科".into(),
            secondary_departments: vec!["发热门诊".into()],
            target_population: vec![TargetPopulation::Adult],
            section_type: SectionType::ClinicalManifestation,
            content: "高热 咽痛 干咳 肌肉酸痛".into(),
            key_symptoms: vec!["高热".into(), "咽痛".into(), "干咳".into()],
            differential_points: vec![],
            red_flag_signs: vec![],
            seasonal_tags: vec!["influenza_season".into()],
            suggested_follow_up_slots: vec![SuggestedFollowUpSlot {
                slot_key: "contact_history".into(),
                short_question: "近期接触过发热病人吗？".into(),
                quick_options: vec!["有接触".into(), "没有接触".into()],
            }],
            updated_at: None,
        },
    ]);

    let mut req = TriageRequest::simple("sess_epi", "pat_epi", "发烧咽痛干咳持续2天");
    req.region_code = Some("CN-110000".into());
    let res = h.engine.run_triage(req).await.unwrap();

    assert!(!res.trace.rag_evidence.items.is_empty());
    let top = &res.trace.rag_evidence.items[0];
    assert_eq!(top.doc_id, "doc_flu");
    assert!((top.seasonal_boost_applied - 0.85).abs() < 1e-4);
    assert!(
        res.trace
            .triage_slots
            .priority_question_hint_injected
            .as_deref()
            == Some("近期是否有发热人员接触史")
    );
    assert!(!res.response.seasonal_context_applied.is_empty());
}

#[tokio::test]
async fn test_total_llm_failure_invokes_fallback_rule_triage_system() {
    let h = build_harness(vec![]);
    // Force both Fast & Reasoning providers to fail
    h.fast_llm.push_step(MockStep::Fail(MockFailure::RateLimit429));
    h.reasoning_llm
        .push_step(MockStep::Fail(MockFailure::ServerError500));

    let res = h
        .engine
        .run_triage(TriageRequest::simple("sess_fb", "pat_fb", "心口窝疼伴胃胀"))
        .await
        .unwrap();

    assert_eq!(res.response.status, TriageStatus::Completed);
    assert!(res.response.is_fallback);
    let card = res.response.triage_result.as_ref().unwrap();
    assert_eq!(card.primary_department, "消化内科");
    assert_eq!(card.disclaimer, MEDICAL_DISCLAIMER);
}

#[tokio::test]
async fn test_stage5_consent_gating_and_l2_l3_consolidation() {
    let h = build_harness(vec![]);

    // 1. consent == false -> nothing written to L2/L3
    h.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"completed","primary_department":"呼吸内科","urgency_level":2,"summary":"建议呼吸内科就诊"}"#.into(),
    ));
    let mut req_no_consent = TriageRequest::simple("sess_nc", "pat_nc", "咳嗽两天");
    req_no_consent.consent_long_term_memory = Some(false);
    let r_nc = h.engine.run_triage(req_no_consent).await.unwrap();
    assert!(!r_nc.trace.consolidation_scheduled);
    assert!(
        h.stores
            .patient_memory
            .get_full_memory("pat_nc", 10)
            .await
            .unwrap()
            .is_none()
    );

    // 2. consent == true + Completed -> encrypted L2 + L3 persisted
    h.fast_llm.push_step(MockStep::Json(
        r#"{"rewritten_query":"咳嗽两天"}"#.into(),
    ));
    h.fast_llm.push_step(MockStep::Json(
        r#"{"chronic_diseases_add":["慢性支气管炎"],"allergies_add":[],"summary":"咳嗽两天建议呼吸内科排查"}"#.into(),
    ));
    h.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"completed","primary_department":"呼吸内科","urgency_level":2,"summary":"建议呼吸内科就诊"}"#.into(),
    ));
    let mut req_consent = TriageRequest::simple("sess_c", "pat_c", "咳嗽两天");
    req_consent.consent_long_term_memory = Some(true);
    let r_c = h.engine.run_triage(req_consent).await.unwrap();
    assert!(r_c.trace.consolidation_scheduled);
    assert!(r_c.trace.consolidation_completed);

    let stored = h
        .stores
        .patient_memory
        .get_full_memory("pat_c", 10)
        .await
        .unwrap()
        .expect("patient memory persisted");
    assert_eq!(stored.l2_profile.chronic_diseases, vec!["慢性支气管炎"]);
    assert_eq!(stored.l3_visit_events.len(), 1);
    assert_eq!(
        stored.l3_visit_events[0].recommended_department,
        "呼吸内科"
    );
}

#[tokio::test]
async fn test_hot_reload_arcswap_resources() {
    let h = build_harness(vec![]);

    // Initially "严重头痛伴喷射性呕吐" is not an Emergency120 rule
    let r1 = h
        .engine
        .run_triage(TriageRequest::simple("sess_hr1", "pat_hr", "剧烈头痛伴喷射性呕吐"))
        .await
        .unwrap();
    assert_ne!(r1.response.red_flag, RedFlagLevel::Emergency120);

    // Hot-swap dictionary with a new neuro emergency rule
    let mut new_dict = default_test_dictionary();
    new_dict.red_flag_rules.push(RedFlagRule {
        rule_id: "rf_neuro_ict".into(),
        level: RedFlagLevel::Emergency120,
        trigger_all_of: vec!["剧烈头痛".into()],
        trigger_any_of: vec!["喷射性呕吐".into()],
        alert_message: "提示颅内高压危急风险，请立即拨打 120！".into(),
    });
    let mut next_res = (*h.engine.resources()).clone();
    next_res.dictionary = Arc::new(CompiledMedicalDictionary::compile(new_dict).unwrap());
    h.engine.reload_resources(next_res);

    let r2 = h
        .engine
        .run_triage(TriageRequest::simple("sess_hr2", "pat_hr", "剧烈头痛伴喷射性呕吐"))
        .await
        .unwrap();
    assert_eq!(r2.response.status, TriageStatus::Emergency);
    assert_eq!(r2.response.red_flag, RedFlagLevel::Emergency120);
}

#[tokio::test]
async fn test_sse_streaming_triage_and_inactive_epidemic_control() {
    let today = Utc::now().date_naive();
    let inactive_epi = EpidemicContext {
        context_id: "epi_inactive".into(),
        region_code: "CN-110000".into(),
        region_name: "北京市".into(),
        season: Season::Winter,
        disease_name: "甲型流行性感冒".into(),
        seasonal_tag: "influenza_season".into(),
        intensity: 0.95,
        boost_symptoms: vec!["高热".into()],
        priority_question_hint: "不应激活".into(),
        active: false,
        valid_from: today - chrono::Duration::days(5),
        valid_to: today + chrono::Duration::days(5),
    };
    let h = build_harness(vec![inactive_epi]);
    h.knowledge.seed_documents(vec![KnowledgeDocument {
        doc_id: "doc_flu_inactive".into(),
        source_title: "内科学".into(),
        source_type: SourceType::Textbook,
        authority_level: 1.0,
        disease_name: "流行性感冒".into(),
        aliases: vec![],
        department: "呼吸内科".into(),
        secondary_departments: vec![],
        target_population: vec![TargetPopulation::Adult],
        section_type: SectionType::ClinicalManifestation,
        content: "高热 咽痛 干咳".into(),
        key_symptoms: vec!["高热".into(), "咽痛".into()],
        differential_points: vec![],
        red_flag_signs: vec![],
        seasonal_tags: vec!["influenza_season".into()],
        suggested_follow_up_slots: vec![],
        updated_at: None,
    }]);

    h.reasoning_llm.push_step(MockStep::StreamTokens(vec![
        r#"{"action":"completed","#.into(),
        r#""primary_department":"呼吸内科","#.into(),
        r#""urgency_level":2,"summary":"建议呼吸内科排查"}"#.into(),
    ]));

    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let mut req = TriageRequest::simple("sess_sse", "pat_sse", "发烧咽痛");
    req.region_code = Some("CN-110000".into());
    req.stream_tx = Some(tx);

    let res = h.engine.run_triage(req).await.unwrap();
    assert!(res.response.seasonal_context_applied.is_empty());
    assert_eq!(res.trace.rag_evidence.items[0].seasonal_boost_applied, 0.0);

    let mut delta_count = 0;
    let mut got_result = false;
    while let Some(ev) = rx.recv().await {
        match ev {
            TriageStreamEvent::Delta { .. } => delta_count += 1,
            TriageStreamEvent::Result(r) => {
                assert_eq!(r.status, TriageStatus::Completed);
                got_result = true;
                break;
            }
            TriageStreamEvent::Error { .. } => panic!("unexpected error event"),
        }
    }
    assert!(delta_count >= 1);
    assert!(got_result);
}

#[tokio::test]
async fn test_overlapping_synonym_shadowing_resolves_red_flag_and_scrubbed_chief_complaint() {
    let mut dict = default_test_dictionary();
    // "心口窝疼" -> "上腹痛" (longer), while "心口" -> "胸痛" (shorter)
    dict.symptom_synonyms.push(SymptomSynonym {
        standard_term: "胸痛".into(),
        colloquial_terms: vec!["心口".into()],
    });

    let h = build_harness(vec![]);
    let mut res = (*h.engine.resources()).clone();
    res.dictionary = Arc::new(CompiledMedicalDictionary::compile(dict).unwrap());
    h.engine.reload_resources(res);

    let r = h
        .engine
        .run_triage(TriageRequest::simple(
            "sess_shadow",
            "pat_shadow",
            "心口窝疼且大汗淋漓",
        ))
        .await
        .unwrap();

    assert_eq!(r.response.status, TriageStatus::Emergency);
    assert_eq!(r.response.red_flag, RedFlagLevel::Emergency120);
    assert_eq!(r.trace.llm_calls_made, 0);
    assert!(r.trace.extracted_symptoms.normalized_terms.contains(&"胸痛".to_string()));

    // Verify slots.chief_complaint uses scrubbed_text and preserves clinical phrase "患者..."
    let h2 = build_harness(vec![]);
    h2.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"follow_up","question":"发烧几天了？","quick_options":["1天","2天"]}"#.into(),
    ));
    let r2 = h2
        .engine
        .run_triage(TriageRequest::simple(
            "sess_scrub_slot",
            "pat_scrub_slot",
            "我是张伟，患者发热咳嗽三天",
        ))
        .await
        .unwrap();
    let cc = r2.trace.triage_slots.chief_complaint.as_deref().unwrap();
    assert!(!cc.contains("张伟"));
    assert!(cc.contains("患者发热咳嗽三天"));
}

#[tokio::test]
async fn test_default_dictionary_colloquial_emergency_single_and_cross_turn() {
    let default_dict = InMemoryDictionaryStore::with_defaults()
        .load_dictionaries()
        .await
        .unwrap();
    let h = build_harness(vec![]);
    let mut res = (*h.engine.resources()).clone();
    res.dictionary = Arc::new(CompiledMedicalDictionary::compile(default_dict).unwrap());
    h.engine.reload_resources(res);

    // Single-turn colloquial emergency
    let r1 = h
        .engine
        .run_triage(TriageRequest::simple(
            "sess_def_emg1",
            "pat_def_emg1",
            "我刚才心口窝撕裂样疼得厉害，而且一直大汗淋漓喘不过气",
        ))
        .await
        .unwrap();
    assert_eq!(r1.response.status, TriageStatus::Emergency);
    assert_eq!(r1.response.red_flag, RedFlagLevel::Emergency120);
    assert_eq!(r1.trace.llm_calls_made, 0);

    // Cross-turn colloquial emergency
    let t1 = h
        .engine
        .run_triage(TriageRequest::simple(
            "sess_def_cross",
            "pat_def_cross",
            "胸口有点闷痛",
        ))
        .await
        .unwrap();
    assert_ne!(t1.response.red_flag, RedFlagLevel::Emergency120);
    let calls_before = h.reasoning_llm.call_count();

    let t2 = h
        .engine
        .run_triage(TriageRequest::simple(
            "sess_def_cross",
            "pat_def_cross",
            "现在突然出冷汗、左臂发麻放射痛",
        ))
        .await
        .unwrap();
    assert_eq!(t2.response.status, TriageStatus::Emergency);
    assert_eq!(t2.response.red_flag, RedFlagLevel::Emergency120);
    assert_eq!(h.reasoning_llm.call_count(), calls_before);
    assert_eq!(t2.trace.llm_calls_made, 0);
}

#[tokio::test]
async fn test_quick_options_sanitized_against_diagnosis_and_dosage_leakage() {
    let h = build_harness(vec![]);
    h.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"follow_up","question":"还有其他症状吗？","quick_options":["确诊为急性支气管炎","阿莫西林胶囊 500mg 口服 每日三次","选项3","选项4","选项5","选项6"]}"#.into(),
    ));

    let res = h
        .engine
        .run_triage(TriageRequest::simple("sess_opts", "pat_opts", "咳嗽两天"))
        .await
        .unwrap();
    assert_eq!(res.response.status, TriageStatus::FollowUp);
    assert!((2..=4).contains(&res.response.quick_options.len()));
    for opt in &res.response.quick_options {
        assert!(
            check_medical_output_violations(opt).is_empty(),
            "quick_option contains violation: {opt}"
        );
        assert!(!opt.contains("确诊"));
        assert!(!opt.contains("500mg"));
        assert!(!opt.contains(MEDICAL_DISCLAIMER));
        assert!(opt.chars().count() <= 30);
    }

    // Even when ALL LLM quick_options are unsafe, safe defaults backfill 2..=4 options
    h.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"follow_up","question":"持续多久了？","quick_options":["初步诊断：急性胃肠炎","口服头孢克肟 200mg"]}"#.into(),
    ));
    let res2 = h
        .engine
        .run_triage(TriageRequest::simple("sess_opts2", "pat_opts2", "腹痛"))
        .await
        .unwrap();
    assert!((2..=4).contains(&res2.response.quick_options.len()));
    for opt in &res2.response.quick_options {
        assert!(check_medical_output_violations(opt).is_empty());
        assert!(!opt.contains(MEDICAL_DISCLAIMER));
    }
}

#[tokio::test]
async fn test_zero_intensity_epidemic_and_reasoning_prompt_label_preservation() {
    let today = Utc::now().date_naive();
    let zero_epi = EpidemicContext {
        context_id: "epi_zero".into(),
        region_code: "CN-110000".into(),
        region_name: "北京市".into(),
        season: Season::Winter,
        disease_name: "流行性感冒".into(),
        seasonal_tag: "influenza_season".into(),
        intensity: 0.0,
        boost_symptoms: vec!["高热".into()],
        priority_question_hint: "".into(),
        active: true,
        valid_from: today - chrono::Duration::days(5),
        valid_to: today + chrono::Duration::days(5),
    };
    let h = build_harness(vec![zero_epi]);
    h.knowledge.seed_documents(vec![KnowledgeDocument {
        doc_id: "doc_flu_zero".into(),
        source_title: "内科学".into(),
        source_type: SourceType::Textbook,
        authority_level: 1.0,
        disease_name: "流行性感冒".into(),
        aliases: vec![],
        department: "呼吸内科".into(),
        secondary_departments: vec![],
        target_population: vec![TargetPopulation::Adult],
        section_type: SectionType::ClinicalManifestation,
        content: "高热 咽痛 咳嗽".into(),
        key_symptoms: vec!["高热".into(), "咳嗽".into()],
        differential_points: vec![],
        red_flag_signs: vec![],
        seasonal_tags: vec!["influenza_season".into()],
        suggested_follow_up_slots: vec![],
        updated_at: None,
    }]);
    h.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"completed","primary_department":"呼吸内科","urgency_level":2,"summary":"建议呼吸内科就诊"}"#.into(),
    ));

    let mut req = TriageRequest::simple("sess_zero", "pat_zero", "患者发热咳嗽三天");
    req.region_code = Some("CN-110000".into());
    let res = h.engine.run_triage(req).await.unwrap();
    assert!(
        res.response.seasonal_context_applied.is_empty(),
        "zero-intensity epidemic must not report seasonal_context_applied: {:?}",
        res.response.seasonal_context_applied
    );
    assert_eq!(res.trace.rag_evidence.items[0].seasonal_boost_applied, 0.0);
    assert!(res.trace.rag_evidence.items[0].matched_seasonal_tags.is_empty());

    // Confirm outbound reasoning prompt preserved "患者本轮主诉:" and "患者发热咳嗽三天"
    let recorded = h.reasoning_llm.recorded_requests();
    let prompt_blob: String = recorded[0]
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(prompt_blob.contains("患者本轮主诉:患者发热咳嗽三天"));
    assert!(!prompt_blob.contains("[NAME_REDACTED]"));
}

#[tokio::test]
async fn test_triage_engine_blocks_colon_diagnosis_and_uppercase_dosage_in_options_and_summary() {
    let h = build_harness(vec![]);

    // FollowUp: LLM attempts to smuggle "确诊：急性支气管炎" and "阿莫西林胶囊 500MG 口服 每日三次" into quick_options
    h.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"follow_up","question":"目前咳嗽几天了？","quick_options":["确诊：急性支气管炎","阿莫西林胶囊 500MG 口服 每日三次","布洛芬 2粒/天","头孢克肟 2片，睡前口服","今天刚开始","超过3天"]}"#.into(),
    ));
    let follow_res = h
        .engine
        .run_triage(TriageRequest::simple("sess_adv_opts", "pat_adv_opts", "咳嗽两天"))
        .await
        .unwrap();
    assert_eq!(follow_res.response.status, TriageStatus::FollowUp);
    assert!((2..=4).contains(&follow_res.response.quick_options.len()));
    for opt in &follow_res.response.quick_options {
        assert!(
            check_medical_output_violations(opt).is_empty(),
            "unsafe quick_option leaked: {opt}"
        );
        assert!(!opt.contains("确诊"));
        assert!(!opt.contains("急性支气管炎"));
        assert!(!opt.contains("500MG"));
        assert!(!opt.contains("2粒/天"));
        assert!(!opt.contains("2片"));
    }
    assert!(
        follow_res
            .response
            .quick_options
            .contains(&"今天刚开始".to_string())
    );
    assert!(
        follow_res
            .response
            .quick_options
            .contains(&"超过3天".to_string())
    );

    // Completed: LLM emits colon diagnosis + uppercase dosage in summary -> neutralized!
    h.reasoning_llm.push_step(MockStep::Json(
        r#"{"action":"completed","primary_department":"呼吸内科","urgency_level":2,"summary":"确诊：急性支气管炎，建议阿莫西林胶囊 500MG 口服 每日三次"}"#.into(),
    ));
    let completed_res = h
        .engine
        .run_triage(TriageRequest::simple("sess_adv_sum", "pat_adv_sum", "咳嗽三天"))
        .await
        .unwrap();
    assert_eq!(completed_res.response.status, TriageStatus::Completed);
    let card = completed_res.response.triage_result.as_ref().unwrap();
    assert_eq!(card.disclaimer, MEDICAL_DISCLAIMER);
    assert!(
        check_medical_output_violations(&card.summary).is_empty(),
        "summary still contains violation: {}",
        card.summary
    );
    assert!(!card.summary.contains("确诊：急性支气管炎"));
    assert!(!card.summary.contains("急性支气管炎"));
    assert!(!card.summary.contains("500MG"));
    assert!(!card.summary.contains("每日三次"));
}

#[tokio::test]
async fn test_stage4_ignores_zero_intensity_epidemic_hint() {
    let today = Utc::now().date_naive();
    let zero_epi_with_hint = EpidemicContext {
        context_id: "epi_zero_hint".into(),
        region_code: "CN-110000".into(),
        region_name: "北京市".into(),
        season: Season::Winter,
        disease_name: "流行性感冒".into(),
        seasonal_tag: "influenza_season".into(),
        intensity: 0.0,
        boost_symptoms: vec!["高热".into()],
        priority_question_hint: "零强度流行病不应注入".into(),
        active: true,
        valid_from: today - chrono::Duration::days(5),
        valid_to: today + chrono::Duration::days(5),
    };
    let h = build_harness(vec![zero_epi_with_hint]);

    // Include duration so Stage 4 evaluates `has_epidemic` after duration slot is satisfied
    let mut req = TriageRequest::simple("sess_zero_hint", "pat_zero_hint", "发热咳嗽两天");
    req.region_code = Some("CN-110000".into());
    let res = h.engine.run_triage(req).await.unwrap();
    assert!(res.response.seasonal_context_applied.is_empty());
    assert!(res.trace.triage_slots.priority_question_hint_injected.is_none());
    assert_ne!(
        res.trace.triage_slots.next_target_slot.as_deref(),
        Some("contact_history")
    );
}

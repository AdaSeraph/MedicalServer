use chrono::NaiveDate;
use medical_core::models::{
    DataCollectionPackage, EpidemicContext, Gender, KnowledgeDocument, L2Profile, L3VisitEvent,
    MedicalDictionaries, PatientRecord, RedFlagLevel, RedFlagRule, Season, SectionType,
    SourceType, SuggestedFollowUpSlot, SymptomSynonym, TargetPopulation,
};
use medical_core::crypto::hash_patient_id;
use medical_core::validation::{Validate, validate_patient_id};

#[test]
fn test_schema_example_parses_and_validates() {
    let raw = include_str!("../../../doc/schemas/data-collection-schema.json");
    let pkg: DataCollectionPackage =
        serde_json::from_str(raw).expect("data-collection-schema.json must deserialize");

    assert_eq!(pkg.schema_version, "1.0.0");
    assert_eq!(pkg.knowledge_documents.len(), 1);
    assert_eq!(pkg.patient_records.len(), 1);
    assert_eq!(pkg.epidemic_contexts.len(), 1);
    let dict = pkg
        .medical_dictionaries
        .as_ref()
        .expect("medical_dictionaries present");
    assert_eq!(dict.symptom_synonyms.len(), 2);
    assert_eq!(dict.red_flag_rules.len(), 1);

    pkg.validate().expect("canonical schema example must validate");
}

#[test]
fn test_knowledge_doc_without_updated_at_deserializes_and_validates() {
    let json = r#"{
        "doc_id": "doc_no_ts_01",
        "source_title": "急诊医学指南",
        "source_type": "guideline",
        "authority_level": 0.95,
        "disease_name": "急性支气管炎",
        "department": "呼吸内科",
        "target_population": ["adult"],
        "section_type": "clinical_manifestation",
        "content": "急性起病，主要表现为咳嗽、咳痰，可伴低热或咽部不适。",
        "key_symptoms": ["咳嗽", "咳痰"]
    }"#;

    let doc: KnowledgeDocument = serde_json::from_str(json).expect("deserializes without updated_at");
    assert!(doc.updated_at.is_none());
    doc.validate().expect("validates cleanly");
}

#[test]
fn test_invalid_enums_rejected_by_serde() {
    let bad_source_type = r#"{
        "doc_id": "d1", "source_title": "s", "source_type": "blog_post",
        "authority_level": 0.8, "disease_name": "d", "department": "内科",
        "target_population": ["adult"], "section_type": "clinical_manifestation",
        "content": "c", "key_symptoms": ["发热"]
    }"#;
    assert!(serde_json::from_str::<KnowledgeDocument>(bad_source_type).is_err());

    let bad_population = r#"{
        "doc_id": "d1", "source_title": "s", "source_type": "textbook",
        "authority_level": 0.8, "disease_name": "d", "department": "内科",
        "target_population": ["martian"], "section_type": "clinical_manifestation",
        "content": "c", "key_symptoms": ["发热"]
    }"#;
    assert!(serde_json::from_str::<KnowledgeDocument>(bad_population).is_err());

    let bad_section = r#"{
        "doc_id": "d1", "source_title": "s", "source_type": "textbook",
        "authority_level": 0.8, "disease_name": "d", "department": "内科",
        "target_population": ["adult"], "section_type": "random_section",
        "content": "c", "key_symptoms": ["发热"]
    }"#;
    assert!(serde_json::from_str::<KnowledgeDocument>(bad_section).is_err());

    let bad_season = r#"{
        "context_id": "e1", "region_code": "CN-110000", "region_name": "北京",
        "season": "monsoon", "disease_name": "flu", "seasonal_tag": "flu",
        "intensity": 0.5, "active": true, "valid_from": "2026-01-01", "valid_to": "2026-02-01"
    }"#;
    assert!(serde_json::from_str::<EpidemicContext>(bad_season).is_err());

    let bad_rf_level = r#"{
        "rule_id": "r1", "level": "Critical999",
        "trigger_any_of": ["胸痛"], "alert_message": "alert"
    }"#;
    assert!(serde_json::from_str::<RedFlagRule>(bad_rf_level).is_err());
}

#[test]
fn test_knowledge_doc_validation_negative_cases() {
    let base = KnowledgeDocument {
        doc_id: "doc_1".into(),
        source_title: "内科学".into(),
        source_type: SourceType::Textbook,
        authority_level: 0.9,
        disease_name: "流感".into(),
        aliases: vec![],
        department: "呼吸内科".into(),
        secondary_departments: vec![],
        target_population: vec![TargetPopulation::Adult],
        section_type: SectionType::ClinicalManifestation,
        content: "发热、咳嗽".into(),
        key_symptoms: vec!["发热".into()],
        differential_points: vec![],
        red_flag_signs: vec![],
        seasonal_tags: vec![],
        suggested_follow_up_slots: vec![],
        updated_at: None,
    };

    // Out-of-range authority_level
    for bad_auth in [0.05_f32, 1.05_f32, f32::NAN] {
        let mut d = base.clone();
        d.authority_level = bad_auth;
        assert!(d.validate().is_err(), "authority_level={bad_auth} must fail");
    }

    // Empty target_population
    let mut d = base.clone();
    d.target_population.clear();
    assert!(d.validate().is_err());

    // Invalid follow-up slot: > 30 chars & < 2 quick options
    let mut d = base.clone();
    d.suggested_follow_up_slots.push(SuggestedFollowUpSlot {
        slot_key: "dur".into(),
        short_question: "请问您出现发热咳嗽全身酸痛乏力这些症状大概已经持续了多少天了呢？".into(),
        quick_options: vec!["1天".into()],
    });
    let err = d.validate().unwrap_err();
    assert!(err.errors.iter().any(|e| e.field.contains("short_question")));
    assert!(err.errors.iter().any(|e| e.field.contains("quick_options")));
}

#[test]
fn test_patient_id_rejects_plaintext_id_card_and_phone() {
    assert!(validate_patient_id("pat_sha256_a1b2c3d4e5f6").is_ok());
    // 18-digit ID card (exact & embedded)
    assert!(validate_patient_id("110101199001011234").is_err());
    assert!(validate_patient_id("11010119900101123X").is_err());
    assert!(validate_patient_id("pat_110101199001011234").is_err());
    // 15-digit ID card
    assert!(validate_patient_id("110101900101123").is_err());
    // 11-digit Chinese mobile (exact & embedded)
    assert!(validate_patient_id("13800138000").is_err());
    assert!(validate_patient_id("pat_13800138000").is_err());

    let bad_record = PatientRecord {
        patient_id: "13912345678".into(),
        consent_long_term_memory: true,
        l2_profile: L2Profile {
            age_group: "30-39".into(),
            gender: Gender::Female,
            ..Default::default()
        },
        l3_visit_events: vec![L3VisitEvent {
            event_id: "e1".into(),
            occurred_at: chrono::Utc::now(),
            chief_complaint: "发热".into(),
            extracted_symptoms: vec!["发热".into()],
            recommended_department: "呼吸内科".into(),
            urgency_level: 5, // invalid: out of 1..=4
            summary: "summary".into(),
        }],
    };
    let errs = bad_record.validate().unwrap_err();
    assert!(errs.errors.iter().any(|e| e.field == "patient_id"));
    assert!(errs.errors.iter().any(|e| e.field.contains("urgency_level")));
}

#[test]
fn test_epidemic_and_dictionary_negative_cases() {
    let bad_epi = EpidemicContext {
        context_id: "epi_1".into(),
        region_code: "CN-110000".into(),
        region_name: "北京".into(),
        season: Season::Winter,
        disease_name: "流感".into(),
        seasonal_tag: "flu".into(),
        intensity: 1.5,
        boost_symptoms: vec![],
        priority_question_hint: "".into(),
        active: true,
        valid_from: NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(),
        valid_to: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
    };
    let epi_errs = bad_epi.validate().unwrap_err();
    assert!(epi_errs.errors.iter().any(|e| e.field == "intensity"));
    assert!(epi_errs.errors.iter().any(|e| e.field == "valid_from"));

    let bad_dict = MedicalDictionaries {
        symptom_synonyms: vec![SymptomSynonym {
            standard_term: "上腹痛".into(),
            colloquial_terms: vec![],
        }],
        red_flag_rules: vec![RedFlagRule {
            rule_id: "rf_empty".into(),
            level: RedFlagLevel::Emergency120,
            trigger_all_of: vec![],
            trigger_any_of: vec![],
            alert_message: "".into(),
        }],
    };
    let dict_errs = bad_dict.validate().unwrap_err();
    assert!(dict_errs.errors.iter().any(|e| e.field.contains("colloquial_terms")));
    assert!(dict_errs.errors.iter().any(|e| e.field.contains("trigger_all_of")));
    assert!(dict_errs.errors.iter().any(|e| e.field.contains("alert_message")));
}

#[test]
fn test_validate_patient_id_canonical_sha256_and_separator_phones() {
    // Canonical 64-hex pat_sha256_ must pass unconditionally, even if hex contains 1[3-9]xxxxxxxxx
    let tricky_canonical = format!("pat_sha256_0000a13800138000{}", "b".repeat(48));
    assert_eq!(tricky_canonical.len(), "pat_sha256_".len() + 64);
    assert!(validate_patient_id(&tricky_canonical).is_ok());

    for i in 0..500 {
        let hashed = hash_patient_id(&format!("wx_openid_{i}"), b"medical_salt");
        assert!(
            validate_patient_id(&hashed).is_ok(),
            "valid hash_patient_id rejected: {hashed}"
        );
    }

    // Separator-formatted and +86 phones MUST be rejected
    for bad_phone in [
        "138-0013-8000",
        "pat_138-0013-8000",
        "138 0013 8000",
        "+86-138-0013-8000",
        "+86 138 0013 8000",
        "pat_sha256_13800138000",
    ] {
        assert!(
            validate_patient_id(bad_phone).is_err(),
            "plaintext phone in patient_id should be rejected: {bad_phone}"
        );
    }
}

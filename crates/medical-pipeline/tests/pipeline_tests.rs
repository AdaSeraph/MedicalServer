use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::NaiveDate;
use docx_rs::{Docx, Paragraph, Run};
use medical_core::llm::MockEmbeddingProvider;
use medical_core::models::{
    EpidemicContext, KnowledgeDocument, Season, SectionType, SourceType, TargetPopulation,
};
use medical_core::storage::{
    CheckpointStatus, InMemoryCheckpointStore, InMemoryKnowledgeStore, KnowledgeSearchEngine,
    KnowledgeSearchQuery,
};
use medical_pipeline::{
    BookMetadataDefaults, CheckpointManager, CustomChunkRule, DualWriteIndexer,
    HybridKnowledgeSearchEngine, OcrPlugin, PatientMaskingConfig, PipelineError, PipelineResult,
    TantivyKnowledgeIndex, export_error_samples_jsonl, import_patients_csv, import_patients_json,
    parse_collection_bundle_file, parse_docx_book, parse_markdown_book, parse_medical_book_file,
    parse_pdf_book, parse_plain_text_book, stream_knowledge_jsonl,
};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn test_parse_collection_bundle_valid_and_invalid() {
    let root = workspace_root();
    let schema_sample = root.join("doc/schemas/data-collection-schema.json");
    let pkg = parse_collection_bundle_file(&schema_sample).expect("valid schema sample");
    assert_eq!(pkg.knowledge_documents.len(), 1);
    assert_eq!(pkg.patient_records.len(), 1);
    assert_eq!(pkg.epidemic_contexts.len(), 1);

    // Verify every negative case in e2e/fixtures/knowledge/invalid_collection.json is rejected
    let invalid_raw =
        std::fs::read_to_string(root.join("e2e/fixtures/knowledge/invalid_collection.json"))
            .unwrap();
    let catalog: serde_json::Value = serde_json::from_str(&invalid_raw).unwrap();
    let cases = catalog["negative_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 13);
    for case in cases {
        let case_id = case["case_id"].as_str().unwrap();
        let entity_type = case["entity_type"].as_str().unwrap();
        let payload = &case["payload"];
        let bundle = match entity_type {
            "knowledge_document" => serde_json::json!({ "knowledge_documents": [payload] }),
            "patient_record" => serde_json::json!({ "patient_records": [payload] }),
            "epidemic_context" => serde_json::json!({ "epidemic_contexts": [payload] }),
            "red_flag_rule" => serde_json::json!({
                "medical_dictionaries": {
                    "symptom_synonyms": [],
                    "red_flag_rules": [payload]
                }
            }),
            other => panic!("unexpected entity_type: {other}"),
        };
        let res = medical_pipeline::parse_collection_bundle_str(&bundle.to_string());
        assert!(
            res.is_err(),
            "negative case `{case_id}` ({entity_type}) must be rejected"
        );
    }
}

#[test]
fn test_stream_knowledge_jsonl_bom_crlf_and_malformed_isolation() {
    let root = workspace_root();

    // 100 valid lines
    let path_100 = root.join("e2e/fixtures/knowledge/knowledge_100.jsonl");
    let r100 = stream_knowledge_jsonl(&path_100, 0, None).expect("stream 100");
    assert_eq!(r100.records.len(), 100);
    assert!(r100.errors.is_empty());

    // UTF-8 BOM + Windows CRLF
    let path_bom = root.join("e2e/fixtures/knowledge/bom_crlf.jsonl");
    let rbom = stream_knowledge_jsonl(&path_bom, 0, None).expect("stream bom_crlf");
    assert_eq!(rbom.records.len(), 10);
    assert!(rbom.errors.is_empty());

    // Malformed line 3 isolated without aborting valid lines
    let path_bad = root.join("e2e/fixtures/knowledge/malformed_line3.jsonl");
    let rbad = stream_knowledge_jsonl(&path_bad, 0, None).expect("stream malformed");
    assert!(!rbad.errors.is_empty(), "expected line-level parse/validation error");
    assert!(!rbad.records.is_empty(), "valid lines around malformed line should still parse");
}

#[test]
fn test_medical_book_markdown_txt_docx_and_pdf_ocr_hook() {
    let root = workspace_root();
    let defaults = BookMetadataDefaults {
        source_title: "内科学第9版".to_string(),
        source_type: SourceType::Textbook,
        authority_level: 1.0,
        default_department: "全科医学科".to_string(),
        default_population: vec![TargetPopulation::Adult],
        seasonal_tags: Vec::new(),
    };

    // 1. Markdown book
    let md_path = root.join("e2e/fixtures/knowledge/books_sample.md");
    let md_docs = parse_medical_book_file(&md_path, &defaults, None, None).expect("parse md");
    assert!(md_docs.len() >= 6);
    assert!(md_docs.iter().any(|d| d.disease_name == "流行性感冒" && d.department == "呼吸内科"));
    assert!(md_docs.iter().any(|d| d.disease_name == "急性心肌梗死" && d.department == "急诊科"));
    assert!(md_docs.iter().any(|d| d.section_type == SectionType::RedFlag));
    assert!(md_docs.iter().any(|d| d.seasonal_tags.contains(&"influenza_season".to_string())));

    // 2. Plain text with custom markers
    let txt_path = root.join("e2e/fixtures/knowledge/custom_markers.txt");
    let txt_content = std::fs::read_to_string(&txt_path).unwrap();
    let rule = CustomChunkRule::default();
    let txt_docs = parse_plain_text_book(&txt_content, &defaults, Some(&rule)).expect("parse txt");
    assert!(txt_docs.iter().any(|d| d.disease_name == "急性胃肠炎" && d.department == "消化内科"));
    assert!(txt_docs.iter().any(|d| d.disease_name == "偏头痛" && d.department == "神经内科"));

    // 3. DOCX generated in-memory via docx-rs
    let mut docx_buf = Cursor::new(Vec::new());
    Docx::new()
        .add_paragraph(Paragraph::new().add_run(Run::new().add_text("【疾病】急性支气管炎")))
        .add_paragraph(
            Paragraph::new().add_run(Run::new().add_text("【临床表现】咳嗽、咳痰，可伴低热乏力。")),
        )
        .add_paragraph(Paragraph::new().add_run(Run::new().add_text("【推荐科室】呼吸内科")))
        .build()
        .pack(&mut docx_buf)
        .expect("pack docx");
    let docx_docs =
        parse_docx_book(&docx_buf.into_inner(), &defaults, Some(&rule)).expect("parse docx");
    assert!(!docx_docs.is_empty());
    assert_eq!(docx_docs[0].disease_name, "急性支气管炎");
    assert_eq!(docx_docs[0].department, "呼吸内科");

    // 4. Scanned PDF triggers OCR plugin hook
    struct StubOcr;
    impl OcrPlugin for StubOcr {
        fn extract_text(&self, _pdf_bytes: &[u8]) -> PipelineResult<String> {
            Ok("# 过敏性鼻炎\n## 临床表现\n阵发性喷嚏、清水样涕、鼻塞。\n## 推荐科室\n耳鼻咽喉科".to_string())
        }
    }
    let fake_scanned_pdf = b"%PDF-1.4\n%%EOF";
    let err = parse_pdf_book(fake_scanned_pdf, &defaults, None, None);
    assert!(matches!(err, Err(PipelineError::ScannedPdfNeedsOcr)));

    let ocr_docs = parse_pdf_book(fake_scanned_pdf, &defaults, None, Some(&StubOcr))
        .expect("ocr plugin fallback");
    assert!(ocr_docs.iter().any(|d| d.disease_name == "过敏性鼻炎" && d.department == "耳鼻咽喉科"));

    // Also verify direct markdown string helper
    let direct_md = parse_markdown_book("# 测试疾病\n## 临床表现\n发热咳嗽", &defaults).unwrap();
    assert_eq!(direct_md.len(), 1);
}

#[test]
fn test_patient_csv_json_rayon_privacy_masking_and_deterministic_id() {
    let root = workspace_root();
    let cfg = PatientMaskingConfig::default();

    let csv_path = root.join("e2e/fixtures/patients/raw_pii_patients.csv");
    let csv_report = import_patients_csv(&csv_path, &cfg).expect("import csv");
    assert_eq!(csv_report.records.len(), 3);
    assert!(csv_report.failed_rows.is_empty());
    assert!(csv_report.detected_pii_columns.iter().any(|c| c.contains("身份证")));
    assert!(csv_report.detected_pii_columns.iter().any(|c| c.contains("手机")));

    let serialized_csv = serde_json::to_string(&csv_report.records).unwrap();
    for forbidden in [
        "张伟",
        "李敏",
        "王芳",
        "110101199001011234",
        "11010119850505122X",
        "310101199208082345",
        "13800138000",
        "13912345678",
        "13700001111",
        "幸福路10号",
    ] {
        assert!(
            !serialized_csv.contains(forbidden),
            "masked patient output leaked PII: {forbidden}"
        );
    }

    assert_eq!(csv_report.records[0].l2_profile.age_group, "30-39");
    assert_eq!(csv_report.records[1].l2_profile.age_group, "40-49");
    assert_eq!(csv_report.records[2].l2_profile.age_group, "60-69");

    // Cross-file deterministic salted SHA-256 patient_id check (CSV vs JSON)
    let json_path = root.join("e2e/fixtures/patients/patients_raw.json");
    let json_report = import_patients_json(&json_path, &cfg).expect("import json");
    assert_eq!(json_report.records.len(), 2);
    assert_eq!(
        csv_report.records[0].patient_id,
        json_report.records[0].patient_id,
        "patient_id for same id_card must be deterministic across CSV and JSON"
    );
    assert_eq!(
        csv_report.records[1].patient_id,
        json_report.records[1].patient_id
    );
    assert!(json_report.records[0].patient_id.starts_with("pat_sha256_"));
}

#[tokio::test]
async fn test_idempotent_upsert_by_doc_id() {
    let embedder = Arc::new(MockEmbeddingProvider::new(64));
    let vector_store = Arc::new(InMemoryKnowledgeStore::new());
    let tantivy = Arc::new(TantivyKnowledgeIndex::in_memory().expect("tantivy ram"));
    let indexer = DualWriteIndexer::new(embedder.clone(), vector_store.clone(), tantivy.clone());

    let mut doc = KnowledgeDocument {
        doc_id: "kb_upsert_001".to_string(),
        source_title: "测试指南".to_string(),
        source_type: SourceType::Guideline,
        authority_level: 1.0,
        disease_name: "急性扁桃体炎".to_string(),
        aliases: vec![],
        department: "耳鼻咽喉科".to_string(),
        secondary_departments: vec![],
        target_population: vec![TargetPopulation::Adult],
        section_type: SectionType::ClinicalManifestation,
        content: "咽痛伴发热第一版".to_string(),
        key_symptoms: vec!["咽痛".to_string(), "发热".to_string()],
        differential_points: vec![],
        red_flag_signs: vec![],
        seasonal_tags: vec![],
        suggested_follow_up_slots: vec![],
        reference_labs: vec![],
        reference_vitals: vec![],
        imaging_features: vec![],
        wound_features: vec![],
        trigger_substances: vec![],
        media_assets: vec![],
        updated_at: None,
    };

    indexer.upsert_documents(&[doc.clone()], "default").await.unwrap();
    assert_eq!(tantivy.num_docs(), 1);

    // Upsert same doc_id again with updated content -> count must remain 1
    doc.content = "剧烈咽痛伴高热吞咽困难第二版".to_string();
    indexer.upsert_documents(&[doc], "default").await.unwrap();
    assert_eq!(tantivy.num_docs(), 1, "upsert by doc_id must be idempotent without duplicates");

    let engine = HybridKnowledgeSearchEngine::new(embedder, vector_store, tantivy);
    let hits = engine
        .search(&KnowledgeSearchQuery {
            query_text: "吞咽困难".to_string(),
            query_vector: None,
            keywords: vec!["咽痛".to_string()],
            target_population: None,
            department: None,
            tenant_namespace: Some("default".to_string()),
            top_k: 5,
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].content.contains("第二版"));
}

#[tokio::test]
async fn test_resumable_checkpoint_import_and_error_export() {
    let root = workspace_root();
    let jsonl_path = root.join("e2e/fixtures/knowledge/knowledge_100.jsonl");

    let embedder = Arc::new(MockEmbeddingProvider::new(64));
    let vector_store = Arc::new(InMemoryKnowledgeStore::new());
    let tantivy = Arc::new(TantivyKnowledgeIndex::in_memory().expect("tantivy ram"));
    let cp_store = Arc::new(InMemoryCheckpointStore::new());

    let indexer = DualWriteIndexer::new(embedder, vector_store, tantivy.clone())
        .with_checkpoint_store(cp_store.clone())
        .with_batch_size(20);

    // First run pauses after 2 batches (40 items)
    let p1 = indexer
        .ingest_knowledge_jsonl_resumable("job_kb_100", &jsonl_path, "default", Some(2))
        .await
        .expect("paused run");
    assert_eq!(p1.status, CheckpointStatus::Paused);
    assert_eq!(p1.processed_items, 40);
    assert!(!p1.completed);
    assert!(p1.byte_offset > 0);
    assert!(p1.throughput_items_per_sec > 0.0);

    // Second run resumes from saved byte_offset and finishes remaining 60 items
    let p2 = indexer
        .ingest_knowledge_jsonl_resumable("job_kb_100", &jsonl_path, "default", None)
        .await
        .expect("resumed run");
    assert_eq!(p2.status, CheckpointStatus::Completed);
    assert_eq!(p2.processed_items, 100);
    assert_eq!(p2.failed_items, 0);
    assert!(p2.completed);
    assert_eq!(tantivy.num_docs(), 100);

    let mgr = CheckpointManager::new(cp_store);
    assert!(mgr.get("job_kb_100").await.unwrap().is_some());

    // Verify error export helper
    let tmp_err = std::env::temp_dir().join("medical_pipeline_test_errors.jsonl");
    let written = export_error_samples_jsonl(&["line 3: invalid json".to_string()], &tmp_err).unwrap();
    assert_eq!(written, 1);
    let _ = std::fs::remove_file(&tmp_err);
}

#[tokio::test]
async fn test_hybrid_search_hard_filters_and_seasonal_epidemic_boost() {
    let embedder = Arc::new(MockEmbeddingProvider::new(64));
    let vector_store = Arc::new(InMemoryKnowledgeStore::new());
    let tantivy = Arc::new(TantivyKnowledgeIndex::in_memory().expect("tantivy ram"));
    let indexer = DualWriteIndexer::new(embedder.clone(), vector_store.clone(), tantivy.clone());

    let flu_doc = KnowledgeDocument {
        doc_id: "kb_flu_01".to_string(),
        source_title: "流感诊疗方案".to_string(),
        source_type: SourceType::Guideline,
        authority_level: 1.0,
        disease_name: "甲型流行性感冒".to_string(),
        aliases: vec!["甲流".to_string()],
        department: "呼吸内科".to_string(),
        secondary_departments: vec!["发热门诊".to_string()],
        target_population: vec![TargetPopulation::Adult, TargetPopulation::Child],
        section_type: SectionType::ClinicalManifestation,
        content: "冬春季高发，突发高热、全身肌肉酸痛、咽痛、干咳。".to_string(),
        key_symptoms: vec!["高热".to_string(), "肌肉酸痛".to_string(), "咳嗽".to_string()],
        differential_points: vec![],
        red_flag_signs: vec![],
        seasonal_tags: vec!["influenza".to_string(), "winter".to_string()],
        suggested_follow_up_slots: vec![],
        reference_labs: vec![],
        reference_vitals: vec![],
        imaging_features: vec![],
        wound_features: vec![],
        trigger_substances: vec![],
        media_assets: vec![],
        updated_at: None,
    };

    let ortho_doc = KnowledgeDocument {
        doc_id: "kb_ortho_01".to_string(),
        source_title: "骨科手册".to_string(),
        source_type: SourceType::Textbook,
        authority_level: 0.9,
        disease_name: "腰椎间盘突出".to_string(),
        aliases: vec![],
        department: "骨科".to_string(),
        secondary_departments: vec![],
        target_population: vec![TargetPopulation::Adult, TargetPopulation::Elderly],
        section_type: SectionType::ClinicalManifestation,
        content: "腰痛伴下肢放射痛麻木。".to_string(),
        key_symptoms: vec!["腰痛".to_string(), "下肢麻木".to_string()],
        differential_points: vec![],
        red_flag_signs: vec![],
        seasonal_tags: vec![],
        suggested_follow_up_slots: vec![],
        reference_labs: vec![],
        reference_vitals: vec![],
        imaging_features: vec![],
        wound_features: vec![],
        trigger_substances: vec![],
        media_assets: vec![],
        updated_at: None,
    };

    indexer
        .upsert_documents(&[flu_doc, ortho_doc], "default")
        .await
        .unwrap();

    let engine = HybridKnowledgeSearchEngine::new(embedder, vector_store, tantivy);

    // Hard department filter: 骨科 must never return 呼吸内科
    let ortho_hits = engine
        .search(&KnowledgeSearchQuery {
            query_text: "高热 咳嗽 腰痛".to_string(),
            query_vector: None,
            keywords: vec![],
            target_population: Some(vec![TargetPopulation::Elderly]),
            department: Some("骨科".to_string()),
            tenant_namespace: Some("default".to_string()),
            top_k: 5,
        })
        .await
        .unwrap();
    assert!(!ortho_hits.is_empty());
    assert!(ortho_hits.iter().all(|h| h.department == "骨科"));

    // Seasonal epidemic boost increases influenza score
    let q = KnowledgeSearchQuery {
        query_text: "高热 肌肉酸痛 咳嗽".to_string(),
        query_vector: None,
        keywords: vec!["高热".to_string()],
        target_population: None,
        department: None,
        tenant_namespace: Some("default".to_string()),
        top_k: 5,
    };
    let (base_hits, base_applied) = engine
        .search_with_seasonal_boost(&q, &[], None)
        .await
        .unwrap();
    assert!(base_applied.is_empty());
    let base_flu = base_hits.iter().find(|h| h.doc_id == "kb_flu_01").unwrap().rrf_score;

    let active_epi = EpidemicContext {
        context_id: "epi_bj_flu".to_string(),
        region_code: "CN-110000".to_string(),
        region_name: "北京市".to_string(),
        season: Season::Winter,
        disease_name: "甲型流感".to_string(),
        seasonal_tag: "influenza".to_string(),
        intensity: 0.9,
        boost_symptoms: vec!["高热".to_string()],
        priority_question_hint: "身边有人发烧吗？".to_string(),
        active: true,
        valid_from: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        valid_to: NaiveDate::from_ymd_opt(2027, 3, 1).unwrap(),
    };
    let (boosted_hits, applied) = engine
        .search_with_seasonal_boost(&q, &[active_epi], None)
        .await
        .unwrap();
    assert!(applied.contains(&"甲型流感".to_string()));
    let boosted_flu = boosted_hits.iter().find(|h| h.doc_id == "kb_flu_01").unwrap().rrf_score;
    assert!(
        boosted_flu > base_flu * 1.25,
        "expected seasonal boost (~1 + 0.35 * 0.9 = 1.315x), base={base_flu}, boosted={boosted_flu}"
    );
}

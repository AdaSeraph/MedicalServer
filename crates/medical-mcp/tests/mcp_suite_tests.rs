use medical_mcp::McpServer;
use medical_mcp::backend::McpBackend;
use medical_mcp::protocol::JsonRpcRequest;
use medical_server::ServerConfig;
use serde_json::{Value, json};

async fn build_test_mcp() -> McpServer {
    let mut cfg = ServerConfig::from_env();
    cfg.offline_mode = true;
    let backend = McpBackend::new_standalone(cfg)
        .await
        .expect("standalone backend");
    McpServer::new(backend)
}

async fn rpc(server: &McpServer, id: i64, method: &str, params: Value) -> Value {
    let resp = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(id)),
            method: method.to_string(),
            params: Some(params),
        })
        .await
        .expect("expected JSON-RPC response");
    assert!(
        resp.error.is_none(),
        "unexpected JSON-RPC error for {method}: {:?}",
        resp.error
    );
    resp.result.expect("result present")
}

async fn call_tool_json(server: &McpServer, id: i64, name: &str, arguments: Value) -> Value {
    let result = rpc(
        server,
        id,
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    )
    .await;
    assert_ne!(
        result.get("isError").and_then(Value::as_bool),
        Some(true),
        "tool {name} returned error: {result}"
    );
    let text = result["content"][0]["text"]
        .as_str()
        .expect("text content");
    serde_json::from_str(text).expect("valid JSON tool payload")
}

#[tokio::test]
async fn test_mcp_protocol_handshake_resources_and_prompts() {
    let server = build_test_mcp().await;

    let init = rpc(
        &server,
        1,
        "initialize",
        json!({ "protocolVersion": "2024-11-05" }),
    )
    .await;
    assert_eq!(init["serverInfo"]["name"], "medical-server-mcp");
    assert_eq!(init["protocolVersion"], "2024-11-05");

    let tools_res = rpc(&server, 2, "tools/list", json!({})).await;
    let tools = tools_res["tools"].as_array().expect("tools array");
    assert!(tools.len() >= 19, "expected >= 19 MCP tools, got {}", tools.len());

    let res_list = rpc(&server, 3, "resources/list", json!({})).await;
    assert_eq!(res_list["resources"].as_array().unwrap().len(), 4);

    let catalog_res = rpc(
        &server,
        4,
        "resources/read",
        json!({ "uri": "medical://catalog/clinical-types" }),
    )
    .await;
    let catalog_text = catalog_res["contents"][0]["text"].as_str().unwrap();
    assert!(catalog_text.contains("complete_blood_count"));
    assert!(catalog_text.contains("wound_image"));
    assert!(catalog_text.contains("trigger_substance_image"));

    let prompts_res = rpc(&server, 5, "prompts/list", json!({})).await;
    assert_eq!(prompts_res["prompts"].as_array().unwrap().len(), 3);

    let prompt = rpc(
        &server,
        6,
        "prompts/get",
        json!({
            "name": "import_clinical_case_with_exams",
            "arguments": { "patient_id": "pat_sha256_demo" }
        }),
    )
    .await;
    assert!(
        prompt["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("patient_add_clinical_exam")
    );
}

#[tokio::test]
async fn test_mcp_clinical_exams_in_knowledge_base_and_patient_cases() {
    let server = build_test_mcp().await;

    // 1. Discover clinical data catalog via MCP tool
    let catalog = call_tool_json(&server, 10, "get_clinical_data_catalog", json!({})).await;
    assert!(!catalog["common_lab_analytes"].as_array().unwrap().is_empty());
    assert!(!catalog["imaging_modalities"].as_array().unwrap().is_empty());
    assert!(!catalog["wound_time_dimensions"].as_array().unwrap().is_empty());

    // 2. Import multimodal KnowledgeDocument (labs + vitals + MRI + wound + trigger substance + images)
    let import_kb = call_tool_json(
        &server,
        11,
        "knowledge_import",
        json!({
            "tenant_namespace": "default",
            "documents": [{
                "doc_id": "kb_diabetic_foot_mri_01",
                "source_title": "糖尿病足创面与影像诊疗指南",
                "source_type": "guideline",
                "authority_level": 0.98,
                "disease_name": "糖尿病足伴骨髓炎",
                "aliases": ["糖足溃疡"],
                "department": "内分泌科",
                "secondary_departments": ["创面修复科", "骨科"],
                "target_population": ["adult", "elderly"],
                "section_type": "imaging_atlas",
                "content": "糖尿病足深部溃疡合并感染时需评估血常规、CRP、糖化血红蛋白及足部核磁共振(MRI)排查骨髓炎。",
                "key_symptoms": ["足部溃疡", "伤口渗液", "红肿"],
                "reference_labs": [
                    {
                        "loinc_code": "4548-4",
                        "item_code": "HbA1c",
                        "item_name": "糖化血红蛋白",
                        "value_numeric": 8.5,
                        "unit": "%",
                        "reference_range": "4.0-6.0",
                        "abnormal_flag": "high"
                    },
                    {
                        "loinc_code": "1988-5",
                        "item_code": "CRP",
                        "item_name": "C反应蛋白",
                        "value_numeric": 45.0,
                        "unit": "mg/L",
                        "reference_range": "<10.0",
                        "abnormal_flag": "high"
                    }
                ],
                "reference_vitals": ["体温 ≥38.0℃ 提示全身感染风险", "血压需控制在 <140/90 mmHg"],
                "imaging_features": [{
                    "modality": "mri",
                    "exam_name": "足部核磁共振(MRI)",
                    "body_part": "足部",
                    "typical_findings": "骨髓T1低信号、T2及STIR高信号伴骨皮质破坏",
                    "diagnostic_significance": "确诊合并骨髓炎的金标准影像",
                    "reference_images": [{
                        "asset_id": "ref_mri_01",
                        "category": "mri_image",
                        "title": "足部MRI骨髓炎典型征象",
                        "mime_type": "image/png",
                        "uri_or_path": "atlas/diabetic_foot_mri.png",
                        "tags": ["核磁", "骨髓炎"]
                    }]
                }],
                "wound_features": [{
                    "wound_type": "糖尿病足神经缺血性溃疡",
                    "common_sites": ["足底承重区", "足趾"],
                    "visual_characteristics": "基底见黄色腐肉及脓性渗液，创缘胼胝增厚",
                    "warning_signs": ["恶臭", "黑痂坏死", "探针触及骨质"],
                    "reference_images": [{
                        "asset_id": "ref_wound_01",
                        "category": "wound_image",
                        "title": "糖尿病足Wagner3级伤口参考图",
                        "mime_type": "image/jpeg",
                        "uri_or_path": "atlas/diabetic_foot_wound.jpg",
                        "tags": ["伤口", "溃疡"]
                    }]
                }],
                "trigger_substances": [{
                    "trigger_id": "trig_kb_01",
                    "category": "contact_material",
                    "substance_name": "劣质硬底磨脚鞋具/热水泡脚烫伤",
                    "exposure_route": "局部摩擦或热接触",
                    "reaction_symptoms": ["水疱破溃", "深部感染"],
                    "images": [{
                        "asset_id": "ref_trig_01",
                        "category": "trigger_substance_image",
                        "title": "高危磨脚鞋具诱因示例",
                        "mime_type": "image/jpeg",
                        "uri_or_path": "atlas/trigger_shoe.jpg",
                        "tags": ["诱因"]
                    }]
                }]
            }]
        }),
    )
    .await;
    assert_eq!(import_kb["imported"], 1);

    // 3. Search Knowledge Base by newly indexed clinical terms ("骨髓炎", "核磁共振", "糖化血红蛋白")
    let search = call_tool_json(
        &server,
        12,
        "knowledge_search",
        json!({ "query": "足部核磁共振 骨髓炎 糖化血红蛋白", "top_k": 5 }),
    )
    .await;
    let hits = search["results"].as_array().unwrap();
    assert!(
        hits.iter().any(|h| h["doc_id"] == "kb_diabetic_foot_mri_01"),
        "expected multimodal knowledge doc to be searchable via MRI/lab terms: {search}"
    );

    // 4. Attach multimodal clinical exams (blood pressure, blood biochem, MRI, wound photo, trigger photo) to a patient case
    let pid = "pat_sha256_00000000000000000000000000000000000000000000000000000000000000aa";
    let attached = call_tool_json(
        &server,
        13,
        "patient_add_clinical_exam",
        json!({
            "patient_id": pid,
            "chief_complaint": "右足背外伤后红肿化脓伴发热，血压偏高",
            "recommended_department": "创面修复科",
            "urgency_level": 2,
            "clinical_exams": {
                "vital_signs": [{
                    "record_id": "vs_01",
                    "systolic_bp_mmhg": 152.0,
                    "diastolic_bp_mmhg": 94.0,
                    "heart_rate_bpm": 92.0,
                    "temperature_c": 38.2,
                    "spo2_percent": 98.0,
                    "blood_glucose_mmol_l": 11.4
                }],
                "lab_panels": [{
                    "panel_id": "lab_01",
                    "category": "complete_blood_count",
                    "panel_name": "急诊血常规+CRP",
                    "specimen": "静脉全血",
                    "items": [
                        { "loinc_code": "6690-2", "item_code": "WBC", "item_name": "白细胞计数", "value_numeric": 14.8, "unit": "10^9/L", "abnormal_flag": "high" },
                        { "loinc_code": "1988-5", "item_code": "CRP", "item_name": "C反应蛋白", "value_numeric": 56.2, "unit": "mg/L", "abnormal_flag": "high" }
                    ]
                }],
                "imaging_exams": [{
                    "exam_id": "mri_01",
                    "modality": "mri",
                    "exam_name": "右足核磁共振平扫",
                    "body_part": "右足",
                    "findings": "右足背软组织肿胀伴局部渗出，骨质未见明显破坏",
                    "impression": "软组织感染，暂除外骨髓炎",
                    "images": [{
                        "asset_id": "img_mri_01",
                        "category": "mri_image",
                        "title": "右足MRI冠状位",
                        "mime_type": "image/png",
                        "data_base64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
                        "tags": ["核磁"]
                    }]
                }],
                "wound_assessments": [{
                    "wound_id": "w_01",
                    "body_site": "右足背",
                    "wound_type": "外伤感染创面",
                    "length_cm": 3.5,
                    "width_cm": 2.0,
                    "depth_cm": 0.4,
                    "tissue_type": "黄色腐肉伴肉芽",
                    "infection_signs": ["红肿", "脓性分泌物"],
                    "exudate_level": "中量脓性",
                    "edge_status": "轻度浸渍",
                    "images": [{
                        "asset_id": "img_wound_01",
                        "category": "wound_image",
                        "title": "右足背伤口首诊照片",
                        "mime_type": "image/jpeg",
                        "uri_or_path": "cases/right_foot_wound.jpg",
                        "tags": ["伤口"]
                    }]
                }],
                "trigger_substances": [{
                    "trigger_id": "trig_01",
                    "category": "insect_animal",
                    "substance_name": "隐翅虫接触毒液",
                    "exposure_route": "皮肤接触",
                    "reaction_symptoms": ["条状红斑", "脓疱破溃"],
                    "severity": "moderate",
                    "images": [{
                        "asset_id": "img_trig_01",
                        "category": "trigger_substance_image",
                        "title": "患者拍摄疑似诱因昆虫图",
                        "mime_type": "image/jpeg",
                        "uri_or_path": "cases/paederus_trigger.jpg",
                        "tags": ["诱因物质"]
                    }]
                }]
            }
        }),
    )
    .await;
    assert_eq!(attached["saved"], true);

    // 5. Read back encrypted patient memory via MCP and verify all clinical exam data persisted
    let mem = call_tool_json(
        &server,
        14,
        "patient_get_memory",
        json!({ "patient_id": pid }),
    )
    .await;
    assert_eq!(mem["l2_profile"]["baseline_vitals"]["systolic_bp_mmhg"], 152.0);
    assert_eq!(mem["l2_profile"]["trigger_substances"][0]["substance_name"], "隐翅虫接触毒液");
    let exams = &mem["l3_visit_events"][0]["clinical_exams"];
    assert_eq!(exams["lab_panels"][0]["items"][0]["item_code"], "WBC");
    assert_eq!(exams["imaging_exams"][0]["modality"], "mri");
    assert_eq!(exams["wound_assessments"][0]["body_site"], "右足背");
    assert_eq!(exams["trigger_substances"][0]["images"][0]["category"], "trigger_substance_image");

    // 6. Batch import CSV with clinical exam columns (血压, 抽血生化, 核磁影像, 伤口图片, 诱因物质)
    let csv = "姓名,身份证号,年龄,性别,主诉,血压,抽血生化,核磁影像,伤口评估,伤口图片,诱因物质,诱因图片\n张三,110101198501011234,40岁,男,头晕伴皮疹,158/96,WBC:11.5 10^9/L;ALT:65 U/L,颅脑MRI未见急性梗死,左前臂红斑水疱,arm_wound.jpg,染发剂,hair_dye.jpg\n";
    let batch = call_tool_json(
        &server,
        15,
        "patient_import_batch",
        json!({ "format": "csv", "content": csv, "default_consent": true }),
    )
    .await;
    assert_eq!(batch["persisted_patients"], 1);
    assert_eq!(batch["persisted_events"], 1);

    // 7. Dictionary & Epidemic & Triage operations via MCP
    let syn = call_tool_json(
        &server,
        16,
        "dictionary_upsert_synonym",
        json!({ "standard_term": "高血压急症", "colloquial_terms": ["血压飙升", "高压爆表"] }),
    )
    .await;
    assert_eq!(syn["saved"], true);

    let triage = call_tool_json(
        &server,
        17,
        "triage_chat",
        json!({
            "session_id": "mcp_sess_01",
            "patient_id": pid,
            "utterance": "突发胸痛放射左臂伴大汗淋漓"
        }),
    )
    .await;
    assert_eq!(triage["status"], "emergency");
    assert_eq!(triage["red_flag"], "Emergency120");

    // 8. Compliance wipe
    let wiped = call_tool_json(
        &server,
        18,
        "patient_delete_memory",
        json!({ "patient_id": pid }),
    )
    .await;
    assert_eq!(wiped["deleted"], true);
}

#[tokio::test]
async fn test_headless_persistence_survives_close_and_restart() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "medical_mcp_persist_test_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let pid = "pat_sha256_00000000000000000000000000000000000000000000000000000000000000bb";

    // Session 1: Boot headless standalone MCP with persistent data_dir, write knowledge & patient exams, then close
    {
        let mut cfg = ServerConfig::from_env().with_persistent_data_dir(tmp_dir.clone());
        cfg.offline_mode = true;
        let backend = McpBackend::new_standalone(cfg)
            .await
            .expect("session 1 backend");
        let server = McpServer::new(backend);

        let kb = call_tool_json(
            &server,
            100,
            "knowledge_import",
            json!({
                "documents": [{
                    "doc_id": "kb_persist_mri_01",
                    "source_title": "持久化验证指南",
                    "source_type": "guideline",
                    "authority_level": 0.95,
                    "disease_name": "脑梗死超急性期",
                    "department": "神经内科",
                    "target_population": ["adult", "elderly"],
                    "section_type": "imaging_atlas",
                    "content": "颅脑MRI弥散加权成像(DWI)呈高信号，需立即静脉溶栓评估。",
                    "key_symptoms": ["偏瘫", "言语不清"],
                    "reference_vitals": ["血压控制目标 <180/105 mmHg"]
                }]
            }),
        )
        .await;
        assert_eq!(kb["imported"], 1);

        let exam = call_tool_json(
            &server,
            101,
            "patient_add_clinical_exam",
            json!({
                "patient_id": pid,
                "chief_complaint": "突发右侧肢体无力2小时",
                "recommended_department": "神经内科",
                "urgency_level": 1,
                "clinical_exams": {
                    "vital_signs": [{
                        "record_id": "vs_persist_1",
                        "systolic_bp_mmhg": 168.0,
                        "diastolic_bp_mmhg": 98.0
                    }],
                    "wound_assessments": [{
                        "wound_id": "w_persist_1",
                        "body_site": "右膝部",
                        "wound_type": "跌倒擦伤",
                        "images": [{
                            "asset_id": "img_persist_1",
                            "category": "wound_image",
                            "title": "右膝擦伤照",
                            "mime_type": "image/jpeg",
                            "uri_or_path": "wounds/knee.jpg"
                        }]
                    }]
                }
            }),
        )
        .await;
        assert_eq!(exam["saved"], true);

        let saved = call_tool_json(&server, 102, "admin_save_snapshot", json!({})).await;
        assert_eq!(saved["saved"], true);
    }

    // Verify encrypted snapshot file & Tantivy directory exist on disk after Session 1 closed
    assert!(tmp_dir.join("medical_local.enc").is_file());
    assert!(tmp_dir.join("tantivy").is_dir());

    // Session 2: Boot a fresh headless MCP server on the same data_dir and verify data survived
    {
        let mut cfg = ServerConfig::from_env().with_persistent_data_dir(tmp_dir.clone());
        cfg.offline_mode = true;
        let backend = McpBackend::new_standalone(cfg)
            .await
            .expect("session 2 backend");
        let server = McpServer::new(backend);

        let search = call_tool_json(
            &server,
            200,
            "knowledge_search",
            json!({ "query": "脑梗死超急性期 DWI", "top_k": 5 }),
        )
        .await;
        let hits = search["results"].as_array().unwrap();
        assert!(
            hits.iter().any(|h| h["doc_id"] == "kb_persist_mri_01"),
            "knowledge document must survive restart: {search}"
        );

        let mem = call_tool_json(
            &server,
            201,
            "patient_get_memory",
            json!({ "patient_id": pid }),
        )
        .await;
        assert_eq!(mem["l2_profile"]["baseline_vitals"]["systolic_bp_mmhg"], 168.0);
        assert_eq!(
            mem["l3_visit_events"][0]["clinical_exams"]["wound_assessments"][0]["body_site"],
            "右膝部"
        );
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_hospital_integration_qr_pull_and_fhir_bundle_import() {
    let server = build_test_mcp().await;
    let pid = "pat_sha256_00000000000000000000000000000000000000000000000000000000000000cc";

    // 1. Register a hospital HIS/LIS/PACS connector
    let reg = call_tool_json(
        &server,
        300,
        "hospital_upsert_connector",
        json!({
            "connector_id": "hosp_bj_01",
            "hospital_code": "BJ001",
            "hospital_name": "北京协和医院(预留对接)",
            "kind": "hl7_fhir_r4",
            "base_url": "mock://hospital-fhir.local",
            "auth_mode": "bearer",
            "api_secret": "hosp-secret-token",
            "enabled": true
        }),
    )
    .await;
    assert_eq!(reg["saved"], true);

    let list = call_tool_json(&server, 301, "hospital_list_connectors", json!({})).await;
    assert_eq!(list["total"], 1);
    assert_eq!(list["connectors"][0]["api_secret"], "***REDACTED***");

    // 2. Pull report from a mobile QR code / cloud-film URL
    let pulled = call_tool_json(
        &server,
        302,
        "hospital_pull_report",
        json!({
            "patient_id": pid,
            "qr_code_payload": "https://cloudfilm.hosp.cn/view?hosp=BJ001&report_no=MRI2026112201&type=mri",
            "auto_attach_to_patient": true
        }),
    )
    .await;
    assert_eq!(pulled["pii_scrubbed"], true);
    assert!(pulled["attached_event_id"].as_str().unwrap().contains("MRI2026112201"));

    // 3. Import a standard HL7 FHIR R4 Bundle (Blood Pressure + WBC + Brain MRI DiagnosticReport)
    let fhir_res = call_tool_json(
        &server,
        303,
        "hospital_import_fhir_bundle",
        json!({
            "patient_id": pid,
            "hospital_name": "市第一人民医院LIS/PACS",
            "report_no": "FHIR20261122",
            "fhir_bundle": {
                "resourceType": "Bundle",
                "type": "collection",
                "entry": [
                    {
                        "resource": {
                            "resourceType": "Observation",
                            "code": { "coding": [{ "system": "http://loinc.org", "code": "85354-9", "display": "Blood pressure" }] },
                            "component": [
                                { "code": { "coding": [{ "code": "8480-6" }] }, "valueQuantity": { "value": 146, "unit": "mmHg" } },
                                { "code": { "coding": [{ "code": "8462-4" }] }, "valueQuantity": { "value": 92, "unit": "mmHg" } }
                            ]
                        }
                    },
                    {
                        "resource": {
                            "resourceType": "Observation",
                            "code": { "coding": [{ "system": "http://loinc.org", "code": "6690-2", "display": "白细胞计数" }] },
                            "valueQuantity": { "value": 12.6, "unit": "10^9/L" },
                            "referenceRange": [{ "text": "3.5-9.5" }],
                            "interpretation": [{ "coding": [{ "code": "H" }] }]
                        }
                    },
                    {
                        "resource": {
                            "resourceType": "DiagnosticReport",
                            "id": "dr-mri-01",
                            "code": { "text": "颅脑MRI平扫" },
                            "conclusion": "右侧额叶少许缺血灶，未见急性梗死。"
                        }
                    }
                ]
            }
        }),
    )
    .await;
    assert_eq!(fhir_res["imported"], true);
    assert_eq!(
        fhir_res["clinical_exams"]["vital_signs"][0]["systolic_bp_mmhg"],
        146.0
    );
    assert_eq!(
        fhir_res["clinical_exams"]["lab_panels"][0]["items"][0]["loinc_code"],
        "6690-2"
    );
    assert_eq!(
        fhir_res["clinical_exams"]["imaging_exams"][0]["modality"],
        "mri"
    );
}

use axum::http::Method;
use serde_json::{Value, json};

use crate::backend::McpBackend;
use crate::protocol::{McpToolCallResult, McpToolDefinition};

/// Returns the complete catalog of MCP tools exposed by MedicalServer.
pub fn list_tools() -> Vec<McpToolDefinition> {
    vec![
        McpToolDefinition {
            name: "server_health".to_string(),
            description: "Check MedicalServer health, storage mode, Tantivy knowledge index document count, and LLM router status.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        McpToolDefinition {
            name: "get_clinical_data_catalog".to_string(),
            description: "Retrieve the standardized catalog of supported medical examination data types (blood/biochem panels with LOINC codes, vital signs/blood pressure, MRI/CT/X-ray/US/ECG imaging modalities, TIME wound assessment fields, trigger substance categories, and medical image asset categories).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        McpToolDefinition {
            name: "triage_chat".to_string(),
            description: "Run an intelligent ECS medical triage turn for a patient (includes 120 emergency red-flag short-circuit, hybrid RAG knowledge retrieval, L2/L3 memory recall, and structured follow-up or department recommendation).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Unique conversation session ID" },
                    "patient_id": { "type": "string", "description": "Optional anonymized patient ID (e.g. pat_sha256_...)" },
                    "utterance": { "type": "string", "description": "Patient symptom description or answer" },
                    "region_code": { "type": "string", "description": "Optional region code for seasonal epidemic boost (e.g. CN-110000)" },
                    "target_population": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["adult", "child", "elderly", "pregnant", "female", "male"] },
                        "description": "Optional population filter"
                    },
                    "consent_long_term_memory": { "type": "boolean", "description": "Whether patient consents to L2/L3 memory persistence" }
                },
                "required": ["session_id", "utterance"]
            }),
        },
        McpToolDefinition {
            name: "knowledge_search".to_string(),
            description: "Search the medical knowledge base using hybrid BM25 + vector RAG with optional department, population, and seasonal epidemic filters.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query (symptoms, disease name, lab analyte, MRI finding, wound type, trigger substance, etc.)" },
                    "keywords": { "type": "array", "items": { "type": "string" } },
                    "department": { "type": "string" },
                    "target_population": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["adult", "child", "elderly", "pregnant", "female", "male"] }
                    },
                    "region_code": { "type": "string" },
                    "season": { "type": "string", "enum": ["spring", "summer", "autumn", "winter", "all_year"] },
                    "seasonal_boost": { "type": "boolean", "default": true },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 50, "default": 5 }
                },
                "required": ["query"]
            }),
        },
        McpToolDefinition {
            name: "knowledge_list".to_string(),
            description: "List/browse knowledge documents currently indexed in MedicalServer, with optional department and keyword filters.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "department": { "type": "string" },
                    "keyword": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 }
                }
            }),
        },
        McpToolDefinition {
            name: "knowledge_import".to_string(),
            description: "Import or upsert structured KnowledgeDocuments into the medical knowledge base (Tantivy + vector store). Supports reference_labs (biochemistry/LOINC), reference_vitals, imaging_features (MRI/CT/ECG), wound_features, trigger_substances, and multimodal media_assets (wound/MRI/trigger images).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tenant_namespace": { "type": "string", "default": "default" },
                    "documents": {
                        "type": "array",
                        "description": "Array of KnowledgeDocument objects",
                        "items": { "type": "object" }
                    }
                },
                "required": ["documents"]
            }),
        },
        McpToolDefinition {
            name: "knowledge_delete".to_string(),
            description: "Delete knowledge documents from Tantivy and vector stores by doc_id list.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "doc_ids": { "type": "array", "items": { "type": "string" } },
                    "tenant_namespace": { "type": "string", "default": "default" }
                },
                "required": ["doc_ids"]
            }),
        },
        McpToolDefinition {
            name: "patient_list".to_string(),
            description: "List anonymized patient IDs stored in MedicalServer.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 },
                    "offset": { "type": "integer", "minimum": 0, "default": 0 }
                }
            }),
        },
        McpToolDefinition {
            name: "patient_get_memory".to_string(),
            description: "Retrieve a patient's full L2 long-term profile (including baseline_vitals and trigger_substances) and L3 visit history (including clinical_exams: vital signs, blood/biochem panels, MRI/CT imaging, wound assessments, trigger substances, and images).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patient_id": { "type": "string", "description": "Anonymized patient ID" }
                },
                "required": ["patient_id"]
            }),
        },
        McpToolDefinition {
            name: "patient_upsert_memory".to_string(),
            description: "Create or update a patient's full PatientRecord (L2 profile + L3 visit events) with AES-256-GCM encryption at rest.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patient_id": { "type": "string" },
                    "consent_long_term_memory": { "type": "boolean", "default": true },
                    "l2_profile": { "type": "object" },
                    "l3_visit_events": { "type": "array", "items": { "type": "object" }, "default": [] }
                },
                "required": ["patient_id", "l2_profile"]
            }),
        },
        McpToolDefinition {
            name: "patient_add_clinical_exam".to_string(),
            description: "Attach structured medical examination data (blood pressure/vital signs, blood/biochemical lab panels, MRI/CT/X-ray/US/ECG imaging & images, wound TIME assessments & photos, trigger substances & photos) to a patient's case.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patient_id": { "type": "string", "description": "Anonymized patient ID" },
                    "event_id": { "type": "string", "description": "Optional existing L3 visit event_id to merge into; omit to create a new visit event" },
                    "chief_complaint": { "type": "string" },
                    "recommended_department": { "type": "string" },
                    "urgency_level": { "type": "integer", "minimum": 1, "maximum": 4 },
                    "summary": { "type": "string" },
                    "baseline_vitals": { "type": "object", "description": "Optional VitalSignsRecord to update on L2 profile" },
                    "l2_trigger_substances": { "type": "array", "items": { "type": "object" } },
                    "clinical_exams": {
                        "type": "object",
                        "description": "ClinicalExamBundle containing vital_signs, lab_panels, imaging_exams, wound_assessments, trigger_substances, media_assets"
                    }
                },
                "required": ["patient_id", "clinical_exams"]
            }),
        },
        McpToolDefinition {
            name: "patient_import_batch".to_string(),
            description: "Batch import and automatically PII-mask raw patient records from JSON or CSV text (supports clinical exam columns/fields: blood pressure, biochemistry, MRI/imaging, wound photos, trigger substances).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "format": { "type": "string", "enum": ["json", "csv"] },
                    "content": { "type": "string", "description": "Raw JSON array string or CSV text" },
                    "default_consent": { "type": "boolean", "default": true }
                },
                "required": ["format", "content"]
            }),
        },
        McpToolDefinition {
            name: "patient_delete_memory".to_string(),
            description: "Permanently wipe a patient's L1 session cache, L2 profile, L3 visit events, and L3 vectors (compliance right-to-be-forgotten).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patient_id": { "type": "string" }
                },
                "required": ["patient_id"]
            }),
        },
        McpToolDefinition {
            name: "epidemic_list".to_string(),
            description: "Query active regional/seasonal epidemic weighting contexts.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "region_code": { "type": "string" },
                    "season": { "type": "string", "enum": ["spring", "summer", "autumn", "winter", "all_year"] }
                }
            }),
        },
        McpToolDefinition {
            name: "epidemic_upsert".to_string(),
            description: "Create or update a regional/seasonal epidemic context and hot-reload the triage engine.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "context_id": { "type": "string" },
                    "region_code": { "type": "string" },
                    "region_name": { "type": "string" },
                    "season": { "type": "string", "enum": ["spring", "summer", "autumn", "winter", "all_year"] },
                    "disease_name": { "type": "string" },
                    "seasonal_tag": { "type": "string" },
                    "intensity": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "boost_symptoms": { "type": "array", "items": { "type": "string" } },
                    "priority_question_hint": { "type": "string" },
                    "active": { "type": "boolean", "default": true },
                    "valid_from": { "type": "string", "description": "YYYY-MM-DD" },
                    "valid_to": { "type": "string", "description": "YYYY-MM-DD" }
                },
                "required": ["context_id", "region_code", "region_name", "season", "disease_name", "seasonal_tag", "intensity", "valid_from", "valid_to"]
            }),
        },
        McpToolDefinition {
            name: "dictionary_get".to_string(),
            description: "Retrieve current medical symptom synonyms and 120 emergency red-flag rules.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        McpToolDefinition {
            name: "dictionary_upsert_synonym".to_string(),
            description: "Add or update a colloquial-to-standard medical symptom synonym mapping and hot-reload.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "standard_term": { "type": "string" },
                    "colloquial_terms": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["standard_term", "colloquial_terms"]
            }),
        },
        McpToolDefinition {
            name: "dictionary_upsert_red_flag".to_string(),
            description: "Add or update an Emergency120 / Urgent red-flag interception rule and hot-reload.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rule_id": { "type": "string" },
                    "level": { "type": "string", "enum": ["None", "Urgent", "Emergency120"] },
                    "trigger_all_of": { "type": "array", "items": { "type": "string" }, "default": [] },
                    "trigger_any_of": { "type": "array", "items": { "type": "string" }, "default": [] },
                    "alert_message": { "type": "string" }
                },
                "required": ["rule_id", "level", "alert_message"]
            }),
        },
        McpToolDefinition {
            name: "admin_reload".to_string(),
            description: "Trigger zero-downtime ArcSwap hot reload of epidemics, medical dictionaries, and LLM router config.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "reload_epidemics": { "type": "boolean", "default": true },
                    "reload_dictionaries": { "type": "boolean", "default": true },
                    "reload_router": { "type": "boolean", "default": true }
                }
            }),
        },
        McpToolDefinition {
            name: "admin_save_snapshot".to_string(),
            description: "Explicitly flush and persist the AES-256-GCM encrypted local data snapshot to disk immediately.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
    ]
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing required string argument '{key}'"))
}

fn encode_query_param(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for b in val.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Dispatches an MCP `tools/call` invocation against `McpBackend`.
pub async fn call_tool(backend: &McpBackend, name: &str, args: Value) -> McpToolCallResult {
    let res = match name {
        "server_health" => backend.request_json(Method::GET, "/health", None, false).await,
        "get_clinical_data_catalog" => {
            backend
                .request_json(Method::GET, "/v1/clinical/types", None, false)
                .await
        }
        "triage_chat" => {
            let mut body = args;
            if let Some(obj) = body.as_object_mut() {
                obj.insert("stream".to_string(), Value::Bool(false));
            }
            backend
                .request_json(Method::POST, "/v1/triage/chat", Some(body), false)
                .await
        }
        "knowledge_search" => {
            backend
                .request_json(Method::POST, "/v1/knowledge/search", Some(args), false)
                .await
        }
        "knowledge_list" => {
            let mut qs = Vec::new();
            if let Some(d) = args.get("department").and_then(Value::as_str)
                && !d.trim().is_empty()
            {
                qs.push(format!("department={}", encode_query_param(d.trim())));
            }
            if let Some(k) = args.get("keyword").and_then(Value::as_str)
                && !k.trim().is_empty()
            {
                qs.push(format!("keyword={}", encode_query_param(k.trim())));
            }
            if let Some(l) = args.get("limit").and_then(Value::as_u64) {
                qs.push(format!("limit={l}"));
            }
            let path = if qs.is_empty() {
                "/v1/knowledge/documents".to_string()
            } else {
                format!("/v1/knowledge/documents?{}", qs.join("&"))
            };
            backend.request_json(Method::GET, &path, None, true).await
        }
        "knowledge_import" => {
            backend
                .request_json(Method::POST, "/v1/knowledge/import", Some(args), true)
                .await
        }
        "knowledge_delete" => {
            backend
                .request_json(Method::DELETE, "/v1/knowledge/documents", Some(args), true)
                .await
        }
        "patient_list" => {
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50);
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let path = format!("/v1/patients?limit={limit}&offset={offset}");
            backend.request_json(Method::GET, &path, None, true).await
        }
        "patient_get_memory" => match require_str(&args, "patient_id") {
            Ok(pid) => {
                let path = format!("/v1/patients/{}/memory", encode_query_param(pid));
                backend.request_json(Method::GET, &path, None, true).await
            }
            Err(e) => Err(e),
        },
        "patient_upsert_memory" => match require_str(&args, "patient_id") {
            Ok(pid) => {
                let path = format!("/v1/patients/{}/memory", encode_query_param(pid));
                let mut body = args;
                if let Some(obj) = body.as_object_mut()
                    && !obj.contains_key("consent_long_term_memory")
                {
                    obj.insert("consent_long_term_memory".to_string(), Value::Bool(true));
                }
                backend
                    .request_json(Method::PUT, &path, Some(body), true)
                    .await
            }
            Err(e) => Err(e),
        },
        "patient_add_clinical_exam" => match require_str(&args, "patient_id") {
            Ok(pid) => {
                let path = format!("/v1/patients/{}/clinical-exams", encode_query_param(pid));
                backend
                    .request_json(Method::POST, &path, Some(args), true)
                    .await
            }
            Err(e) => Err(e),
        },
        "patient_import_batch" => {
            backend
                .request_json(Method::POST, "/v1/patients/import", Some(args), true)
                .await
        }
        "patient_delete_memory" => match require_str(&args, "patient_id") {
            Ok(pid) => {
                let path = format!("/v1/patients/{}/memory", encode_query_param(pid));
                backend
                    .request_json(Method::DELETE, &path, None, true)
                    .await
            }
            Err(e) => Err(e),
        },
        "epidemic_list" => {
            let mut qs = Vec::new();
            if let Some(r) = args.get("region_code").and_then(Value::as_str)
                && !r.trim().is_empty()
            {
                qs.push(format!("region_code={}", encode_query_param(r.trim())));
            }
            if let Some(s) = args.get("season").and_then(Value::as_str)
                && !s.trim().is_empty()
            {
                qs.push(format!("season={}", encode_query_param(s.trim())));
            }
            let path = if qs.is_empty() {
                "/v1/epidemic/current".to_string()
            } else {
                format!("/v1/epidemic/current?{}", qs.join("&"))
            };
            backend.request_json(Method::GET, &path, None, false).await
        }
        "epidemic_upsert" => {
            backend
                .request_json(Method::POST, "/v1/admin/epidemics", Some(args), true)
                .await
        }
        "dictionary_get" => {
            backend
                .request_json(Method::GET, "/v1/dictionaries", None, true)
                .await
        }
        "dictionary_upsert_synonym" => {
            backend
                .request_json(Method::POST, "/v1/dictionaries/synonyms", Some(args), true)
                .await
        }
        "dictionary_upsert_red_flag" => {
            backend
                .request_json(Method::POST, "/v1/dictionaries/red-flags", Some(args), true)
                .await
        }
        "admin_reload" => {
            let body = if args.is_null() { json!({}) } else { args };
            backend
                .request_json(Method::POST, "/v1/admin/reload", Some(body), true)
                .await
        }
        "admin_save_snapshot" => {
            backend
                .request_json(Method::POST, "/v1/admin/save", Some(json!({})), true)
                .await
        }
        other => Err(format!("unknown MCP tool: '{other}'")),
    };

    // In standalone headless mode, eagerly flush the encrypted snapshot after every mutating call
    // so data is safely persisted even if the calling AI Harness terminates the process abruptly.
    if res.is_ok()
        && matches!(
            name,
            "patient_upsert_memory"
                | "patient_add_clinical_exam"
                | "patient_import_batch"
                | "patient_delete_memory"
                | "epidemic_upsert"
                | "dictionary_upsert_synonym"
                | "dictionary_upsert_red_flag"
                | "triage_chat"
        )
    {
        let _ = backend.flush_if_standalone().await;
    }

    match res {
        Ok(val) => McpToolCallResult::json_ok(&val),
        Err(err) => McpToolCallResult::error(err),
    }
}

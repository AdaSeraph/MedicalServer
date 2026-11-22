pub mod backend;
pub mod protocol;
pub mod tools;

use axum::http::Method;
use serde_json::{Value, json};

use crate::backend::McpBackend;
use crate::protocol::{
    JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION, McpPromptArgument, McpPromptDefinition,
    McpResourceDefinition,
};

pub struct McpServer {
    backend: McpBackend,
}

impl McpServer {
    pub fn new(backend: McpBackend) -> Self {
        Self { backend }
    }

    pub fn list_resources() -> Vec<McpResourceDefinition> {
        vec![
            McpResourceDefinition {
                uri: "medical://health".to_string(),
                name: "MedicalServer Health".to_string(),
                description: "Live health status, storage mode, and knowledge index document count."
                    .to_string(),
                mime_type: "application/json".to_string(),
            },
            McpResourceDefinition {
                uri: "medical://catalog/clinical-types".to_string(),
                name: "Clinical Data Type Catalog".to_string(),
                description: "Standardized LOINC/FHIR catalog of vital signs, lab/biochemistry panels, imaging modalities, TIME wound dimensions, trigger categories, and medical image categories.".to_string(),
                mime_type: "application/json".to_string(),
            },
            McpResourceDefinition {
                uri: "medical://epidemics/current".to_string(),
                name: "Active Seasonal Epidemics".to_string(),
                description: "Currently active seasonal/regional epidemic contexts.".to_string(),
                mime_type: "application/json".to_string(),
            },
            McpResourceDefinition {
                uri: "medical://dictionaries".to_string(),
                name: "Medical Dictionaries & Red-Flag Rules".to_string(),
                description: "Colloquial-to-standard symptom synonyms and 120 emergency red-flag rules."
                    .to_string(),
                mime_type: "application/json".to_string(),
            },
        ]
    }

    pub fn list_prompts() -> Vec<McpPromptDefinition> {
        vec![
            McpPromptDefinition {
                name: "medical_triage_workflow".to_string(),
                description: "Guide an AI agent to conduct safe medical triage via MedicalServer tools."
                    .to_string(),
                arguments: vec![McpPromptArgument {
                    name: "symptoms".to_string(),
                    description: "Initial patient symptom description".to_string(),
                    required: true,
                }],
            },
            McpPromptDefinition {
                name: "import_clinical_case_with_exams".to_string(),
                description: "Step-by-step instructions to import a patient case with blood pressure, biochemistry, MRI/CT, wound photos, and trigger substance photos.".to_string(),
                arguments: vec![McpPromptArgument {
                    name: "patient_id".to_string(),
                    description: "Target anonymized patient ID".to_string(),
                    required: false,
                }],
            },
            McpPromptDefinition {
                name: "import_knowledge_with_multimodal_exams".to_string(),
                description: "Step-by-step instructions to import a disease knowledge document with reference labs, imaging features, wound features, trigger substances, and medical images.".to_string(),
                arguments: vec![McpPromptArgument {
                    name: "disease_name".to_string(),
                    description: "Disease name to author".to_string(),
                    required: false,
                }],
            },
        ]
    }

    /// Handles a single JSON-RPC 2.0 request. Returns `None` for notifications (no `id`).
    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        // Notifications have no `id` and expect no response.
        let id = req.id?;

        if req.jsonrpc != "2.0" {
            return Some(JsonRpcResponse::err(
                id,
                -32600,
                "Invalid Request: jsonrpc must be '2.0'",
            ));
        }

        match req.method.as_str() {
            "initialize" => {
                let requested_version = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(Value::as_str)
                    .unwrap_or(MCP_PROTOCOL_VERSION);
                Some(JsonRpcResponse::ok(
                    id,
                    json!({
                        "protocolVersion": requested_version,
                        "capabilities": {
                            "tools": { "listChanged": false },
                            "resources": { "subscribe": false, "listChanged": false },
                            "prompts": { "listChanged": false }
                        },
                        "serverInfo": {
                            "name": "medical-server-mcp",
                            "version": env!("CARGO_PKG_VERSION")
                        },
                        "instructions": "MedicalServer MCP suite: operate intelligent medical triage, hybrid RAG knowledge base, encrypted L2/L3 patient cases, clinical exams (blood pressure, biochemistry/LOINC, MRI/CT, wound TIME & photos, trigger substances & photos), epidemics, and 120 red-flag dictionaries."
                    }),
                ))
            }
            "ping" => Some(JsonRpcResponse::ok(id, json!({}))),
            "tools/list" => Some(JsonRpcResponse::ok(
                id,
                json!({ "tools": tools::list_tools() }),
            )),
            "tools/call" => {
                let Some(params) = req.params else {
                    return Some(JsonRpcResponse::err(id, -32602, "missing params"));
                };
                let Some(tool_name) = params.get("name").and_then(Value::as_str) else {
                    return Some(JsonRpcResponse::err(id, -32602, "missing params.name"));
                };
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let call_res = tools::call_tool(&self.backend, tool_name, args).await;
                Some(JsonRpcResponse::ok(
                    id,
                    serde_json::to_value(call_res).unwrap_or_else(|_| json!({})),
                ))
            }
            "resources/list" => Some(JsonRpcResponse::ok(
                id,
                json!({ "resources": Self::list_resources() }),
            )),
            "resources/read" => {
                let uri = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("uri"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let result = match uri {
                    "medical://health" => {
                        self.backend
                            .request_json(Method::GET, "/health", None, false)
                            .await
                    }
                    "medical://catalog/clinical-types" => {
                        self.backend
                            .request_json(Method::GET, "/v1/clinical/types", None, false)
                            .await
                    }
                    "medical://epidemics/current" => {
                        self.backend
                            .request_json(Method::GET, "/v1/epidemic/current", None, false)
                            .await
                    }
                    "medical://dictionaries" => {
                        self.backend
                            .request_json(Method::GET, "/v1/dictionaries", None, true)
                            .await
                    }
                    _ => Err(format!("unknown resource URI: {uri}")),
                };
                match result {
                    Ok(val) => Some(JsonRpcResponse::ok(
                        id,
                        json!({
                            "contents": [{
                                "uri": uri,
                                "mimeType": "application/json",
                                "text": serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string())
                            }]
                        }),
                    )),
                    Err(e) => Some(JsonRpcResponse::err(id, -32002, e)),
                }
            }
            "prompts/list" => Some(JsonRpcResponse::ok(
                id,
                json!({ "prompts": Self::list_prompts() }),
            )),
            "prompts/get" => {
                let name = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let args = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("arguments"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let text = match name {
                    "medical_triage_workflow" => {
                        let s = args.get("symptoms").and_then(Value::as_str).unwrap_or("(未提供症状)");
                        format!(
                            "请使用 MedicalServer MCP 工具完成安全分诊：\n1. 调用 `triage_chat` 传入患者症状「{s}」，检查是否触发 Emergency120 红线。\n2. 如需补充循证依据，调用 `knowledge_search` 检索相关疾病及检查指标。\n3. 严禁下达确诊结论或开具具体药物剂量。"
                        )
                    }
                    "import_clinical_case_with_exams" => {
                        let pid = args.get("patient_id").and_then(Value::as_str).unwrap_or("pat_sha256_...");
                        format!(
                            "请先调用 `get_clinical_data_catalog` 查看标准 LOINC 生化指标、生命体征、影像模态、伤口 TIME 评估与诱因分类，再调用 `patient_add_clinical_exam` 为患者 `{pid}` 写入血压、抽血生化、核磁/CT影像、伤口图片或诱因物质图片。"
                        )
                    }
                    "import_knowledge_with_multimodal_exams" => {
                        let dis = args.get("disease_name").and_then(Value::as_str).unwrap_or("目标疾病");
                        format!(
                            "请先调用 `get_clinical_data_catalog` 获取标准医学检查数据类型，然后组织包含 `reference_labs`、`reference_vitals`、`imaging_features`、`wound_features`、`trigger_substances` 与 `media_assets` 的 `KnowledgeDocument`，调用 `knowledge_import` 写入疾病「{dis}」的知识库文档。"
                        )
                    }
                    _ => return Some(JsonRpcResponse::err(id, -32602, format!("unknown prompt: {name}"))),
                };
                Some(JsonRpcResponse::ok(
                    id,
                    json!({
                        "description": format!("Prompt template: {name}"),
                        "messages": [{
                            "role": "user",
                            "content": { "type": "text", "text": text }
                        }]
                    }),
                ))
            }
            other => Some(JsonRpcResponse::err(
                id,
                -32601,
                format!("Method not found: {other}"),
            )),
        }
    }
}

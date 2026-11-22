use std::collections::HashMap;
use std::path::Path;

use base64::Engine;
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};
use medical_core::crypto::{hash_api_key, hash_patient_id};
use medical_core::llm::{
    LlmMessage, LlmProvider, LlmRequest, ModelTier, OpenAiCompatibleLlmProvider, ProviderConfig,
};
use medical_core::models::{
    Gender, KnowledgeDocument, L2Profile, L3VisitEvent, PatientRecord, SectionType, SourceType,
    SuggestedFollowUpSlot, TargetPopulation,
};
use medical_core::privacy::scrub_pii;
use medical_core::validation::Validate;
use medical_pipeline::{
    BookMetadataDefaults, bucket_age_group, extract_docx_text, extract_pdf_text,
    extract_xlsx_as_text,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::StudioState;

pub const DEFAULT_KNOWLEDGE_NORMALIZE_PROMPT: &str = r#"你是一名专业的临床医学知识库架构师。用户会提供任意格式的医学资料（纯文本、Markdown、表格/CSV、JSON、问答记录、指南摘录、PDF/Word提取文本等）。
请将其清洗、拆分并统一规范化为严格的 JSON 对象，格式如下：
{
  "documents": [
    {
      "disease_name": "标准疾病名称（必填）",
      "aliases": ["常见别名或简称"],
      "source_title": "来源标题（如无则使用文件名或默认来源）",
      "source_type": "textbook | guideline | hospital_manual | qa_dataset",
      "authority_level": 0.9,
      "department": "首选推荐科室（必填，如：呼吸内科、心血管内科、急诊科、全科医学科）",
      "secondary_departments": ["次选科室"],
      "target_population": ["adult"],
      "section_type": "clinical_manifestation | differential_diagnosis | red_flag | triage_rule",
      "content": "规范化、结构清晰的医学知识正文（必填）",
      "key_symptoms": ["核心症状1", "核心症状2"],
      "differential_points": ["鉴别诊断要点"],
      "red_flag_signs": ["急危重症危险指征"],
      "seasonal_tags": ["winter", "influenza"]
    }
  ]
}
规范要求：
1. 仅输出合法 JSON，不要输出任何额外解释文字或 Markdown 说明。
2. target_population 仅限以下枚举值：adult, child, elderly, pregnant, female, male；至少包含 1 项。
3. section_type 仅限：clinical_manifestation（临床表现）, differential_diagnosis（鉴别诊断）, red_flag（急症红线）, triage_rule（分诊规则）。
4. authority_level 为 0.1 到 1.0 之间的数字。
5. 若原文包含多种疾病或多个独立知识点，请拆分为多条 document；key_symptoms 不能为空。"#;

pub const DEFAULT_PATIENT_NORMALIZE_PROMPT: &str = r#"你是一名专业的医院病历数据结构化与隐私合规专家。用户会提供任意格式的病例资料（门诊/住院病历文本、出院小结、表格/CSV/Excel、JSON、聊天问诊记录等）。
请从中提取每位患者的结构化信息，并统一规范化为严格的 JSON 对象，格式如下：
{
  "patients": [
    {
      "patient_identifier": "病历号或用于区分患者的标识（程序将在本地加盐哈希为匿名ID；如原文已脱敏或无标识可留空）",
      "age": "年龄数字或年龄段（如 35 或 30-39）",
      "gender": "male | female | other | unknown",
      "is_pregnant": false,
      "consent_long_term_memory": true,
      "chronic_diseases": ["高血压", "2型糖尿病"],
      "allergies": ["青霉素"],
      "surgical_history": ["阑尾切除术"],
      "long_term_medications": ["氨氯地平"],
      "visit_events": [
        {
          "occurred_at": "2026-05-01T10:00:00Z",
          "chief_complaint": "脱敏后的规范化主诉（严禁包含姓名、身份证号、手机号、详细住址）",
          "extracted_symptoms": ["胸闷", "干咳"],
          "recommended_department": "呼吸内科",
          "urgency_level": 2,
          "summary": "脱敏后的规范化就诊摘要（客观概括病情与分诊建议）"
        }
      ]
    }
  ]
}
规范要求：
1. 仅输出合法 JSON，不要输出任何额外解释文字。
2. 严禁在 chief_complaint、summary、chronic_diseases、allergies 等临床字段中保留任何姓名、身份证号、手机号、详细住址。
3. urgency_level 为 1~4 的整数（1=危急120/抢救，2=急诊/优先，3=常规门诊，4=健康咨询/慢病随访）。
4. 若输入包含多份病例或表格多行，请全部提取至 patients 数组中；同一患者多次就诊可放入同一条记录的 visit_events 中。"#;

pub const DEFAULT_KNOWLEDGE_INSPECT_PROMPT: &str = r#"你是一名医院医学知识库质控与分类巡查专家。
用户会提供「当前知识库已有科室分类」以及一批「待入库医学知识条目」。
请你逐条巡查其质量、隐私合规、重复/冲突风险，并给出最合理的标准分类建议，严格输出如下 JSON：
{
  "inspections": [
    {
      "doc_id": "对应条目的 doc_id",
      "status": "pass | warning | reject",
      "suggested_department": "优先对齐已有科室名称；如属新科室请用标准临床科室名",
      "suggested_section_type": "clinical_manifestation | differential_diagnosis | red_flag | triage_rule",
      "category_tags": ["主题/亚专科/人群标签1", "标签2"],
      "is_duplicate": false,
      "duplicate_of": null,
      "issues": ["发现的质量/分类/缺失/合规问题（无问题则空数组）"],
      "recommendation": "简短巡查结论与改进建议（<=50字）"
    }
  ]
}
巡查标准：
1. 仅输出合法 JSON，不要输出解释文字。
2. 若内容属于非医学噪声、空洞无临床价值、或含未脱敏个人隐私（姓名/身份证/手机号），标记 status="reject"。
3. 若科室名称与已有知识库分类不一致（如“心内科”应统一为“心血管内科”）、或缺少核心症状/急症指征、或与已有条目疑似重复，标记 status="warning" 并在 suggested_department / issues 中给出修正建议。
4. 若质量良好且分类准确，标记 status="pass"。
5. 必须覆盖输入中的每一条 doc_id。"#;

pub const DEFAULT_PATIENT_INSPECT_PROMPT: &str = r#"你是一名医院临床病历质控与分诊归类巡查专家。
用户会提供一批待入库的匿名化病例记录，请你逐条巡查是否存在残留隐私(PII)、逻辑矛盾、急危重症分级偏差，并按临床科室与风险维度归类，严格输出如下 JSON：
{
  "inspections": [
    {
      "patient_id": "对应病例的 patient_id",
      "status": "pass | warning | reject",
      "suggested_department": "最匹配的主要临床科室（如呼吸内科、心血管内科、急诊科）",
      "risk_level": "high | medium | low",
      "category_tags": ["慢病管理", "呼吸系统", "高龄风险"],
      "pii_risk": false,
      "issues": ["发现的问题（无问题则空数组）"],
      "recommendation": "简短巡查结论与建议（<=50字）"
    }
  ]
}
巡查标准：
1. 仅输出合法 JSON，不要输出解释文字。
2. 如发现任何疑似姓名、手机号、身份证号、详细门牌号等残留隐私，必须标记 pii_risk=true 且 status="reject"。
3. 如就诊事件紧急度与主诉明显不符、或缺少有效症状/主诉，标记 status="warning"。
4. 必须覆盖输入中的每一条 patient_id。"#;

#[derive(Debug, Clone, Serialize)]
pub struct AiImportPrompts {
    pub knowledge_prompt: String,
    pub patient_prompt: String,
    pub knowledge_inspect_prompt: String,
    pub patient_inspect_prompt: String,
}

pub fn default_ai_import_prompts() -> AiImportPrompts {
    AiImportPrompts {
        knowledge_prompt: DEFAULT_KNOWLEDGE_NORMALIZE_PROMPT.to_string(),
        patient_prompt: DEFAULT_PATIENT_NORMALIZE_PROMPT.to_string(),
        knowledge_inspect_prompt: DEFAULT_KNOWLEDGE_INSPECT_PROMPT.to_string(),
        patient_inspect_prompt: DEFAULT_PATIENT_INSPECT_PROMPT.to_string(),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BatchImportItem {
    pub name: String,
    #[serde(default)]
    pub content_text: Option<String>,
    #[serde(default)]
    pub content_base64: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchItemSummary {
    pub name: String,
    pub ok: bool,
    pub extracted_count: usize,
    pub segments_processed: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiKnowledgeNormalizeResponse {
    pub documents: Vec<KnowledgeDocument>,
    pub item_results: Vec<BatchItemSummary>,
    pub errors: Vec<String>,
    pub provider_used: String,
    pub model_used: String,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiPatientNormalizeResponse {
    pub records: Vec<PatientRecord>,
    pub detected_pii_types: Vec<String>,
    pub item_results: Vec<BatchItemSummary>,
    pub errors: Vec<String>,
    pub provider_used: String,
    pub model_used: String,
    pub total_tokens: u32,
}

/// Extracts UTF-8 plain text from any `BatchImportItem` (text, base64 binary docx/pdf/xlsx, or local file path).
pub fn extract_item_text(item: &BatchImportItem) -> Result<String, String> {
    if let Some(text) = &item.content_text {
        let trimmed = text.trim_start_matches('\u{feff}').trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let ext = Path::new(&item.name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if let Some(b64) = &item.content_base64 {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("base64 decode failed for {}: {e}", item.name))?;
        return extract_text_from_bytes(&ext, &bytes, &item.name);
    }

    if let Some(path_str) = &item.file_path {
        let p = Path::new(path_str);
        let file_ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or(ext);
        let bytes = std::fs::read(p)
            .map_err(|e| format!("failed to read file {}: {e}", path_str))?;
        return extract_text_from_bytes(&file_ext, &bytes, path_str);
    }

    Err(format!("导入项 {} 缺少有效内容", item.name))
}

fn extract_text_from_bytes(ext: &str, bytes: &[u8], label: &str) -> Result<String, String> {
    match ext {
        "docx" => extract_docx_text(bytes)
            .map_err(|e| format!("解析 DOCX ({label}) 失败: {e}")),
        "pdf" => extract_pdf_text(bytes, None)
            .map_err(|e| format!("解析 PDF ({label}) 失败: {e}")),
        "xlsx" | "xls" => extract_xlsx_as_text(bytes)
            .map_err(|e| format!("解析 Excel ({label}) 失败: {e}")),
        _ => {
            let text = String::from_utf8_lossy(bytes);
            let cleaned = text.trim_start_matches('\u{feff}').trim().to_string();
            if cleaned.is_empty() {
                Err(format!("文件 {label} 内容为空"))
            } else {
                Ok(cleaned)
            }
        }
    }
}

/// Splits long input text into UTF-8-safe segments of at most `max_chars` characters,
/// preferring paragraph (`\n\n`) or line (`\n`) boundaries.
pub fn split_text_for_llm(text: &str, max_chars: usize) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let max_chars = max_chars.max(500);
    if trimmed.chars().count() <= max_chars {
        return vec![trimmed.to_string()];
    }

    let mut segments = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;

    for line in trimmed.lines() {
        let line_chars = line.chars().count();
        if line_chars > max_chars {
            if !current.trim().is_empty() {
                segments.push(current.trim().to_string());
                current.clear();
                current_chars = 0;
            }
            // Hard-slice very long single line safely by char boundary
            let mut buf = String::new();
            let mut cnt = 0usize;
            for ch in line.chars() {
                buf.push(ch);
                cnt += 1;
                if cnt >= max_chars {
                    segments.push(buf.trim().to_string());
                    buf.clear();
                    cnt = 0;
                }
            }
            if !buf.trim().is_empty() {
                current.push_str(&buf);
                current_chars = buf.chars().count();
            }
            continue;
        }

        if current_chars + line_chars + 1 > max_chars && !current.trim().is_empty() {
            segments.push(current.trim().to_string());
            current.clear();
            current_chars = 0;
        }
        if !current.is_empty() {
            current.push('\n');
            current_chars += 1;
        }
        current.push_str(line);
        current_chars += line_chars;
    }

    if !current.trim().is_empty() {
        segments.push(current.trim().to_string());
    }
    segments
}

/// Extracts the JSON object/array from an LLM response, stripping `<think>...</think>` and code fences.
pub fn extract_json_payload(raw: &str) -> String {
    let mut s = raw.trim().to_string();

    // Strip <think>...</think> blocks emitted by reasoning models
    while let Some(start) = s.find("<think>") {
        if let Some(end) = s[start..].find("</think>") {
            let cut_end = start + end + "</think>".len();
            s.replace_range(start..cut_end, "");
        } else {
            break;
        }
    }

    let trimmed = s.trim();
    // Strip ```json ... ``` or ``` ... ``` fences
    let unfenced = if let Some(rest) = trimmed.strip_prefix("```") {
        let after_lang = rest
            .find('\n')
            .map(|idx| &rest[idx + 1..])
            .unwrap_or(rest);
        after_lang.trim_end().trim_end_matches("```").trim()
    } else {
        trimmed
    };

    // Find outermost JSON object or array
    let first_obj = unfenced.find('{');
    let first_arr = unfenced.find('[');
    match (first_obj, first_arr) {
        (Some(o), Some(a)) if a < o => {
            if let Some(last) = unfenced.rfind(']') {
                return unfenced[a..=last].to_string();
            }
        }
        (Some(o), _) => {
            if let Some(last) = unfenced.rfind('}') {
                return unfenced[o..=last].to_string();
            }
        }
        (None, Some(a)) => {
            if let Some(last) = unfenced.rfind(']') {
                return unfenced[a..=last].to_string();
            }
        }
        _ => {}
    }

    unfenced.to_string()
}

/// Resolves an enabled `ProviderConfig` from `StudioState` (preferring `provider_id` if given).
pub async fn resolve_llm_provider_config(
    state: &StudioState,
    provider_id: Option<&str>,
) -> Result<ProviderConfig, String> {
    let providers = state
        .stores
        .llm_providers
        .list_providers()
        .await
        .map_err(|e| format!("读取 AI 模型配置失败: {e}"))?;

    if let Some(pid) = provider_id.map(str::trim).filter(|s| !s.is_empty()) {
        return providers
            .into_iter()
            .find(|p| p.id == pid)
            .ok_or_else(|| format!("未找到指定的 AI 模型配置: {pid}"));
    }

    let mut enabled: Vec<ProviderConfig> = providers.into_iter().filter(|p| p.enabled).collect();
    enabled.sort_by_key(|p| p.priority);
    enabled
        .into_iter()
        .next()
        .ok_or_else(|| "未找到已启用的 AI 模型，请先在「6. AI 大模型与网关配置」中添加并启用模型".to_string())
}

pub async fn ai_normalize_knowledge_batch_with_state(
    state: &StudioState,
    items: &[BatchImportItem],
    provider_id: Option<&str>,
    custom_prompt: Option<&str>,
    defaults: &BookMetadataDefaults,
) -> Result<AiKnowledgeNormalizeResponse, String> {
    let cfg = resolve_llm_provider_config(state, provider_id).await?;
    let provider = OpenAiCompatibleLlmProvider::new(cfg.clone())
        .map_err(|e| format!("初始化 AI 客户端失败: {e}"))?;
    let timeout_ms = cfg.timeout_ms.max(60_000);
    ai_normalize_knowledge_with_provider(
        &provider,
        items,
        custom_prompt,
        defaults,
        timeout_ms,
    )
    .await
}

pub async fn ai_normalize_knowledge_with_provider(
    provider: &dyn LlmProvider,
    items: &[BatchImportItem],
    custom_prompt: Option<&str>,
    defaults: &BookMetadataDefaults,
    timeout_ms: u64,
) -> Result<AiKnowledgeNormalizeResponse, String> {
    if items.is_empty() {
        return Err("请至少添加一个文件或粘贴文本再进行 AI 规范化".to_string());
    }

    let system_prompt = custom_prompt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_KNOWLEDGE_NORMALIZE_PROMPT);

    let mut all_docs = Vec::new();
    let mut item_results = Vec::new();
    let mut errors = Vec::new();
    let mut total_tokens = 0u32;

    for item in items {
        let raw_text = match extract_item_text(item) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("[{}] {e}", item.name));
                item_results.push(BatchItemSummary {
                    name: item.name.clone(),
                    ok: false,
                    extracted_count: 0,
                    segments_processed: 0,
                    error: Some(e),
                });
                continue;
            }
        };

        let segments = split_text_for_llm(&raw_text, 5500);
        let seg_count = segments.len();
        let item_name = item.name.as_str();

        // Execute segments concurrently (up to 4 in-flight) to dramatically speed up multi-segment imports
        let mut seg_outcomes: Vec<(usize, Result<medical_core::llm::LlmResponse, medical_core::llm::LlmError>)> =
            stream::iter(segments.into_iter().enumerate().map(|(seg_idx, seg)| async move {
                let user_msg = format!(
                    "来源名称: {}\n分段: {}/{}\n默认来源标题: {}\n默认科室: {}\n\n原始待规范化数据:\n{}",
                    item_name,
                    seg_idx + 1,
                    seg_count,
                    defaults.source_title,
                    defaults.default_department,
                    seg
                );
                let mut req = LlmRequest::new(
                    ModelTier::Reasoning,
                    vec![LlmMessage::system(system_prompt), LlmMessage::user(user_msg)],
                );
                req.temperature = Some(0.1);
                req.max_tokens = Some(4096);
                req.response_format_json = true;
                req.timeout_ms = Some(timeout_ms);
                (seg_idx, provider.complete_json(&req).await)
            }))
            .buffer_unordered(4)
            .collect()
            .await;
        seg_outcomes.sort_by_key(|(idx, _)| *idx);

        let mut item_docs = Vec::new();
        let mut item_err: Option<String> = None;

        for (seg_idx, outcome) in seg_outcomes {
            match outcome {
                Ok(resp) => {
                    total_tokens = total_tokens.saturating_add(resp.usage.total_tokens);
                    match parse_ai_knowledge_response(&resp.content, &item.name, defaults, seg_idx)
                    {
                        Ok((docs, parse_warnings)) => {
                            item_docs.extend(docs);
                            for w in parse_warnings {
                                errors.push(format!("[{}#{}] {w}", item.name, seg_idx + 1));
                            }
                        }
                        Err(e) => {
                            let msg = format!("AI 输出 JSON 解析失败: {e}");
                            errors.push(format!("[{}#{}] {msg}", item.name, seg_idx + 1));
                            item_err = Some(msg);
                        }
                    }
                }
                Err(e) => {
                    let msg = format!("调用 AI 模型失败: {e}");
                    errors.push(format!("[{}#{}] {msg}", item.name, seg_idx + 1));
                    item_err = Some(msg);
                }
            }
        }

        let extracted_count = item_docs.len();
        all_docs.extend(item_docs);
        item_results.push(BatchItemSummary {
            name: item.name.clone(),
            ok: extracted_count > 0 && item_err.is_none(),
            extracted_count,
            segments_processed: seg_count,
            error: item_err,
        });
    }

    Ok(AiKnowledgeNormalizeResponse {
        documents: all_docs,
        item_results,
        errors,
        provider_used: provider.id().to_string(),
        model_used: provider.model_name().to_string(),
        total_tokens,
    })
}

pub async fn ai_normalize_patient_batch_with_state(
    state: &StudioState,
    items: &[BatchImportItem],
    provider_id: Option<&str>,
    custom_prompt: Option<&str>,
    salt: &[u8],
    pre_scrub_pii: bool,
) -> Result<AiPatientNormalizeResponse, String> {
    let cfg = resolve_llm_provider_config(state, provider_id).await?;
    if !cfg.is_intranet && !pre_scrub_pii {
        return Err(
            "安全拦截：公网/非院内大模型禁止关闭「发送前强制隐私脱敏 (pre_scrub_pii)」，请开启预脱敏或切换为院内内网模型。"
                .to_string(),
        );
    }
    let provider = OpenAiCompatibleLlmProvider::new(cfg.clone())
        .map_err(|e| format!("初始化 AI 客户端失败: {e}"))?;
    let timeout_ms = cfg.timeout_ms.max(60_000);
    ai_normalize_patient_with_provider(
        &provider,
        items,
        custom_prompt,
        salt,
        pre_scrub_pii,
        timeout_ms,
    )
    .await
}

pub async fn ai_normalize_patient_with_provider(
    provider: &dyn LlmProvider,
    items: &[BatchImportItem],
    custom_prompt: Option<&str>,
    salt: &[u8],
    pre_scrub_pii: bool,
    timeout_ms: u64,
) -> Result<AiPatientNormalizeResponse, String> {
    if items.is_empty() {
        return Err("请至少添加一个病例文件或粘贴病例文本再进行 AI 规范化".to_string());
    }

    let system_prompt = custom_prompt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_PATIENT_NORMALIZE_PROMPT);

    let mut all_records = Vec::new();
    let mut detected_pii_types: Vec<String> = Vec::new();
    let mut item_results = Vec::new();
    let mut errors = Vec::new();
    let mut total_tokens = 0u32;

    for item in items {
        let raw_text = match extract_item_text(item) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("[{}] {e}", item.name));
                item_results.push(BatchItemSummary {
                    name: item.name.clone(),
                    ok: false,
                    extracted_count: 0,
                    segments_processed: 0,
                    error: Some(e),
                });
                continue;
            }
        };

        let text_for_llm = if pre_scrub_pii {
            let scrubbed = scrub_pii(&raw_text);
            for r in scrubbed.redactions {
                if !detected_pii_types.contains(&r) {
                    detected_pii_types.push(r);
                }
            }
            scrubbed.scrubbed_text
        } else {
            raw_text
        };

        let segments = split_text_for_llm(&text_for_llm, 5500);
        let seg_count = segments.len();
        let item_name = item.name.as_str();

        // Execute segments concurrently (up to 4 in-flight) for high-throughput batch normalization
        let mut seg_outcomes: Vec<(usize, Result<medical_core::llm::LlmResponse, medical_core::llm::LlmError>)> =
            stream::iter(segments.into_iter().enumerate().map(|(seg_idx, seg)| async move {
                let user_msg = format!(
                    "来源名称: {}\n分段: {}/{}\n\n原始病例数据:\n{}",
                    item_name,
                    seg_idx + 1,
                    seg_count,
                    seg
                );
                let mut req = LlmRequest::new(
                    ModelTier::Reasoning,
                    vec![LlmMessage::system(system_prompt), LlmMessage::user(user_msg)],
                );
                req.temperature = Some(0.1);
                req.max_tokens = Some(4096);
                req.response_format_json = true;
                req.timeout_ms = Some(timeout_ms);
                (seg_idx, provider.complete_json(&req).await)
            }))
            .buffer_unordered(4)
            .collect()
            .await;
        seg_outcomes.sort_by_key(|(idx, _)| *idx);

        let mut item_records = Vec::new();
        let mut item_err: Option<String> = None;

        for (seg_idx, outcome) in seg_outcomes {
            match outcome {
                Ok(resp) => {
                    total_tokens = total_tokens.saturating_add(resp.usage.total_tokens);
                    match parse_ai_patient_response(&resp.content, &item.name, salt, seg_idx) {
                        Ok((recs, pii_types, parse_warnings)) => {
                            item_records.extend(recs);
                            for t in pii_types {
                                if !detected_pii_types.contains(&t) {
                                    detected_pii_types.push(t);
                                }
                            }
                            for w in parse_warnings {
                                errors.push(format!("[{}#{}] {w}", item.name, seg_idx + 1));
                            }
                        }
                        Err(e) => {
                            let msg = format!("AI 输出 JSON 解析失败: {e}");
                            errors.push(format!("[{}#{}] {msg}", item.name, seg_idx + 1));
                            item_err = Some(msg);
                        }
                    }
                }
                Err(e) => {
                    let msg = format!("调用 AI 模型失败: {e}");
                    errors.push(format!("[{}#{}] {msg}", item.name, seg_idx + 1));
                    item_err = Some(msg);
                }
            }
        }

        let extracted_count = item_records.len();
        all_records.extend(item_records);
        item_results.push(BatchItemSummary {
            name: item.name.clone(),
            ok: extracted_count > 0 && item_err.is_none(),
            extracted_count,
            segments_processed: seg_count,
            error: item_err,
        });
    }

    Ok(AiPatientNormalizeResponse {
        records: all_records,
        detected_pii_types,
        item_results,
        errors,
        provider_used: provider.id().to_string(),
        model_used: provider.model_name().to_string(),
        total_tokens,
    })
}

// ===========================================================================
// Lenient JSON Parsers & Validators for AI Normalized Output
// ===========================================================================

pub fn parse_ai_knowledge_response(
    raw: &str,
    source_name: &str,
    defaults: &BookMetadataDefaults,
    seg_idx: usize,
) -> Result<(Vec<KnowledgeDocument>, Vec<String>), String> {
    let payload = extract_json_payload(raw);
    let val: Value =
        serde_json::from_str(&payload).map_err(|e| format!("invalid json: {e}"))?;

    let items: Vec<Value> = if let Some(arr) = val.as_array() {
        arr.clone()
    } else if let Some(arr) = val
        .get("documents")
        .or_else(|| val.get("knowledge_documents"))
        .or_else(|| val.get("items"))
        .and_then(Value::as_array)
    {
        arr.clone()
    } else if val.is_object() {
        vec![val]
    } else {
        return Err("JSON 根节点必须是对象或数组".to_string());
    };

    let mut docs = Vec::new();
    let mut warnings = Vec::new();

    for (idx, item) in items.into_iter().enumerate() {
        let Some(obj) = item.as_object() else {
            warnings.push(format!("第 {} 项不是有效的 JSON 对象，已跳过", idx + 1));
            continue;
        };

        let disease_name = get_str(obj, &["disease_name", "disease", "疾病名称", "疾病"])
            .unwrap_or_else(|| defaults.source_title.clone());
        let content = get_str(obj, &["content", "body", "正文", "内容"]).unwrap_or_default();
        if content.is_empty() {
            warnings.push(format!("第 {} 项 ({disease_name}) 缺少正文 content，已跳过", idx + 1));
            continue;
        }

        let source_title = get_str(obj, &["source_title", "source", "来源"])
            .unwrap_or_else(|| {
                if defaults.source_title.trim().is_empty() {
                    source_name.to_string()
                } else {
                    defaults.source_title.clone()
                }
            });
        let source_type = get_str(obj, &["source_type"])
            .map(|s| parse_source_type(&s, defaults.source_type))
            .unwrap_or(defaults.source_type);
        let authority_level = obj
            .get("authority_level")
            .and_then(Value::as_f64)
            .map(|v| (v as f32).clamp(0.1, 1.0))
            .unwrap_or(defaults.authority_level.clamp(0.1, 1.0));
        let department = get_str(obj, &["department", "recommended_department", "推荐科室", "科室"])
            .unwrap_or_else(|| defaults.default_department.clone());
        let secondary_departments = get_str_list(obj, &["secondary_departments", "次选科室"]);
        let aliases = get_str_list(obj, &["aliases", "别名"]);
        let section_type = get_str(obj, &["section_type", "section", "类型"])
            .map(|s| parse_section_type(&s))
            .unwrap_or(SectionType::ClinicalManifestation);

        let mut target_population: Vec<TargetPopulation> =
            get_str_list(obj, &["target_population", "适用人群"])
                .into_iter()
                .filter_map(|s| parse_target_population(&s))
                .collect();
        if target_population.is_empty() {
            target_population = if defaults.default_population.is_empty() {
                vec![TargetPopulation::Adult]
            } else {
                defaults.default_population.clone()
            };
        }

        let mut key_symptoms = get_str_list(obj, &["key_symptoms", "symptoms", "核心症状", "症状"]);
        if key_symptoms.is_empty() {
            key_symptoms.push(disease_name.clone());
        }
        let differential_points = get_str_list(obj, &["differential_points", "鉴别要点"]);
        let red_flag_signs = get_str_list(obj, &["red_flag_signs", "急症指征", "危险信号"]);
        let mut seasonal_tags = get_str_list(obj, &["seasonal_tags", "季节标签"]);
        for t in &defaults.seasonal_tags {
            if !seasonal_tags.contains(t) {
                seasonal_tags.push(t.clone());
            }
        }

        let suggested_follow_up_slots: Vec<SuggestedFollowUpSlot> = obj
            .get("suggested_follow_up_slots")
            .and_then(|v| serde_json::from_value::<Vec<SuggestedFollowUpSlot>>(v.clone()).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|slot| {
                let qlen = slot.short_question.trim().chars().count();
                !slot.slot_key.trim().is_empty()
                    && (1..=30).contains(&qlen)
                    && (2..=4).contains(&slot.quick_options.len())
                    && !slot.quick_options.iter().any(|o| o.trim().is_empty())
            })
            .collect();

        let doc_id = get_str(obj, &["doc_id"]).unwrap_or_else(|| {
            let digest = hash_api_key(&format!(
                "ai_kb|{source_name}|{seg_idx}|{idx}|{disease_name}|{}|{content}",
                section_type.as_str()
            ));
            format!("kb_ai_{}", &digest[..16])
        });

        let doc = KnowledgeDocument {
            doc_id,
            source_title,
            source_type,
            authority_level,
            disease_name,
            aliases,
            department,
            secondary_departments,
            target_population,
            section_type,
            content,
            key_symptoms,
            differential_points,
            red_flag_signs,
            seasonal_tags,
            suggested_follow_up_slots,
            updated_at: Some(Utc::now()),
        };

        match doc.validate() {
            Ok(()) => docs.push(doc),
            Err(e) => warnings.push(format!("第 {} 项校验未通过: {e}", idx + 1)),
        }
    }

    Ok((docs, warnings))
}

pub fn parse_ai_patient_response(
    raw: &str,
    source_name: &str,
    salt: &[u8],
    seg_idx: usize,
) -> Result<(Vec<PatientRecord>, Vec<String>, Vec<String>), String> {
    let payload = extract_json_payload(raw);
    let val: Value =
        serde_json::from_str(&payload).map_err(|e| format!("invalid json: {e}"))?;

    let items: Vec<Value> = if let Some(arr) = val.as_array() {
        arr.clone()
    } else if let Some(arr) = val
        .get("patients")
        .or_else(|| val.get("records"))
        .or_else(|| val.get("cases"))
        .and_then(Value::as_array)
    {
        arr.clone()
    } else if val.is_object() {
        vec![val]
    } else {
        return Err("JSON 根节点必须是对象或数组".to_string());
    };

    let mut records = Vec::new();
    let mut detected_pii_types = Vec::new();
    let mut warnings = Vec::new();

    for (idx, item) in items.into_iter().enumerate() {
        let Some(obj) = item.as_object() else {
            warnings.push(format!("第 {} 条病例不是有效的 JSON 对象，已跳过", idx + 1));
            continue;
        };

        // Derive deterministic salted SHA-256 patient_id (never store plaintext identifier)
        let raw_ident = get_str(
            obj,
            &[
                "patient_identifier",
                "id_card",
                "patient_id",
                "phone",
                "name",
                "病历号",
                "姓名",
            ],
        );
        let patient_id = match raw_ident.as_deref() {
            Some(id) if id.starts_with("pat_sha256_") && id.len() == 75 => id.to_string(),
            Some(id) if !id.contains("REDACTED") => {
                if !detected_pii_types.contains(&"patient_identifier".to_string()) {
                    detected_pii_types.push("patient_identifier".to_string());
                }
                hash_patient_id(id, salt)
            }
            _ => hash_patient_id(
                &format!("ai_case|{source_name}|{seg_idx}|{idx}"),
                salt,
            ),
        };

        let age_raw = get_str_or_num(obj, &["age_group", "age", "年龄段", "年龄"]);
        let age_group = age_raw
            .as_deref()
            .and_then(bucket_age_group)
            .unwrap_or_else(|| "unknown".to_string());

        let gender = get_str(obj, &["gender", "性别"])
            .map(|g| parse_gender_lenient(&g))
            .unwrap_or(Gender::Unknown);

        let is_pregnant = obj
            .get("is_pregnant")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let consent = obj
            .get("consent_long_term_memory")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let l2_profile = L2Profile {
            age_group,
            gender,
            is_pregnant,
            chronic_diseases: get_str_list(obj, &["chronic_diseases", "慢病史"])
                .into_iter()
                .map(|s| scrub_and_track(&s, &mut detected_pii_types))
                .filter(|s| !s.is_empty())
                .collect(),
            allergies: get_str_list(obj, &["allergies", "过敏史"])
                .into_iter()
                .map(|s| scrub_and_track(&s, &mut detected_pii_types))
                .filter(|s| !s.is_empty())
                .collect(),
            surgical_history: get_str_list(obj, &["surgical_history", "手术史"])
                .into_iter()
                .map(|s| scrub_and_track(&s, &mut detected_pii_types))
                .filter(|s| !s.is_empty())
                .collect(),
            long_term_medications: get_str_list(obj, &["long_term_medications", "长期用药"])
                .into_iter()
                .map(|s| scrub_and_track(&s, &mut detected_pii_types))
                .filter(|s| !s.is_empty())
                .collect(),
        };

        // Collect L3 visit events (supports `visit_events` array, `l3_visit_events`, single `visit_event`, or flat top-level fields)
        let mut raw_events: Vec<&serde_json::Map<String, Value>> = Vec::new();
        if let Some(arr) = obj
            .get("visit_events")
            .or_else(|| obj.get("l3_visit_events"))
            .and_then(Value::as_array)
        {
            for ev in arr {
                if let Some(m) = ev.as_object() {
                    raw_events.push(m);
                }
            }
        } else if let Some(ev_obj) = obj.get("visit_event").and_then(Value::as_object) {
            raw_events.push(ev_obj);
        } else if get_str(obj, &["chief_complaint", "主诉", "summary", "摘要"]).is_some() {
            raw_events.push(obj);
        }

        let mut l3_visit_events = Vec::new();
        for (ev_idx, ev_obj) in raw_events.into_iter().enumerate() {
            let raw_cc = get_str(ev_obj, &["chief_complaint", "主诉"])
                .or_else(|| get_str(ev_obj, &["summary", "摘要"]))
                .unwrap_or_default();
            let chief_complaint = scrub_and_track(&raw_cc, &mut detected_pii_types);
            if chief_complaint.is_empty() {
                continue;
            }
            let summary = get_str(ev_obj, &["summary", "摘要"])
                .map(|s| scrub_and_track(&s, &mut detected_pii_types))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| chief_complaint.clone());

            let occurred_at = get_str(ev_obj, &["occurred_at", "就诊时间"])
                .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);

            let recommended_department =
                get_str(ev_obj, &["recommended_department", "department", "推荐科室"])
                    .unwrap_or_else(|| "全科医学科".to_string());
            let urgency_level = ev_obj
                .get("urgency_level")
                .and_then(Value::as_u64)
                .map(|u| (u as u8).clamp(1, 4))
                .unwrap_or(2);
            let extracted_symptoms: Vec<String> =
                get_str_list(ev_obj, &["extracted_symptoms", "symptoms", "症状"])
                    .into_iter()
                    .map(|s| scrub_and_track(&s, &mut detected_pii_types))
                    .filter(|s| !s.is_empty())
                    .collect();

            let event_id = get_str(ev_obj, &["event_id"]).unwrap_or_else(|| {
                let digest = hash_api_key(&format!(
                    "{patient_id}|{seg_idx}|{idx}|{ev_idx}|{chief_complaint}"
                ));
                format!("evt_ai_{}", &digest[..12])
            });

            l3_visit_events.push(L3VisitEvent {
                event_id,
                occurred_at,
                chief_complaint,
                extracted_symptoms,
                recommended_department,
                urgency_level,
                summary,
            });
        }

        let record = PatientRecord {
            patient_id,
            consent_long_term_memory: consent,
            l2_profile,
            l3_visit_events,
        };

        match record.validate() {
            Ok(()) => records.push(record),
            Err(e) => warnings.push(format!("第 {} 条病例校验未通过: {e}", idx + 1)),
        }
    }

    Ok((records, detected_pii_types, warnings))
}

fn scrub_and_track(s: &str, detected_pii_types: &mut Vec<String>) -> String {
    let res = scrub_pii(s);
    for r in res.redactions {
        if !detected_pii_types.contains(&r) {
            detected_pii_types.push(r);
        }
    }
    res.scrubbed_text.trim().to_string()
}

fn get_str(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = obj.get(*k).and_then(Value::as_str) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn get_str_or_num(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            match v {
                Value::String(s) if !s.trim().is_empty() => return Some(s.trim().to_string()),
                Value::Number(n) => return Some(n.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn get_str_list(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Vec<String> {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            match v {
                Value::Array(arr) => {
                    return arr
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && *s != "无" && !s.eq_ignore_ascii_case("none"))
                        .map(ToString::to_string)
                        .collect();
                }
                Value::String(s) => {
                    return s
                        .split([';', '；', '、', ',', '，', '|'])
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && *s != "无" && !s.eq_ignore_ascii_case("none"))
                        .map(ToString::to_string)
                        .collect();
                }
                _ => {}
            }
        }
    }
    Vec::new()
}

fn parse_source_type(raw: &str, default: SourceType) -> SourceType {
    match raw.trim().to_ascii_lowercase().as_str() {
        "textbook" | "教材" | "书籍" => SourceType::Textbook,
        "guideline" | "指南" | "临床指南" => SourceType::Guideline,
        "hospital_manual" | "医院手册" | "手册" => SourceType::HospitalManual,
        "qa_dataset" | "问答" | "qa" => SourceType::QaDataset,
        _ => default,
    }
}

fn parse_section_type(raw: &str) -> SectionType {
    let lower = raw.trim().to_ascii_lowercase();
    if lower.contains("differential") || lower.contains("鉴别") {
        SectionType::DifferentialDiagnosis
    } else if lower.contains("red_flag") || lower.contains("急症") || lower.contains("红线") {
        SectionType::RedFlag
    } else if lower.contains("triage") || lower.contains("分诊") || lower.contains("科室") {
        SectionType::TriageRule
    } else {
        SectionType::ClinicalManifestation
    }
}

fn parse_target_population(raw: &str) -> Option<TargetPopulation> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "adult" | "成人" | "成年人" => Some(TargetPopulation::Adult),
        "child" | "children" | "pediatric" | "儿童" | "小儿" | "婴幼儿" => {
            Some(TargetPopulation::Child)
        }
        "elderly" | "senior" | "老人" | "老年人" | "高龄" => Some(TargetPopulation::Elderly),
        "pregnant" | "孕妇" | "孕产妇" => Some(TargetPopulation::Pregnant),
        "female" | "woman" | "女性" | "女" => Some(TargetPopulation::Female),
        "male" | "man" | "男性" | "男" => Some(TargetPopulation::Male),
        _ => None,
    }
}

fn parse_gender_lenient(raw: &str) -> Gender {
    match raw.trim().to_ascii_lowercase().as_str() {
        "male" | "m" | "男" | "男性" => Gender::Male,
        "female" | "f" | "女" | "女性" => Gender::Female,
        "other" | "其他" => Gender::Other,
        _ => Gender::Unknown,
    }
}

// ===========================================================================
// AI Inspection & Auto-Classification (AI 巡查分类)
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectionStatus {
    Pass,
    Warning,
    Reject,
}

impl InspectionStatus {
    fn parse_lenient(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "reject" | "rejected" | "fail" | "failed" | "异常" | "剔除" => Self::Reject,
            "warning" | "warn" | "review" | "复核" | "警告" => Self::Warning,
            _ => Self::Pass,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryCount {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InspectionSummary {
    pub total: usize,
    pub pass_count: usize,
    pub warning_count: usize,
    pub reject_count: usize,
    pub duplicate_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeInspectionItem {
    pub doc_id: String,
    pub disease_name: String,
    pub status: InspectionStatus,
    pub original_department: String,
    pub suggested_department: String,
    pub suggested_section_type: SectionType,
    pub category_tags: Vec<String>,
    pub is_duplicate: bool,
    pub duplicate_of: Option<String>,
    pub issues: Vec<String>,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiKnowledgeInspectionResponse {
    pub items: Vec<KnowledgeInspectionItem>,
    pub summary: InspectionSummary,
    pub category_distribution: Vec<CategoryCount>,
    pub provider_used: String,
    pub model_used: String,
    pub total_tokens: u32,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatientInspectionItem {
    pub patient_id: String,
    pub status: InspectionStatus,
    pub suggested_department: String,
    pub risk_level: String,
    pub category_tags: Vec<String>,
    pub pii_risk: bool,
    pub issues: Vec<String>,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiPatientInspectionResponse {
    pub items: Vec<PatientInspectionItem>,
    pub summary: InspectionSummary,
    pub category_distribution: Vec<CategoryCount>,
    pub provider_used: String,
    pub model_used: String,
    pub total_tokens: u32,
    pub errors: Vec<String>,
}

pub async fn ai_inspect_knowledge_batch_with_state(
    state: &StudioState,
    documents: &[KnowledgeDocument],
    provider_id: Option<&str>,
    custom_prompt: Option<&str>,
    tenant_namespace: Option<&str>,
) -> Result<AiKnowledgeInspectionResponse, String> {
    let cfg = resolve_llm_provider_config(state, provider_id).await?;
    let provider = OpenAiCompatibleLlmProvider::new(cfg.clone())
        .map_err(|e| format!("初始化 AI 客户端失败: {e}"))?;
    let existing_docs = state
        .tantivy
        .list_all_documents(tenant_namespace)
        .unwrap_or_default();
    let timeout_ms = cfg.timeout_ms.max(60_000);
    ai_inspect_knowledge_with_provider(
        &provider,
        documents,
        &existing_docs,
        custom_prompt,
        timeout_ms,
    )
    .await
}

pub async fn ai_inspect_knowledge_with_provider(
    provider: &dyn LlmProvider,
    documents: &[KnowledgeDocument],
    existing_docs: &[KnowledgeDocument],
    custom_prompt: Option<&str>,
    timeout_ms: u64,
) -> Result<AiKnowledgeInspectionResponse, String> {
    if documents.is_empty() {
        return Err("当前没有待巡查分类的知识条目".to_string());
    }

    let system_prompt = custom_prompt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_KNOWLEDGE_INSPECT_PROMPT);

    // Build existing taxonomy & deterministic duplicate lookup
    let mut existing_dept_counts: HashMap<String, usize> = HashMap::new();
    let mut existing_by_id: HashMap<String, String> = HashMap::new();
    let mut existing_by_key: HashMap<(String, String), String> = HashMap::new();
    for d in existing_docs {
        *existing_dept_counts.entry(d.department.clone()).or_insert(0) += 1;
        existing_by_id.insert(d.doc_id.clone(), d.doc_id.clone());
        existing_by_key.insert(
            (
                d.disease_name.trim().to_ascii_lowercase(),
                d.section_type.as_str().to_string(),
            ),
            d.doc_id.clone(),
        );
    }

    let mut existing_dept_list: Vec<(String, usize)> = existing_dept_counts.into_iter().collect();
    existing_dept_list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let existing_taxonomy_desc = if existing_dept_list.is_empty() {
        "当前知识库为空，请使用国家标准临床科室名称分类".to_string()
    } else {
        existing_dept_list
            .iter()
            .map(|(dept, cnt)| format!("{dept}({cnt}条)"))
            .collect::<Vec<_>>()
            .join("、")
    };

    // Pre-check local duplicates & PII
    let mut local_dup_map: HashMap<String, Option<String>> = HashMap::new();
    let mut local_pii_map: HashMap<String, Vec<String>> = HashMap::new();
    for doc in documents {
        let dup = existing_by_id
            .get(&doc.doc_id)
            .or_else(|| {
                existing_by_key.get(&(
                    doc.disease_name.trim().to_ascii_lowercase(),
                    doc.section_type.as_str().to_string(),
                ))
            })
            .cloned();
        local_dup_map.insert(doc.doc_id.clone(), dup);

        let scrubbed = scrub_pii(&doc.content);
        if !scrubbed.redactions.is_empty() {
            local_pii_map.insert(doc.doc_id.clone(), scrubbed.redactions);
        }
    }

    // Batch documents (up to 10 per LLM request, max 4 concurrent)
    let batches: Vec<Vec<KnowledgeDocument>> =
        documents.chunks(10).map(|c| c.to_vec()).collect();
    let taxonomy_ref = existing_taxonomy_desc.as_str();
    let local_dup_ref = &local_dup_map;

    let mut batch_outcomes: Vec<(
        usize,
        Vec<KnowledgeDocument>,
        Result<medical_core::llm::LlmResponse, medical_core::llm::LlmError>,
    )> = stream::iter(batches.into_iter().enumerate().map(|(batch_idx, batch)| async move {
        let compact_items: Vec<Value> = batch
            .iter()
            .map(|d| {
                let snippet: String = d.content.chars().take(360).collect();
                serde_json::json!({
                    "doc_id": d.doc_id,
                    "disease_name": d.disease_name,
                    "department": d.department,
                    "section_type": d.section_type.as_str(),
                    "key_symptoms": d.key_symptoms,
                    "red_flag_signs": d.red_flag_signs,
                    "seasonal_tags": d.seasonal_tags,
                    "authority_level": d.authority_level,
                    "local_duplicate_hint": local_dup_ref.get(&d.doc_id).and_then(|o| o.clone()),
                    "content_snippet": snippet,
                })
            })
            .collect();

        let user_msg = format!(
            "当前知识库已有科室分类：{}\n\n待巡查分类条目（共 {} 条）：\n{}",
            taxonomy_ref,
            batch.len(),
            serde_json::to_string_pretty(&compact_items).unwrap_or_default()
        );
        let mut req = LlmRequest::new(
            ModelTier::Reasoning,
            vec![LlmMessage::system(system_prompt), LlmMessage::user(user_msg)],
        );
        req.temperature = Some(0.1);
        req.max_tokens = Some(4096);
        req.response_format_json = true;
        req.timeout_ms = Some(timeout_ms);
        let res = provider.complete_json(&req).await;
        (batch_idx, batch, res)
    }))
    .buffer_unordered(4)
    .collect()
    .await;
    batch_outcomes.sort_by_key(|(idx, _, _)| *idx);

    let mut inspected_items = Vec::with_capacity(documents.len());
    let mut errors = Vec::new();
    let mut total_tokens = 0u32;

    for (batch_idx, batch, outcome) in batch_outcomes {
        let mut parsed_by_id: HashMap<String, Value> = HashMap::new();
        match outcome {
            Ok(resp) => {
                total_tokens = total_tokens.saturating_add(resp.usage.total_tokens);
                let payload = extract_json_payload(&resp.content);
                match serde_json::from_str::<Value>(&payload) {
                    Ok(val) => {
                        let arr = val
                            .as_array()
                            .cloned()
                            .or_else(|| {
                                val.get("inspections")
                                    .or_else(|| val.get("items"))
                                    .or_else(|| val.get("results"))
                                    .and_then(Value::as_array)
                                    .cloned()
                            })
                            .unwrap_or_default();
                        for entry in arr {
                            if let Some(obj) = entry.as_object()
                                && let Some(id) = get_str(obj, &["doc_id", "id"])
                            {
                                parsed_by_id.insert(id, entry);
                            }
                        }
                    }
                    Err(e) => {
                        errors.push(format!("批次 #{} AI 巡查 JSON 解析失败: {e}", batch_idx + 1));
                    }
                }
            }
            Err(e) => {
                errors.push(format!("批次 #{} 调用 AI 巡查失败: {e}", batch_idx + 1));
            }
        }

        for doc in batch {
            let local_dup = local_dup_map.get(&doc.doc_id).and_then(|o| o.clone());
            let local_pii = local_pii_map.get(&doc.doc_id).cloned().unwrap_or_default();

            let mut item = if let Some(obj) =
                parsed_by_id.get(&doc.doc_id).and_then(Value::as_object)
            {
                let status = get_str(obj, &["status"])
                    .map(|s| InspectionStatus::parse_lenient(&s))
                    .unwrap_or(InspectionStatus::Pass);
                let suggested_department =
                    get_str(obj, &["suggested_department", "department", "建议科室"])
                        .unwrap_or_else(|| doc.department.clone());
                let suggested_section_type =
                    get_str(obj, &["suggested_section_type", "section_type"])
                        .map(|s| parse_section_type(&s))
                        .unwrap_or(doc.section_type);
                let category_tags = get_str_list(obj, &["category_tags", "tags", "分类标签"]);
                let ai_dup = obj
                    .get("is_duplicate")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let duplicate_of =
                    get_str(obj, &["duplicate_of"]).or_else(|| local_dup.clone());
                let is_duplicate = ai_dup || duplicate_of.is_some();
                let issues = get_str_list(obj, &["issues", "问题"]);
                let recommendation = get_str(obj, &["recommendation", "建议"])
                    .unwrap_or_else(|| "巡查完成".to_string());

                KnowledgeInspectionItem {
                    doc_id: doc.doc_id.clone(),
                    disease_name: doc.disease_name.clone(),
                    status,
                    original_department: doc.department.clone(),
                    suggested_department,
                    suggested_section_type,
                    category_tags,
                    is_duplicate,
                    duplicate_of,
                    issues,
                    recommendation,
                }
            } else {
                KnowledgeInspectionItem {
                    doc_id: doc.doc_id.clone(),
                    disease_name: doc.disease_name.clone(),
                    status: InspectionStatus::Warning,
                    original_department: doc.department.clone(),
                    suggested_department: doc.department.clone(),
                    suggested_section_type: doc.section_type,
                    category_tags: doc.seasonal_tags.clone(),
                    is_duplicate: local_dup.is_some(),
                    duplicate_of: local_dup.clone(),
                    issues: vec!["AI 巡查未返回该条目评估结果，已降级为人工复核".to_string()],
                    recommendation: "请人工核对临床准确性与科室归属后再入库".to_string(),
                }
            };

            if !local_pii.is_empty() {
                item.status = InspectionStatus::Reject;
                item.issues.push(format!(
                    "本地隐私防火墙检出疑似未脱敏信息: {}",
                    local_pii.join("、")
                ));
            }
            if item.is_duplicate && item.status == InspectionStatus::Pass {
                item.status = InspectionStatus::Warning;
                if item.issues.is_empty() {
                    item.issues.push("与已有知识库条目疑似重复".to_string());
                }
            }

            inspected_items.push(item);
        }
    }

    let mut summary = InspectionSummary {
        total: inspected_items.len(),
        ..Default::default()
    };
    let mut dept_counts: HashMap<String, usize> = HashMap::new();
    for it in &inspected_items {
        match it.status {
            InspectionStatus::Pass => summary.pass_count += 1,
            InspectionStatus::Warning => summary.warning_count += 1,
            InspectionStatus::Reject => summary.reject_count += 1,
        }
        if it.is_duplicate {
            summary.duplicate_count += 1;
        }
        *dept_counts.entry(it.suggested_department.clone()).or_insert(0) += 1;
    }

    let mut category_distribution: Vec<CategoryCount> = dept_counts
        .into_iter()
        .map(|(name, count)| CategoryCount { name, count })
        .collect();
    category_distribution.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));

    Ok(AiKnowledgeInspectionResponse {
        items: inspected_items,
        summary,
        category_distribution,
        provider_used: provider.id().to_string(),
        model_used: provider.model_name().to_string(),
        total_tokens,
        errors,
    })
}

pub async fn ai_inspect_patient_batch_with_state(
    state: &StudioState,
    records: &[PatientRecord],
    provider_id: Option<&str>,
    custom_prompt: Option<&str>,
) -> Result<AiPatientInspectionResponse, String> {
    let cfg = resolve_llm_provider_config(state, provider_id).await?;
    let provider = OpenAiCompatibleLlmProvider::new(cfg.clone())
        .map_err(|e| format!("初始化 AI 客户端失败: {e}"))?;
    let existing_docs = state.tantivy.list_all_documents(None).unwrap_or_default();
    let mut existing_depts: Vec<String> = existing_docs
        .into_iter()
        .map(|d| d.department)
        .collect();
    existing_depts.sort();
    existing_depts.dedup();
    let timeout_ms = cfg.timeout_ms.max(60_000);
    ai_inspect_patient_with_provider(&provider, records, &existing_depts, custom_prompt, timeout_ms)
        .await
}

pub async fn ai_inspect_patient_with_provider(
    provider: &dyn LlmProvider,
    records: &[PatientRecord],
    existing_departments: &[String],
    custom_prompt: Option<&str>,
    timeout_ms: u64,
) -> Result<AiPatientInspectionResponse, String> {
    if records.is_empty() {
        return Err("当前没有待巡查分类的病例记录".to_string());
    }

    let system_prompt = custom_prompt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_PATIENT_INSPECT_PROMPT);

    let dept_hint = if existing_departments.is_empty() {
        "全科医学科、呼吸内科、心血管内科、消化内科、急诊科、儿科".to_string()
    } else {
        existing_departments.join("、")
    };

    let batches: Vec<Vec<PatientRecord>> = records.chunks(10).map(|c| c.to_vec()).collect();
    let dept_hint_ref = dept_hint.as_str();

    let mut batch_outcomes: Vec<(
        usize,
        Vec<PatientRecord>,
        Result<medical_core::llm::LlmResponse, medical_core::llm::LlmError>,
    )> = stream::iter(batches.into_iter().enumerate().map(|(batch_idx, batch)| async move {
        let compact: Vec<Value> = batch
            .iter()
            .map(|r| {
                serde_json::json!({
                    "patient_id": r.patient_id,
                    "age_group": r.l2_profile.age_group,
                    "gender": r.l2_profile.gender,
                    "is_pregnant": r.l2_profile.is_pregnant,
                    "chronic_diseases": r.l2_profile.chronic_diseases,
                    "allergies": r.l2_profile.allergies,
                    "visit_events": r.l3_visit_events.iter().take(3).map(|e| serde_json::json!({
                        "chief_complaint": e.chief_complaint,
                        "extracted_symptoms": e.extracted_symptoms,
                        "recommended_department": e.recommended_department,
                        "urgency_level": e.urgency_level,
                        "summary": e.summary,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();

        let user_msg = format!(
            "参考临床科室分类：{}\n\n待巡查分类病例（共 {} 位）：\n{}",
            dept_hint_ref,
            batch.len(),
            serde_json::to_string_pretty(&compact).unwrap_or_default()
        );
        let mut req = LlmRequest::new(
            ModelTier::Reasoning,
            vec![LlmMessage::system(system_prompt), LlmMessage::user(user_msg)],
        );
        req.temperature = Some(0.1);
        req.max_tokens = Some(4096);
        req.response_format_json = true;
        req.timeout_ms = Some(timeout_ms);
        let res = provider.complete_json(&req).await;
        (batch_idx, batch, res)
    }))
    .buffer_unordered(4)
    .collect()
    .await;
    batch_outcomes.sort_by_key(|(idx, _, _)| *idx);

    let mut inspected_items = Vec::with_capacity(records.len());
    let mut errors = Vec::new();
    let mut total_tokens = 0u32;

    for (batch_idx, batch, outcome) in batch_outcomes {
        let mut parsed_by_id: HashMap<String, Value> = HashMap::new();
        match outcome {
            Ok(resp) => {
                total_tokens = total_tokens.saturating_add(resp.usage.total_tokens);
                let payload = extract_json_payload(&resp.content);
                match serde_json::from_str::<Value>(&payload) {
                    Ok(val) => {
                        let arr = val
                            .as_array()
                            .cloned()
                            .or_else(|| {
                                val.get("inspections")
                                    .or_else(|| val.get("items"))
                                    .or_else(|| val.get("results"))
                                    .and_then(Value::as_array)
                                    .cloned()
                            })
                            .unwrap_or_default();
                        for entry in arr {
                            if let Some(obj) = entry.as_object()
                                && let Some(id) = get_str(obj, &["patient_id", "id"])
                            {
                                parsed_by_id.insert(id, entry);
                            }
                        }
                    }
                    Err(e) => {
                        errors.push(format!("批次 #{} AI 病例巡查 JSON 解析失败: {e}", batch_idx + 1));
                    }
                }
            }
            Err(e) => {
                errors.push(format!("批次 #{} 调用 AI 病例巡查失败: {e}", batch_idx + 1));
            }
        }

        for rec in batch {
            let local_val_err = rec.validate().err().map(|e| e.to_string());
            let fallback_dept = rec
                .l3_visit_events
                .first()
                .map(|e| e.recommended_department.clone())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "全科医学科".to_string());

            let mut item = if let Some(obj) =
                parsed_by_id.get(&rec.patient_id).and_then(Value::as_object)
            {
                let status = get_str(obj, &["status"])
                    .map(|s| InspectionStatus::parse_lenient(&s))
                    .unwrap_or(InspectionStatus::Pass);
                let suggested_department =
                    get_str(obj, &["suggested_department", "department", "建议科室"])
                        .unwrap_or(fallback_dept);
                let risk_level = get_str(obj, &["risk_level", "风险等级"])
                    .unwrap_or_else(|| "low".to_string());
                let category_tags = get_str_list(obj, &["category_tags", "tags", "分类标签"]);
                let pii_risk = obj
                    .get("pii_risk")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let issues = get_str_list(obj, &["issues", "问题"]);
                let recommendation = get_str(obj, &["recommendation", "建议"])
                    .unwrap_or_else(|| "病例巡查完成".to_string());

                PatientInspectionItem {
                    patient_id: rec.patient_id.clone(),
                    status,
                    suggested_department,
                    risk_level,
                    category_tags,
                    pii_risk,
                    issues,
                    recommendation,
                }
            } else {
                PatientInspectionItem {
                    patient_id: rec.patient_id.clone(),
                    status: InspectionStatus::Warning,
                    suggested_department: fallback_dept,
                    risk_level: "medium".to_string(),
                    category_tags: rec.l2_profile.chronic_diseases.clone(),
                    pii_risk: false,
                    issues: vec!["AI 巡查未返回该病例评估结果，已降级为人工复核".to_string()],
                    recommendation: "请人工确认脱敏完整性与既往史准确后再入库".to_string(),
                }
            };

            if let Some(err_msg) = local_val_err {
                item.status = InspectionStatus::Reject;
                item.pii_risk = true;
                item.issues.push(format!("隐私/结构校验未通过: {err_msg}"));
            }
            if item.pii_risk {
                item.status = InspectionStatus::Reject;
            }

            inspected_items.push(item);
        }
    }

    let mut summary = InspectionSummary {
        total: inspected_items.len(),
        ..Default::default()
    };
    let mut dept_counts: HashMap<String, usize> = HashMap::new();
    for it in &inspected_items {
        match it.status {
            InspectionStatus::Pass => summary.pass_count += 1,
            InspectionStatus::Warning => summary.warning_count += 1,
            InspectionStatus::Reject => summary.reject_count += 1,
        }
        *dept_counts
            .entry(it.suggested_department.clone())
            .or_insert(0) += 1;
    }

    let mut category_distribution: Vec<CategoryCount> = dept_counts
        .into_iter()
        .map(|(name, count)| CategoryCount { name, count })
        .collect();
    category_distribution.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));

    Ok(AiPatientInspectionResponse {
        items: inspected_items,
        summary,
        category_distribution,
        provider_used: provider.id().to_string(),
        model_used: provider.model_name().to_string(),
        total_tokens,
        errors,
    })
}

# 06. AI 大模型 API 集成与对外开放 API 体系规划

本项目核心是 **“AI LLM 大模型 + 海量医学知识库 + 病人长期记忆”** 的双引擎架构。本规划涵盖两个方向：
1. **上游集成**：统一接入多种开放大模型 API（云端商业模型 / 院内私有化 vLLM/Ollama）并实现智能路由与容灾。
2. **下游开放**：将本系统的“医疗知识库 + 长期记忆 + 急诊拦截 + 分诊推理”封装为标准化 **Open API**，不仅服务微信小程序，还可无缝对接医院 HIS 系统、院内自助导诊机、Web 端与第三方医疗应用。

---

## 1. 上游：多模型统一适配与分级路由（`medical-llm` / `medical-core`）

### 1.1 统一 Provider 抽象（Rust Trait）
通过标准 OpenAI-Compatible HTTP/SSE 协议适配绝大多数开放大模型，核心抽象为三个异步 Trait：
- `LlmProvider`：支持非流式结构化输出（Structured Output / JSON Mode）与 SSE 流式生成（`chat_stream`）。
- `EmbeddingProvider`：支持批量文本向量化（兼容云端 API 与本地 `fastembed-rs`）。
- `RerankProvider`：支持召回文档精排（兼容云端 Rerank API 与本地 ONNX `BGE-Reranker`）。

### 1.2 支持的上游模型生态
| 场景 | 推荐接入方式 | 典型模型示例 |
| :--- | :--- | :--- |
| **云端高性价比商用 API** | OpenAI 兼容接口（HTTPS） | DeepSeek、通义千问（Qwen）、智谱 GLM、Gemini 等 |
| **医院内网私有化部署** | 内网 vLLM / SGLang / Ollama（OpenAI 兼容接口） | Qwen-Med / Baichuan-Med / 开源 32B~72B 医学微调模型 |
| **本地离线向量与精排** | `fastembed-rs` / 本地 ONNX | `BGE-M3`（Embedding）、`BGE-Reranker-v2-m3` |

### 1.3 任务分级路由策略（降本 + 降延迟）
不同 ECS `System` 对模型能力与延迟要求不同，由统一 `ModelRouter` Resource 动态分发：
- **Fast Tier（轻量高速模型）**：
  - 负责：结合 L2/L3 长期记忆的 **Query 改写**、口语复杂症状补全、会话结束后的 **L2 病历摘要异步提取**。
  - 目标：低成本、首字延迟 `< 500ms`。
- **Reasoning Tier（高精度主力模型）**：
  - 负责：结合多路召回医学书籍依据 + 季节流行病权重，生成 **≤30 字短追问 + 快捷选项** 或 **最终结构化分诊科室建议**。
  - 目标：高临床准确性、强遵循 JSON Schema 与医学引用溯源约束。

### 1.4 容灾、限流与防幻觉护栏
1. **故障自动转移（Failover）**：主模型 API 触发 `429 Rate Limit`、`5xx` 或超时（如 > 8s）时，自动无缝切换至备用 Provider（如云端 A -> 云端 B -> 院内私有化）。
2. **全熔断降级**：当所有上游大模型均不可用时，ECS 自动降级为本地规则 + 纯知识库检索直接输出推荐科室，确保医院导诊零中断。
3. **严格结构化约束**：Prompt + JSON Schema 强制约束输出字段，并由 Rust `serde` 反序列化校验；若格式异常自动触发轻量修复或降级，绝不把乱码/非法医疗建议吐给前端。
4. **Prompt 隐私脱敏层**：调用外部云端大模型 API 前，中间件强制扫描并剔除任何残留的手机号、身份证号、真实姓名，确保仅发送脱敏后的医学特征与匿名 ID。

---

## 2. 下游：对外开放的标准化 AI + 知识库 API（`medical-server`）

为方便微信小程序、医院 HIS、导诊大屏及第三方系统调用，提供遵循 **OpenAPI 3.0** 规范的 REST + SSE 接口体系。

### 2.1 认证与多租户治理
- **微信小程序端**：`wx.login` -> `POST /v1/auth/wechat` 换取短期 JWT（绑定匿名 `patient_id`）。
- **外部系统 / 医院集成端**：通过 `Authorization: Bearer mk_live_xxxx`（由 `MedicalStudio` GUI 签发的 API Key）鉴权，支持按租户隔离：
  - 独立 QPM / TPM（每分钟请求数 / Token 限额）；
  - 独立知识库命名空间（例如 A 医院与 B 医院的科室设置、流行病权重相互隔离）。

### 2.2 核心开放接口清单

| 接口路径 | 方法 | 说明 |
| :--- | :--- | :--- |
| `/v1/triage/chat` | `POST` (支持 SSE) | **核心分诊对话接口**：输入 `session_id`、`patient_id`、`utterance`，自动执行 ECS 全链路（红线拦截 -> 长期记忆 -> 季节加权 RAG -> 短追问/科室卡） |
| `/v1/knowledge/search` | `POST` | **医学知识库混合检索接口**：供医生辅助查阅或外部应用调用，支持人群/科室过滤与季节加权开关 |
| `/v1/patients/{patient_id}/memory` | `GET / PUT / DELETE` | **病人长期记忆管理接口**：查询/更新 L2 画像与 L3 事件，以及患者合规**一键清空注销** |
| `/v1/epidemic/current` | `GET` | 查询当前地区生效的季节与流行病上下文 |
| `/v1/openai/chat/completions` | `POST` (支持 SSE) | **OpenAI 兼容增强代理端点**：外部系统只需修改 `base_url` 和 `api_key`，即可零改造接入本系统的“医学知识库 + 急诊红线 + 长期记忆”增强能力 |

### 2.3 `/v1/triage/chat` 响应示例（短追问与分诊结果）

**场景 A：需要继续追问（严格 ≤ 30 字 + 快捷选项）**
```json
{
  "session_id": "sess_9f8a7b6c",
  "status": "follow_up",
  "red_flag": "None",
  "turn": 2,
  "max_turns": 4,
  "question": "发烧大概持续多久了？",
  "quick_options": ["今天刚开始", "2~3天", "超过3天"],
  "seasonal_context_applied": ["甲型流行性感冒(高发)"]
}
```

**场景 B：完成分诊（附带知识库溯源依据）**
```json
{
  "session_id": "sess_9f8a7b6c",
  "status": "completed",
  "red_flag": "None",
  "triage_result": {
    "primary_department": "呼吸内科",
    "secondary_departments": ["发热门诊"],
    "urgency_level": 2,
    "summary": "高热伴全身酸痛及咽痛2天，结合当前冬季流感高发背景及既往轻度哮喘史，建议尽快前往呼吸内科或发热门诊评估。",
    "disclaimer": "本建议由 AI 智能导诊结合医学知识库生成，仅供参考，不替代执业医师诊断。"
  },
  "citations": [
    {
      "doc_id": "book_internal_med_9th_ch02_001",
      "source_title": "内科学（第9版）",
      "disease_name": "流行性感冒"
    }
  ]
}
```

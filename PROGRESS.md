# MedicalServer 开发进度存档（Checkpoint）

- **存档时间**: 2026-11-21
- **工作区**: `e:\RustP\MedicalServer`
- **当前构建与测试状态**: **全部里程碑 M1–M6 已完成并验收通过！**
  - `cargo build --workspace` / `cargo test --workspace`：exit code 0，**50/50 Rust 测试全绿**（`medical-core` 37 + `medical-pipeline` 7 + `medical-server` 4 + `medical-studio` 2）
  - `medical-studio` 前端（Vue 3 + Tailwind）：`vite build` 生产构建通过
  - `python -m e2e.run_e2e --spawn-server --tier 1`：**123/123 全量端到端测试通过（0 失败）**

---

## 一、已完成内容

### 1. Cargo Workspace 与基础设施 Schema（Phase 1）
- `Cargo.toml`（Workspace 配置，包含 `medical-core`、`medical-pipeline`、`medical-server`、`medical-studio` 四个成员）
- `migrations/001_init.sql`（PostgreSQL DDL：L2 长期画像、L3 历史就诊事件、流行病配置、词典表、分诊审计日志等）
- `config/qdrant/collections.json`（Qdrant 1024 维余弦相似度 Collection Schema）

### 2. `crates/medical-core`（核心领域、ECS 分诊引擎、安全与模型路由，已完成并通过 37 项测试）
- **数据模型与校验** (`src/models/`, `src/validation.rs`)：对齐 `doc/schemas/data-collection-schema.json`，7/7 测试通过（`models_validation_tests.rs`）
- **加解密与医疗安全护栏** (`src/crypto.rs`, `src/privacy.rs`)：AES-256-GCM 敏感字段加解密、密钥轮换、外发 PII 脱敏、确诊/处方药剂量输出拦截与免责声明，8/8 测试通过（`crypto_privacy_tests.rs`）
- **5 阶段 `bevy_ecs` 分诊流水线** (`src/ecs/`)：口语同义词标准化、急诊红线零 LLM 调用短路拦截（`Emergency120`）、L2/L3 记忆召回与同意门控沉淀、混合 RAG + 季节流行病动态加权、槽位状态机短追问约束（≤30 字、2~4 快捷选项、最多 4 轮）、全故障本地规则降级、ArcSwap 热更新，15/15 测试通过（`ecs_triage_tests.rs`）
- **存储与 LLM 接入层** (`src/storage/`, `src/llm/`)：PostgreSQL/Qdrant/Redis 生产客户端接口 + 离线内存适配器、`ModelRouter`（Fast/Reasoning 路由、故障转移、熔断、OpenAI 兼容 SSE 流式解析），7/7 测试通过（`storage_llm_tests.rs`）

### 3. `e2e/` 端到端测试框架与夹具
- 已建立 `e2e/run_e2e.py`、`e2e/lib/`（含 `mock_upstream.py`、`selftest_harness.py`、`http_client.py`、`assertions.py` 等）、`e2e/fixtures/`（知识库/病历/负载测试夹具）以及 `e2e/tests/`（Tier 1~4 测试套件）。

---

## 二、待开发内容（后续从此处继续）

### 里程碑 M2：`crates/medical-pipeline`（Phase 2 数据采集与双写管线，已完成并通过 7 项集成测试）
- [x] `sonic-rs` JSON/JSONL 采集包流式解析与 Schema 校验、UTF-8 BOM/CRLF 兼容、断点续传（`src/json_stream.rs`, `src/checkpoint.rs`）
- [x] `pulldown-cmark` / `docx-rs` / `pdf-extract` + `lopdf`（含 OCR 插件接口）医学书籍结构化切块与元数据注入（`src/book_chunker.rs`）
- [x] `calamine` / `csv` / `sonic-rs` 病历读取与基于 `rayon` 的并行隐私脱敏引擎、跨文件确定性加盐 SHA-256 `patient_id`（`src/patient_import.rs`）
- [x] 向量化适配接口（含可选 `local-fastembed`）+ `KnowledgeVectorStore` & `tantivy`（`tantivy-jieba` 中文分词）幂等双写与季节流行病加权混合检索（`src/indexer.rs`）
- [x] 自动化测试套件（`tests/pipeline_tests.rs`，7/7 通过）

### 里程碑 M3：`crates/medical-server` & 运维压测配置（Phase 4 服务端，已完成并通过 4 项集成测试）
- [x] Axum HTTP/SSE 服务：`/health`、`/metrics`、`/v1/auth/wechat`（code2Session + JWT）、`/v1/triage/chat`（非流式 + SSE）、`/v1/knowledge/search`、`/v1/openai/chat/completions`（JSON + SSE 增强代理）、`/v1/epidemic/current`、`/v1/patients/{patient_id}/memory`（GET/PUT/DELETE 物理清空）、`/v1/admin/reload`、`/v1/admin/epidemics`
- [x] 多租户 API Key 鉴权与 QPM/TPM 令牌桶限流（`src/auth.rs`, `src/rate_limit.rs`）、语义响应缓存、7 项核心 Prometheus 指标暴露（`src/metrics.rs`）
- [x] 端到端集成测试（`tests/server_integration_tests.rs`，4/4 通过，离线 Mock 适配器独立运行）
- [x] 部署与压测配置：`deploy/Dockerfile`（多阶段生产构建）、`k6/triage_load_test.js`、`deploy/prometheus/{prometheus,alerts}.yml`、`deploy/grafana/provisioning/*` & `deploy/grafana/dashboards/medical-server.json`

### 里程碑 M4：`crates/medical-studio`（Phase 2 & 3 Tauri 2.0 桌面工具箱，已完成并通过 2 项全模块集成测试与前端构建）
- [x] Tauri 2.0 Rust 后端（`src/state.rs`, `src/commands.rs`, `src/lib.rs`）：注册全部 28 个 `#[tauri::command]` 并通过 `tests/studio_commands_tests.rs`（2/2 通过）
- [x] Vue 3 + Vite + Tailwind CSS 前端（`frontend/src/components/`）与生产构建产出（`frontend/dist/`），交付全部六大模块：
  1. 医学知识库导入工坊（`KnowledgeImportPanel.vue`，切片预览、双写与断点续传、错误日志导出）
  2. 病人数据导入与脱敏中心（`PatientMaskingPanel.vue`，PII 列检测高亮、脱敏预检与入库）
  3. 季节与流行病上下文控制台（`EpidemicConsolePanel.vue`，强度滑块与 `medical-server` 热更新推送）
  4. 关键词与急危重症词典管理器（`DictionaryManagerPanel.vue`，双词典维护与实时测试沙盒）
  5. ECS 分诊白盒调试台（`EcsDebuggerPanel.vue`，左右分栏模拟聊天 + 实时 ECS Component Inspector）
  6. AI 大模型与开放 API 网关配置中心（`LlmGatewayPanel.vue`，多模型连通性测速、路由策略、API Key 签发吊销与指标统计）

### 里程碑 M5：`miniapp/`（Phase 4 原生微信小程序，已完成）
- [x] 完整原生小程序结构：`app.json`、`app.js`、`app.wxss`、`project.config.json`、`sitemap.json`、`utils/api.js`（含 `wx.login` 与 `enableChunked` SSE 流式分诊支持）
- [x] 隐私知情同意与 AI 导诊免责声明弹窗（`pages/triage/index.*`）
- [x] 流式对话与底部 2~4 个快捷选项按钮组
- [x] 急危重症红色警示弹窗（`Emergency120` + 一键 `wx.makePhoneCall('120')`）与结构化推荐科室挂号引导卡
- [x] 长期记忆查看（L2 画像 / L3 历次分诊）与一键物理清空注销入口（`pages/memory/index.*`）

### 里程碑 M6：全量集成验收（已完成）
- [x] `cargo check --workspace`、`cargo build --workspace`、`cargo test --workspace` 全绿（50/50 通过，0 warnings）
- [x] `medical-studio` 前端构建通过（`frontend/dist`）、`miniapp/` 静态结构与 WXML/WXSS/JS 校验通过
- [x] 运行 `python -m e2e.run_e2e --spawn-server --tier 1` 全量端到端验证：**123/123 全部通过（0 failed）**

---

## 三、恢复开发指引
下次恢复时，直接说明：**“读取 `PROGRESS.md`，从里程碑 M5（`miniapp/`）继续开发”** 即可无缝衔接。

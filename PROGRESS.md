# MedicalServer 开发进度存档（Checkpoint）

- **存档时间**: 2026-11-21
- **工作区**: `e:\RustP\MedicalServer`
- **当前构建与测试状态**: `cargo test --workspace` 全量通过（exit code 0，44/44 测试通过：`medical-core` 37 项 + `medical-pipeline` 7 项）

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

### 里程碑 M3：`crates/medical-server` & 运维压测配置（Phase 4 服务端）
- [ ] Axum HTTP/SSE 服务：`/v1/auth/wechat`（code2Session + JWT）、`/v1/triage/chat`（非流式 + SSE）、`/v1/knowledge/search`、`/v1/openai/chat/completions`、`/v1/epidemic/current`、`/v1/patients/{patient_id}/memory`（GET/PUT/DELETE 物理清空）
- [ ] 多租户 API Key 鉴权与 QPM/TPM 令牌桶限流、Redis 语义缓存、Prometheus 指标暴露
- [ ] 端到端集成测试（离线 Mock 适配器）
- [ ] 部署与压测配置：多阶段 `Dockerfile`、`k6` 压测脚本、Prometheus + Grafana 监控告警配置

### 里程碑 M4：`crates/medical-studio`（Phase 2 & 3 Tauri 2.0 桌面工具箱）
- [ ] 前端（Vue 3 + Tailwind）与 Tauri Rust Commands 绑定，交付六大模块：
  1. 医学知识库导入工坊（切片预览与断点续传）
  2. 病人数据导入与脱敏中心
  3. 季节与流行病上下文控制台（热更新通知）
  4. 关键词与急危重症词典管理器（测试沙盒）
  5. ECS 分诊白盒调试台（左右分栏透视各 `Component` 状态）
  6. AI 大模型与开放 API 网关配置中心

### 里程碑 M5：`miniapp/`（Phase 4 原生微信小程序）
- [ ] 隐私知情同意与 AI 导诊免责声明弹窗
- [ ] 流式对话与底部快捷选项按钮组
- [ ] 急危重症红色警示弹窗（一键拨打 120）与推荐科室挂号引导卡
- [ ] 长期记忆查看与一键清空入口

### 里程碑 M6：全量集成验收
- [ ] `cargo check --workspace`、`cargo build --workspace`、`cargo test --workspace` 全绿
- [ ] `medical-studio` 前端构建通过、`miniapp/` 静态结构校验通过
- [ ] 运行 `e2e/run_e2e.py` 全量端到端验证

---

## 三、恢复开发指引
下次恢复时，直接说明：**“读取 `PROGRESS.md`，从里程碑 M3（`medical-server`）继续开发”** 即可无缝衔接。

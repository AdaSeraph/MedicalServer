# 04. 分阶段开发步骤、压测与上线规划

## Phase 1：Workspace 初始化与核心模型（第 1~2 周）
- [ ] 将 `e:\RustP\MedicalServer` 重构为 Cargo Workspace，建立 `medical-core`、`medical-pipeline`、`medical-server`、`medical-studio` 四个成员。
- [ ] 在 `medical-core` 中实现与 `doc/schemas/data-collection-schema.json` 严格对齐的 Rust 数据结构（`serde` + 校验逻辑）。
- [ ] 设计并初始化 PostgreSQL 表结构（L2 病人长期画像、L3 历史就诊事件、流行病配置、词典表、分诊审计日志）与 Qdrant Collection Schema。
- [ ] 搭建 `bevy_ecs` 基础分诊 World 骨架，跑通单元测试（急诊红线拦截短路验证）。

## Phase 2：纯 Rust 数据管线 + Tauri 导入工具（第 3~5 周）
- [ ] 开发 `medical-pipeline`：
  - 基于 `sonic-rs` 的标准 JSON 采集包高速解析器；
  - 基于 `pulldown-cmark`/`lopdf` 的医学书籍结构化切块器；
  - 基于 `calamine`/`csv` 的病人历史数据解析与隐私脱敏引擎；
  - 集成向量化（`fastembed-rs` / HTTP Batch Embedding）与 `qdrant-client` + `tantivy` 双写。
- [ ] 初始化 Tauri 2.0 `medical-studio`，优先交付：
  - **模块一（医学知识库导入工坊）**；
  - **模块二（病人数据导入与脱敏中心）**。
- [ ] 使用采集规范灌入首批医学教材/临床指南与脱敏测试病历，验证混合检索召回准确率。

## Phase 3：ECS 分诊引擎、长期记忆与白盒调试（第 6~8 周）
- [ ] 完善 `medical-core` 中的全部 ECS Systems 与上游大模型接入层：
  - `LlmProvider` / `ModelRouter` 统一抽象（支持 OpenAI 兼容云端 API 与私有化 vLLM/Ollama、Fast/Reasoning 分级路由、超时故障转移与隐私脱敏过滤）；
  - `keyword_extraction_system`（`aho-corasick` + `jieba-rs`）；
  - `memory_recall_system`（L2 必读 + L3 向量召回 + Query 改写）；
  - `hybrid_rag_system`（Qdrant + Tantivy + Rerank + 季节流行病动态加权）；
  - `slot_question_system`（槽位信息增益计算 + 强制 ≤30 字短追问 + 快捷选项生成）；
  - `memory_consolidation_system`（会话结束后异步提取并更新 L2/L3 长期记忆）。
- [ ] 在 `medical-studio` 中上线 **模块三（季节流行病控制台）**、**模块四（词典管理器）**、**模块五（ECS 分诊白盒调试台）** 与 **模块六（AI 模型与开放 API 网关配置中心）**。
- [ ] 联合医学背景人员在白盒调试台中进行多轮典型病例（含流感季/非流感季对照、慢病复诊记忆对照）盲测调优。

## Phase 4：高并发服务端、微信小程序与云端上线（第 9~12 周）
- [ ] 完善 `medical-server`（`Axum`）：
  - 微信小程序 `code2Session` 登录与 JWT 鉴权；
  - 对外开放标准化 Open API（`/v1/triage/chat`、`/v1/knowledge/search`、`/v1/openai/chat/completions` 兼容代理端点）与多租户 API Key 鉴权/配额限流；
  - SSE 流式分诊接口、按 `OpenID`/租户令牌桶限流、常见病语义缓存（Redis）、大模型全熔断本地降级；
  - 病人长期记忆查询与**一键注销/物理删除**合规接口。
- [ ] 开发微信小程序端：
  - 首次进入隐私知情同意与“AI 辅助导诊免责声明”弹窗；
  - 流式对话界面 + 底部快捷选项按钮组（一键点选，减少打字）；
  - 急危重症红色警示弹窗（一键拨打 120）与结构化推荐科室挂号引导卡。
- [ ] 压测与云端部署：
  - 使用 `k6` 模拟医院早高峰并发流量，压测 SSE 长连接数、PostgreSQL 连接池、Qdrant P99 检索延迟；
  - Docker 多阶段构建部署至云端医疗合规区，接入 Prometheus + Grafana 监控告警；
  - 完成微信小程序类目资质准备与审核上线，先在 1~2 个科室小流量灰度运行。

---

## 医疗合规与上线红线检查清单
1. **免责与定位**：明确标注“AI 智能导诊仅供参考，不替代执业医师诊断”，严禁输出确诊结论与具体处方药剂量。
2. **数据安全**：病人 L2/L3 长期记忆中的敏感健康字段落库采用 AES-256-GCM 加密，全程 HTTPS 传输，不采集非必要真实身份信息。
3. **患者权利**：小程序内必须提供清晰入口，允许患者查看或一键清空自己的长期记忆档案。
4. **急症兜底**：所有急危重症红线拦截必须由本地规则引擎在调用大模型前完成，确保 100% 稳定触发、零幻觉。

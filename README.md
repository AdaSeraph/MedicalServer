# MedicalServer — 高并发智能医疗分诊与知识库系统

[![CI](https://github.com/AdaSeraph/MedicalServer/actions/workflows/ci.yml/badge.svg)](https://github.com/AdaSeraph/MedicalServer/actions/workflows/ci.yml)
[![Release](https://github.com/AdaSeraph/MedicalServer/actions/workflows/release.yml/badge.svg)](https://github.com/AdaSeraph/MedicalServer/actions/workflows/release.yml)

**MedicalServer** 是一套面向医院高并发场景构建的纯 Rust 智能医疗分诊、医学知识库检索与开放 API 平台。系统以 **“AI 大模型分级路由 + 混合 RAG 医学知识库 + 病人三层长期记忆 + 急危重症本地红线拦截”** 为核心，同时配套提供 **Tauri 2.0 桌面运营工具箱（`MedicalStudio`）** 与 **患者端原生微信小程序（`miniapp/`）**。

---

## 核心特性

- **5 阶段 `bevy_ecs` 高并发分诊引擎（`medical-core`）**
  - **急诊红线零大模型短路拦截**：基于 `aho-corasick` + `jieba-rs` 自动机，命中胸痛放射左臂、呼吸窘迫、昏迷等高危组合时毫秒级直接返回 `Emergency120`（0 次 LLM 调用），保障生命安全。
  - **短而准的智能追问约束**：槽位状态机强制单次追问仅问 1 个核心问题、正文 ≤ 30 字、附带 `2~4` 个快捷选项，最多追问 4 轮自动收敛。
  - **三层长期记忆（L1/L2/L3）与隐私加密**：L1 Redis 实时会话状态、L2 PostgreSQL 长期健康画像（慢病/过敏/用药采用 **AES-256-GCM** 字段级加密）、L3 历史就诊向量按需召回；支持患者合规**一键物理清空注销**。
  - **季节与流行病动态加权**：按地区/季节实时加权高发传染病得分（`final_score = score * (1.0 + 0.35 * intensity)`），支持 `ArcSwap` 秒级热更新无需重启。
  - **多模型分级路由与全熔断降级**：Fast / Reasoning 双层路由、超时与 `429/5xx` 自动故障转移、外发前 PII 脱敏扫描；上游大模型全部不可用时自动降级为本地规则 + 知识库分诊。
- **纯 Rust 高性能数据采集与双写管线（`medical-pipeline`）**
  - `sonic-rs` 极速流式解析 JSON/JSONL（兼容 UTF-8 BOM 与 Windows CRLF）与 Schema 校验；
  - `pulldown-cmark` / `docx-rs` / `pdf-extract` + `lopdf`（预留 OCR 插件接口）多格式医学书籍语义切块与元数据注入；
  - `csv` / `calamine` + `rayon` 并行病历脱敏（跨文件确定性加盐 SHA-256 匿名 `patient_id`、年龄分段、自由文本 PII 清洗）；
  - `tantivy`（`tantivy-jieba` 中文医学分词）+ 向量库（Qdrant / 离线内存）按 `doc_id` 幂等双写与断点续传。
- **高并发 Axum 开放 API 网关（`medical-server`）**
  - 微信小程序 `code2Session` + JWT 鉴权、多租户 `mk_live_*` API Key 鉴权与独立 **QPM/TPM 双令牌桶限流**、语义缓存、SSE 流式分诊、OpenAI 兼容增强代理与 Prometheus 监控指标。
- **Tauri 2.0 桌面工作台（`medical-studio`）**
  - Vue 3 + Tailwind CSS 六大运营管理模块：医学知识库导入工坊、病人数据脱敏中心、季节流行病控制台、同义词与 120 红线词典沙盒、ECS 白盒调试台、AI 大模型与 API 网关配置中心。
- **零外部依赖离线开发与测试**
  - 完整保留 PostgreSQL / Redis / Qdrant / OpenAI 兼容生产客户端接口，同时内置内存适配器，无外部数据库或 API 密钥即可直接编译、运行并通过全部自动化测试。

---

## 项目目录结构

```text
MedicalServer/
├── Cargo.toml                        # 5-Crate Rust Workspace 配置
├── crates/
│   ├── medical-core/                 # 领域模型（含医学检查/生化/影像/伤口/诱因多模态模型）、AES-256-GCM、bevy_ecs 引擎
│   ├── medical-pipeline/             # sonic-rs 流式解析、书籍切块、病历脱敏与检查导入、Tantivy + Qdrant 双写
│   ├── medical-server/               # Axum HTTP/SSE 线上服务、临床检查/知识库管理接口、JWT/API Key 鉴权、Prometheus 指标
│   ├── medical-studio/               # Tauri 2.0 桌面工具箱（含多模态医学检查、生化与图像导入中心）
│   └── medical-mcp/                  # 标准 MCP (JSON-RPC 2.0 stdio) 工具套件（19 Tools + 4 Resources + 3 Prompts）
├── miniapp/                          # 患者端原生微信小程序（隐私弹窗、SSE 分诊、120 警示、长期记忆清空）
├── migrations/001_init.sql           # PostgreSQL 初始化 DDL
├── config/qdrant/collections.json    # Qdrant 1024 维余弦相似度 Collection Schema
├── deploy/                           # 生产部署套件（Dockerfile、docker-compose、systemd、install-linux.sh、监控）
├── k6/triage_load_test.js            # k6 高并发压测脚本
├── e2e/                              # Python 4 层端到端自动化测试框架与测试夹具
└── doc/                              # 完整架构与需求设计文档（01~06 + data-collection-schema.json）
```

---

## 本地快速开始

### 环境要求
- **Rust**（stable，推荐 1.85+）
- **Node.js** 20+（仅修改 `medical-studio` 前端时需要）
- **Python** 3.11+（仅运行 `e2e/` 端到端测试时需要）

### 1. 启动后端服务（`medical-server`）

无外部数据库时自动启用离线内存模式，启动后监听 `http://127.0.0.1:8080`：

```bash
cargo run --package medical-server --bin medical-server
```

验证服务健康与 Prometheus 指标：

```bash
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/metrics
```

> **RustRover 用户提示**：请将运行配置命令设置为 `run --package medical-server --bin medical-server`（或 `run --package medical-studio --bin medical-studio`），不要使用旧的单包名 `MedicalServer`。

### 2. 启动桌面工具箱（`medical-studio`）

```bash
# 如修改了前端页面，先重新构建 Vue 3 资源
cd crates/medical-studio/frontend && npm install && npm run build && cd ../../..

# 启动 Tauri 2.0 桌面应用
cargo run --package medical-studio --bin medical-studio
```

### 3. 运行全量测试与 E2E 验收

```bash
# 全工作区 Rust 单元测试与集成测试（50/50）
cargo test --workspace

# 端到端 Tier 1 测试（自动编译拉起 medical-server + Mock 上游 LLM，123 项用例）
cargo build -p medical-server
python -m e2e.run_e2e --spawn-server --tier 1
```

### 4. 导入微信小程序（`miniapp/`）
1. 打开 **微信开发者工具**，导入 `miniapp/` 目录。
2. 在 `miniapp/app.js` 中将 `apiBaseUrl` 修改为你的后端地址（本地调试默认 `http://127.0.0.1:8080`）。

---

## 云端服务器部署指南

每次推送至 `master`/`main` 或打 `v*` 标签后，GitHub Actions 会自动构建并发布最新的云端部署包与 Docker 镜像。

### 方式一：Docker Compose 一键部署（推荐）

自动拉起 `medical-server` + PostgreSQL 16 + Redis 7 + Qdrant + Prometheus + Grafana：

```bash
git clone https://github.com/AdaSeraph/MedicalServer.git
cd MedicalServer/deploy
cp .env.example .env
# 编辑 .env，务必替换 JWT_SECRET、FIELD_ENCRYPTION_KEY、OPENAI_API_KEY 等生产密钥

# 拉取 GHCR 预构建镜像并启动全套服务
docker compose pull medical-server
docker compose up -d
```

- 服务端入口：`http://<服务器IP>:8080`
- Prometheus：`http://<服务器IP>:9090`
- Grafana 监控大盘：`http://<服务器IP>:3000`（默认账号 `admin` / `medical_admin`）

### 方式二：Linux 云服务器独立 Release 包（`systemd` 托管）

从 [GitHub Releases](https://github.com/AdaSeraph/MedicalServer/releases) 下载 `medical-server-cloud-linux-x86_64.tar.gz`：

```bash
mkdir -p medical-release && tar -xzf medical-server-cloud-linux-x86_64.tar.gz -C medical-release
cd medical-release
bash deploy/install-linux.sh
```

脚本会自动安装到 `/opt/medical-server`、生成 `/opt/medical-server/.env` 并注册 `medical-server.service` 开机自启。编辑 `/opt/medical-server/.env` 后执行：

```bash
sudo systemctl restart medical-server
curl http://127.0.0.1:8080/health
```

---

## 核心 Open API 接口速查

| 接口路径 | 方法 | 鉴权方式 | 说明 |
| :--- | :--- | :--- | :--- |
| `/health` | `GET` | 无 | 服务健康与存储/索引状态检查 |
| `/metrics` | `GET` | 无 | Prometheus 监控指标（QPS、SSE 连接数、P95 延迟、缓存命中、LLM 错误） |
| `/v1/auth/wechat` | `POST` | 无 | 微信小程序 `wx.login` `code` 换取 JWT 与匿名 `patient_id` |
| `/v1/triage/chat` | `POST` | JWT / API Key | 核心 ECS 分诊对话（支持 `stream: true` SSE 流式与 `stream: false` JSON） |
| `/v1/knowledge/search` | `POST` | JWT / API Key | 混合 RAG 医学知识库检索（支持人群/科室硬过滤、租户隔离、季节加权与缓存） |
| `/v1/openai/chat/completions` | `POST` | JWT / API Key | OpenAI 兼容增强代理（自动注入急诊红线拦截、PII 脱敏与免责声明） |
| `/v1/epidemic/current` | `GET` | 无 | 查询指定地区与季节当前生效的流行病上下文 |
| `/v1/patients/{patient_id}/memory` | `GET / PUT / DELETE` | JWT / API Key | 病人 L2/L3 长期记忆查看、更新与**合规一键物理清空注销** |
| `/v1/admin/reload` | `POST` | Admin Token | 触发流行病配置、医学词典与 LLM 路由的 `ArcSwap` 零停机热更新 |

---

## 关键环境变量

完整模板见 [deploy/.env.example](file:///e:/RustP/MedicalServer/deploy/.env.example)：

| 变量名 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `SERVER_ADDR` | `127.0.0.1:8080` | 服务监听地址（容器/云端建议 `0.0.0.0:8080`） |
| `MEDICAL_R5_OFFLINE` | 未设 `DATABASE_URL` 时为 `true` | `true` 使用内存适配器；生产接入 PG/Redis/Qdrant 时设为 `false` |
| `DATABASE_URL` | — | PostgreSQL 连接串 |
| `REDIS_URL` | — | Redis 连接串（L1 会话记忆与语义缓存） |
| `QDRANT_URL` | — | Qdrant gRPC 地址（默认 `http://127.0.0.1:6334`） |
| `JWT_SECRET` | dev 默认值 | 生产环境必须替换为高熵随机密钥 |
| `FIELD_ENCRYPTION_KEY` | dev 默认值 | 32 字节 AES-256-GCM 密钥（64 位 Hex 或 Base64），用于加密病人 L2/L3 敏感字段 |
| `OPENAI_BASE_URL` / `OPENAI_API_KEY` | — | 上游 OpenAI 兼容大模型端点（支持 DeepSeek、Qwen、院内 vLLM/Ollama） |
| `MEDICAL_ADMIN_TOKEN` | `e2e-admin-secret` | `/v1/admin/*` 热更新与管理接口鉴权令牌 |

---

## CI/CD 工作流与桌面安装包

- **[CI](file:///e:/RustP/MedicalServer/.github/workflows/ci.yml)**：每次 `push` / `PR` 自动在 `ubuntu-latest` 与 `windows-latest` 双平台执行 Vue 3 前端构建、`cargo check/build/test --workspace` 及 123 项 Tier 1 E2E 测试。
- **[Release](file:///e:/RustP/MedicalServer/.github/workflows/release.yml)**：推送至 `master`/`main`、打 `v*` 标签或手动触发时，自动编译并发布：
  - **Windows 桌面安装包（推荐新电脑直接下载）**：`MedicalStudio_*_x64-setup.exe`（NSIS 中文安装向导）与 `.msi` —— 已启用 `+crt-static` 静态链接（**无需 VC++ 运行库**），并内嵌微软官方 **WebView2 Bootstrapper**，新电脑缺少 WebView2 时安装过程会自动下载安装（手动触发 `workflow_dispatch` 时还可勾选 `offline_webview2: true` 生成离线全量安装包）。
  - **Linux 桌面安装包**：`MedicalStudio_*_amd64.deb`（`sudo apt install ./MedicalStudio_*.deb` 自动补齐系统库）、`.rpm` 与 `.AppImage`。
  - **Linux 云服务器静态部署包**：`medical-server-cloud-linux-x86_64.tar.gz`（`x86_64-unknown-linux-musl` 全静态编译，不依赖宿主 `glibc` 版本）。
  - **云端 Docker 镜像**：`ghcr.io/adaseraph/medicalserver/medical-server:latest`。
  - **全套便携压缩包**：`medical-suite-x86_64-pc-windows-msvc.zip`（内含 `start-medical-studio.bat` 自动检测并引导安装 WebView2）/ `medical-suite-x86_64-unknown-linux-gnu.tar.gz`（内含 `deploy/linux/install-studio-deps.sh`）。
  - **完整性校验**：`SHA256SUMS.txt`。

### 本地一键打包 Windows 安装包

```powershell
# 默认生成带 WebView2 引导程序的 NSIS / MSI 安装包与便携 zip（新电脑安装时自动补齐 WebView2）
.\deploy\package-windows.ps1

# 如目标电脑为纯内网/离线环境，可追加 -OfflineWebView2 嵌入完整离线 WebView2 安装包
.\deploy\package-windows.ps1 -OfflineWebView2
```

---

## 医疗安全与免责声明

> 本系统所有 AI 分诊建议均结合医学知识库生成，仅供参考，不替代执业医师诊断。遇到胸痛、呼吸困难、意识障碍、大出血等急危重症情况，请立即拨打 **120** 急救电话或前往最近医院急诊科救治。

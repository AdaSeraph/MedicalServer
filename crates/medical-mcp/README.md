# MedicalServer MCP Tool Suite (`medical-mcp`)

`medical-mcp` 是面向外部 AI Agent（Claude Desktop、Cursor、Windsurf、Antigravity 等）的标准 **Model Context Protocol (MCP)** 工具套件，通过 JSON-RPC 2.0 (`stdio`) 直接操作 MedicalServer 的全部核心能力。

## 三种无头（Headless）运行与持久化模式

1. **独立嵌入式无头模式（AI Harness 直接拉起，关闭后自动加密保存数据）**：
   ```bash
   # 默认持久化至 ./data（含 Tantivy 知识库索引 + AES-256-GCM 加密病例/配置快照），每次写入及退出时自动落盘
   medical-mcp --standalone --data-dir ./data
   ```
2. **连接运行中的本机/云端 `medical-server` 或 `MedicalStudio`（HTTP 模式）**：
   ```bash
   medical-mcp --url http://<本机或云端IP>:8080 --admin-token <ADMIN_TOKEN>
   ```
3. **云端 Docker 容器内直连无头操作**：
   ```bash
   docker exec -i medical-server medical-mcp --url http://127.0.0.1:8080
   ```
   云端 Docker 部署已自动挂载持久卷 `medical_data:/app/data`（保留 Tantivy 索引与本地加密快照）及 `pg_data` / `redis_data` / `qdrant_data`，并监听 `SIGTERM` 在容器停止时自动刷盘。

## MCP 客户端配置示例（Claude Desktop / Cursor / Antigravity）

参考 [config/mcp/mcp_client_config.example.json](../../config/mcp/mcp_client_config.example.json)：

```json
{
  "mcpServers": {
    "medical-server": {
      "command": "E:/RustP/MedicalServer/target/release/medical-mcp.exe",
      "args": ["--url", "http://127.0.0.1:8080"],
      "env": {
        "MEDICAL_ADMIN_TOKEN": "e2e-admin-secret"
      }
    }
  }
}
```

## 内置 20 个 MCP 工具

| 工具名称 | 功能说明 |
| :--- | :--- |
| `server_health` | 检查服务健康状态、存储模式、知识库文档数、LLM 路由状态 |
| `get_clinical_data_catalog` | 获取支持的医学检查、生化(LOINC)、血压/生命体征、核磁/CT影像、伤口TIME评估、诱因物质与图片数据类型目录 |
| `triage_chat` | 执行 ECS 智能分诊对话（含 120 红线拦截、混合 RAG、三层记忆） |
| `knowledge_search` | 混合 RAG 知识库检索（支持科室/人群/季节流行病加权） |
| `knowledge_list` | 浏览/筛选知识库文档列表 |
| `knowledge_import` | 导入/更新含参考生化、影像特征、伤口图谱、诱因物质与图片的知识库文档 |
| `knowledge_delete` | 按 `doc_id` 删除知识库文档 |
| `patient_list` | 列出匿名患者 ID |
| `patient_get_memory` | 查询患者 L2/L3 长期记忆及多模态医学检查记录 |
| `patient_upsert_memory` | 创建或更新患者 L2/L3 加密病历 |
| `patient_add_clinical_exam` | 向患者病例追加血压、抽血生化、核磁/CT影像、伤口评估与照片、诱因物质与照片 |
| `patient_import_batch` | 批量导入并脱敏原始患者病例（JSON/CSV，自动解析检查与图像列） |
| `patient_delete_memory` | 患者合规一键物理清空 L1/L2/L3 记忆 |
| `epidemic_list` | 查询当前生效的地区/季节流行病上下文 |
| `epidemic_upsert` | 新增或更新流行病动态加权规则 |
| `dictionary_get` | 查询同义词与 120 急诊红线词典 |
| `dictionary_upsert_synonym` | 新增或更新口语-标准医学症状同义词 |
| `dictionary_upsert_red_flag` | 新增或更新 120/急诊红线拦截规则 |
| `admin_reload` | 触发 `ArcSwap` 零停机热更新 |
| `admin_save_snapshot` | 立即将当前加密数据快照刷盘持久化 |

## 内置 MCP Resources & Prompts

- Resources: `medical://health`, `medical://catalog/clinical-types`, `medical://epidemics/current`, `medical://dictionaries`
- Prompts: `medical_triage_workflow`, `import_clinical_case_with_exams`, `import_knowledge_with_multimodal_exams`

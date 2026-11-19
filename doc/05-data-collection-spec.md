# 05. 数据采集 JSON 规范说明

标准模板文件位于：[data-collection-schema.json](file:///e:/RustP/MedicalServer/doc/schemas/data-collection-schema.json)。

无论是人工整理医学书籍、爬取/清洗公开医学数据集，还是从医院导出脱敏历史病历，只要按此 JSON 格式（或按每条对象一行导出为 `.jsonl`）组织数据，即可直接被 `medical-pipeline` 和 `MedicalStudio` GUI 一键导入。

---

## 1. `knowledge_documents`（医学书籍与知识库条目）

| 字段 | 类型 | 必填 | 说明 |
| :--- | :--- | :--- | :--- |
| `doc_id` | string | 是 | 全局唯一 ID（用于幂等导入、更新与分诊引用溯源） |
| `source_title` | string | 是 | 书名或指南名称（如 `内科学（第9版）`） |
| `source_type` | enum | 是 | `textbook`（教材）/ `guideline`（临床指南）/ `hospital_manual`（院内科室手册）/ `qa_dataset`（问答数据集） |
| `authority_level` | float | 是 | 权威权重 `0.1 ~ 1.0`（教材/指南建议 `0.9~1.0`，普通问答库建议 `0.5~0.6`） |
| `disease_name` | string | 是 | 标准疾病名称 |
| `aliases` | string[] | 否 | 疾病俗称或别名 |
| `department` | string | 是 | 首选推荐科室 |
| `secondary_departments` | string[] | 否 | 备选或急诊科室 |
| `target_population` | string[] | 是 | 适用人群：`adult` / `child` / `elderly` / `pregnant` / `female` / `male`（用于检索硬过滤） |
| `section_type` | enum | 是 | `clinical_manifestation`（临床表现）/ `differential_diagnosis`（鉴别诊断）/ `red_flag`（急症指征）/ `triage_rule`（分诊规则） |
| `content` | string | 是 | 切块后的医学正文（建议单块控制在 200~600 字，语义完整） |
| `key_symptoms` | string[] | 是 | 核心标准症状词列表 |
| `differential_points` | string[] | 否 | 鉴别诊断要点（供 AI 判断追问方向） |
| `red_flag_signs` | string[] | 否 | 危重预警信号 |
| `seasonal_tags` | string[] | 否 | 季节/流行病标签（与 `epidemic_contexts.seasonal_tag` 联动加权） |
| `suggested_follow_up_slots` | object[] | 否 | 推荐短追问模板（`short_question` 建议 ≤ 25 字，`quick_options` 2~4 个） |

---

## 2. `patient_records`（病人长期记忆采集格式）

- **`patient_id`**：必须是脱敏后的匿名 ID（严禁直接填入明文身份证号或手机号）。
- **`consent_long_term_memory`**：布尔值，标记患者是否授权开启长期记忆留存。
- **`l2_profile`**：固定结构化长期画像（`age_group`、`gender`、`is_pregnant`、`chronic_diseases`、`allergies`、`surgical_history`、`long_term_medications`）。
- **`l3_visit_events`**：历史分诊/就诊事件数组，每次就诊一条精简摘要，供后续跨时间按需向量召回。

---

## 3. `epidemic_contexts`（季节与流行病上下文）

- 通过 `region_code`（行政区划码）+ `season` + `active` 控制生效范围。
- `intensity`（`0.0 ~ 1.0`）控制当季高发疾病在检索精排时的加权系数。
- `boost_symptoms` 与 `priority_question_hint` 共同决定 AI 在流行季的敏感度与优先追问方向。

---

## 4. `medical_dictionaries`（词典与急诊红线规则）

- **`symptom_synonyms`**：将患者口语化描述批量映射为标准医学术语。
- **`red_flag_rules`**：组合式急诊拦截规则（满足 `trigger_all_of` 且命中任意一个 `trigger_any_of` 即立刻触发 `Emergency120` 并短路大模型）。

---

## 5. 超大数据集采集建议（JSONL 格式）

当医学书籍切片或医患问答数据超过 **10 万条** 时，请不要把全部数据放进单个巨型 JSON 数组，而是按类型拆分为 **`.jsonl`（JSON Lines）** 文件（每行一个独立的 `knowledge_document` 或 `patient_record` JSON 对象）。`medical-pipeline` 会使用 `sonic-rs` + `rayon` 按行流式并发读取，内存占用恒定极低。

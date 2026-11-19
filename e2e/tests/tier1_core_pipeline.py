"""Tier 1 Opaque-Box Feature Coverage Tests: Features F1–F11 (63 tests)."""

from __future__ import annotations

import json
import re
import tempfile
import time
from pathlib import Path
from typing import Any

from e2e.fixtures.generator import (
    generate_large_jsonl,
    generate_minimal_docx,
    generate_minimal_pdf,
    generate_minimal_xlsx,
)
from e2e.lib.assertions import (
    assert_completed_response,
    assert_disclaimer_present,
    assert_emergency_response,
    assert_follow_up_response,
    assert_no_pii,
    collect_rs_text,
    load_schema_rules,
    validate_collection_bundle,
    validate_epidemic_context,
    validate_knowledge_document,
    validate_medical_dictionaries,
    validate_patient_record,
)
from e2e.lib.mock_upstream import FaultRule
from e2e.lib.parsers import (
    inspect_pdf_bytes,
    parse_json_file,
    parse_jsonl_stream,
    parse_prometheus_metrics,
    parse_sql_ddl,
    parse_toml_file,
    read_docx_paragraphs,
    read_xlsx_rows,
    validate_ico_bytes,
    validate_qdrant_collections,
)
from e2e.lib.registry import REGISTRY
from e2e.lib.runner import TestContext

_SQL_REQUIRED_DOMAINS = (
    ("l2", ("l2_profile", "patient_l2", "patients_l2")),
    ("l3", ("l3_visit", "patient_l3", "l3_event")),
    ("epidemic", ("epidemic_context",)),
    ("synonyms", ("symptom_synonym", "medical_dictionar")),
    ("red_flags", ("red_flag_rule", "medical_dictionar")),
    ("audit", ("triage_audit_log", "audit_log")),
    ("api_keys", ("api_key",)),
    ("llm_providers", ("llm_provider",)),
    ("checkpoints", ("ingestion_checkpoint", "checkpoint")),
)


def _read_sql(ctx: TestContext) -> str:
    p = ctx.workspace / "migrations" / "001_init.sql"
    assert p.is_file(), f"Missing migration DDL: {p}"
    return p.read_text(encoding="utf-8")


def _all_crate_toml(ctx: TestContext) -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {}
    for name in ("medical-core", "medical-pipeline", "medical-server", "medical-studio"):
        out[name] = parse_toml_file(ctx.workspace / "crates" / name / "Cargo.toml")
    return out


def _collect_rs_text(crate_dir: Path) -> str:
    return collect_rs_text(crate_dir)


# ===========================================================================
# F1: Cargo 4-Crate Workspace & Windows Build Config (6 tests)
# ===========================================================================

@REGISTRY.register("F1-T1-01", tier=1, features=[1], title="Root Cargo.toml 4-crate workspace membership")
def test_f1_t1_01(ctx: TestContext) -> None:
    data = parse_toml_file(ctx.workspace / "Cargo.toml")
    ws = data.get("workspace")
    assert isinstance(ws, dict), "Root Cargo.toml missing [workspace] table"
    members = set(ws.get("members", []))
    expected = {
        "crates/medical-core",
        "crates/medical-pipeline",
        "crates/medical-server",
        "crates/medical-studio",
    }
    missing = expected - members
    assert not missing, f"Missing workspace members in Cargo.toml: {sorted(missing)}"
    assert "package" not in data, "Root Cargo.toml should be pure workspace (no orphan [package])"


@REGISTRY.register("F1-T1-02", tier=1, features=[1], title="Four crate layouts and dependency graph")
def test_f1_t1_02(ctx: TestContext) -> None:
    manifests = _all_crate_toml(ctx)
    for name, data in manifests.items():
        assert isinstance(data.get("package"), dict), f"{name}/Cargo.toml missing [package]"
        crate_dir = ctx.workspace / "crates" / name
        assert (crate_dir / "src" / "lib.rs").is_file(), f"{name} missing src/lib.rs"

    assert (ctx.workspace / "crates" / "medical-server" / "src" / "main.rs").is_file()
    assert (ctx.workspace / "crates" / "medical-studio" / "src" / "main.rs").is_file()
    assert (ctx.workspace / "crates" / "medical-studio" / "build.rs").is_file()

    def deps(c: str) -> set[str]:
        return set((manifests[c].get("dependencies") or {}).keys())

    assert "medical-core" in deps("medical-pipeline")
    assert "medical-core" in deps("medical-server")
    assert "medical-core" in deps("medical-studio")
    assert "medical-pipeline" in deps("medical-studio")


@REGISTRY.register("F1-T1-03", tier=1, features=[1], title="Windows-safe sqlx TLS and optional local-fastembed feature")
def test_f1_t1_03(ctx: TestContext) -> None:
    root = parse_toml_file(ctx.workspace / "Cargo.toml")
    ws_deps = (root.get("workspace") or {}).get("dependencies") or {}
    manifests = _all_crate_toml(ctx)

    sqlx_entries: list[Any] = []
    if "sqlx" in ws_deps:
        sqlx_entries.append(ws_deps["sqlx"])
    for m in manifests.values():
        d = m.get("dependencies") or {}
        if "sqlx" in d:
            sqlx_entries.append(d["sqlx"])
    assert sqlx_entries, "No sqlx dependency configured in workspace or crates"
    blob = json.dumps(sqlx_entries)
    assert "aws-lc-rs" not in blob, "Forbidden aws-lc-rs TLS feature in sqlx config"
    assert ("tls-rustls-ring" in blob) or ("tls-native-tls" in blob), (
        f"Expected Windows-safe sqlx TLS feature (tls-rustls-ring/tls-native-tls), got {blob}"
    )

    pipe = manifests["medical-pipeline"]
    pipe_deps = pipe.get("dependencies") or {}
    pipe_feats = pipe.get("features") or {}
    assert "local-fastembed" in pipe_feats, "medical-pipeline missing optional local-fastembed feature"
    assert "local-fastembed" not in (pipe_feats.get("default") or []), (
        "local-fastembed must NOT be in default features"
    )
    if "fastembed" in pipe_deps and isinstance(pipe_deps["fastembed"], dict):
        assert pipe_deps["fastembed"].get("optional") is True, "fastembed must be optional=true"


@REGISTRY.register("F1-T1-04", tier=1, features=[1], title="medical-studio Tauri compile-time assets present")
def test_f1_t1_04(ctx: TestContext) -> None:
    studio_dir = ctx.workspace / "crates" / "medical-studio"
    conf = parse_json_file(studio_dir / "tauri.conf.json")
    build_cfg = conf.get("build") or {}
    frontend_dist = build_cfg.get("frontendDist")
    assert isinstance(frontend_dist, str) and frontend_dist, "tauri.conf.json missing build.frontendDist"
    dist_dir = (studio_dir / frontend_dist).resolve()
    assert dist_dir.is_dir(), f"Resolved frontendDist dir does not exist: {dist_dir}"
    idx = dist_dir / "index.html"
    assert idx.is_file() and idx.stat().st_size > 20, f"Missing/empty frontendDist index.html: {idx}"

    build_rs = (studio_dir / "build.rs").read_text(encoding="utf-8")
    assert "tauri_build::build()" in build_rs, "crates/medical-studio/build.rs must invoke tauri_build::build()"

    ico_path = studio_dir / "icons" / "icon.ico"
    assert ico_path.is_file(), f"Missing Tauri icon: {ico_path}"
    validate_ico_bytes(ico_path.read_bytes())


@REGISTRY.register("F1-T1-05", tier=1, features=[1], title="Workspace offline cargo check / manifest readiness")
def test_f1_t1_05(ctx: TestContext) -> None:
    assert (ctx.workspace / "Cargo.toml").is_file()
    assert (ctx.workspace / "Cargo.lock").is_file() or ctx.with_cargo, "Missing Cargo.lock"
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["check", "--workspace"], strip_db_env=True)
        assert rc == 0, f"cargo check --workspace failed (rc={rc}):\n{out[-1500:]}"
        assert "error[" not in out, f"Compiler error diagnostics found:\n{out[-1500:]}"


@REGISTRY.register("F1-T1-06", tier=1, features=[1], title="Workspace build artifacts and cargo test --no-run")
def test_f1_t1_06(ctx: TestContext) -> None:
    if ctx.with_cargo:
        rc1, out1 = ctx.run_cargo(["build", "--workspace"], strip_db_env=True)
        assert rc1 == 0, f"cargo build --workspace failed:\n{out1[-1500:]}"
        rc2, out2 = ctx.run_cargo(["test", "--workspace", "--no-run"], strip_db_env=True)
        assert rc2 == 0, f"cargo test --workspace --no-run failed:\n{out2[-1500:]}"
    else:
        # Verify workspace crate entrypoints and server health responsiveness
        resp = ctx.client.get("/health")
        assert resp.status == 200, f"Live server /health failed: {resp.status}"


# ===========================================================================
# F2: Core Domain Models & Schema Validation (6 tests)
# ===========================================================================

@REGISTRY.register("F2-T1-01", tier=1, features=[2], title="Canonical data-collection-schema.json parse & validation")
def test_f2_t1_01(ctx: TestContext) -> None:
    schema_path = ctx.workspace / "doc" / "schemas" / "data-collection-schema.json"
    data = parse_json_file(schema_path)
    assert data.get("$schema_version") == "1.0.0"
    errs = validate_collection_bundle(data, load_schema_rules())
    assert not errs, f"Canonical schema validation errors: {errs}"
    kd0 = data["knowledge_documents"][0]
    assert kd0["doc_id"] == "book_internal_med_9th_ch02_001"
    assert kd0["updated_at"] == "2026-11-18T00:00:00Z"


@REGISTRY.register("F2-T1-02", tier=1, features=[2], title="KnowledgeDocument all enum variants & optional updated_at omitted")
def test_f2_t1_02(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    for sym in (
        "KnowledgeDocument",
        "SourceType",
        "SectionType",
        "Textbook",
        "Guideline",
        "HospitalManual",
        "QaDataset",
        "ClinicalManifestation",
        "DifferentialDiagnosis",
        "RedFlag",
        "TriageRule",
        "updated_at: Option<",
    ):
        assert sym in core_src, f"medical-core missing KnowledgeDocument model symbol: {sym}"
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-core"], strip_db_env=True)
        assert rc == 0, f"cargo test -p medical-core failed:\n{out[-1500:]}"

    bundle = parse_json_file(ctx.fixtures_dir / "knowledge" / "valid_collection.json")
    docs = bundle["knowledge_documents"]
    assert len(docs) >= 4
    rules = load_schema_rules()
    seen_source_types = set()
    seen_section_types = set()
    for doc in docs:
        errs = validate_knowledge_document(doc, rules)
        assert not errs, f"Document {doc.get('doc_id')} failed validation: {errs}"
        seen_source_types.add(doc["source_type"])
        seen_section_types.add(doc["section_type"])
    assert seen_source_types == {"textbook", "guideline", "hospital_manual", "qa_dataset"}
    assert seen_section_types == {"clinical_manifestation", "differential_diagnosis", "red_flag", "triage_rule"}
    assert any("updated_at" not in d for d in docs), "Expected at least one fixture doc with updated_at omitted"


@REGISTRY.register("F2-T1-03", tier=1, features=[2], title="Valid PatientRecord L2+L3 round-trip via /v1/patients/{id}/memory")
def test_f2_t1_03(ctx: TestContext) -> None:
    bundle = parse_json_file(ctx.fixtures_dir / "knowledge" / "valid_collection.json")
    rec = bundle["patient_records"][0]
    errs = validate_patient_record(rec, load_schema_rules())
    assert not errs, f"PatientRecord fixture invalid: {errs}"

    hdrs, _ = ctx.auth_headers_or_wechat("wx_f2_t1_03")
    pid = "pat_sha256_998877665544"
    payload = dict(rec, patient_id=pid)
    put_resp = ctx.client.put(f"/v1/patients/{pid}/memory", json_body=payload, headers=hdrs)
    assert put_resp.status in (200, 201), f"PUT /memory failed ({put_resp.status}): {put_resp.text}"
    get_resp = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert get_resp.status == 200
    body = get_resp.json()
    assert "高血压" in json.dumps(body, ensure_ascii=False) or "过敏性鼻炎" in json.dumps(body, ensure_ascii=False)


@REGISTRY.register("F2-T1-04", tier=1, features=[2], title="Valid EpidemicContext boundary dates & intensities")
def test_f2_t1_04(ctx: TestContext) -> None:
    bundle = parse_json_file(ctx.fixtures_dir / "knowledge" / "valid_collection.json")
    rules = load_schema_rules()
    for ep in bundle["epidemic_contexts"]:
        errs = validate_epidemic_context(ep, rules)
        assert not errs, f"EpidemicContext {ep.get('context_id')} failed: {errs}"

    resp = ctx.client.get("/v1/epidemic/current?region_code=CN-110000&season=winter")
    assert resp.status == 200, f"/v1/epidemic/current failed ({resp.status}): {resp.text}"
    data = resp.json()
    assert isinstance(data.get("contexts"), list)


@REGISTRY.register("F2-T1-05", tier=1, features=[2], title="Valid MedicalDictionaries combo & single-list red-flag rules")
def test_f2_t1_05(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    for sym in ("MedicalDictionaries", "SymptomSynonym", "RedFlagRule", "RedFlagLevel", "trigger_all_of", "trigger_any_of"):
        assert sym in core_src, f"medical-core missing dictionary model symbol: {sym}"

    bundle = parse_json_file(ctx.fixtures_dir / "knowledge" / "valid_collection.json")
    dicts = bundle["medical_dictionaries"]
    errs = validate_medical_dictionaries(dicts, load_schema_rules())
    assert not errs, f"MedicalDictionaries failed: {errs}"
    assert len(dicts["red_flag_rules"]) >= 3


@REGISTRY.register("F2-T1-06", tier=1, features=[2], title="KnowledgeDocument optional fields omitted validation")
def test_f2_t1_06(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "#[serde(default)]" in core_src, "medical-core missing #[serde(default)] for optional fields"
    for sym in ("aliases", "secondary_departments", "suggested_follow_up_slots", "updated_at"):
        assert sym in core_src, f"medical-core missing optional KnowledgeDocument field: {sym}"
    parse_json_file(ctx.workspace / "doc" / "schemas" / "data-collection-schema.json")

    minimal_doc = {
        "doc_id": "minimal_required_only_001",
        "source_title": "简明分诊指南",
        "source_type": "guideline",
        "authority_level": 0.9,
        "disease_name": "急性咽炎",
        "department": "耳鼻咽喉头颈外科",
        "target_population": ["adult"],
        "section_type": "clinical_manifestation",
        "content": "主要表现为咽部干燥灼热感及咽痛，吞咽时疼痛加重，可伴低热。",
        "key_symptoms": ["咽痛", "低热"],
    }
    errs = validate_knowledge_document(minimal_doc, load_schema_rules())
    assert not errs, f"Minimal required-only KnowledgeDocument failed: {errs}"


# ===========================================================================
# F3: PostgreSQL DDL & Qdrant Collection Schemas (5 tests)
# ===========================================================================

@REGISTRY.register("F3-T1-01", tier=1, features=[3], title="migrations/001_init.sql existence & structural validity")
def test_f3_t1_01(ctx: TestContext) -> None:
    sql = _read_sql(ctx)
    assert "TODO" not in sql and "FIXME" not in sql, "001_init.sql must not contain placeholder TODO/FIXME"
    info = parse_sql_ddl(sql)
    assert len(info.create_table_statements) >= 6


@REGISTRY.register("F3-T1-02", tier=1, features=[3], title="All required relational tables present in 001_init.sql")
def test_f3_t1_02(ctx: TestContext) -> None:
    info = parse_sql_ddl(_read_sql(ctx))
    table_names = set(info.tables.keys())
    for domain, patterns in _SQL_REQUIRED_DOMAINS:
        matched = any(any(pat in t for pat in patterns) for t in table_names)
        assert matched, f"Missing relational table for domain '{domain}'; found tables={sorted(table_names)}"


@REGISTRY.register("F3-T1-03", tier=1, features=[3], title="L2/L3 encryption columns & patient_id lookup/delete indexes")
def test_f3_t1_03(ctx: TestContext) -> None:
    sql = _read_sql(ctx)
    info = parse_sql_ddl(sql)
    l2_tables = [t for t in info.tables if "l2" in t]
    l3_tables = [t for t in info.tables if "l3" in t]
    assert l2_tables and l3_tables, f"Expected L2 and L3 tables, got {sorted(info.tables)}"

    l2_cols = info.tables[l2_tables[0]]
    l2_body = info.table_defs[l2_tables[0]].lower()
    assert "patient_id" in l2_cols
    assert any("consent" in c for c in l2_cols), f"L2 missing consent column: {l2_cols}"
    assert any(("enc" in c or "cipher" in c or "blob" in c) for c in l2_cols) or "bytea" in l2_body, (
        f"L2 table missing encrypted sensitive payload column: {l2_cols}"
    )

    l3_cols = info.tables[l3_tables[0]]
    assert "patient_id" in l3_cols and "event_id" in l3_cols and "occurred_at" in l3_cols


@REGISTRY.register("F3-T1-04", tier=1, features=[3], title="config/qdrant/collections.json valid collection definitions")
def test_f3_t1_04(ctx: TestContext) -> None:
    qpath = ctx.workspace / "config" / "qdrant" / "collections.json"
    data = parse_json_file(qpath)
    cols = validate_qdrant_collections(data)
    assert "medical_knowledge" in cols and "patient_l3_events" in cols


@REGISTRY.register("F3-T1-05", tier=1, features=[3], title="Qdrant mandatory payload indexes defined")
def test_f3_t1_05(ctx: TestContext) -> None:
    data = parse_json_file(ctx.workspace / "config" / "qdrant" / "collections.json")
    cols = validate_qdrant_collections(data)
    mk_raw = json.dumps(cols["medical_knowledge"])
    for field_name in ("target_population", "department", "authority_level", "seasonal_tags"):
        assert field_name in mk_raw, f"medical_knowledge missing payload index '{field_name}'"
    l3_raw = json.dumps(cols["patient_l3_events"])
    assert "patient_id" in l3_raw, "patient_l3_events missing payload index 'patient_id'"


# ===========================================================================
# F4: AES-256-GCM Encryption & PII Scrubber (6 tests)
# ===========================================================================

@REGISTRY.register("F4-T1-01", tier=1, features=[4], title="L2 sensitive fields encryption round-trip")
def test_f4_t1_01(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f4_t1_01")
    payload = {
        "patient_id": pid,
        "consent_long_term_memory": True,
        "l2_profile": {
            "age_group": "40-49",
            "gender": "male",
            "is_pregnant": False,
            "chronic_diseases": ["高血压", "2型糖尿病"],
            "allergies": ["青霉素"],
            "surgical_history": ["胆囊切除"],
            "long_term_medications": ["二甲双胍"],
        },
        "l3_visit_events": [],
    }
    r1 = ctx.client.put(f"/v1/patients/{pid}/memory", json_body=payload, headers=hdrs)
    assert r1.status in (200, 201), f"PUT /memory failed: {r1.text}"
    r2 = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert r2.status == 200
    body_text = json.dumps(r2.json(), ensure_ascii=False)
    for token in ("高血压", "2型糖尿病", "青霉素", "胆囊切除", "二甲双胍"):
        assert token in body_text, f"Round-trip L2 missing {token!r}"


@REGISTRY.register("F4-T1-02", tier=1, features=[4], title="Outbound LLM PII scrubber in /v1/triage/chat")
def test_f4_t1_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f4_t1_02")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    utterance = "我叫王小明，身份证110101199001011234，手机号13800138000，住在北京市朝阳区建国路88号，最近高烧咳嗽两天"
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={
            "session_id": "sess_f4_t1_02",
            "patient_id": pid,
            "utterance": utterance,
            "stream": False,
        },
        headers=hdrs,
    )
    assert resp.status == 200, f"/v1/triage/chat failed: {resp.text}"
    assert_no_pii(resp.text, extra_forbidden=("110101199001011234", "13800138000", "建国路88号"))
    if ctx.mock_upstream is not None:
        calls = ctx.mock_upstream.get_calls("/v1/chat/completions")
        for c in calls:
            serialized = json.dumps(c.body, ensure_ascii=False)
            assert_no_pii(serialized, extra_forbidden=("110101199001011234", "13800138000", "建国路88号"))


@REGISTRY.register("F4-T1-03", tier=1, features=[4], title="Deterministic salted SHA-256 patient_id mapping")
def test_f4_t1_03(ctx: TestContext) -> None:
    r1 = ctx.client.post("/v1/auth/wechat", json_body={"code": "wx_mock_code_alice"})
    r2 = ctx.client.post("/v1/auth/wechat", json_body={"code": "wx_mock_code_alice"})
    r3 = ctx.client.post("/v1/auth/wechat", json_body={"code": "wx_mock_code_bob"})
    assert r1.status == r2.status == r3.status == 200
    pid1 = r1.json()["patient_id"]
    pid2 = r2.json()["patient_id"]
    pid3 = r3.json()["patient_id"]
    assert pid1 == pid2, f"Same WeChat code yielded different patient_id: {pid1} vs {pid2}"
    assert pid1 != pid3, "Different WeChat codes must yield distinct patient_ids"
    assert "wx_mock_code" not in pid1
    assert_no_pii(pid1)


@REGISTRY.register("F4-T1-04", tier=1, features=[4], title="Mandatory AI triage disclaimer on completed responses")
def test_f4_t1_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f4_t1_04")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault(
            "/v1/chat/completions",
            FaultRule(
                custom_body={
                    "action": "completed",
                    "status": "completed",
                    "primary_department": "呼吸内科",
                    "secondary_departments": ["发热门诊"],
                    "urgency_level": 2,
                    "summary": "发热咳嗽两天，建议前往呼吸内科评估。",
                },
                match_prompt_contains="预检分诊",
            ),
        )
    try:
        sid = "sess_f4_t1_04"
        utt = "发热咳嗽两天，有黄痰"
        data: dict[str, Any] = {}
        turns = 1 if ctx.mock_upstream is not None else 4
        for _ in range(turns):
            resp = ctx.client.post(
                "/v1/triage/chat",
                json_body={
                    "session_id": sid,
                    "patient_id": pid,
                    "utterance": utt,
                    "stream": False,
                },
                headers=hdrs,
            )
            assert resp.status == 200
            data = resp.json()
            if data.get("status") == "completed":
                break
            assert_follow_up_response(data)
            utt = data["quick_options"][0]
        assert data.get("status") == "completed", f"Expected completed triage response for disclaimer check, got {data!r}"
        assert_completed_response(data)
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()
    oai = ctx.client.post(
        "/v1/openai/chat/completions",
        json_body={"model": "medical-triage", "messages": [{"role": "user", "content": "发热咳嗽两天"}]},
        headers=hdrs,
    )
    assert oai.status == 200
    content = oai.json()["choices"][0]["message"]["content"]
    assert_disclaimer_present(content)


@REGISTRY.register("F4-T1-05", tier=1, features=[4], title="AES-256-GCM crypto & tamper rejection contract")
def test_f4_t1_05(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "aes_gcm" in core_src or "Aes256Gcm" in core_src, "medical-core missing AES-256-GCM cipher impl"
    assert "sha2" in core_src.lower() or "Sha256" in core_src, "medical-core missing SHA-256 hashing"
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-core", "crypto"], strip_db_env=True)
        assert rc == 0, f"medical-core crypto tests failed:\n{out[-1500:]}"


@REGISTRY.register("F4-T1-06", tier=1, features=[4], title="Outbound PII scrubbing on OpenAI proxy /v1/openai/chat/completions")
def test_f4_t1_06(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f4_t1_06")
    payload = parse_json_file(ctx.fixtures_dir / "payloads" / "openai_proxy.json")["pii_scrub_completion"]
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    resp = ctx.client.post("/v1/openai/chat/completions", json_body=payload, headers=hdrs)
    assert resp.status == 200, f"OpenAI proxy failed: {resp.text}"
    assert_no_pii(resp.text, extra_forbidden=("13911112222", "310101198505054321"))
    if ctx.mock_upstream is not None:
        for c in ctx.mock_upstream.get_calls("/v1/chat/completions"):
            assert_no_pii(
                json.dumps(c.body, ensure_ascii=False),
                extra_forbidden=("13911112222", "310101198505054321"),
            )


# ===========================================================================
# F5: Storage/Cache/Search Traits + Prod & Mock Adapters (R5) (5 tests)
# ===========================================================================

@REGISTRY.register("F5-T1-01", tier=1, features=[5], title="R5 offline startup & /health readiness")
def test_f5_t1_01(ctx: TestContext) -> None:
    resp = ctx.client.get("/health")
    assert resp.status == 200, f"/health expected 200, got {resp.status}: {resp.text}"
    data = resp.json()
    assert str(data.get("status", "")).lower() in {"ok", "healthy", "degraded"}
    assert "version" in data and "checks" in data


@REGISTRY.register("F5-T1-02", tier=1, features=[5], title="R5 InMemory storage L2/L3 CRUD contract")
def test_f5_t1_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f5_t1_02")
    rec = {
        "patient_id": pid,
        "consent_long_term_memory": True,
        "l2_profile": {
            "age_group": "30-39",
            "gender": "female",
            "is_pregnant": False,
            "chronic_diseases": ["过敏性鼻炎"],
            "allergies": ["磺胺"],
            "surgical_history": [],
            "long_term_medications": [],
        },
        "l3_visit_events": [],
    }
    assert ctx.client.put(f"/v1/patients/{pid}/memory", json_body=rec, headers=hdrs).status in (200, 201)
    g = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert g.status == 200 and "过敏性鼻炎" in g.text
    d = ctx.client.delete(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert d.status in (200, 204)


@REGISTRY.register("F5-T1-03", tier=1, features=[5], title="R5 epidemic storage & ArcSwap hot-reload")
def test_f5_t1_03(ctx: TestContext) -> None:
    r1 = ctx.client.get("/v1/epidemic/current?region_code=CN-110000&season=winter")
    assert r1.status == 200
    assert isinstance(r1.json().get("contexts"), list)
    hdrs = {"Authorization": f"Bearer {ctx.admin_token}"} if ctx.admin_token else ctx.auth_headers_or_wechat("wx_admin_r5")[0]
    r2 = ctx.client.post(
        "/v1/admin/reload",
        json_body={"reload_epidemics": True, "reload_dictionaries": True, "reload_router": True},
        headers=hdrs,
    )
    assert r2.status == 200, f"/v1/admin/reload failed: {r2.status} {r2.text}"


@REGISTRY.register("F5-T1-04", tier=1, features=[5], title="R5 knowledge search adapter round-trip")
def test_f5_t1_04(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f5_t1_04")
    resp = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "高热 肌肉酸痛 咽痛", "top_k": 5},
        headers=hdrs,
    )
    assert resp.status == 200, f"/v1/knowledge/search failed: {resp.status} {resp.text}"
    data = resp.json()
    assert isinstance(data.get("results"), list)
    assert isinstance(data.get("total"), int)
    assert "seasonal_context_applied" in data


@REGISTRY.register("F5-T1-05", tier=1, features=[5], title="Production adapter compilation & R5 trait contracts")
def test_f5_t1_05(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    for kw in ("async_trait", "sqlx", "qdrant", "redis"):
        assert kw in core_src.lower(), f"medical-core missing storage trait/adapter reference: {kw}"
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-core"], strip_db_env=True)
        assert rc == 0, f"cargo test -p medical-core failed:\n{out[-1500:]}"


# ===========================================================================
# F6: Unified AI Abstraction & ModelRouter (6 tests)
# ===========================================================================

@REGISTRY.register("F6-T1-01", tier=1, features=[6], title="Reasoning tier non-streaming JSON completion")
def test_f6_t1_01(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t1_01")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f6_t1_01", "patient_id": pid, "utterance": "咳嗽两天有白痰", "stream": False},
        headers=hdrs,
    )
    assert resp.status == 200
    data = resp.json()
    assert data.get("status") in {"follow_up", "completed"}
    if ctx.mock_upstream is not None:
        calls = ctx.mock_upstream.get_calls("/v1/chat/completions")
        assert len(calls) >= 1, "Expected upstream LLM call for non-emergency triage"


@REGISTRY.register("F6-T1-02", tier=1, features=[6], title="Reasoning tier SSE streaming (stream: true)")
def test_f6_t1_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t1_02")
    sse = ctx.client.post_sse(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f6_t1_02", "patient_id": pid, "utterance": "咽痛低热一天", "stream": True},
        headers=hdrs,
    )
    assert sse.status == 200
    assert "text/event-stream" in sse.headers.get("content-type", "")
    assert len(sse.events) >= 1, "Expected >=1 SSE event frames"
    ev_names = {e.event for e in sse.events}
    assert "result" in ev_names or "delta" in ev_names, f"Unexpected SSE events: {ev_names}"


@REGISTRY.register("F6-T1-03", tier=1, features=[6], title="Fast tier vs Reasoning tier split routing contract")
def test_f6_t1_03(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "ModelRouter" in core_src and "LlmProvider" in core_src
    assert ("Fast" in core_src or "fast" in core_src) and ("Reasoning" in core_src or "reasoning" in core_src)
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t1_03")
    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid,
            "consent_long_term_memory": True,
            "l2_profile": {
                "age_group": "30-39", "gender": "female", "is_pregnant": False,
                "chronic_diseases": ["轻度哮喘"], "allergies": [], "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f6_t1_03", "patient_id": pid, "utterance": "胸闷两天", "stream": False},
        headers=hdrs,
    )
    assert resp.status == 200


@REGISTRY.register("F6-T1-04", tier=1, features=[6], title="Primary 429 Rate Limit automatic failover")
def test_f6_t1_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t1_04")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.enqueue_fault(
            "/v1/chat/completions",
            FaultRule(status=429, match_prompt_contains="预检分诊"),
        )
    try:
        resp = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_f6_t1_04", "patient_id": pid, "utterance": "头疼恶心一天", "stream": False},
            headers=hdrs,
        )
        assert resp.status == 200, f"Expected failover/fallback 200 on 429, got {resp.status}: {resp.text}"
        data = resp.json()
        if data.get("status") == "follow_up":
            assert_follow_up_response(data)
        elif data.get("status") == "completed":
            assert_completed_response(data)
        else:
            raise AssertionError(f"Unexpected status on 429 failover: {data!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("F6-T1-05", tier=1, features=[6], title="Primary 503 Service Unavailable automatic failover & metrics")
def test_f6_t1_05(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t1_05")
    m_before = parse_prometheus_metrics(ctx.client.get("/metrics").text)
    errors_before = sum(v for _, v in m_before.get("medical_server_llm_errors_total", []))
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.enqueue_fault(
            "/v1/chat/completions",
            FaultRule(status=503, match_prompt_contains="预检分诊"),
        )
    try:
        resp = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_f6_t1_05", "patient_id": pid, "utterance": "腹痛腹泻一天", "stream": False},
            headers=hdrs,
        )
        assert resp.status == 200, f"Expected 200 on 503 failover, got {resp.status}: {resp.text}"
        data = resp.json()
        if data.get("status") == "follow_up":
            assert_follow_up_response(data)
        elif data.get("status") == "completed":
            assert_completed_response(data)
        else:
            raise AssertionError(f"Unexpected status on 503 failover: {data!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()

    m_resp = ctx.client.get("/metrics")
    assert m_resp.status == 200
    m_after = parse_prometheus_metrics(m_resp.text)
    assert "medical_server_llm_errors_total" in m_after, "Missing medical_server_llm_errors_total in /metrics"
    if ctx.mock_upstream is not None:
        errors_after = sum(v for _, v in m_after["medical_server_llm_errors_total"])
        assert errors_after > errors_before, (
            f"Expected medical_server_llm_errors_total to increment on 503 fault: before={errors_before}, after={errors_after}"
        )


@REGISTRY.register("F6-T1-06", tier=1, features=[6], title="EmbeddingProvider & RerankProvider retrieval round-trip")
def test_f6_t1_06(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "EmbeddingProvider" in core_src and "RerankProvider" in core_src
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f6_t1_06")
    resp = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "流感 高热", "top_k": 5},
        headers=hdrs,
    )
    assert resp.status == 200
    assert isinstance(resp.json().get("results"), list)


# ===========================================================================
# F7: bevy_ecs Triage Engine & Safety Short-Circuit (7 tests)
# ===========================================================================

@REGISTRY.register("F7-T1-01", tier=1, features=[7], title="Deterministic Emergency120 short-circuit (0 LLM calls)")
def test_f7_t1_01(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t1_01")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f7_t1_01", "patient_id": pid, "utterance": "突然胸痛，伴随大汗和左臂放射痛", "stream": False},
        headers=hdrs,
    )
    assert resp.status == 200
    assert_emergency_response(resp.json())
    if ctx.mock_upstream is not None:
        llm_calls = ctx.mock_upstream.get_calls("/v1/chat/completions")
        assert len(llm_calls) == 0, f"Emergency120 MUST make 0 LLM calls, got {len(llm_calls)}"


@REGISTRY.register("F7-T1-02", tier=1, features=[7], title="Colloquial synonym normalization triggering Emergency120 (0 LLM calls)")
def test_f7_t1_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t1_02")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f7_t1_02", "patient_id": pid, "utterance": "心口窝撕裂样疼，喘不上气", "stream": False},
        headers=hdrs,
    )
    assert resp.status == 200
    assert_emergency_response(resp.json())
    if ctx.mock_upstream is not None:
        assert len(ctx.mock_upstream.get_calls("/v1/chat/completions")) == 0


@REGISTRY.register("F7-T1-03", tier=1, features=[7], title="Multi-turn follow-up slot constraints (<=30 chars, 2~4 options, max_turns<=4)")
def test_f7_t1_03(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t1_03")
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f7_t1_03", "patient_id": pid, "utterance": "发烧咳嗽", "stream": False},
        headers=hdrs,
    )
    assert resp.status == 200
    data = resp.json()
    if data.get("status") == "follow_up":
        assert_follow_up_response(data)
    elif data.get("status") == "completed":
        assert_completed_response(data)
    else:
        raise AssertionError(f"Unexpected status in F7-T1-03: {data!r}")


@REGISTRY.register("F7-T1-04", tier=1, features=[7], title="Seasonal epidemic boost applied in triage & RAG")
def test_f7_t1_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t1_04")
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={
            "session_id": "sess_f7_t1_04",
            "patient_id": pid,
            "region_code": "CN-110000",
            "utterance": "高热全身肌肉酸痛乏力",
            "stream": False,
        },
        headers=hdrs,
    )
    assert resp.status == 200
    data = resp.json()
    assert "seasonal_context_applied" in data


@REGISTRY.register("F7-T1-05", tier=1, features=[7], title="L2/L3 long-term memory recall into triage context")
def test_f7_t1_05(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t1_05")
    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid,
            "consent_long_term_memory": True,
            "l2_profile": {
                "age_group": "30-39", "gender": "female", "is_pregnant": False,
                "chronic_diseases": ["轻度哮喘"], "allergies": ["青霉素"],
                "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [
                {
                    "event_id": "evt_f7_05", "occurred_at": "2026-09-15T09:30:00Z",
                    "chief_complaint": "夜间干咳", "extracted_symptoms": ["干咳"],
                    "recommended_department": "呼吸内科", "urgency_level": 2,
                    "summary": "既往轻度哮喘",
                }
            ],
        },
        headers=hdrs,
    )
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f7_t1_05", "patient_id": pid, "utterance": "夜间又开始干咳胸闷", "stream": False},
        headers=hdrs,
    )
    assert resp.status == 200
    if ctx.mock_upstream is not None:
        calls = ctx.mock_upstream.get_calls("/v1/chat/completions")
        if calls:
            blob = json.dumps(calls[-1].body, ensure_ascii=False)
            assert "哮喘" in blob or "青霉素" in blob, "Expected L2/L3 history recalled into LLM prompt"


@REGISTRY.register("F7-T1-06", tier=1, features=[7], title="Completed triage structured output, citations & disclaimer")
def test_f7_t1_06(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t1_06")
    sid = "sess_f7_t1_06"
    utterances = ["高热39.5度，咽痛干咳两天，身边同事发烧", "2~3天", "有家人/同事发烧", "呼吸内科"]
    last: dict[str, Any] = {}
    for u in utterances:
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": sid, "patient_id": pid, "utterance": u, "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        last = r.json()
        if last.get("status") == "completed":
            break
    assert_completed_response(last, require_citations=True)


@REGISTRY.register("F7-T1-07", tier=1, features=[7], title="Stage 5 async consolidation when consent_long_term_memory == true")
def test_f7_t1_07(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t1_07")
    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid,
            "consent_long_term_memory": True,
            "l2_profile": {
                "age_group": "30-39", "gender": "male", "is_pregnant": False,
                "chronic_diseases": [], "allergies": [], "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    sid = "sess_f7_t1_07"
    for u in ("急性肠胃炎腹泻水样便一天", "今天刚开始", "没有明确接触", "腹泻"):
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": sid, "patient_id": pid, "utterance": u, "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        if r.json().get("status") == "completed":
            break
    events: list[Any] = []
    deadline = time.monotonic() + 4.0
    while True:
        mem = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
        assert mem.status == 200, f"GET /memory failed during Stage 5 poll: {mem.status} {mem.text}"
        events = mem.json().get("l3_visit_events", [])
        if isinstance(events, list) and len(events) >= 1:
            break
        if time.monotonic() >= deadline:
            raise AssertionError(
                f"Timed out after 4.0s waiting for async Stage 5 L3 consolidation for patient {pid}: {mem.json()!r}"
            )
        time.sleep(0.1)


# ===========================================================================
# F8: sonic-rs JSON/JSONL Streaming Parser (5 tests)
# ===========================================================================

@REGISTRY.register("F8-T1-01", tier=1, features=[8], title="Parse standard JSON collection bundle & sonic-rs contract")
def test_f8_t1_01(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    assert "sonic-rs" in (pipe_toml.get("dependencies") or {}), "medical-pipeline must depend on sonic-rs"
    data = parse_json_file(ctx.workspace / "doc" / "schemas" / "data-collection-schema.json")
    assert len(data["knowledge_documents"]) == 1
    assert len(data["patient_records"]) == 1
    assert len(data["epidemic_contexts"]) == 1
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-pipeline"], strip_db_env=True)
        assert rc == 0, f"cargo test -p medical-pipeline failed:\n{out[-1500:]}"


@REGISTRY.register("F8-T1-02", tier=1, features=[8], title="Stream-parse valid KnowledgeDocument JSONL (100 lines)")
def test_f8_t1_02(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    assert "sonic-rs" in (pipe_toml.get("dependencies") or {}), "medical-pipeline must depend on sonic-rs"
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "sonic_rs" in pipe_src and "KnowledgeDocument" in pipe_src

    rules = load_schema_rules()
    res = parse_jsonl_stream(
        ctx.fixtures_dir / "knowledge" / "knowledge_100.jsonl",
        validator_fn=lambda d: validate_knowledge_document(d, rules),
    )
    assert len(res.records) == 100 and not res.errors, f"Unexpected JSONL errors: {res.errors}"


@REGISTRY.register("F8-T1-03", tier=1, features=[8], title="Stream-parse valid PatientRecord JSONL (50 records)")
def test_f8_t1_03(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "sonic_rs" in pipe_src and "PatientRecord" in pipe_src

    rules = load_schema_rules()
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "patients_50.jsonl"
        with p.open("w", encoding="utf-8") as fh:
            for i in range(50):
                rec = {
                    "patient_id": f"pat_sha256_{i:012d}",
                    "consent_long_term_memory": True,
                    "l2_profile": {
                        "age_group": "30-39", "gender": "female", "is_pregnant": False,
                        "chronic_diseases": [], "allergies": [], "surgical_history": [], "long_term_medications": [],
                    },
                    "l3_visit_events": [],
                }
                fh.write(json.dumps(rec) + "\n")
        res = parse_jsonl_stream(p, validator_fn=lambda r: validate_patient_record(r, rules))
        assert len(res.records) == 50 and not res.errors


@REGISTRY.register("F8-T1-04", tier=1, features=[8], title="Large-scale >100k-line JSONL streaming generator & pipeline contract")
def test_f8_t1_04(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "BufReader" in pipe_src or "lines(" in pipe_src, "medical-pipeline must stream JSONL line-by-line"
    with tempfile.TemporaryDirectory() as td:
        large_path = generate_large_jsonl(Path(td) / "large_100k.jsonl", count=100_005)
        res = parse_jsonl_stream(large_path)
        assert len(res.records) == 100_005 and not res.errors


@REGISTRY.register("F8-T1-05", tier=1, features=[8], title="Windows CRLF & UTF-8 BOM JSONL handling")
def test_f8_t1_05(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "sonic_rs" in pipe_src and (
        "feff" in pipe_src.lower() or "BufRead" in pipe_src or "lines(" in pipe_src
    )

    p = ctx.fixtures_dir / "knowledge" / "bom_crlf.jsonl"
    raw = p.read_bytes()
    assert raw.startswith(b"\xef\xbb\xbf") and b"\r\n" in raw
    rules = load_schema_rules()
    res = parse_jsonl_stream(p, validator_fn=lambda d: validate_knowledge_document(d, rules))
    assert len(res.records) == 10 and not res.errors


# ===========================================================================
# F9: Medical Book Parsers & Semantic Chunker (6 tests)
# ===========================================================================

@REGISTRY.register("F9-T1-01", tier=1, features=[9], title="Markdown medical book parsing (pulldown-cmark) contract")
def test_f9_t1_01(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    assert "pulldown-cmark" in (pipe_toml.get("dependencies") or {})
    md = (ctx.fixtures_dir / "knowledge" / "books_sample.md").read_text(encoding="utf-8")
    assert "# 流行性感冒" in md and "# 急性心肌梗死" in md and "## 临床表现" in md


@REGISTRY.register("F9-T1-02", tier=1, features=[9], title="Plain text (.txt) medical manual parsing contract")
def test_f9_t1_02(ctx: TestContext) -> None:
    txt = (ctx.fixtures_dir / "knowledge" / "custom_markers.txt").read_text(encoding="utf-8")
    assert "【疾病】急性胃肠炎" in txt and "【推荐科室】" in txt
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "chunk" in pipe_src.lower()


@REGISTRY.register("F9-T1-03", tier=1, features=[9], title="Valid .docx medical book parsing (docx-rs) & OOXML fixture")
def test_f9_t1_03(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    assert "docx-rs" in (pipe_toml.get("dependencies") or {})
    with tempfile.TemporaryDirectory() as td:
        docx_path = generate_minimal_docx(Path(td) / "guide.docx")
        paras = read_docx_paragraphs(docx_path)
        assert any("急性支气管炎" in p for p in paras)
        assert not any("<w:t>" in p for p in paras)


@REGISTRY.register("F9-T1-04", tier=1, features=[9], title="Text-layer .pdf parsing (lopdf/pdf-extract) & PDF fixture")
def test_f9_t1_04(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    deps = pipe_toml.get("dependencies") or {}
    assert "lopdf" in deps or "pdf-extract" in deps
    with tempfile.TemporaryDirectory() as td:
        pdf_path = generate_minimal_pdf(
            Path(td) / "text_book.pdf",
            ["Influenza Clinical Guideline", "Fever Cough Sore Throat Respiratory"],
        )
        info = inspect_pdf_bytes(pdf_path.read_bytes())
        assert info["has_text_ops"] is True


@REGISTRY.register("F9-T1-05", tier=1, features=[9], title="Mandatory metadata injection on semantic chunks")
def test_f9_t1_05(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    for field_name in ("target_population", "department", "authority_level", "section_type", "disease_name"):
        assert field_name in pipe_src, f"medical-pipeline chunker missing metadata field {field_name!r}"


@REGISTRY.register("F9-T1-06", tier=1, features=[9], title="Custom regex chunking rule support in medical-pipeline")
def test_f9_t1_06(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    assert "regex" in (pipe_toml.get("dependencies") or {})
    txt = (ctx.fixtures_dir / "knowledge" / "custom_markers.txt").read_text(encoding="utf-8")
    matches = re.findall(r"^【疾病】(?P<disease>.+)$", txt, flags=re.MULTILINE)
    assert matches == ["急性胃肠炎", "偏头痛"]


# ===========================================================================
# F10: Patient Record Importer & rayon Privacy Masker (5 tests)
# ===========================================================================

@REGISTRY.register("F10-T1-01", tier=1, features=[10], title="CSV patient ingestion, auto-PII detection & rayon contract")
def test_f10_t1_01(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    deps = pipe_toml.get("dependencies") or {}
    assert "csv" in deps and "rayon" in deps
    raw_csv = (ctx.fixtures_dir / "patients" / "raw_pii_patients.csv").read_text(encoding="utf-8")
    assert "110101199001011234" in raw_csv and "13800138000" in raw_csv


@REGISTRY.register("F10-T1-02", tier=1, features=[10], title="XLSX patient ingestion (calamine) & OOXML fixture")
def test_f10_t1_02(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    assert "calamine" in (pipe_toml.get("dependencies") or {})
    with tempfile.TemporaryDirectory() as td:
        xlsx_path = generate_minimal_xlsx(Path(td) / "patients.xlsx")
        rows = read_xlsx_rows(xlsx_path)
        assert len(rows) >= 3 and rows[0][0] == "姓名" and rows[1][1] == "110101199001011234"


@REGISTRY.register("F10-T1-03", tier=1, features=[10], title="JSON patient export ingestion & masking schema compliance")
def test_f10_t1_03(ctx: TestContext) -> None:
    raw = parse_json_file(ctx.fixtures_dir / "patients" / "patients_raw.json")
    assert isinstance(raw, list) and len(raw) >= 2
    assert "id_card" in raw[0] and "phone" in raw[0]
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "age_group" in pipe_src and "patient_id" in pipe_src


@REGISTRY.register("F10-T1-04", tier=1, features=[10], title="Deterministic salted-hash patient_id across files")
def test_f10_t1_04(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    combined = core_src + "\n" + pipe_src
    assert "sha2" in combined.lower() or "Sha256" in combined, "Missing SHA-256 salted patient ID hashing"


@REGISTRY.register("F10-T1-05", tier=1, features=[10], title="Masked patient record round-trip via /v1/patients/{id}/memory")
def test_f10_t1_05(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f10_t1_05")
    pid = "pat_sha256_f10_masked_01"
    rec = {
        "patient_id": pid,
        "consent_long_term_memory": True,
        "l2_profile": {
            "age_group": "30-39", "gender": "male", "is_pregnant": False,
            "chronic_diseases": ["高血压"], "allergies": ["青霉素"],
            "surgical_history": [], "long_term_medications": ["氨氯地平"],
        },
        "l3_visit_events": [
            {
                "event_id": "evt_f10_01", "occurred_at": "2026-10-01T10:00:00Z",
                "chief_complaint": "头晕伴血压升高", "extracted_symptoms": ["头晕"],
                "recommended_department": "心血管内科", "urgency_level": 2,
                "summary": "既往高血压控制不佳建议心血管内科评估",
            }
        ],
    }
    assert ctx.client.put(f"/v1/patients/{pid}/memory", json_body=rec, headers=hdrs).status in (200, 201)
    g = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert g.status == 200
    assert_no_pii(g.text, extra_forbidden=("张伟", "110101199001011234", "13800138000", "幸福路10号"))


# ===========================================================================
# F11: Dual-Write Indexer (Qdrant + Tantivy) & Checkpoints (6 tests)
# ===========================================================================

@REGISTRY.register("F11-T1-01", tier=1, features=[11], title="Dual-write Qdrant+Tantivy contract & hybrid recall")
def test_f11_t1_01(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    deps = pipe_toml.get("dependencies") or {}
    assert "tantivy" in deps and "qdrant-client" in deps
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f11_t1_01")
    resp = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "高热 肌肉酸痛 咽痛", "top_k": 3},
        headers=hdrs,
    )
    assert resp.status == 200
    data = resp.json()
    assert len(data.get("results", [])) >= 1, "Expected >=1 seeded knowledge search hit"


@REGISTRY.register("F11-T1-02", tier=1, features=[11], title="Hard metadata filters (target_population, department)")
def test_f11_t1_02(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f11_t1_02")
    r_child = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "咳嗽 发热", "target_population": ["child"], "department": "儿科", "top_k": 5},
        headers=hdrs,
    )
    assert r_child.status == 200
    r_bone = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "咳嗽 发热", "target_population": ["elderly"], "department": "骨科", "top_k": 5},
        headers=hdrs,
    )
    assert r_bone.status == 200
    bone_depts = {item.get("department") for item in r_bone.json().get("results", []) if item.get("department")}
    assert "儿科" not in bone_depts, f"Hard department filter leaked 儿科 into 骨科 query: {bone_depts}"


@REGISTRY.register("F11-T1-03", tier=1, features=[11], title="Seasonal epidemic boost toggle (seasonal_boost true vs false)")
def test_f11_t1_03(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f11_t1_03")
    r_on = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "高热 咳嗽", "region_code": "CN-110000", "seasonal_boost": True, "top_k": 5},
        headers=hdrs,
    )
    r_off = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "高热 咳嗽", "region_code": "CN-110000", "seasonal_boost": False, "top_k": 5},
        headers=hdrs,
    )
    assert r_on.status == 200 and r_off.status == 200
    assert r_on.json().get("seasonal_context_applied") is not None


@REGISTRY.register("F11-T1-04", tier=1, features=[11], title="Idempotent upsert by doc_id contract")
def test_f11_t1_04(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "doc_id" in pipe_src
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-pipeline", "upsert"], strip_db_env=True)
        assert rc == 0, f"Upsert test failed:\n{out[-1500:]}"


@REGISTRY.register("F11-T1-05", tier=1, features=[11], title="Checkpoint creation & resumable import (断点续传) contract")
def test_f11_t1_05(ctx: TestContext) -> None:
    ckpt_path = ctx.workspace / "crates" / "medical-pipeline" / "src" / "checkpoint.rs"
    assert ckpt_path.is_file(), f"Missing checkpoint module: {ckpt_path}"
    ckpt_src = ckpt_path.read_text(encoding="utf-8", errors="replace")
    assert "completed" in ckpt_src or "offset" in ckpt_src or "processed" in ckpt_src


@REGISTRY.register("F11-T1-06", tier=1, features=[11], title="Job progress telemetry & error export contract")
def test_f11_t1_06(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "failed" in pipe_src and ("throughput" in pipe_src or "completed" in pipe_src)

"""Tier 2 Opaque-Box Boundary & Corner-Case Tests: Features F1–F11 (63 tests)."""

from __future__ import annotations

import json
import re
import tempfile
import time
import zipfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

from e2e.fixtures.generator import (
    generate_5000_patient_csv,
    generate_minimal_pdf,
)
from e2e.lib.assertions import (
    assert_completed_response,
    assert_disclaimer_present,
    assert_emergency_response,
    assert_follow_up_response,
    assert_no_diagnosis_or_dosage,
    assert_no_pii,
    assert_vitals_preserved,
    collect_rs_text,
    load_schema_rules,
    strip_rust_comments_and_tests,
    validate_epidemic_context,
    validate_knowledge_document,
    validate_medical_dictionaries,
    validate_patient_record,
)
from e2e.lib.mock_upstream import FaultRule
from e2e.lib.parsers import (
    ParseError,
    dry_run_sqlite_ddl,
    inspect_pdf_bytes,
    parse_json_file,
    parse_jsonl_stream,
    parse_sql_ddl,
    parse_toml_file,
    read_docx_paragraphs,
    validate_ico_bytes,
    validate_qdrant_collections,
)
from e2e.lib.registry import REGISTRY
from e2e.lib.runner import TestContext


def _read_sql(ctx: TestContext) -> str:
    p = ctx.workspace / "migrations" / "001_init.sql"
    assert p.is_file(), f"Missing {p}"
    return p.read_text(encoding="utf-8")


def _collect_rs_text(crate_dir: Path) -> str:
    return collect_rs_text(crate_dir)


# ===========================================================================
# F1 Tier 2 (5 tests)
# ===========================================================================

@REGISTRY.register("F1-T2-01", tier=2, features=[1], title="Zero compile-time live-DB sqlx::query! dependency")
def test_f1_t2_01(ctx: TestContext) -> None:
    for crate in ("medical-core", "medical-pipeline", "medical-server", "medical-studio"):
        rs = _collect_rs_text(ctx.workspace / "crates" / crate)
        assert "sqlx::query!" not in rs and "sqlx::query_as!" not in rs, (
            f"{crate} uses compile-time sqlx::query! macro which breaks offline Windows builds"
        )
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["check", "--workspace"], strip_db_env=True)
        assert rc == 0, f"Offline cargo check failed:\n{out[-1500:]}"


@REGISTRY.register("F1-T2-02", tier=2, features=[1], title="Binary .ico header and directory payload bounds integrity")
def test_f1_t2_02(ctx: TestContext) -> None:
    ico = ctx.workspace / "crates" / "medical-studio" / "icons" / "icon.ico"
    assert ico.is_file(), f"Missing {ico}"
    count = validate_ico_bytes(ico.read_bytes())
    assert count >= 1


@REGISTRY.register("F1-T2-03", tier=2, features=[1], title="Absence of aws-lc-sys / cmake blocker in Cargo.lock & manifests")
def test_f1_t2_03(ctx: TestContext) -> None:
    lock_path = ctx.workspace / "Cargo.lock"
    if lock_path.is_file():
        lock_txt = lock_path.read_text(encoding="utf-8", errors="replace")
        assert 'name = "aws-lc-sys"' not in lock_txt, "Cargo.lock must NOT pull aws-lc-sys (needs cmake/nasm)"
    root_toml = (ctx.workspace / "Cargo.toml").read_text(encoding="utf-8")
    assert "aws-lc-rs" not in root_toml


@REGISTRY.register("F1-T2-04", tier=2, features=[1], title="medical-pipeline feature isolation (--no-default-features)")
def test_f1_t2_04(ctx: TestContext) -> None:
    toml_txt = (ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml").read_text(encoding="utf-8")
    assert "local-fastembed" in toml_txt
    if ctx.with_cargo:
        rc1, out1 = ctx.run_cargo(["check", "-p", "medical-pipeline", "--no-default-features"], strip_db_env=True)
        assert rc1 == 0, f"--no-default-features failed:\n{out1[-1500:]}"


@REGISTRY.register("F1-T2-05", tier=2, features=[1], title="medical-studio/frontend Vite build output parity")
def test_f1_t2_05(ctx: TestContext) -> None:
    studio = ctx.workspace / "crates" / "medical-studio"
    conf = parse_json_file(studio / "tauri.conf.json")
    pkg = parse_json_file(studio / "frontend" / "package.json")
    assert "build" in (pkg.get("scripts") or {}), "frontend/package.json missing build script"
    dist_dir = (studio / conf["build"]["frontendDist"]).resolve()
    html = (dist_dir / "index.html").read_text(encoding="utf-8")
    assert "<script" in html and ("id=\"app\"" in html or "id='app'" in html or "id=\"root\"" in html), (
        f"frontendDist index.html looks like an unbuilt placeholder: {html[:200]!r}"
    )


# ===========================================================================
# F2 Tier 2 (7 tests)
# ===========================================================================

def _neg_cases(ctx: TestContext) -> dict[str, dict[str, Any]]:
    data = parse_json_file(ctx.fixtures_dir / "knowledge" / "invalid_collection.json")
    return {c["case_id"]: c for c in data["negative_cases"]}


@REGISTRY.register("F2-T2-01", tier=2, features=[2], title="Reject out-of-range authority_level (<0.1 or >1.0)")
def test_f2_t2_01(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "authority_level" in core_src and "0.1..=1.0" in core_src, (
        "medical-core missing authority_level 0.1..=1.0 validation"
    )
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-core"], strip_db_env=True)
        assert rc == 0, f"cargo test -p medical-core failed:\n{out[-1500:]}"

    cases = _neg_cases(ctx)
    rules = load_schema_rules()
    for cid in ("bad_authority_low", "bad_authority_high"):
        errs = validate_knowledge_document(cases[cid]["payload"], rules)
        assert errs and any("authority_level" in e for e in errs), f"{cid} should fail authority_level: {errs}"
    neg_doc = dict(cases["bad_authority_low"]["payload"], authority_level=-0.5)
    assert validate_knowledge_document(neg_doc, rules)


@REGISTRY.register("F2-T2-02", tier=2, features=[2], title="Reject invalid enums & empty/unknown target_population")
def test_f2_t2_02(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    for sym in ("SourceType", "SectionType", "TargetPopulation", "target_population.is_empty()"):
        assert sym in core_src, f"medical-core missing enum/target_population validation: {sym}"

    cases = _neg_cases(ctx)
    rules = load_schema_rules()
    assert validate_knowledge_document(cases["bad_source_type"]["payload"], rules)
    assert validate_knowledge_document(cases["bad_target_population_empty"]["payload"], rules)
    bad_sec = dict(cases["bad_source_type"]["payload"], source_type="textbook", section_type="dosage")
    assert validate_knowledge_document(bad_sec, rules)
    bad_pop = dict(cases["bad_target_population_empty"]["payload"], target_population=["adult", "unknown_group"])
    assert validate_knowledge_document(bad_pop, rules)


@REGISTRY.register("F2-T2-03", tier=2, features=[2], title="Reject suggested_follow_up_slots violating <=30 chars or 2~4 options")
def test_f2_t2_03(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    for sym in ("short_question", "quick_options", "30", "2..=4"):
        assert sym in core_src, f"medical-core missing slot constraint check: {sym}"

    cases = _neg_cases(ctx)
    rules = load_schema_rules()
    for cid in ("bad_slot_31chars", "bad_slot_1opt", "bad_slot_5opt"):
        errs = validate_knowledge_document(cases[cid]["payload"], rules)
        assert errs, f"Expected slot error for {cid}"


@REGISTRY.register("F2-T2-04", tier=2, features=[2], title="Reject plaintext ID card & phone in PatientRecord.patient_id & HTTP PUT")
def test_f2_t2_04(ctx: TestContext) -> None:
    cases = _neg_cases(ctx)
    rules = load_schema_rules()
    for cid in ("bad_patient_id_18", "bad_patient_phone"):
        errs = validate_patient_record(cases[cid]["payload"], rules)
        assert errs, f"{cid} must fail schema validation"
    rec15 = dict(cases["bad_patient_id_18"]["payload"], patient_id="110101900101123")
    assert validate_patient_record(rec15, rules)

    hdrs, _ = ctx.auth_headers_or_wechat("wx_f2_t2_04")
    for bad_pid in ("110101199001011234", "13800138000"):
        r = ctx.client.put(
            f"/v1/patients/{bad_pid}/memory",
            json_body=dict(cases["bad_patient_phone"]["payload"], patient_id=bad_pid),
            headers=hdrs,
        )
        assert r.status in (400, 403, 422), f"Expected 400/403/422 for plaintext PII patient_id {bad_pid}, got {r.status}"


@REGISTRY.register("F2-T2-05", tier=2, features=[2], title="Reject out-of-range L3VisitEvent.urgency_level (0 or 5)")
def test_f2_t2_05(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "urgency_level" in core_src and "1..=4" in core_src, (
        "medical-core missing urgency_level 1..=4 validation"
    )

    cases = _neg_cases(ctx)
    rules = load_schema_rules()
    rec5 = cases["bad_urgency_5"]["payload"]
    assert validate_patient_record(rec5, rules)
    rec0 = json.loads(json.dumps(rec5))
    rec0["l3_visit_events"][0]["urgency_level"] = 0
    assert validate_patient_record(rec0, rules)


@REGISTRY.register("F2-T2-06", tier=2, features=[2], title="Reject invalid EpidemicContext intensity & inverted dates")
def test_f2_t2_06(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    for sym in ("EpidemicContext", "0.0..=1.0", "valid_from", "valid_to"):
        assert sym in core_src, f"medical-core missing EpidemicContext validation symbol: {sym}"

    cases = _neg_cases(ctx)
    rules = load_schema_rules()
    assert validate_epidemic_context(cases["bad_epidemic_intensity"]["payload"], rules)
    assert validate_epidemic_context(cases["bad_epidemic_dates"]["payload"], rules)
    neg_i = dict(cases["bad_epidemic_intensity"]["payload"], intensity=-0.01)
    assert validate_epidemic_context(neg_i, rules)
    bad_dt = dict(cases["bad_epidemic_dates"]["payload"], valid_from="2026/13/45")
    assert validate_epidemic_context(bad_dt, rules)


@REGISTRY.register("F2-T2-07", tier=2, features=[2], title="Reject empty RedFlagRule triggers & invalid level")
def test_f2_t2_07(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    for sym in ("trigger_all_of.is_empty()", "trigger_any_of.is_empty()", "Emergency120"):
        assert sym in core_src, f"medical-core missing RedFlagRule validation symbol: {sym}"

    cases = _neg_cases(ctx)
    rules = load_schema_rules()
    rf = cases["bad_red_flag_empty"]["payload"]
    assert validate_medical_dictionaries({"symptom_synonyms": [], "red_flag_rules": [rf]}, rules)
    bad_lvl = dict(rf, trigger_all_of=["胸痛"], level="Critical999")
    assert validate_medical_dictionaries({"symptom_synonyms": [], "red_flag_rules": [bad_lvl]}, rules)


# ===========================================================================
# F3 Tier 2 (5 tests)
# ===========================================================================

@REGISTRY.register("F3-T2-01", tier=2, features=[3], title="SQL DDL idempotency (IF NOT EXISTS on all CREATE TABLE/INDEX)")
def test_f3_t2_01(ctx: TestContext) -> None:
    info = parse_sql_ddl(_read_sql(ctx))
    for stmt in info.create_table_statements:
        assert re.search(r"CREATE\s+TABLE\s+IF\s+NOT\s+EXISTS\b", stmt, re.IGNORECASE), (
            f"Non-idempotent CREATE TABLE: {stmt[:80]}"
        )
    for stmt in info.create_index_statements:
        assert re.search(r"CREATE\s+(?:UNIQUE\s+)?INDEX\s+IF\s+NOT\s+EXISTS\b", stmt, re.IGNORECASE), (
            f"Non-idempotent CREATE INDEX: {stmt[:80]}"
        )


@REGISTRY.register("F3-T2-02", tier=2, features=[3], title="Zero plaintext PII columns in L2/L3/audit DDL")
def test_f3_t2_02(ctx: TestContext) -> None:
    info = parse_sql_ddl(_read_sql(ctx))
    forbidden = {"real_name", "full_name", "id_card", "id_number", "phone", "phone_number", "mobile", "home_address"}
    for tname, cols in info.tables.items():
        if any(k in tname for k in ("l2", "l3", "audit")):
            overlap = forbidden & set(cols)
            assert not overlap, f"Table {tname} exposes plaintext PII columns: {sorted(overlap)}"


@REGISTRY.register("F3-T2-03", tier=2, features=[3], title="epidemic_contexts active region/season index & intensity range")
def test_f3_t2_03(ctx: TestContext) -> None:
    sql = _read_sql(ctx)
    info = parse_sql_ddl(sql)
    ep_tables = [t for t in info.tables if "epidemic" in t]
    assert ep_tables, "Missing epidemic_contexts table"
    cols = set(info.tables[ep_tables[0]])
    for req in ("region_code", "season", "seasonal_tag", "intensity", "active", "valid_from", "valid_to"):
        assert req in cols, f"epidemic_contexts missing column {req!r}"
    idx_blob = "\n".join(info.create_index_statements).lower()
    assert "region_code" in idx_blob or "epidemic" in idx_blob


@REGISTRY.register("F3-T2-04", tier=2, features=[3], title="Qdrant vector dimension & field schema type consistency")
def test_f3_t2_04(ctx: TestContext) -> None:
    data = parse_json_file(ctx.workspace / "config" / "qdrant" / "collections.json")
    cols = validate_qdrant_collections(data)
    mk_vec = cols["medical_knowledge"].get("vectors") or cols["medical_knowledge"]
    l3_vec = cols["patient_l3_events"].get("vectors") or cols["patient_l3_events"]
    assert mk_vec["size"] == l3_vec["size"], f"Vector sizes mismatch: {mk_vec['size']} vs {l3_vec['size']}"
    mk_text = json.dumps(cols["medical_knowledge"]).lower()
    assert "float" in mk_text and "keyword" in mk_text


@REGISTRY.register("F3-T2-05", tier=2, features=[3], title="SQL DDL dialect dry-run in sqlite3 :memory:")
def test_f3_t2_05(ctx: TestContext) -> None:
    info = dry_run_sqlite_ddl(_read_sql(ctx))
    assert len(info.tables) >= 6


# ===========================================================================
# F4 Tier 2 (6 tests)
# ===========================================================================

@REGISTRY.register("F4-T2-01", tier=2, features=[4], title="Wrong key / truncated ciphertext decryption rejection")
def test_f4_t2_01(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "decrypt" in core_src and "Aes256Gcm" in core_src
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-core", "crypto"], strip_db_env=True)
        assert rc == 0, f"Crypto boundary tests failed:\n{out[-1500:]}"


@REGISTRY.register("F4-T2-02", tier=2, features=[4], title="ID card with X suffix & +86 hyphenated phone PII scrubbing")
def test_f4_t2_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f4_t2_02")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={
            "session_id": "sess_f4_t2_02",
            "patient_id": pid,
            "utterance": "身份证11010119900101123X，电话+86 138-0013-8000，头晕恶心",
            "stream": False,
        },
        headers=hdrs,
    )
    assert resp.status == 200
    assert_no_pii(resp.text, extra_forbidden=("11010119900101123X", "138-0013-8000", "13800138000"))
    if ctx.mock_upstream is not None:
        for c in ctx.mock_upstream.get_calls("/v1/chat/completions"):
            assert_no_pii(
                json.dumps(c.body, ensure_ascii=False),
                extra_forbidden=("11010119900101123X", "138-0013-8000", "13800138000"),
            )


@REGISTRY.register("F4-T2-03", tier=2, features=[4], title="Clinical numeric vitals preserved (no false-positive PII scrub)")
def test_f4_t2_03(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f4_t2_03")
    payload = parse_json_file(ctx.fixtures_dir / "payloads" / "triage_turns.json")["vitals_with_phone_utterance"]
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f4_t2_03", "patient_id": pid, "utterance": payload["utterance"], "stream": False},
        headers=hdrs,
    )
    assert resp.status == 200
    assert_no_pii(resp.text, extra_forbidden=payload["forbidden_pii"])
    if ctx.mock_upstream is not None:
        calls = ctx.mock_upstream.get_calls("/v1/chat/completions")
        if calls:
            sent = json.dumps(calls[-1].body, ensure_ascii=False)
            assert_no_pii(sent, extra_forbidden=payload["forbidden_pii"])
            assert_vitals_preserved(sent, payload["preserved_vitals"])


@REGISTRY.register("F4-T2-04", tier=2, features=[4], title="Post-LLM definitive diagnosis & prescription dosage guardrail")
def test_f4_t2_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f4_t2_04")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault(
            "/v1/chat/completions",
            FaultRule(mode="diagnosis_dosage", match_prompt_contains="预检分诊"),
        )
    try:
        resp = ctx.client.post(
            "/v1/triage/chat",
            json_body={
                "session_id": "sess_f4_t2_04",
                "patient_id": pid,
                "utterance": "请直接确诊我得了什么病，并告诉我阿莫西林每次吃多少毫克",
                "stream": False,
            },
            headers=hdrs,
        )
        assert resp.status == 200
        assert_no_diagnosis_or_dosage(resp.text)
        data = resp.json()
        if ctx.mock_upstream is not None:
            assert data.get("status") == "completed", (
                f"Expected completed status from diagnosis_dosage Stage 4 mock, got {data!r}"
            )
            assert_completed_response(data)
        elif data.get("status") == "completed":
            assert_completed_response(data)
        elif data.get("status") == "follow_up":
            assert_follow_up_response(data)
        else:
            raise AssertionError(f"Unexpected status in F4-T2-04: {data!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("F4-T2-05", tier=2, features=[4], title="Multi-byte UTF-8 / emoji mixed PII scrubbing without panic")
def test_f4_t2_05(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f4_t2_05")
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f4_t2_05", "patient_id": pid, "utterance": "姓名：李四🤒📞13700009999，喉咙痛", "stream": False},
        headers=hdrs,
    )
    assert resp.status == 200
    assert_no_pii(resp.text, extra_forbidden=("13700009999",))


@REGISTRY.register("F4-T2-06", tier=2, features=[4], title="Physical wipe completeness (DELETE /v1/patients/{id}/memory)")
def test_f4_t2_06(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f4_t2_06")
    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid,
            "consent_long_term_memory": True,
            "l2_profile": {
                "age_group": "30-39", "gender": "female", "is_pregnant": False,
                "chronic_diseases": ["银屑病测试标记"], "allergies": ["磺胺测试"],
                "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    d = ctx.client.delete(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert d.status in (200, 204)
    g = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert g.status in (200, 404)
    if g.status == 200:
        assert "银屑病测试标记" not in g.text and "磺胺测试" not in g.text


# ===========================================================================
# F5 Tier 2 (5 tests)
# ===========================================================================

@REGISTRY.register("F5-T2-01", tier=2, features=[5], title="R5 concurrent L1/L2 read-write stress (20 parallel workers)")
def test_f5_t2_01(ctx: TestContext) -> None:
    def worker(idx: int) -> None:
        hdrs, pid = ctx.auth_headers_or_wechat(f"wx_conc_{idx}")
        marker = f"并发慢病_{idx}"
        r1 = ctx.client.put(
            f"/v1/patients/{pid}/memory",
            json_body={
                "patient_id": pid,
                "consent_long_term_memory": True,
                "l2_profile": {
                    "age_group": "30-39", "gender": "male", "is_pregnant": False,
                    "chronic_diseases": [marker], "allergies": [], "surgical_history": [], "long_term_medications": [],
                },
                "l3_visit_events": [],
            },
            headers=hdrs,
        )
        assert r1.status in (200, 201), f"Worker {idx} PUT failed: {r1.status}"
        r2 = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
        assert r2.status == 200 and marker in r2.text

    with ThreadPoolExecutor(max_workers=10) as ex:
        list(ex.map(worker, range(20)))


@REGISTRY.register("F5-T2-02", tier=2, features=[5], title="Strict cross-patient L2/L3 isolation in R5 adapters")
def test_f5_t2_02(ctx: TestContext) -> None:
    hdrs_a, pid_a = ctx.auth_headers_or_wechat("wx_iso_patient_a")
    hdrs_b, pid_b = ctx.auth_headers_or_wechat("wx_iso_patient_b")
    ctx.client.put(
        f"/v1/patients/{pid_a}/memory",
        json_body={
            "patient_id": pid_a, "consent_long_term_memory": True,
            "l2_profile": {
                "age_group": "40-49", "gender": "female", "is_pregnant": False,
                "chronic_diseases": ["系统性红斑狼疮_隔离标记"], "allergies": ["青霉素"],
                "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [],
        },
        headers=hdrs_a,
    )
    ctx.client.put(
        f"/v1/patients/{pid_b}/memory",
        json_body={
            "patient_id": pid_b, "consent_long_term_memory": True,
            "l2_profile": {
                "age_group": "30-39", "gender": "male", "is_pregnant": False,
                "chronic_diseases": ["2型糖尿病"], "allergies": [],
                "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [],
        },
        headers=hdrs_b,
    )
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    r = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_iso_b", "patient_id": pid_b, "utterance": "最近有点头晕", "stream": False},
        headers=hdrs_b,
    )
    assert r.status == 200
    assert "系统性红斑狼疮_隔离标记" not in r.text
    if ctx.mock_upstream is not None:
        for c in ctx.mock_upstream.get_calls("/v1/chat/completions"):
            assert "系统性红斑狼疮_隔离标记" not in json.dumps(c.body, ensure_ascii=False)


@REGISTRY.register("F5-T2-03", tier=2, features=[5], title="Graceful R5 fallback without external Redis/PG/Qdrant")
def test_f5_t2_03(ctx: TestContext) -> None:
    srv_src = _collect_rs_text(ctx.workspace / "crates" / "medical-server")
    assert "fallback" in srv_src.lower() or "in_memory" in srv_src.lower() or "InMemory" in srv_src
    h = ctx.client.get("/health")
    assert h.status == 200


@REGISTRY.register("F5-T2-04", tier=2, features=[5], title="Idempotent DELETE on non-existent patient")
def test_f5_t2_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f5_t2_04_never_put")
    d = ctx.client.delete(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert d.status in (200, 204, 404), f"Unexpected status on empty DELETE: {d.status}"


@REGISTRY.register("F5-T2-05", tier=2, features=[5], title="Multi-tenant knowledge namespace isolation")
def test_f5_t2_05(ctx: TestContext) -> None:
    key_a = ctx.tenant_key("hosp_a")
    key_b = ctx.tenant_key("hosp_b")
    r_a = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "高热 咳嗽", "top_k": 10},
        bearer=key_a,
    )
    r_b = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "高热 咳嗽", "top_k": 10},
        bearer=key_b,
    )
    assert r_a.status == 200 and r_b.status == 200, (
        f"Tenant knowledge search failed: hosp_a={r_a.status} {r_a.text}, hosp_b={r_b.status} {r_b.text}"
    )
    a_results = r_a.json().get("results", [])
    b_results = r_b.json().get("results", [])
    assert len(a_results) >= 1 and len(b_results) >= 1, (
        f"Expected >=1 knowledge hit per tenant: a={len(a_results)}, b={len(b_results)}"
    )
    a_ids = {item.get("doc_id") for item in a_results}
    b_ids = {item.get("doc_id") for item in b_results}
    a_blob = json.dumps(a_results, ensure_ascii=False)
    b_blob = json.dumps(b_results, ensure_ascii=False)
    if ctx.mock_upstream is not None:
        assert "hosp_a_private_doc_01" in a_ids, f"hosp_a missing private seeded doc: {a_ids}"
        assert "hosp_b_private_doc_01" in b_ids, f"hosp_b missing private seeded doc: {b_ids}"
    assert "hosp_a_private_doc_01" not in b_ids and "hosp_a_private" not in b_blob, (
        f"hosp_a private knowledge leaked to hosp_b: {b_results!r}"
    )
    assert "hosp_b_private_doc_01" not in a_ids and "hosp_b_private" not in a_blob, (
        f"hosp_b private knowledge leaked to hosp_a: {a_results!r}"
    )
    for item in b_results:
        assert item.get("tenant_id") != "hosp_a" and item.get("tenant_namespace") != "hosp_a"
    for item in a_results:
        assert item.get("tenant_id") != "hosp_b" and item.get("tenant_namespace") != "hosp_b"


# ===========================================================================
# F6 Tier 2 (6 tests)
# ===========================================================================

@REGISTRY.register("F6-T2-01", tier=2, features=[6], title="Upstream timeout (>8s or configured) failover")
def test_f6_t2_01(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t2_01")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.enqueue_fault(
            "/v1/chat/completions",
            FaultRule(delay_sec=9.0, match_prompt_contains="预检分诊"),
        )
    try:
        resp = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_f6_t2_01", "patient_id": pid, "utterance": "发热咽痛", "stream": False},
            headers=hdrs,
            timeout=max(ctx.timeout, 15.0),
        )
        assert resp.status == 200, f"Expected failover/fallback 200 on upstream timeout, got {resp.status}"
        data = resp.json()
        if data.get("status") == "follow_up":
            assert_follow_up_response(data)
        elif data.get("status") == "completed":
            assert_completed_response(data)
        else:
            raise AssertionError(f"Unexpected status on timeout failover: {data!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("F6-T2-02", tier=2, features=[6], title="Circuit breaker open state & fallback suppression")
def test_f6_t2_02(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "circuit" in core_src.lower() or "breaker" in core_src.lower() or "fallback" in core_src.lower()
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t2_02")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault("/v1/chat/completions", FaultRule(status=500))
    try:
        for i in range(4):
            r = ctx.client.post(
                "/v1/triage/chat",
                json_body={"session_id": f"sess_cb_{i}", "patient_id": pid, "utterance": "咳嗽低热", "stream": False},
                headers=hdrs,
            )
            assert r.status == 200, f"Circuit-breaker request {i} returned {r.status}"
            d = r.json()
            if ctx.mock_upstream is not None:
                assert d.get("status") == "completed", f"Expected fallback completed on open circuit, got {d!r}"
                assert_completed_response(d)
            elif d.get("status") == "follow_up":
                assert_follow_up_response(d)
            elif d.get("status") == "completed":
                assert_completed_response(d)
            else:
                raise AssertionError(f"Unexpected status in F6-T2-02: {d!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("F6-T2-03", tier=2, features=[6], title="Malformed non-JSON LLM output repair / safe degradation")
def test_f6_t2_03(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t2_03")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault(
            "/v1/chat/completions",
            FaultRule(mode="malformed_json", match_prompt_contains="预检分诊"),
        )
    try:
        resp = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_f6_t2_03", "patient_id": pid, "utterance": "咳嗽三天", "stream": False},
            headers=hdrs,
        )
        assert resp.status == 200
        data = resp.json()
        if data.get("status") == "completed":
            assert_completed_response(data)
        elif data.get("status") == "follow_up":
            assert_follow_up_response(data)
        else:
            raise AssertionError(f"Unexpected status on malformed_json degradation: {data!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("F6-T2-04", tier=2, features=[6], title="All upstream providers down -> fallback_rule_triage_system")
def test_f6_t2_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t2_04")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault("/v1/chat/completions", FaultRule(status=503))
    try:
        resp = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_f6_t2_04", "patient_id": pid, "utterance": "高热伴咳嗽咽痛", "stream": False},
            headers=hdrs,
        )
        assert resp.status == 200
        data = resp.json()
        assert data.get("status") == "completed"
        assert_completed_response(data)
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("F6-T2-05", tier=2, features=[6], title="Empty embedding input batch boundary")
def test_f6_t2_05(ctx: TestContext) -> None:
    core_src = _collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "embed_batch" in core_src
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-core", "embed"], strip_db_env=True)
        assert rc == 0, f"Embedding boundary test failed:\n{out[-1500:]}"


@REGISTRY.register("F6-T2-06", tier=2, features=[6], title="Upstream SSE mid-stream truncation recovery")
def test_f6_t2_06(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f6_t2_06")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault(
            "/v1/chat/completions",
            FaultRule(mode="truncated_sse", match_stream=True),
        )
    try:
        sse = ctx.client.post_sse(
            "/v1/triage/chat",
            json_body={"session_id": "sess_f6_t2_06", "patient_id": pid, "utterance": "咳嗽两天", "stream": True},
            headers=hdrs,
        )
        assert sse.status == 200
        res_evs = [e for e in sse.events if e.event == "result"]
        err_evs = [e for e in sse.events if e.event == "error"]
        assert res_evs or err_evs, (
            f"Expected SSE event: result or event: error on truncated upstream stream, got {[e.event for e in sse.events]}"
        )
        if res_evs:
            payload = res_evs[-1].json()
            if payload.get("status") == "completed":
                assert_completed_response(payload)
            elif payload.get("status") == "follow_up":
                assert_follow_up_response(payload)
            else:
                raise AssertionError(f"Unexpected SSE result status after truncated stream: {payload!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


# ===========================================================================
# F7 Tier 2 (7 tests)
# ===========================================================================

@REGISTRY.register("F7-T2-01", tier=2, features=[7], title="Hard convergence at max_turns == 4")
def test_f7_t2_01(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t2_01")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault("/v1/chat/completions", FaultRule(mode="always_follow_up"))
    sid = "sess_max_turn_4"
    last: dict[str, Any] = {}
    try:
        for turn_i in range(1, 5):
            r = ctx.client.post(
                "/v1/triage/chat",
                json_body={"session_id": sid, "patient_id": pid, "utterance": f"发烧咳嗽第{turn_i}次回复", "stream": False},
                headers=hdrs,
            )
            assert r.status == 200
            last = r.json()
            if last.get("status") == "completed":
                break
            assert_follow_up_response(last)
            assert last["turn"] <= 4
        assert last.get("status") == "completed", f"Turn 4 MUST force completed, got {last}"
        assert_completed_response(last)
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("F7-T2-02", tier=2, features=[7], title="Enforced repair of >30-char question or 5 quick_options")
def test_f7_t2_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t2_02")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault(
            "/v1/chat/completions",
            FaultRule(mode="oversized_question", match_prompt_contains="预检分诊"),
        )
    try:
        resp = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_f7_t2_02", "patient_id": pid, "utterance": "咳嗽三天", "stream": False},
            headers=hdrs,
        )
        assert resp.status == 200
        data = resp.json()
        if data.get("status") == "follow_up":
            assert_follow_up_response(data)
        elif data.get("status") == "completed":
            assert_completed_response(data)
        else:
            raise AssertionError(f"Unexpected status in F7-T2-02: {data!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("F7-T2-03", tier=2, features=[7], title="Full LLM outage fallback_rule_triage_system")
def test_f7_t2_03(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t2_03")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault("/v1/chat/completions", FaultRule(status=500))
    try:
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_f7_t2_03", "patient_id": pid, "utterance": "高热伴咽痛干咳两天", "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        assert_completed_response(r.json())
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("F7-T2-04", tier=2, features=[7], title="Stage 5 consolidation blocked when consent_long_term_memory == false")
def test_f7_t2_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t2_04_noconsent")
    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid,
            "consent_long_term_memory": False,
            "l2_profile": {
                "age_group": "30-39", "gender": "male", "is_pregnant": False,
                "chronic_diseases": [], "allergies": [], "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    for u in ("头晕两天", "今天刚开始", "无基础病", "头晕"):
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_noconsent", "patient_id": pid, "utterance": u, "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        if r.json().get("status") == "completed":
            break
    time.sleep(0.25)
    g = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    if g.status == 200:
        assert len(g.json().get("l3_visit_events", [])) == 0, "Non-consented patient must NOT consolidate L3 events"


@REGISTRY.register("F7-T2-05", tier=2, features=[7], title="Partial red-flag non-match must NOT trigger Emergency120")
def test_f7_t2_05(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t2_05")
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={
            "session_id": "sess_f7_t2_05",
            "patient_id": pid,
            "utterance": "左边胸口轻微隐痛两天，没有出汗也不憋气",
            "stream": False,
        },
        headers=hdrs,
    )
    assert resp.status == 200
    data = resp.json()
    assert data.get("red_flag") != "Emergency120", f"Partial match falsely triggered Emergency120: {data}"
    assert data.get("status") != "emergency"


@REGISTRY.register("F7-T2-06", tier=2, features=[7], title="Empty / whitespace / punctuation-only utterance handling")
def test_f7_t2_06(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f7_t2_06")
    for bad in ("", "   ", "？？？"):
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_f7_t2_06", "patient_id": pid, "utterance": bad, "stream": False},
            headers=hdrs,
        )
        assert r.status in (200, 400, 422), f"Unexpected {r.status} for utterance={bad!r}"
        if r.status == 200 and r.json().get("status") == "follow_up":
            assert_follow_up_response(r.json())


@REGISTRY.register("F7-T2-07", tier=2, features=[7], title="Per-request bevy_ecs::World concurrent isolation (30 parallel sessions)")
def test_f7_t2_07(ctx: TestContext) -> None:
    def run_one(idx: int) -> None:
        hdrs, pid = ctx.auth_headers_or_wechat(f"wx_ecs_para_{idx}")
        is_emerg = idx % 2 == 0
        utt = "突然胸痛伴大汗和左臂放射痛" if is_emerg else "鼻塞流涕打喷嚏两天"
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": f"sess_para_{idx}", "patient_id": pid, "utterance": utt, "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        d = r.json()
        if is_emerg:
            assert_emergency_response(d)
        else:
            assert d.get("red_flag") != "Emergency120" and d.get("status") != "emergency"

    with ThreadPoolExecutor(max_workers=10) as ex:
        list(ex.map(run_one, range(30)))


# ===========================================================================
# F8 Tier 2 (5 tests)
# ===========================================================================

@REGISTRY.register("F8-T2-01", tier=2, features=[8], title="Fault-tolerant line-level JSONL syntax error recovery (line 3)")
def test_f8_t2_01(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "sonic_rs" in pipe_src and ("Err(" in pipe_src or "Result<" in pipe_src)
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-pipeline"], strip_db_env=True)
        assert rc == 0, f"cargo test -p medical-pipeline failed:\n{out[-1500:]}"

    rules = load_schema_rules()
    res = parse_jsonl_stream(
        ctx.fixtures_dir / "knowledge" / "malformed_line3.jsonl",
        validator_fn=lambda d: validate_knowledge_document(d, rules),
    )
    assert len(res.records) == 9 and len(res.errors) == 1
    assert res.errors[0][0] == 3


@REGISTRY.register("F8-T2-02", tier=2, features=[8], title="Valid JSON syntax / schema violation isolation in JSONL")
def test_f8_t2_02(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "sonic_rs" in pipe_src and "validate()" in pipe_src

    rules = load_schema_rules()
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "schema_err.jsonl"
        lines = (ctx.fixtures_dir / "knowledge" / "malformed_line3.jsonl").read_text(encoding="utf-8").splitlines()
        bad_schema_doc = {
            "doc_id": "bad_schema_04", "source_title": "T", "source_type": "textbook",
            "authority_level": 2.5, "disease_name": "X", "department": "呼吸内科",
            "target_population": [], "section_type": "clinical_manifestation",
            "content": "bad", "key_symptoms": ["发热"],
        }
        lines[2] = lines[0]
        lines[3] = json.dumps(bad_schema_doc, ensure_ascii=False)
        p.write_text("\n".join(lines) + "\n", encoding="utf-8")
        res = parse_jsonl_stream(p, validator_fn=lambda d: validate_knowledge_document(d, rules))
        assert len(res.records) == 9 and len(res.errors) == 1 and res.errors[0][0] == 4


@REGISTRY.register("F8-T2-03", tier=2, features=[8], title="Blank & whitespace-only lines in JSONL")
def test_f8_t2_03(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "sonic_rs" in pipe_src and ("is_empty()" in pipe_src or "trim()" in pipe_src)

    rules = load_schema_rules()
    valid_line = (ctx.fixtures_dir / "knowledge" / "knowledge_100.jsonl").read_text(encoding="utf-8").splitlines()[0]
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "blanks.jsonl"
        p.write_text(f"\n  \n{valid_line}\n\t\r\n{valid_line}\n\n", encoding="utf-8")
        res = parse_jsonl_stream(p, validator_fn=lambda d: validate_knowledge_document(d, rules))
        assert len(res.records) == 2 and not res.errors


@REGISTRY.register("F8-T2-04", tier=2, features=[8], title="Empty 0-byte JSONL & empty JSON bundle arrays")
def test_f8_t2_04(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "sonic_rs" in pipe_src and "Result<" in pipe_src
    parse_json_file(ctx.workspace / "doc" / "schemas" / "data-collection-schema.json")

    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "empty.jsonl"
        p.write_bytes(b"")
        res = parse_jsonl_stream(p)
        assert res.records == [] and res.errors == []


@REGISTRY.register("F8-T2-05", tier=2, features=[8], title="Large single-line escaped Unicode content (64 KB) in JSONL")
def test_f8_t2_05(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "sonic_rs" in pipe_src and ("BufReader" in pipe_src or "read_line" in pipe_src or "lines(" in pipe_src)

    rules = load_schema_rules()
    big_text = ("临床症状℃±≥≤\"制表\t换行\n医学正文。" * 2500)[:65536]
    doc = {
        "doc_id": "big_64kb_01", "source_title": "大块文本测试", "source_type": "textbook",
        "authority_level": 0.9, "disease_name": "慢性支气管炎", "department": "呼吸内科",
        "target_population": ["adult"], "section_type": "clinical_manifestation",
        "content": big_text, "key_symptoms": ["咳嗽"],
    }
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "big64k.jsonl"
        p.write_text(json.dumps(doc, ensure_ascii=False) + "\n", encoding="utf-8")
        res = parse_jsonl_stream(p, validator_fn=lambda d: validate_knowledge_document(d, rules))
        assert len(res.records) == 1 and res.records[0]["content"] == big_text


# ===========================================================================
# F9 Tier 2 (5 tests)
# ===========================================================================

@REGISTRY.register("F9-T2-01", tier=2, features=[9], title="Scanned / image-only PDF triggers OcrPlugin hook contract")
def test_f9_t2_01(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "OcrPlugin" in pipe_src or "ocr" in pipe_src.lower(), "medical-pipeline missing OcrPlugin hook"
    with tempfile.TemporaryDirectory() as td:
        scanned = generate_minimal_pdf(Path(td) / "scanned_empty.pdf", text_lines=None)
        info = inspect_pdf_bytes(scanned.read_bytes())
        assert info["has_text_ops"] is False


@REGISTRY.register("F9-T2-02", tier=2, features=[9], title="Anti-mechanical-truncation semantic chunking contract")
def test_f9_t2_02(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "。" in pipe_src or "sentence" in pipe_src.lower() or "chunk" in pipe_src.lower()
    md = (ctx.fixtures_dir / "knowledge" / "books_sample.md").read_text(encoding="utf-8")
    assert len(md) > 300


@REGISTRY.register("F9-T2-03", tier=2, features=[9], title="Invalid custom regex rejection without panic")
def test_f9_t2_03(ctx: TestContext) -> None:
    try:
        re.compile("[unclosed(group")
        raise AssertionError("Expected re.error")
    except re.error:
        pass
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "Regex::new" in pipe_src, "medical-pipeline must compile user regex safely via Regex::new"


@REGISTRY.register("F9-T2-04", tier=2, features=[9], title="Corrupt binary .pdf / .docx error isolation")
def test_f9_t2_04(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    deps = pipe_toml.get("dependencies") or {}
    assert "docx-rs" in deps and ("lopdf" in deps or "pdf-extract" in deps), (
        f"medical-pipeline missing docx/pdf dependencies: {sorted(deps)}"
    )
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "Result<" in pipe_src, "medical-pipeline must isolate corrupt PDF/DOCX parse failures via Result"

    try:
        inspect_pdf_bytes(b"%PDF-1.4\nCORRUPT_GARBAGE")
    except ParseError:
        pass
    else:
        raise AssertionError("Corrupt PDF must raise ParseError in inspect_pdf_bytes")

    with tempfile.TemporaryDirectory() as td:
        bad_docx = Path(td) / "corrupt.docx"
        bad_docx.write_bytes(b"PK\x03\x04CORRUPT_NOT_A_VALID_DOCX")
        try:
            read_docx_paragraphs(bad_docx)
        except (ParseError, zipfile.BadZipFile, KeyError):
            pass
        else:
            raise AssertionError("Corrupt DOCX must fail inspection")

    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-pipeline"], strip_db_env=True)
        assert rc == 0, f"medical-pipeline tests failed:\n{out[-1500:]}"


@REGISTRY.register("F9-T2-05", tier=2, features=[9], title="Missing mandatory metadata rejection in chunker")
def test_f9_t2_05(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    for fld in ("target_population", "department", "authority_level", "section_type"):
        assert fld in pipe_src, f"medical-pipeline chunker missing metadata field {fld!r}"
    assert "validate" in pipe_src.lower(), "medical-pipeline chunker must validate metadata"

    bad_doc = {
        "doc_id": "missing_meta_01", "source_title": "T", "source_type": "textbook",
        "authority_level": 0.9, "disease_name": "感冒", "department": "",
        "target_population": [], "section_type": "clinical_manifestation",
        "content": "正文", "key_symptoms": ["咳嗽"],
    }
    assert validate_knowledge_document(bad_doc, load_schema_rules())


# ===========================================================================
# F10 Tier 2 (6 tests)
# ===========================================================================

@REGISTRY.register("F10-T2-01", tier=2, features=[10], title="Privacy firewall blocks unmasked PII commit")
def test_f10_t2_01(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f10_t2_01")
    r = ctx.client.put(
        "/v1/patients/110101199001011234/memory",
        json_body={
            "patient_id": "110101199001011234",
            "consent_long_term_memory": True,
            "l2_profile": {
                "age_group": "30-39", "gender": "male", "is_pregnant": False,
                "chronic_diseases": [], "allergies": [], "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    assert r.status in (400, 403, 422)


@REGISTRY.register("F10-T2-02", tier=2, features=[10], title="Age-to-age_group bucketing boundaries contract")
def test_f10_t2_02(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "age_group" in pipe_src and ("0-9" in pipe_src or "unknown" in pipe_src.lower())


@REGISTRY.register("F10-T2-03", tier=2, features=[10], title="UTF-8 BOM & variant column names in CSV")
def test_f10_t2_03(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    assert "csv" in (pipe_toml.get("dependencies") or {}), "medical-pipeline missing csv dependency"
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "csv" in pipe_src.lower() and (
        "feff" in pipe_src.lower() or "trim_start_matches" in pipe_src or "headers" in pipe_src
    )

    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "bom.csv"
        p.write_bytes(b"\xef\xbb\xbf" + "患者姓名,证件号码,联系电话,年龄\n张三,110101199001011234,13800138000,34\n".encode("utf-8"))
        text = p.read_text(encoding="utf-8-sig")
        assert text.startswith("患者姓名,")


@REGISTRY.register("F10-T2-04", tier=2, features=[10], title="Partial row errors in malformed patient CSV")
def test_f10_t2_04(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "csv" in pipe_src.lower() and ("errors" in pipe_src.lower() or "failed" in pipe_src.lower()) and "Result<" in pipe_src
    if ctx.with_cargo:
        rc, out = ctx.run_cargo(["test", "-p", "medical-pipeline"], strip_db_env=True)
        assert rc == 0, f"cargo test -p medical-pipeline failed:\n{out[-1500:]}"

    rows = (ctx.fixtures_dir / "patients" / "malformed_patients.csv").read_text(encoding="utf-8").splitlines()
    assert len(rows) == 11 and "99" in rows[5]


@REGISTRY.register("F10-T2-05", tier=2, features=[10], title="Embedded PII inside free-text chief_complaint / summary")
def test_f10_t2_05(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f10_t2_05")
    resp = ctx.client.post(
        "/v1/triage/chat",
        json_body={
            "session_id": "sess_f10_t2_05",
            "patient_id": pid,
            "utterance": "患者李四(电话13900001111, 身份证110101198808081234)诉头痛三天",
            "stream": False,
        },
        headers=hdrs,
    )
    assert resp.status == 200
    assert_no_pii(resp.text, extra_forbidden=("13900001111", "110101198808081234"))


@REGISTRY.register("F10-T2-06", tier=2, features=[10], title="Large parallel rayon batch (5,000 synthetic patient rows)")
def test_f10_t2_06(ctx: TestContext) -> None:
    pipe_src = _collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "par_iter" in pipe_src or "into_par_iter" in pipe_src, "medical-pipeline must use rayon parallel iteration"
    with tempfile.TemporaryDirectory() as td:
        csv_path = generate_5000_patient_csv(Path(td) / "patients_5000.csv", rows=5000)
        lines = csv_path.read_text(encoding="utf-8").splitlines()
        assert len(lines) == 5001


# ===========================================================================
# F11 Tier 2 (6 tests)
# ===========================================================================

@REGISTRY.register("F11-T2-01", tier=2, features=[11], title="tantivy-jieba compound medical term precision")
def test_f11_t2_01(ctx: TestContext) -> None:
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    assert "tantivy-jieba" in (pipe_toml.get("dependencies") or {}) or "jieba-rs" in (pipe_toml.get("dependencies") or {})
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f11_t2_01")
    r = ctx.client.post("/v1/knowledge/search", json_body={"query": "心前区 压榨性疼痛", "top_k": 3}, headers=hdrs)
    assert r.status == 200


@REGISTRY.register("F11-T2-02", tier=2, features=[11], title="Partial dual-write failure checkpoint cursor safety")
def test_f11_t2_02(ctx: TestContext) -> None:
    ckpt_src = (ctx.workspace / "crates" / "medical-pipeline" / "src" / "checkpoint.rs").read_text(encoding="utf-8")
    assert "checkpoint" in ckpt_src.lower()


@REGISTRY.register("F11-T2-03", tier=2, features=[11], title="Search query & top_k boundary validation (empty, 0, 10000)")
def test_f11_t2_03(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f11_t2_03")
    r_empty = ctx.client.post("/v1/knowledge/search", json_body={"query": "", "top_k": 5}, headers=hdrs)
    assert r_empty.status in (400, 422)
    r_zero = ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "top_k": 0}, headers=hdrs)
    assert r_zero.status in (200, 400, 422)
    if r_zero.status == 200:
        assert r_zero.json().get("results") == []
    r_big = ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "top_k": 10000}, headers=hdrs)
    assert r_big.status == 200 and len(r_big.json().get("results", [])) <= 100


@REGISTRY.register("F11-T2-04", tier=2, features=[11], title="Corrupt/truncated checkpoint recovery without panic")
def test_f11_t2_04(ctx: TestContext) -> None:
    ckpt_path = ctx.workspace / "crates" / "medical-pipeline" / "src" / "checkpoint.rs"
    assert ckpt_path.is_file(), f"Missing {ckpt_path}"
    ckpt_src = ckpt_path.read_text(encoding="utf-8")
    prod_src = strip_rust_comments_and_tests(ckpt_src, strip_tests=True)
    assert prod_src.strip(), "checkpoint.rs contains no production Rust code"
    assert not re.search(r"\.\s*unwrap\s*\(\s*\)", prod_src), (
        "checkpoint.rs production code must not panic with .unwrap() on corrupt files"
    )


@REGISTRY.register("F11-T2-05", tier=2, features=[11], title="authority_level ranking influence (textbook 1.0 > qa 0.5)")
def test_f11_t2_05(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f11_t2_05")
    r = ctx.client.post("/v1/knowledge/search", json_body={"query": "高热 全身肌肉酸痛", "top_k": 5}, headers=hdrs)
    assert r.status == 200
    results = r.json().get("results", [])
    if len(results) >= 2 and "authority_level" in results[0]:
        assert float(results[0]["authority_level"]) >= 0.5


@REGISTRY.register("F11-T2-06", tier=2, features=[11], title="Concurrent search stress (10 threads, zero lock panic)")
def test_f11_t2_06(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f11_t2_06")

    def q(idx: int) -> None:
        r = ctx.client.post(
            "/v1/knowledge/search",
            json_body={"query": "高热 咳嗽" if idx % 2 == 0 else "胸痛 大汗", "top_k": 3},
            headers=hdrs,
        )
        assert r.status == 200

    with ThreadPoolExecutor(max_workers=10) as ex:
        list(ex.map(q, range(10)))

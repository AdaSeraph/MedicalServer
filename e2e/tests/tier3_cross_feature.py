"""Tier 3 Pairwise Cross-Feature E2E Tests: T3-01..T3-28 (28 tests covering F1–F22)."""

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
    assert_emergency_response,
    assert_follow_up_response,
    assert_no_diagnosis_or_dosage,
    assert_no_pii,
    assert_studio_commands,
    collect_rs_sources,
    collect_studio_sources,
    discover_miniapp_page_text,
    load_schema_rules,
    validate_collection_bundle,
)
from e2e.lib.mock_upstream import FaultRule
from e2e.lib.parsers import (
    parse_dockerfile,
    parse_json_file,
    parse_jsonl_stream,
    parse_prometheus_metrics,
    parse_sql_ddl,
    parse_toml_file,
    parse_yaml_subset,
    read_docx_paragraphs,
    read_xlsx_rows,
    validate_grafana_dashboard,
    validate_ico_bytes,
    validate_k6_script,
    validate_qdrant_collections,
)
from e2e.lib.registry import REGISTRY
from e2e.lib.runner import TestContext


def _read_dir_rs(path: Path) -> str:
    return "\n".join(collect_rs_sources(path).values())


def _read_studio_fe(ctx: TestContext) -> str:
    return collect_studio_sources(ctx.workspace)[1]


@REGISTRY.register("T3-01", tier=3, features=[2, 8, 11, 14], title="Schema + sonic-rs JSONL + Dual-write + /v1/knowledge/search")
def test_t3_01(ctx: TestContext) -> None:
    bundle = parse_json_file(ctx.fixtures_dir / "knowledge" / "valid_collection.json")
    assert not validate_collection_bundle(bundle, load_schema_rules())
    res_bad = parse_jsonl_stream(ctx.fixtures_dir / "knowledge" / "malformed_line3.jsonl")
    assert len(res_bad.records) == 9 and len(res_bad.errors) == 1
    pipe_toml = parse_toml_file(ctx.workspace / "crates" / "medical-pipeline" / "Cargo.toml")
    assert "sonic-rs" in (pipe_toml.get("dependencies") or {})
    hdrs, _ = ctx.auth_headers_or_wechat("wx_t3_01")
    r = ctx.client.post("/v1/knowledge/search", json_body={"query": "流行性感冒 全身肌肉酸痛", "top_k": 5}, headers=hdrs)
    assert r.status == 200 and len(r.json().get("results", [])) >= 1


@REGISTRY.register("T3-02", tier=3, features=[2, 9, 11, 16], title="Book parsers + Chunker + Dual-write + Studio Mod1 + Search filters")
def test_t3_02(ctx: TestContext) -> None:
    with tempfile.TemporaryDirectory() as td:
        docx = generate_minimal_docx(Path(td) / "book.docx")
        pdf = generate_minimal_pdf(Path(td) / "book.pdf", ["Influenza Respiratory Guideline"])
        assert read_docx_paragraphs(docx) and pdf.is_file()
    assert_studio_commands(ctx, ["preview_knowledge_chunks", "start_knowledge_import"])
    hdrs, _ = ctx.auth_headers_or_wechat("wx_t3_02")
    r = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "发热 咳嗽", "department": "呼吸内科", "target_population": ["adult"], "top_k": 5},
        headers=hdrs,
    )
    assert r.status == 200


@REGISTRY.register("T3-03", tier=3, features=[3, 4, 10, 17], title="DDL encryption + PII masker + Studio Mod2 + Plaintext patient_id reject")
def test_t3_03(ctx: TestContext) -> None:
    sql_info = parse_sql_ddl((ctx.workspace / "migrations" / "001_init.sql").read_text(encoding="utf-8"))
    assert any("l2" in t for t in sql_info.tables)
    with tempfile.TemporaryDirectory() as td:
        xlsx = generate_minimal_xlsx(Path(td) / "patients.xlsx")
        assert len(read_xlsx_rows(xlsx)) >= 3
    assert_studio_commands(ctx, ["preview_patient_import", "apply_masking_and_import_patients"])
    hdrs, _ = ctx.auth_headers_or_wechat("wx_t3_03")
    r_bad = ctx.client.put(
        "/v1/patients/110101199001011234/memory",
        json_body={"patient_id": "110101199001011234", "consent_long_term_memory": True,
                   "l2_profile": {"age_group": "30-39", "gender": "male", "is_pregnant": False,
                                  "chronic_diseases": [], "allergies": [], "surgical_history": [], "long_term_medications": []},
                   "l3_visit_events": []},
        headers=hdrs,
    )
    assert r_bad.status in (400, 403, 422)


@REGISTRY.register("T3-04", tier=3, features=[4, 7, 14], title="Outbound PII scrub + Triage guardrails + Disclaimer")
def test_t3_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_04")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    utt = "我叫张伟，身份证110101199001011234，电话13800138000，咽痛发热两天"
    r1 = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_04", "patient_id": pid, "utterance": utt, "stream": False},
        headers=hdrs,
    )
    assert r1.status == 200
    assert_no_pii(r1.text, extra_forbidden=("110101199001011234", "13800138000"))
    assert_no_diagnosis_or_dosage(r1.text)
    r2 = ctx.client.post(
        "/v1/openai/chat/completions",
        json_body={"model": "triage", "messages": [{"role": "user", "content": utt}]},
        headers=hdrs,
    )
    assert r2.status == 200
    assert_no_pii(r2.text, extra_forbidden=("110101199001011234", "13800138000"))
    assert_no_diagnosis_or_dosage(r2.text)
    if ctx.mock_upstream is not None:
        calls = ctx.mock_upstream.get_calls("/v1/chat/completions")
        assert len(calls) >= 1, "Expected upstream LLM calls in T3-04"
        for c in calls:
            assert_no_pii(
                json.dumps(c.body, ensure_ascii=False),
                extra_forbidden=("张伟", "110101199001011234", "13800138000"),
            )


@REGISTRY.register("T3-05", tier=3, features=[4, 12, 14, 22], title="WeChat Auth + Cross-patient isolation + Miniapp API")
def test_t3_05(ctx: TestContext) -> None:
    tok_a, pid_a = ctx.get_wechat_auth("wx_t3_05_user_a")
    _, pid_b = ctx.get_wechat_auth("wx_t3_05_user_b")
    assert pid_a != pid_b
    assert_no_pii(pid_a)
    r_forbidden = ctx.client.get(f"/v1/patients/{pid_b}/memory", bearer=tok_a)
    assert r_forbidden.status in (401, 403)
    r_own = ctx.client.get(f"/v1/patients/{pid_a}/memory", bearer=tok_a)
    assert r_own.status in (200, 404)
    api_js = (ctx.workspace / "miniapp" / "utils" / "api.js").read_text(encoding="utf-8")
    assert "/v1/auth/wechat" in api_js and "Bearer" in api_js


@REGISTRY.register("T3-06", tier=3, features=[3, 4, 13, 14, 22], title="L2/L3 memory lifecycle + One-click physical wipe + Miniapp")
def test_t3_06(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_06")
    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid, "consent_long_term_memory": True,
            "l2_profile": {"age_group": "30-39", "gender": "female", "is_pregnant": False,
                           "chronic_diseases": ["T3擦除测试病"], "allergies": ["磺胺"],
                           "surgical_history": [], "long_term_medications": []},
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    assert "T3擦除测试病" in ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs).text
    assert ctx.client.delete(f"/v1/patients/{pid}/memory", headers=hdrs).status in (200, 204)
    r_after = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_06_after", "patient_id": pid, "utterance": "咳嗽", "stream": False},
        headers=hdrs,
    )
    assert r_after.status == 200 and "T3擦除测试病" not in r_after.text
    mem_js = discover_miniapp_page_text(ctx.workspace, ".js", ("memory",))
    assert "DELETE" in mem_js or "wipe" in mem_js.lower() or "clear" in mem_js.lower()


@REGISTRY.register("T3-07", tier=3, features=[6, 7, 14], title="Emergency120 dual-channel (JSON, SSE, OpenAI proxy) w/ 0 LLM calls")
def test_t3_07(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_07")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    utt = "患者胸痛伴左臂放射痛和大汗"
    r_json = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_07_j", "patient_id": pid, "utterance": utt, "stream": False},
        headers=hdrs,
    )
    assert r_json.status == 200
    assert_emergency_response(r_json.json())
    r_sse = ctx.client.post_sse(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_07_s", "patient_id": pid, "utterance": utt, "stream": True},
        headers=hdrs,
    )
    assert r_sse.status == 200 and any("Emergency120" in e.data for e in r_sse.events)
    r_oai = ctx.client.post(
        "/v1/openai/chat/completions",
        json_body={"model": "triage", "messages": [{"role": "user", "content": utt}]},
        headers=hdrs,
    )
    assert r_oai.status == 200 and ("120" in r_oai.text or "急诊" in r_oai.text)
    if ctx.mock_upstream is not None:
        assert len(ctx.mock_upstream.get_calls("/v1/chat/completions")) == 0


@REGISTRY.register("T3-08", tier=3, features=[6, 7, 14, 15], title="LLM outage -> fallback_rule_triage_system + /metrics errors")
def test_t3_08(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_08")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault("/v1/chat/completions", FaultRule(status=503))
    try:
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_t3_08", "patient_id": pid, "utterance": "连续低烧咳嗽两天", "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        assert_completed_response(r.json())
        m = parse_prometheus_metrics(ctx.client.get("/metrics").text)
        assert "medical_server_llm_errors_total" in m
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("T3-09", tier=3, features=[7, 13, 14], title="4-turn slot convergence + L1 state machine + triage completion")
def test_t3_09(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_09")
    sid = "sess_t3_09"
    utt = "发烧咽痛"
    last: dict[str, Any] = {}
    for _ in range(4):
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": sid, "patient_id": pid, "utterance": utt, "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        last = r.json()
        if last.get("status") == "completed":
            break
        assert_follow_up_response(last)
        utt = last["quick_options"][0]
    assert_completed_response(last)


@REGISTRY.register("T3-10", tier=3, features=[7, 13, 14, 15], title="Semantic cache hit metrics + Emergency120 cache bypass")
def test_t3_10(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_10")
    q = {"query": "鼻塞 流涕", "top_k": 3}
    assert ctx.client.post("/v1/knowledge/search", json_body=q, headers=hdrs).status == 200
    assert ctx.client.post("/v1/knowledge/search", json_body=q, headers=hdrs).status == 200
    assert "medical_server_cache_hits_total" in ctx.client.get("/metrics").text
    r_emg = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_10_e", "patient_id": pid, "utterance": "胸痛伴大汗和左臂放射痛", "stream": False},
        headers=hdrs,
    )
    assert r_emg.status == 200
    assert_emergency_response(r_emg.json())


@REGISTRY.register("T3-11", tier=3, features=[2, 7, 14, 18], title="EpidemicContext seasonal boost across /epidemic/current, /search & /triage")
def test_t3_11(ctx: TestContext) -> None:
    r_ep = ctx.client.get("/v1/epidemic/current?region_code=CN-110000&season=winter")
    assert r_ep.status == 200
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_11")
    r_s = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "高热 全身肌肉酸痛 干咳", "region_code": "CN-110000", "seasonal_boost": True, "top_k": 5},
        headers=hdrs,
    )
    assert r_s.status == 200
    r_t = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_11", "patient_id": pid, "region_code": "CN-110000", "utterance": "高热全身肌肉酸痛干咳", "stream": False},
        headers=hdrs,
    )
    assert r_t.status == 200 and "seasonal_context_applied" in r_t.json()


@REGISTRY.register("T3-12", tier=3, features=[7, 14, 18, 19], title="Live /v1/admin/reload hot-swap without restart")
def test_t3_12(ctx: TestContext) -> None:
    assert ctx.client.get("/health").status == 200
    hdrs = {"Authorization": f"Bearer {ctx.admin_token}"} if ctx.admin_token else ctx.auth_headers_or_wechat("wx_t3_12")[0]
    r_rel = ctx.client.post(
        "/v1/admin/reload",
        json_body={"reload_epidemics": True, "reload_dictionaries": True, "reload_router": True},
        headers=hdrs,
    )
    assert r_rel.status == 200
    assert ctx.client.get("/v1/epidemic/current?region_code=CN-110000&season=winter").status == 200


@REGISTRY.register("T3-13", tier=3, features=[7, 19, 20], title="Colloquial synonym normalization shared across Studio Mod4/Mod5 & /triage")
def test_t3_13(ctx: TestContext) -> None:
    assert_studio_commands(ctx, ["test_dictionary_sandbox", "run_ecs_whitebox_triage"])
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_13")
    r = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_13", "patient_id": pid, "utterance": "心口窝疼两天", "stream": False},
        headers=hdrs,
    )
    assert r.status == 200


@REGISTRY.register("T3-14", tier=3, features=[7, 14, 20], title="TriageTrace whitebox contract parity with /v1/triage/chat")
def test_t3_14(ctx: TestContext) -> None:
    core_rs = _read_dir_rs(ctx.workspace / "crates" / "medical-core" / "src")
    assert "TriageTrace" in core_rs and "ExtractedSymptoms" in core_rs
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_14")
    r = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_14", "patient_id": pid, "utterance": "咳嗽三天", "stream": False},
        headers=hdrs,
    )
    assert r.status == 200 and "red_flag" in r.json()


@REGISTRY.register("T3-15", tier=3, features=[6, 12, 14, 21], title="Studio Mod6 gateway commands + API key auth & throttling")
def test_t3_15(ctx: TestContext) -> None:
    assert_studio_commands(
        ctx, ["issue_api_key", "revoke_api_key", "test_llm_provider_connectivity", "get_gateway_metrics"]
    )
    r_ok = ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "top_k": 1}, bearer=ctx.tenant_key("hosp_a"))
    r_rev = ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "top_k": 1}, bearer=ctx.tenant_key("revoked"))
    assert r_ok.status == 200 and r_rev.status in (401, 403)


@REGISTRY.register("T3-16", tier=3, features=[12, 13, 14, 22], title="Authenticated SSE streaming triage & Miniapp SSE consumption")
def test_t3_16(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_16")
    sse = ctx.client.post_sse(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_16", "patient_id": pid, "utterance": "咽痛咳嗽两天", "stream": True},
        headers=hdrs,
    )
    assert sse.status == 200 and "text/event-stream" in sse.headers.get("content-type", "")
    assert any(e.event == "result" for e in sse.events)
    api_js = (ctx.workspace / "miniapp" / "utils" / "api.js").read_text(encoding="utf-8")
    assert "/v1/triage/chat" in api_js


@REGISTRY.register("T3-17", tier=3, features=[1, 5, 15], title="Workspace build config + R5 offline health + Dockerfile parity")
def test_t3_17(ctx: TestContext) -> None:
    ws = parse_toml_file(ctx.workspace / "Cargo.toml")
    assert len(ws["workspace"]["members"]) == 4
    df = parse_dockerfile((ctx.workspace / "deploy" / "Dockerfile").read_text(encoding="utf-8"))
    assert len(df.stages) >= 2
    assert ctx.client.get("/health").status == 200


@REGISTRY.register("T3-18", tier=3, features=[3, 11, 16], title="ingestion_checkpoints DDL + pipeline checkpoint.rs + Studio Mod1 resume")
def test_t3_18(ctx: TestContext) -> None:
    sql_info = parse_sql_ddl((ctx.workspace / "migrations" / "001_init.sql").read_text(encoding="utf-8"))
    assert any("checkpoint" in t for t in sql_info.tables)
    assert (ctx.workspace / "crates" / "medical-pipeline" / "src" / "checkpoint.rs").is_file()
    assert_studio_commands(ctx, ["resume_knowledge_import", "export_import_errors"])


@REGISTRY.register("T3-19", tier=3, features=[3, 12, 14], title="Tenant namespace isolation across DDL, Qdrant & live search")
def test_t3_19(ctx: TestContext) -> None:
    q = parse_json_file(ctx.workspace / "config" / "qdrant" / "collections.json")
    assert validate_qdrant_collections(q)
    r_a = ctx.client.post("/v1/knowledge/search", json_body={"query": "高热 咳嗽", "top_k": 10}, bearer=ctx.tenant_key("hosp_a"))
    r_b = ctx.client.post("/v1/knowledge/search", json_body={"query": "高热 咳嗽", "top_k": 10}, bearer=ctx.tenant_key("hosp_b"))
    assert r_a.status == 200 and r_b.status == 200, (
        f"Tenant knowledge search failed: hosp_a={r_a.status}, hosp_b={r_b.status}"
    )
    a_results = r_a.json().get("results", [])
    b_results = r_b.json().get("results", [])
    assert len(a_results) >= 1 and len(b_results) >= 1
    a_ids = {item.get("doc_id") for item in a_results}
    b_ids = {item.get("doc_id") for item in b_results}
    a_blob = json.dumps(a_results, ensure_ascii=False)
    b_blob = json.dumps(b_results, ensure_ascii=False)
    if ctx.mock_upstream is not None:
        assert "hosp_a_private_doc_01" in a_ids and "hosp_b_private_doc_01" in b_ids
    assert "hosp_a_private_doc_01" not in b_ids and "hosp_a_private" not in b_blob
    assert "hosp_b_private_doc_01" not in a_ids and "hosp_b_private" not in a_blob
    for item in b_results:
        assert item.get("tenant_id") != "hosp_a" and item.get("tenant_namespace") != "hosp_a"
    for item in a_results:
        assert item.get("tenant_id") != "hosp_b" and item.get("tenant_namespace") != "hosp_b"


@REGISTRY.register("T3-20", tier=3, features=[14, 15, 22], title="Miniapp API endpoints & response contract parity with live server")
def test_t3_20(ctx: TestContext) -> None:
    api_js = (ctx.workspace / "miniapp" / "utils" / "api.js").read_text(encoding="utf-8")
    for ep in ("/v1/auth/wechat", "/v1/triage/chat", "/v1/patients/"):
        assert ep in api_js
    assert ctx.client.post("/v1/auth/wechat", json_body={"code": "wx_t3_20"}).status == 200


@REGISTRY.register("T3-21", tier=3, features=[1, 16, 17, 18, 19, 20, 21], title="MedicalStudio 6-module Rust/Frontend command parity & dist bundle")
def test_t3_21(ctx: TestContext) -> None:
    studio = ctx.workspace / "crates" / "medical-studio"
    conf = parse_json_file(studio / "tauri.conf.json")
    assert (studio / conf["build"]["frontendDist"] / "index.html").is_file()
    validate_ico_bytes((studio / "icons" / "icon.ico").read_bytes())
    mods = load_schema_rules()["triage_invariants"]["required_studio_commands"]
    all_cmds = [cmd for mod_cmds in mods.values() for cmd in mod_cmds]
    assert_studio_commands(ctx, all_cmds)


@REGISTRY.register("T3-22", tier=3, features=[14, 15], title="k6/triage_load_test.js target endpoint replay against live server")
def test_t3_22(ctx: TestContext) -> None:
    k6_txt = (ctx.workspace / "k6" / "triage_load_test.js").read_text(encoding="utf-8")
    info = validate_k6_script(k6_txt)
    assert "/v1/triage/chat" in info["endpoints"]
    assert ctx.client.get("/health").status == 200
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_22")
    assert ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_k6_replay", "patient_id": pid, "utterance": "发热咳嗽", "stream": False},
        headers=hdrs,
    ).status == 200


@REGISTRY.register("T3-23", tier=3, features=[3, 14, 15], title="Prometheus alerts + Grafana dashboard alignment with live /metrics")
def test_t3_23(ctx: TestContext) -> None:
    alerts_txt = (ctx.workspace / "deploy" / "prometheus" / "alerts.yml").read_text(encoding="utf-8")
    parse_yaml_subset(alerts_txt)
    dash = parse_json_file(ctx.workspace / "deploy" / "grafana" / "dashboards" / "medical-server.json")
    validate_grafana_dashboard(dash)
    live = parse_prometheus_metrics(ctx.client.get("/metrics").text)
    refs = set(re.findall(r"\b(medical_server_[a-zA-Z0-9_]+)\b", alerts_txt + json.dumps(dash)))
    for ref in refs:
        base = re.sub(r"_(?:bucket|sum|count|total)$", "", ref)
        assert any(k == ref or k.startswith(base) for k in live), f"Metric {ref} missing from /metrics"


@REGISTRY.register("T3-24", tier=3, features=[2, 4, 7, 14], title="Pregnant target_population filter & allergy-safe triage")
def test_t3_24(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_24_preg")
    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid, "consent_long_term_memory": True,
            "l2_profile": {"age_group": "30-39", "gender": "female", "is_pregnant": True,
                           "chronic_diseases": ["轻度哮喘"], "allergies": ["青霉素"],
                           "surgical_history": [], "long_term_medications": []},
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    r_s = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "发热 咳嗽", "target_population": ["pregnant"], "top_k": 5},
        headers=hdrs,
    )
    assert r_s.status == 200
    for item in r_s.json().get("results", []):
        pops = item.get("target_population")
        if isinstance(pops, list):
            assert "pregnant" in pops
    r_t = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t3_24", "patient_id": pid, "utterance": "低烧伴咳嗽", "target_population": ["pregnant"], "stream": False},
        headers=hdrs,
    )
    assert r_t.status == 200
    assert_no_diagnosis_or_dosage(r_t.text)


@REGISTRY.register("T3-25", tier=3, features=[5, 6, 13, 14], title="R5 fallback + Fast-tier failure resilience")
def test_t3_25(ctx: TestContext) -> None:
    assert ctx.client.get("/health").status == 200
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_25")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault(
            "/v1/chat/completions",
            FaultRule(status=500, match_prompt_contains="Query改写"),
        )
    try:
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_t3_25", "patient_id": pid, "utterance": "喉咙痛咳嗽", "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        d = r.json()
        if d.get("status") == "follow_up":
            assert_follow_up_response(d)
        elif d.get("status") == "completed":
            assert_completed_response(d)
        else:
            raise AssertionError(f"Unexpected status in T3-25: {d!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("T3-26", tier=3, features=[2, 7, 14, 22], title="Consent gating (false -> no L3 write; true -> L3 consolidated)")
def test_t3_26(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_26")
    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid, "consent_long_term_memory": False,
            "l2_profile": {"age_group": "30-39", "gender": "male", "is_pregnant": False,
                           "chronic_diseases": [], "allergies": [], "surgical_history": [], "long_term_medications": []},
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    for u in ("咽痛两天", "今天刚开始", "无", "咽痛"):
        r = ctx.client.post("/v1/triage/chat", json_body={"session_id": "s_no_c", "patient_id": pid, "utterance": u}, headers=hdrs)
        assert r.status == 200
        if r.json().get("status") == "completed":
            break
    time.sleep(0.25)
    g1 = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    if g1.status == 200:
        assert len(g1.json().get("l3_visit_events", [])) == 0

    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid, "consent_long_term_memory": True,
            "l2_profile": {"age_group": "30-39", "gender": "male", "is_pregnant": False,
                           "chronic_diseases": [], "allergies": [], "surgical_history": [], "long_term_medications": []},
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    for u in ("咽痛咳嗽两天", "2~3天", "没有明确接触", "呼吸内科"):
        r = ctx.client.post("/v1/triage/chat", json_body={"session_id": "s_yes_c", "patient_id": pid, "utterance": u}, headers=hdrs)
        assert r.status == 200
        if r.json().get("status") == "completed":
            break
    deadline = time.monotonic() + 4.0
    while True:
        g2 = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
        if g2.status == 200 and len(g2.json().get("l3_visit_events", [])) >= 1:
            break
        if time.monotonic() >= deadline:
            raise AssertionError(
                f"Timed out after 4.0s waiting for consented L3 consolidation in T3-26: status={g2.status}, body={g2.text}"
            )
        time.sleep(0.1)


@REGISTRY.register("T3-27", tier=3, features=[6, 14, 21], title="ModelRouter Fast/Reasoning config + oversized/malformed LLM repair")
def test_t3_27(ctx: TestContext) -> None:
    assert_studio_commands(ctx, ["get_router_config", "save_router_config"])
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_27")
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault(
            "/v1/chat/completions",
            FaultRule(mode="oversized_question", match_prompt_contains="预检分诊"),
        )
    try:
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_t3_27", "patient_id": pid, "utterance": "咳嗽发热", "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        d = r.json()
        if d.get("status") == "follow_up":
            assert_follow_up_response(d)
        elif d.get("status") == "completed":
            assert_completed_response(d)
        else:
            raise AssertionError(f"Unexpected status in T3-27: {d!r}")
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register("T3-28", tier=3, features=[3, 7, 12, 14], title="Audit log DDL + Normal/Emergency/Fallback health stability")
def test_t3_28(ctx: TestContext) -> None:
    sql_info = parse_sql_ddl((ctx.workspace / "migrations" / "001_init.sql").read_text(encoding="utf-8"))
    assert any("audit" in t for t in sql_info.tables)
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t3_28")
    assert ctx.client.post("/v1/triage/chat", json_body={"session_id": "s_norm", "patient_id": pid, "utterance": "头晕一天"}, headers=hdrs).status == 200
    assert ctx.client.post("/v1/triage/chat", json_body={"session_id": "s_emg", "patient_id": pid, "utterance": "胸痛伴大汗左臂放射痛"}, headers=hdrs).status == 200
    assert ctx.client.get("/health").status == 200

"""Tier 4 Real-World Hospital Clinical & Operations E2E Scenarios: T4-01..T4-11 (11 scenarios)."""

from __future__ import annotations

import json
import re
import tempfile
from concurrent.futures import ThreadPoolExecutor
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
)
from e2e.lib.registry import REGISTRY
from e2e.lib.runner import TestContext


@REGISTRY.register(
    "T4-01",
    tier=4,
    features=[2, 3, 5, 6, 7, 11, 12, 13, 14, 15, 18, 22],
    title="Winter influenza peak outpatient triage (WeChat -> epidemic -> SSE multi-turn -> completion)",
)
def test_t4_01(ctx: TestContext) -> None:
    chat_wxml = discover_miniapp_page_text(ctx.workspace, ".wxml", ("chat", "triage", "index"))
    assert "consent" in chat_wxml.lower() or "免责" in chat_wxml or "同意" in chat_wxml
    token, pid = ctx.get_wechat_auth("wx_test_flu_01")
    r_ep = ctx.client.get("/v1/epidemic/current?region_code=CN-110000&season=winter")
    assert r_ep.status == 200 and isinstance(r_ep.json().get("contexts"), list)

    payload = parse_json_file(ctx.fixtures_dir / "payloads" / "triage_turns.json")["winter_flu_peak"]
    sid = "sess_t4_01_flu"
    last_payload: dict[str, Any] = {}
    for idx, utt in enumerate(payload["turns"], start=1):
        if idx == 1:
            sse = ctx.client.post_sse(
                "/v1/triage/chat",
                json_body={
                    "session_id": sid, "patient_id": pid, "utterance": utt,
                    "region_code": payload["region_code"], "target_population": payload["target_population"],
                    "stream": True,
                },
                bearer=token,
            )
            assert sse.status == 200
            res_evs = [e for e in sse.events if e.event == "result"]
            assert res_evs, "Turn 1 SSE missing result frame"
            last_payload = res_evs[-1].json()
        else:
            r = ctx.client.post(
                "/v1/triage/chat",
                json_body={
                    "session_id": sid, "patient_id": pid, "utterance": utt,
                    "region_code": payload["region_code"], "target_population": payload["target_population"],
                    "stream": False,
                },
                bearer=token,
            )
            assert r.status == 200
            last_payload = r.json()
        if last_payload.get("status") == "completed":
            break
        assert_follow_up_response(last_payload)
        utt = last_payload["quick_options"][0]

    # Ensure convergence if not yet completed
    if last_payload.get("status") != "completed":
        r_final = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": sid, "patient_id": pid, "utterance": "超过3天", "region_code": "CN-110000", "stream": False},
            bearer=token,
        )
        assert r_final.status == 200
        last_payload = r_final.json()

    assert_completed_response(last_payload, require_citations=True)
    m = parse_prometheus_metrics(ctx.client.get("/metrics").text)
    assert "medical_server_requests_total" in m


@REGISTRY.register(
    "T4-02",
    tier=4,
    features=[4, 6, 7, 12, 14, 19, 22],
    title="Acute chest pain Emergency120 red-line interception (Miniapp + HIS OpenAI proxy, 0 LLM calls)",
)
def test_t4_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t4_02_chest")
    utt = parse_json_file(ctx.fixtures_dir / "payloads" / "triage_turns.json")["emergency_chest_pain_colloquial"]["utterance"]
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()

    r_json = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t4_02_j", "patient_id": pid, "utterance": utt, "stream": False},
        headers=hdrs,
    )
    assert r_json.status == 200
    assert_emergency_response(r_json.json())

    r_sse = ctx.client.post_sse(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t4_02_s", "patient_id": pid, "utterance": utt, "stream": True},
        headers=hdrs,
    )
    assert r_sse.status == 200 and any("Emergency120" in e.data for e in r_sse.events)

    r_oai = ctx.client.post(
        "/v1/openai/chat/completions",
        json_body={"model": "medical-triage", "messages": [{"role": "user", "content": utt}]},
        headers=hdrs,
    )
    assert r_oai.status == 200 and ("120" in r_oai.text or "急诊" in r_oai.text)

    if ctx.mock_upstream is not None:
        assert len(ctx.mock_upstream.get_calls("/v1/chat/completions")) == 0
    chat_js = discover_miniapp_page_text(ctx.workspace, ".js", ("chat", "triage", "index"))
    assert "makePhoneCall" in chat_js and "120" in chat_js


@REGISTRY.register(
    "T4-03",
    tier=4,
    features=[2, 3, 4, 6, 7, 11, 13, 14, 22],
    title="Pregnant patient repeat visit with chronic allergy & L2/L3 recall",
)
def test_t4_03(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t4_03_preg")
    fx = parse_json_file(ctx.fixtures_dir / "payloads" / "triage_turns.json")["pregnant_allergy_repeat"]
    rec = dict(fx["patient_profile"], patient_id=pid)
    assert ctx.client.put(f"/v1/patients/{pid}/memory", json_body=rec, headers=hdrs).status in (200, 201)
    assert "青霉素" in ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs).text

    r_search = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "胸闷气短 低烧", "target_population": ["pregnant"], "top_k": 5},
        headers=hdrs,
    )
    assert r_search.status == 200
    for hit in r_search.json().get("results", []):
        pops = hit.get("target_population")
        if isinstance(pops, list):
            assert "pregnant" in pops

    sid = "sess_t4_03_preg"
    utt = fx["utterance"]
    last: dict[str, Any] = {}
    for _ in range(4):
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": sid, "patient_id": pid, "utterance": utt, "target_population": ["pregnant"], "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        last = r.json()
        if last.get("status") == "completed":
            break
        assert_follow_up_response(last)
        utt = last["quick_options"][0]
    assert_completed_response(last)
    assert_no_diagnosis_or_dosage(json.dumps(last, ensure_ascii=False))
    assert "青霉素" in ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs).text


@REGISTRY.register(
    "T4-04",
    tier=4,
    features=[5, 6, 7, 11, 14, 15, 21],
    title="Upstream cloud LLM outage & multi-tier failover to fallback_rule_triage_system",
)
def test_t4_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t4_04_outage")
    m_before = parse_prometheus_metrics(ctx.client.get("/metrics").text)
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.set_persistent_fault("/v1/chat/completions", FaultRule(status=503))
    try:
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_t4_04", "patient_id": pid, "utterance": "发烧38.5度，咳嗽黄痰两天", "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        assert_completed_response(r.json())
        r_emg = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "sess_t4_04_emg", "patient_id": pid, "utterance": "胸痛伴大汗左臂放射痛", "stream": False},
            headers=hdrs,
        )
        assert r_emg.status == 200
        assert_emergency_response(r_emg.json())
        m_after = parse_prometheus_metrics(ctx.client.get("/metrics").text)
        assert "medical_server_llm_errors_total" in m_after
    finally:
        if ctx.mock_upstream is not None:
            ctx.mock_upstream.reset_faults()


@REGISTRY.register(
    "T4-05",
    tier=4,
    features=[3, 4, 7, 12, 14, 22],
    title="Patient privacy audit, outbound PII scrubbing & one-click GDPR/PIPL wipe",
)
def test_t4_05(ctx: TestContext) -> None:
    tok_a, pid_a = ctx.get_wechat_auth("wx_t4_05_a")
    tok_b, _ = ctx.get_wechat_auth("wx_t4_05_b")
    ctx.client.put(
        f"/v1/patients/{pid_a}/memory",
        json_body={
            "patient_id": pid_a, "consent_long_term_memory": True,
            "l2_profile": {"age_group": "30-39", "gender": "female", "is_pregnant": False,
                           "chronic_diseases": ["GDPR抹除测试疾病"], "allergies": ["头孢"],
                           "surgical_history": [], "long_term_medications": []},
            "l3_visit_events": [],
        },
        bearer=tok_a,
    )
    assert ctx.client.get(f"/v1/patients/{pid_a}/memory", bearer=tok_b).status in (401, 403)
    assert ctx.client.delete(f"/v1/patients/{pid_a}/memory", bearer=tok_b).status in (401, 403)

    pii_fx = parse_json_file(ctx.fixtures_dir / "payloads" / "triage_turns.json")["pii_mixed_utterance"]
    r_chat = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t4_05", "patient_id": pid_a, "utterance": pii_fx["utterance"], "stream": False},
        bearer=tok_a,
    )
    assert r_chat.status == 200
    assert_no_pii(r_chat.text, extra_forbidden=pii_fx["forbidden_echo"])

    assert ctx.client.delete(f"/v1/patients/{pid_a}/memory", bearer=tok_a).status in (200, 204)
    g_after = ctx.client.get(f"/v1/patients/{pid_a}/memory", bearer=tok_a)
    assert g_after.status in (200, 404)
    if g_after.status == 200:
        assert "GDPR抹除测试疾病" not in g_after.text


@REGISTRY.register(
    "T4-06",
    tier=4,
    features=[1, 2, 3, 4, 8, 9, 10, 11, 14, 16, 17],
    title="Hospital textbook + Excel patient batch import -> privacy firewall -> hybrid physician search",
)
def test_t4_06(ctx: TestContext) -> None:
    with tempfile.TemporaryDirectory() as td:
        p = Path(td)
        assert len(read_xlsx_rows(generate_minimal_xlsx(p / "pat.xlsx"))) >= 3
        assert read_docx_paragraphs(generate_minimal_docx(p / "book.docx"))
        assert generate_minimal_pdf(p / "book.pdf", ["Medical Book"]).is_file()
        assert len(parse_jsonl_stream(generate_large_jsonl(p / "k.jsonl", count=200)).records) == 200
    bundle = parse_json_file(ctx.fixtures_dir / "knowledge" / "valid_collection.json")
    assert not validate_collection_bundle(bundle, load_schema_rules())
    hdrs, _ = ctx.auth_headers_or_wechat("wx_t4_06")
    r_exact = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "流行性感冒", "target_population": ["adult"], "department": "呼吸内科", "seasonal_boost": True, "top_k": 5},
        headers=hdrs,
    )
    r_colloq = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "心口窝疼", "top_k": 5},
        headers=hdrs,
    )
    assert r_exact.status == 200 and r_colloq.status == 200
    assert len(r_exact.json().get("results", [])) >= 1


@REGISTRY.register(
    "T4-07",
    tier=4,
    features=[3, 6, 12, 14, 15, 21],
    title="Multi-tenant HIS & self-service kiosk API gateway throttling & namespace isolation",
)
def test_t4_07(ctx: TestContext) -> None:
    key_a = ctx.tenant_key("hosp_a")
    key_b = ctx.tenant_key("qpm_test_t4_07")
    key_rev = ctx.tenant_key("revoked")

    assert ctx.client.post("/v1/knowledge/search", json_body={"query": "发热"}, bearer=key_rev).status in (401, 403)
    assert ctx.client.post("/v1/knowledge/search", json_body={"query": "发热"}, bearer=key_a).status == 200

    def burst_b(_: int) -> int:
        return ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "top_k": 1}, bearer=key_b).status

    with ThreadPoolExecutor(max_workers=4) as ex:
        b_codes = list(ex.map(burst_b, range(8)))
    assert all(c in (200, 429) for c in b_codes), f"Unexpected qpm_test_t4_07 status codes: {b_codes}"
    assert 429 in b_codes, f"Expected 429 rate-limit under 8-request burst for qpm=3 key: {b_codes}"
    assert 1 <= b_codes.count(200) <= 3, f"QPM=3 bucket allowed {b_codes.count(200)} (>3) requests: {b_codes}"
    assert ctx.client.post("/v1/knowledge/search", json_body={"query": "发热"}, bearer=key_a).status == 200
    assert ctx.client.post(
        "/v1/openai/chat/completions",
        json_body={"model": "medical-triage", "messages": [{"role": "user", "content": "发热两天"}]},
        bearer=key_a,
    ).status == 200


@REGISTRY.register(
    "T4-08",
    tier=4,
    features=[3, 7, 12, 14, 18, 19, 20, 22],
    title="MedicalStudio live epidemic & dictionary hot-reload affecting Miniapp triage without restart",
)
def test_t4_08(ctx: TestContext) -> None:
    assert ctx.client.get("/health").status == 200
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t4_08")
    admin_hdrs = {"Authorization": f"Bearer {ctx.admin_token}"} if ctx.admin_token else hdrs
    reload_body = parse_json_file(ctx.fixtures_dir / "payloads" / "epidemic_hotreload.json")["reload_request"]
    r_rel = ctx.client.post("/v1/admin/reload", json_body=reload_body, headers=admin_hdrs)
    assert r_rel.status == 200
    assert ctx.client.get("/v1/epidemic/current?region_code=CN-110000&season=winter").status == 200
    r_t = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_t4_08", "patient_id": pid, "utterance": "儿童持续干咳伴发热4天", "target_population": ["child"], "stream": False},
        headers=hdrs,
    )
    assert r_t.status == 200


@REGISTRY.register(
    "T4-09",
    tier=4,
    features=[2, 7, 11, 13, 14, 18, 22],
    title="Pediatric high fever & mycoplasma seasonal triage (target_population: child, max_turns<=4)",
)
def test_t4_09(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_t4_09_ped")
    fx = parse_json_file(ctx.fixtures_dir / "payloads" / "triage_turns.json")["pediatric_mycoplasma_fever"]
    r_s = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "持续干咳 发热", "target_population": ["child"], "seasonal_boost": True, "region_code": fx["region_code"], "top_k": 5},
        headers=hdrs,
    )
    assert r_s.status == 200
    sid = "sess_t4_09_child"
    utt = fx["utterance"]
    last: dict[str, Any] = {}
    for _ in range(4):
        r = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": sid, "patient_id": pid, "utterance": utt, "target_population": ["child"], "region_code": fx["region_code"], "stream": False},
            headers=hdrs,
        )
        assert r.status == 200
        last = r.json()
        if last.get("status") == "completed":
            break
        assert_follow_up_response(last)
        utt = last["quick_options"][0]
    assert_completed_response(last)
    assert_no_diagnosis_or_dosage(json.dumps(last, ensure_ascii=False))


@REGISTRY.register(
    "T4-10",
    tier=4,
    features=[1, 16, 17, 18, 19, 20, 21],
    title="MedicalStudio 6-module Tauri command parity, frontend dist & whitebox trace alignment",
)
def test_t4_10(ctx: TestContext) -> None:
    studio = ctx.workspace / "crates" / "medical-studio"
    conf = parse_json_file(studio / "tauri.conf.json")
    dist_idx = (studio / conf["build"]["frontendDist"] / "index.html").resolve()
    assert dist_idx.is_file() and "<script" in dist_idx.read_text(encoding="utf-8")
    validate_ico_bytes((studio / "icons" / "icon.ico").read_bytes())

    mods = load_schema_rules()["triage_invariants"]["required_studio_commands"]
    all_28_cmds = [cmd for mod_cmds in mods.values() for cmd in mod_cmds]
    rs_all, fe_all = assert_studio_commands(ctx, all_28_cmds)
    for comp in ("ExtractedSymptoms", "RedFlagAlert", "RagEvidence", "TriageSlots"):
        assert comp in rs_all and comp in fe_all, f"Missing Studio ECS component {comp!r} in Rust or frontend"


@REGISTRY.register(
    "T4-11",
    tier=4,
    features=[1, 3, 12, 14, 15],
    title="Hospital morning peak observability & k6/Prometheus/Grafana/Dockerfile audit",
)
def test_t4_11(ctx: TestContext) -> None:
    df = parse_dockerfile((ctx.workspace / "deploy" / "Dockerfile").read_text(encoding="utf-8"))
    assert len(df.stages) >= 2 and df.users and df.exposes
    k6 = validate_k6_script((ctx.workspace / "k6" / "triage_load_test.js").read_text(encoding="utf-8"))
    assert k6["has_thresholds"] and k6["uses_env"] and k6["uses_sleep"]
    prom = parse_yaml_subset((ctx.workspace / "deploy" / "prometheus" / "prometheus.yml").read_text(encoding="utf-8"))
    alerts_txt = (ctx.workspace / "deploy" / "prometheus" / "alerts.yml").read_text(encoding="utf-8")
    alerts = parse_yaml_subset(alerts_txt)
    assert prom and alerts.get("groups")
    dash = parse_json_file(ctx.workspace / "deploy" / "grafana" / "dashboards" / "medical-server.json")
    validate_grafana_dashboard(dash)
    live_metrics = parse_prometheus_metrics(ctx.client.get("/metrics").text)
    for req in load_schema_rules()["triage_invariants"]["required_prometheus_metrics"]:
        assert any(k == req or k.startswith(req + "_") for k in live_metrics)

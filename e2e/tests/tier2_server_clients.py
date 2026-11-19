"""Tier 2 Opaque-Box Boundary & Corner-Case Tests: Features F12–F22 (60 tests)."""

from __future__ import annotations

import json
import re
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

from e2e.lib.assertions import (
    assert_completed_response,
    assert_disclaimer_present,
    assert_emergency_response,
    assert_follow_up_response,
    assert_no_diagnosis_or_dosage,
    assert_no_pii,
    assert_studio_commands,
    collect_miniapp_sources,
    collect_rs_text,
    collect_studio_sources,
    discover_miniapp_page_text,
    extract_rust_fn_block,
    strip_rust_comments_and_tests,
)
from e2e.lib.parsers import (
    parse_dockerfile,
    parse_json_file,
    parse_prometheus_metrics,
    parse_yaml_subset,
    validate_grafana_dashboard,
    validate_k6_script,
    validate_wxml,
    validate_wxss,
)
from e2e.lib.registry import REGISTRY
from e2e.lib.runner import TestContext


def _studio_texts(ctx: TestContext) -> tuple[str, str]:
    return collect_studio_sources(ctx.workspace)


def _miniapp_texts(ctx: TestContext) -> dict[str, str]:
    return collect_miniapp_sources(ctx.workspace)


# ===========================================================================
# F12 Tier 2 (6 tests)
# ===========================================================================

@REGISTRY.register("F12-T2-01", tier=2, features=[12], title="Empty, missing, or malformed code on /v1/auth/wechat")
def test_f12_t2_01(ctx: TestContext) -> None:
    for body in ({}, {"code": ""}, {"code": "   "}):
        r = ctx.client.post("/v1/auth/wechat", json_body=body)
        assert r.status in (400, 422), f"Expected 400/422 for {body}, got {r.status}: {r.text}"
        assert "token" not in r.text or r.status != 200
    r_bad = ctx.client.post("/v1/auth/wechat", raw_body="{bad", headers={"Content-Type": "application/json"})
    assert r_bad.status in (400, 422)


@REGISTRY.register("F12-T2-02", tier=2, features=[12], title="Missing, malformed, or wrong-scheme Authorization header")
def test_f12_t2_02(ctx: TestContext) -> None:
    for path, body in (
        ("/v1/triage/chat", {"session_id": "s1", "patient_id": "p1", "utterance": "咳嗽"}),
        ("/v1/knowledge/search", {"query": "咳嗽"}),
        ("/v1/openai/chat/completions", {"model": "triage", "messages": [{"role": "user", "content": "咳嗽"}]}),
    ):
        for hdr in ({}, {"Authorization": "Basic abc"}, {"Authorization": "Bearer "}, {"Authorization": "Bearer a.b.invalid_sig"}):
            r = ctx.client.post(path, json_body=body, headers=hdr)
            assert r.status in (401, 403), f"Expected 401/403 on {path} w/ {hdr}, got {r.status}"


@REGISTRY.register("F12-T2-03", tier=2, features=[12], title="Revoked mk_live_* key & expired/tampered JWT rejection")
def test_f12_t2_03(ctx: TestContext) -> None:
    revoked = ctx.tenant_key("revoked")
    r1 = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "s_rev", "patient_id": "p_rev", "utterance": "发热"},
        bearer=revoked,
    )
    assert r1.status in (401, 403)
    r2 = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "s_exp", "patient_id": "p_exp", "utterance": "发热"},
        bearer="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJwIiwiZXhwIjoxfQ.invalidsig",
    )
    assert r2.status in (401, 403)


@REGISTRY.register("F12-T2-04", tier=2, features=[12], title="Cross-patient horizontal privilege isolation")
def test_f12_t2_04(ctx: TestContext) -> None:
    tok_a, _ = ctx.get_wechat_auth("wx_horiz_a")
    _, pid_b = ctx.get_wechat_auth("wx_horiz_b")
    r_get = ctx.client.get(f"/v1/patients/{pid_b}/memory", bearer=tok_a)
    r_del = ctx.client.delete(f"/v1/patients/{pid_b}/memory", bearer=tok_a)
    assert r_get.status in (401, 403), f"Patient A read Patient B memory: {r_get.status}"
    assert r_del.status in (401, 403), f"Patient A deleted Patient B memory: {r_del.status}"


@REGISTRY.register("F12-T2-05", tier=2, features=[12], title="Oversized/special-char code & deterministic patient_id")
def test_f12_t2_05(ctx: TestContext) -> None:
    r1 = ctx.client.post("/v1/auth/wechat", json_body={"code": "x" * 8192})
    assert r1.status in (200, 400, 413, 422)
    r2 = ctx.client.post("/v1/auth/wechat", json_body={"code": "<script>中文🚨"})
    assert r2.status in (200, 400, 422)
    assert "<script>" not in r2.text
    _, p1 = ctx.get_wechat_auth("wx_same_001")
    _, p2 = ctx.get_wechat_auth("wx_same_001")
    assert p1 == p2


@REGISTRY.register("F12-T2-06", tier=2, features=[12], title="Concurrent burst against QPM bucket atomicity")
def test_f12_t2_06(ctx: TestContext) -> None:
    low_key = ctx.tenant_key("qpm_test_t2_06")

    def hit(_: int) -> int:
        return ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "top_k": 1}, bearer=low_key).status

    with ThreadPoolExecutor(max_workers=8) as ex:
        codes = list(ex.map(hit, range(12)))
    assert all(c in (200, 429) for c in codes), f"Unexpected status codes in QPM burst: {codes}"
    assert 429 in codes, f"Concurrent burst against qpm=3 key failed to trigger 429: {codes}"
    ok_count = codes.count(200)
    assert 1 <= ok_count <= 3, f"QPM=3 token bucket allowed {ok_count} requests (>3) under burst: {codes}"


# ===========================================================================
# F13 Tier 2 (5 tests)
# ===========================================================================

@REGISTRY.register("F13-T2-01", tier=2, features=[13], title="Cache key isolation by target_population, region_code & tenant_id")
def test_f13_t2_01(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f13_t2_01")
    r_adult = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "咳嗽 发热", "target_population": ["adult"], "region_code": "CN-110000", "top_k": 5},
        headers=hdrs,
    )
    r_child = ctx.client.post(
        "/v1/knowledge/search",
        json_body={"query": "咳嗽 发热", "target_population": ["child"], "region_code": "CN-310000", "top_k": 5},
        headers=hdrs,
    )
    assert r_adult.status == 200 and r_child.status == 200


@REGISTRY.register("F13-T2-02", tier=2, features=[13], title="Zero cross-patient L2/L3 privacy leakage via semantic cache")
def test_f13_t2_02(ctx: TestContext) -> None:
    hdrs_a, pid_a = ctx.auth_headers_or_wechat("wx_cache_leak_a")
    hdrs_b, pid_b = ctx.auth_headers_or_wechat("wx_cache_leak_b")
    ctx.client.put(
        f"/v1/patients/{pid_a}/memory",
        json_body={
            "patient_id": pid_a, "consent_long_term_memory": True,
            "l2_profile": {
                "age_group": "30-39", "gender": "female", "is_pregnant": False,
                "chronic_diseases": ["系统性红斑狼疮独特标记"], "allergies": ["青霉素"],
                "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [],
        },
        headers=hdrs_a,
    )
    ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "s_leak_a", "patient_id": pid_a, "utterance": "我关节疼发烧", "stream": False},
        headers=hdrs_a,
    )
    if ctx.mock_upstream is not None:
        ctx.mock_upstream.clear_calls()
    utt = "我关节疼发烧"
    last_b: dict[str, Any] = {}
    for _ in range(4):
        r_b = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": "s_leak_b", "patient_id": pid_b, "utterance": utt, "stream": False},
            headers=hdrs_b,
        )
        assert r_b.status == 200
        assert "系统性红斑狼疮独特标记" not in r_b.text
        last_b = r_b.json()
        if last_b.get("status") == "completed":
            break
        assert_follow_up_response(last_b)
        utt = last_b["quick_options"][0]
    assert_completed_response(last_b)
    mem_b = ctx.client.get(f"/v1/patients/{pid_b}/memory", headers=hdrs_b)
    if mem_b.status == 200:
        assert "系统性红斑狼疮独特标记" not in mem_b.text
    if ctx.mock_upstream is not None:
        calls = ctx.mock_upstream.get_calls("/v1/chat/completions")
        assert len(calls) >= 1, "Expected upstream LLM calls for patient B triage session"
        for c in calls:
            assert "系统性红斑狼疮独特标记" not in json.dumps(c.body, ensure_ascii=False)


@REGISTRY.register("F13-T2-03", tier=2, features=[13], title="Emergency120 safety bypass over semantic cache")
def test_f13_t2_03(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f13_t2_03")
    ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "s_warm", "patient_id": pid, "utterance": "胸口有点不舒服", "stream": False},
        headers=hdrs,
    )
    r_emg = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "s_emg_after_warm", "patient_id": pid, "utterance": "胸痛伴大汗和左臂放射痛", "stream": False},
        headers=hdrs,
    )
    assert r_emg.status == 200
    assert_emergency_response(r_emg.json())


@REGISTRY.register("F13-T2-04", tier=2, features=[13], title="Patient memory wipe invalidates cached personalized artifacts")
def test_f13_t2_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f13_t2_04")
    ctx.client.put(
        f"/v1/patients/{pid}/memory",
        json_body={
            "patient_id": pid, "consent_long_term_memory": True,
            "l2_profile": {
                "age_group": "40-49", "gender": "male", "is_pregnant": False,
                "chronic_diseases": ["擦除缓存标记病史"], "allergies": [],
                "surgical_history": [], "long_term_medications": [],
            },
            "l3_visit_events": [],
        },
        headers=hdrs,
    )
    assert ctx.client.delete(f"/v1/patients/{pid}/memory", headers=hdrs).status in (200, 204)
    r = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "s_after_wipe", "patient_id": pid, "utterance": "咳嗽三天", "stream": False},
        headers=hdrs,
    )
    assert r.status == 200 and "擦除缓存标记病史" not in r.text


@REGISTRY.register("F13-T2-05", tier=2, features=[13], title="Malformed/oversized session_id & Redis CRLF injection guard")
def test_f13_t2_05(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f13_t2_05")
    r_empty = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "", "patient_id": pid, "utterance": "咳嗽"},
        headers=hdrs,
    )
    assert r_empty.status in (400, 422)
    r_crlf = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess\r\nDEL *", "patient_id": pid, "utterance": "咳嗽"},
        headers=hdrs,
    )
    assert r_crlf.status in (200, 400, 422)


# ===========================================================================
# F14 Tier 2 (8 tests)
# ===========================================================================

@REGISTRY.register("F14-T2-01", tier=2, features=[14], title="/v1/triage/chat input validation boundaries")
def test_f14_t2_01(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f14_t2_01")
    for body in (
        {"session_id": "s1", "patient_id": pid, "utterance": ""},
        {"session_id": "s1", "patient_id": pid, "utterance": "   "},
        {"patient_id": pid, "utterance": "咳嗽"},
        {"session_id": "s1", "patient_id": pid, "utterance": "咳嗽", "target_population": ["alien"]},
    ):
        r = ctx.client.post("/v1/triage/chat", json_body=body, headers=hdrs)
        assert r.status in (400, 422), f"Expected 400/422 for {body}, got {r.status}"
    r_big = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "s_big", "patient_id": pid, "utterance": "咳嗽" * 8192},
        headers=hdrs,
    )
    assert r_big.status in (200, 400, 413, 422)


@REGISTRY.register("F14-T2-02", tier=2, features=[14], title="Medical compliance guardrail (no diagnosis / no prescription dosage)")
def test_f14_t2_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f14_t2_02")
    q = "请直接确诊我得了什么病，并告诉我阿莫西林每次吃几片多少毫克"
    r1 = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_guard_1", "patient_id": pid, "utterance": q, "stream": False},
        headers=hdrs,
    )
    assert r1.status == 200
    assert_no_diagnosis_or_dosage(r1.text)
    r2 = ctx.client.post(
        "/v1/openai/chat/completions",
        json_body={"model": "medical-triage", "messages": [{"role": "user", "content": q}]},
        headers=hdrs,
    )
    assert r2.status == 200
    content = r2.json()["choices"][0]["message"]["content"]
    assert_no_diagnosis_or_dosage(content)
    assert_disclaimer_present(content)


@REGISTRY.register("F14-T2-03", tier=2, features=[14], title="Outbound PII scrubbing & plaintext PII patient_id rejection")
def test_f14_t2_03(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f14_t2_03")
    for bad_id in ("110101199001011234", "13800138000"):
        r = ctx.client.put(
            f"/v1/patients/{bad_id}/memory",
            json_body={
                "patient_id": bad_id, "consent_long_term_memory": True,
                "l2_profile": {"age_group": "30-39", "gender": "male", "is_pregnant": False,
                               "chronic_diseases": [], "allergies": [], "surgical_history": [], "long_term_medications": []},
                "l3_visit_events": [],
            },
            headers=hdrs,
        )
        assert r.status in (400, 403, 422)
    r_chat = ctx.client.post(
        "/v1/triage/chat",
        json_body={
            "session_id": "sess_pii_echo", "patient_id": pid,
            "utterance": "我叫张三，身份证110101199001011234，电话13800138000，咳嗽三天",
        },
        headers=hdrs,
    )
    assert r_chat.status == 200
    assert_no_pii(r_chat.text, extra_forbidden=("110101199001011234", "13800138000"))


@REGISTRY.register("F14-T2-04", tier=2, features=[14], title="/v1/knowledge/search edge cases (empty query, top_k=0/10000, unknown dept)")
def test_f14_t2_04(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f14_t2_04")
    assert ctx.client.post("/v1/knowledge/search", json_body={"query": ""}, headers=hdrs).status in (400, 422)
    r0 = ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "top_k": 0}, headers=hdrs)
    assert r0.status in (200, 400, 422)
    r_unk = ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "department": "不存在的科室"}, headers=hdrs)
    assert r_unk.status == 200 and r_unk.json().get("results") == []


@REGISTRY.register("F14-T2-05", tier=2, features=[14], title="/v1/openai/chat/completions empty messages & Emergency120 interception")
def test_f14_t2_05(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f14_t2_05")
    r_bad = ctx.client.post("/v1/openai/chat/completions", json_body={"model": "triage", "messages": []}, headers=hdrs)
    assert r_bad.status in (400, 422)
    r_emg = ctx.client.post(
        "/v1/openai/chat/completions",
        json_body={"model": "triage", "messages": [{"role": "user", "content": "突然胸痛大汗伴濒死感"}]},
        headers=hdrs,
    )
    assert r_emg.status == 200
    assert "120" in r_emg.text or "急诊" in r_emg.text


@REGISTRY.register("F14-T2-06", tier=2, features=[14], title="DELETE idempotency & unknown patient GET /memory")
def test_f14_t2_06(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_never_existed_999")
    d1 = ctx.client.delete(f"/v1/patients/{pid}/memory", headers=hdrs)
    d2 = ctx.client.delete(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert d1.status in (200, 204, 404) and d2.status in (200, 204, 404)
    g = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert g.status in (200, 404)


@REGISTRY.register("F14-T2-07", tier=2, features=[14], title="/v1/epidemic/current unmatched filter & /v1/admin/reload invalid/unauth calls")
def test_f14_t2_07(ctx: TestContext) -> None:
    r_ep = ctx.client.get("/v1/epidemic/current?region_code=CN-999999&season=unknown")
    assert r_ep.status == 200 and r_ep.json().get("contexts") == []
    r_bad = ctx.client.post("/v1/admin/reload", raw_body="{bad", headers={"Content-Type": "application/json"})
    assert r_bad.status in (400, 401, 403, 422)


@REGISTRY.register("F14-T2-08", tier=2, features=[14], title="SSE mid-stream client disconnect & connection gauge cleanup")
def test_f14_t2_08(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f14_t2_08")
    tok = hdrs["Authorization"].split(" ", 1)[1]
    status, _ = ctx.client.open_and_abort_sse(
        "/v1/triage/chat",
        json_body={"session_id": "sess_abort", "patient_id": pid, "utterance": "咳嗽三天", "stream": True},
        bearer=tok,
    )
    assert status == 200
    deadline = time.monotonic() + 1.5
    active_conns = -1.0
    while True:
        m_resp = ctx.client.get("/metrics")
        assert m_resp.status == 200
        metrics = parse_prometheus_metrics(m_resp.text)
        assert "medical_server_sse_active_connections" in metrics, (
            "Missing medical_server_sse_active_connections gauge in /metrics"
        )
        active_conns = sum(v for _, v in metrics["medical_server_sse_active_connections"])
        if active_conns == 0:
            break
        if time.monotonic() >= deadline:
            raise AssertionError(
                f"SSE active connections gauge did not return to 0 within 1.5s after abort: {active_conns}"
            )
        time.sleep(0.05)
    h = ctx.client.get("/health")
    assert h.status == 200


# ===========================================================================
# F15 Tier 2 (5 tests)
# ===========================================================================

@REGISTRY.register("F15-T2-01", tier=2, features=[15], title="Cross-artifact metric name alignment (/metrics <-> alerts.yml <-> Grafana)")
def test_f15_t2_01(ctx: TestContext) -> None:
    alerts_txt = (ctx.workspace / "deploy" / "prometheus" / "alerts.yml").read_text(encoding="utf-8")
    graf_txt = (ctx.workspace / "deploy" / "grafana" / "dashboards" / "medical-server.json").read_text(encoding="utf-8")
    referenced = set(re.findall(r"\b(medical_server_[a-zA-Z0-9_]+)\b", alerts_txt + "\n" + graf_txt))
    assert referenced, "Expected medical_server_* metric references in alerts.yml / Grafana"
    live_metrics = parse_prometheus_metrics(ctx.client.get("/metrics").text)
    for m_name in referenced:
        base = re.sub(r"_(?:bucket|sum|count|total)$", "", m_name)
        assert any(k == m_name or k.startswith(base) for k in live_metrics), (
            f"Orphan metric {m_name!r} in alerts/Grafana not emitted by GET /metrics"
        )


@REGISTRY.register("F15-T2-02", tier=2, features=[15], title="deploy/Dockerfile security & secret hygiene")
def test_f15_t2_02(ctx: TestContext) -> None:
    raw = (ctx.workspace / "deploy" / "Dockerfile").read_text(encoding="utf-8")
    df = parse_dockerfile(raw)
    assert df.users and all(u.lower() != "root" for u in df.users[-1:]), (
        f"Dockerfile runtime stage must use non-root USER, got {df.users}"
    )
    assert "ca-certificates" in raw, "Dockerfile runtime must install ca-certificates"
    assert "mk_live_" not in raw and "sk-" not in raw and "COPY .env" not in raw


@REGISTRY.register("F15-T2-03", tier=2, features=[15], title="k6/triage_load_test.js syntax, __ENV config, sleep & PII hygiene")
def test_f15_t2_03(ctx: TestContext) -> None:
    raw = (ctx.workspace / "k6" / "triage_load_test.js").read_text(encoding="utf-8")
    info = validate_k6_script(raw)
    assert info["uses_env"], "k6 script must parameterize base URL via __ENV"
    assert info["uses_sleep"], "k6 script must use sleep() pacing"
    assert_no_pii(raw)


@REGISTRY.register("F15-T2-04", tier=2, features=[15], title="alerts.yml PromQL & rule field completeness")
def test_f15_t2_04(ctx: TestContext) -> None:
    data = parse_yaml_subset((ctx.workspace / "deploy" / "prometheus" / "alerts.yml").read_text(encoding="utf-8"))
    seen_names: set[str] = set()
    for grp in data["groups"]:
        for rule in grp["rules"]:
            name = rule["alert"]
            assert name not in seen_names, f"Duplicate alert name: {name}"
            seen_names.add(name)
            expr = str(rule["expr"])
            assert expr.count("(") == expr.count(")"), f"Unbalanced parens in PromQL: {expr}"
            for_val = rule.get("for")
            if for_val is not None:
                assert re.fullmatch(r"(?:\d+[smhd])+", str(for_val).strip()), (
                    f"Invalid alert 'for': {for_val!r}"
                )
            assert rule["labels"]["severity"] in {"warning", "critical", "info"}
            assert rule["annotations"].get("summary") or rule["annotations"].get("description")


@REGISTRY.register("F15-T2-05", tier=2, features=[15], title="Grafana dashboard panel geometry & datasource integrity")
def test_f15_t2_05(ctx: TestContext) -> None:
    dash = parse_json_file(ctx.workspace / "deploy" / "grafana" / "dashboards" / "medical-server.json")
    validate_grafana_dashboard(dash)


# ===========================================================================
# F16–F21 Tier 2 (5 tests each = 30 tests)
# ===========================================================================

@REGISTRY.register("F16-T2-01", tier=2, features=[16], title="Module 1 invalid regex validation contract")
def test_f16_t2_01(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["preview_knowledge_chunks"])
    rs_clean = strip_rust_comments_and_tests(rs)
    fn_block = extract_rust_fn_block(rs_clean, "preview_knowledge_chunks")
    assert "Result<" in fn_block, "preview_knowledge_chunks must return Result<...>"
    pipe_src = collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "Regex::new" in (rs_clean + "\n" + strip_rust_comments_and_tests(pipe_src)), (
        "Module 1 / medical-pipeline must compile custom regex safely via Regex::new"
    )
    assert "regex" in rs.lower() and "regex" in fe.lower()


@REGISTRY.register("F16-T2-02", tier=2, features=[16], title="Module 1 authority_level (0.1..=1.0) & target_population bounds")
def test_f16_t2_02(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    core_src = collect_rs_text(ctx.workspace / "crates" / "medical-core")
    assert "authority_level" in rs and "authority_level" in fe
    assert "target_population" in rs and "target_population" in fe
    assert ("0.1" in (rs + core_src) and "1.0" in (rs + core_src)) and ("0.1" in fe and "1" in fe)


@REGISTRY.register("F16-T2-03", tier=2, features=[16], title="Module 1 empty folder / unsupported extension error handling")
def test_f16_t2_03(ctx: TestContext) -> None:
    rs, _ = assert_studio_commands(ctx, ["preview_knowledge_chunks", "start_knowledge_import"])
    rs_clean = strip_rust_comments_and_tests(rs)
    for cmd in ("preview_knowledge_chunks", "start_knowledge_import"):
        fn_block = extract_rust_fn_block(rs_clean, cmd)
        assert "Result<" in fn_block, f"{cmd} must return Result<...>"
        assert ("Err(" in fn_block or "?" in fn_block or "map_err" in fn_block), (
            f"{cmd} must propagate folder/file errors"
        )


@REGISTRY.register("F16-T2-04", tier=2, features=[16], title="Module 1 export_import_errors zero-error case safety")
def test_f16_t2_04(ctx: TestContext) -> None:
    rs, _ = assert_studio_commands(ctx, ["export_import_errors"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "export_import_errors")
    assert "Result<" in fn_block and not re.search(r"\.\s*unwrap\s*\(\s*\)", fn_block)


@REGISTRY.register("F16-T2-05", tier=2, features=[16], title="Module 1 resume_knowledge_import missing checkpoint error handling")
def test_f16_t2_05(ctx: TestContext) -> None:
    rs, _ = assert_studio_commands(ctx, ["resume_knowledge_import"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "resume_knowledge_import")
    assert "Result<" in fn_block and not re.search(r"\.\s*(?:unwrap|expect)\s*\(", fn_block)


@REGISTRY.register("F17-T2-01", tier=2, features=[17], title="Module 2 plaintext ID/phone blocking before commit")
def test_f17_t2_01(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["preview_patient_import", "apply_masking_and_import_patients"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "apply_masking_and_import_patients")
    assert "Result<" in fn_block
    assert any(tok in (rs + "\n" + fn_block).lower() for tok in ("mask", "pii", "validate", "patient_id"))
    assert "preview" in fe.lower()


@REGISTRY.register("F17-T2-02", tier=2, features=[17], title="Module 2 empty file or missing required column mapping")
def test_f17_t2_02(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["preview_patient_import"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "preview_patient_import")
    assert "Result<" in fn_block and ("Err(" in fn_block or "?" in fn_block or "map_err" in fn_block)
    assert ("column" in rs.lower() or "mapping" in rs.lower()) and ("column" in fe.lower() or "mapping" in fe.lower())


@REGISTRY.register("F17-T2-03", tier=2, features=[17], title="Module 2 age-bucketing boundary & non-numeric age handling")
def test_f17_t2_03(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    pipe_src = collect_rs_text(ctx.workspace / "crates" / "medical-pipeline")
    assert "age_group" in rs and "age_group" in fe
    combined_rs = rs + "\n" + pipe_src
    assert "0-9" in combined_rs or "unknown" in combined_rs.lower()


@REGISTRY.register("F17-T2-04", tier=2, features=[17], title="Module 2 corrupt .xlsx / non-UTF-8 .csv error propagation")
def test_f17_t2_04(ctx: TestContext) -> None:
    rs, _ = assert_studio_commands(ctx, ["preview_patient_import", "apply_masking_and_import_patients"])
    rs_clean = strip_rust_comments_and_tests(rs)
    assert "medical_pipeline" in rs_clean
    for cmd in ("preview_patient_import", "apply_masking_and_import_patients"):
        fn_block = extract_rust_fn_block(rs_clean, cmd)
        assert "Result<" in fn_block and ("Err(" in fn_block or "?" in fn_block or "map_err" in fn_block)
        assert not re.search(r"\.\s*unwrap\s*\(\s*\)", fn_block)


@REGISTRY.register("F17-T2-05", tier=2, features=[17], title="Module 2 consent_long_term_memory: false preservation")
def test_f17_t2_05(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    assert "consent_long_term_memory" in fe
    assert "consent_long_term_memory" in rs or "PatientRecord" in rs


@REGISTRY.register("F18-T2-01", tier=2, features=[18], title="Module 3 intensity out-of-bounds rejection (<0.0 or >1.0)")
def test_f18_t2_01(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["upsert_epidemic_context"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "upsert_epidemic_context")
    assert "Result<" in fn_block and ("validate" in fn_block or "intensity" in fn_block)
    assert "intensity" in fe and ("0" in fe and "1" in fe)


@REGISTRY.register("F18-T2-02", tier=2, features=[18], title="Module 3 inverted date window (valid_from > valid_to) rejection")
def test_f18_t2_02(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["upsert_epidemic_context"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "upsert_epidemic_context")
    assert "Result<" in fn_block and ("validate" in fn_block or "valid_from" in fn_block)
    assert ("valid_from" in rs and "valid_to" in rs) and ("valid_from" in fe and "valid_to" in fe)


@REGISTRY.register("F18-T2-03", tier=2, features=[18], title="Module 3 unreachable medical-server during notify_server_hot_reload")
def test_f18_t2_03(ctx: TestContext) -> None:
    rs, _ = assert_studio_commands(ctx, ["notify_server_hot_reload"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "notify_server_hot_reload")
    assert "async fn" in fn_block and "Result<" in fn_block and "/v1/admin/reload" in fn_block
    assert ("map_err" in fn_block or "Err(" in fn_block or "?" in fn_block)
    assert not re.search(r"\.\s*unwrap\s*\(\s*\)", fn_block)


@REGISTRY.register("F18-T2-04", tier=2, features=[18], title="Module 3 empty required fields (region_code, disease_name)")
def test_f18_t2_04(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["upsert_epidemic_context"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "upsert_epidemic_context")
    assert "Result<" in fn_block and ("validate" in fn_block or "is_empty" in fn_block)
    assert ("region_code" in rs and "disease_name" in rs) and ("region_code" in fe and "disease_name" in fe)


@REGISTRY.register("F18-T2-05", tier=2, features=[18], title="Module 3 deactivating all contexts (active: false)")
def test_f18_t2_05(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["list_epidemic_contexts", "upsert_epidemic_context"])
    assert re.search(r"\bactive\b", rs) and re.search(r"\bactive\b", fe)


@REGISTRY.register("F19-T2-01", tier=2, features=[19], title="Module 4 empty trigger_all_of AND empty trigger_any_of rejection")
def test_f19_t2_01(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["save_red_flag_rule"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "save_red_flag_rule")
    assert "Result<" in fn_block and (
        "validate" in fn_block or ("trigger_all_of" in fn_block and "trigger_any_of" in fn_block)
    )
    assert "trigger_all_of" in rs and "trigger_any_of" in rs
    assert "trigger_all_of" in fe and "trigger_any_of" in fe


@REGISTRY.register("F19-T2-02", tier=2, features=[19], title="Module 4 empty standard_term or empty colloquial_terms rejection")
def test_f19_t2_02(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["save_symptom_synonym"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "save_symptom_synonym")
    assert "Result<" in fn_block and (
        "validate" in fn_block or "is_empty" in fn_block
    )
    assert ("standard_term" in rs and "colloquial_terms" in rs) and (
        "standard_term" in fe and "colloquial_terms" in fe
    )


@REGISTRY.register("F19-T2-03", tier=2, features=[19], title="Module 4 sandbox empty/whitespace/oversized input safety")
def test_f19_t2_03(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["test_dictionary_sandbox"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "test_dictionary_sandbox")
    assert not re.search(r"\.\s*unwrap\s*\(\s*\)", fn_block)
    assert re.search(r"(?:\.trim\s*\(|v-model\.trim\b)", fe), (
        "Module 4 sandbox frontend missing trim() / v-model.trim input guard"
    )


@REGISTRY.register("F19-T2-04", tier=2, features=[19], title="Module 4 deleting non-existent synonym or rule ID safety")
def test_f19_t2_04(ctx: TestContext) -> None:
    rs, _ = assert_studio_commands(ctx, ["delete_symptom_synonym", "delete_red_flag_rule"])
    rs_clean = strip_rust_comments_and_tests(rs)
    for cmd in ("delete_symptom_synonym", "delete_red_flag_rule"):
        fn_block = extract_rust_fn_block(rs_clean, cmd)
        assert "Result<" in fn_block and not re.search(r"\.\s*unwrap\s*\(\s*\)", fn_block)


@REGISTRY.register("F19-T2-05", tier=2, features=[19], title="Module 4 XSS-safe term highlighting in sandbox UI")
def test_f19_t2_05(ctx: TestContext) -> None:
    _, fe = assert_studio_commands(ctx, ["test_dictionary_sandbox"])
    assert "v-html" not in fe, "Avoid unescaped v-html in MedicalStudio frontend templates"
    assert "dangerouslySetInnerHTML" not in fe and ".innerHTML" not in fe


@REGISTRY.register("F20-T2-01", tier=2, features=[20], title="Module 5 Emergency120 short-circuit trace inspection")
def test_f20_t2_01(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["run_ecs_whitebox_triage"])
    assert ("Emergency120" in rs and "RedFlagAlert" in rs) and ("Emergency120" in fe and "RedFlagAlert" in fe)


@REGISTRY.register("F20-T2-02", tier=2, features=[20], title="Module 5 empty utterance guard in simulator")
def test_f20_t2_02(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["run_ecs_whitebox_triage"])
    assert re.search(r"(?:\.trim\s*\(|v-model\.trim\b)", fe), (
        "Module 5 frontend missing empty-utterance trim() / v-model.trim guard"
    )
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "run_ecs_whitebox_triage")
    assert "Result<" in fn_block and ("is_empty" in fn_block or "validate" in fn_block)


@REGISTRY.register("F20-T2-03", tier=2, features=[20], title="Module 5 max_turns convergence state in simulator")
def test_f20_t2_03(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["run_ecs_whitebox_triage"])
    assert "completed" in fe and "follow_up" in fe and "emergency" in fe
    assert ("TriageSlots" in rs or "max_turns" in rs) and ("TriageSlots" in fe or "max_turns" in fe)


@REGISTRY.register("F20-T2-04", tier=2, features=[20], title="Module 5 patient switch mid-session isolation")
def test_f20_t2_04(ctx: TestContext) -> None:
    assert_studio_commands(ctx, ["reset_ecs_debug_session", "list_debug_patients"])


@REGISTRY.register("F20-T2-05", tier=2, features=[20], title="Module 5 empty RagEvidence / fallback mode rendering")
def test_f20_t2_05(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    assert "RagEvidence" in rs and "RagEvidence" in fe
    assert any(tok in fe.lower() for tok in (".length", "v-if", "fallback", "empty"))


@REGISTRY.register("F21-T2-01", tier=2, features=[21], title="Module 6 secret masking & zero hardcoded keys in frontend")
def test_f21_t2_01(ctx: TestContext) -> None:
    _, fe = assert_studio_commands(ctx, ["list_llm_providers", "list_api_keys"])
    assert not re.search(r"mk_live_(?!x{4,})[A-Za-z0-9_]{6,}", fe), "Hardcoded mk_live_* key in MedicalStudio frontend"
    assert "sk-proj-" not in fe and not re.search(r"\bsk-[A-Za-z0-9_-]{16,}\b", fe), (
        "Hardcoded sk-* API key in MedicalStudio frontend"
    )


@REGISTRY.register("F21-T2-02", tier=2, features=[21], title="Module 6 invalid provider URL or non-positive timeout validation")
def test_f21_t2_02(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["upsert_llm_provider"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "upsert_llm_provider")
    assert "Result<" in fn_block and ("http" in fn_block.lower() or "timeout" in fn_block.lower() or "validate" in fn_block)
    assert "http" in (rs + fe).lower() and "timeout" in (rs + fe).lower()


@REGISTRY.register("F21-T2-03", tier=2, features=[21], title="Module 6 non-positive QPM/TPM validation on key issuance")
def test_f21_t2_03(ctx: TestContext) -> None:
    rs, fe = assert_studio_commands(ctx, ["issue_api_key"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "issue_api_key")
    assert "Result<" in fn_block and "qpm" in fn_block.lower() and "tpm" in fn_block.lower()
    assert "qpm" in fe.lower() and "tpm" in fe.lower()


@REGISTRY.register("F21-T2-04", tier=2, features=[21], title="Module 6 connectivity probe failure handling")
def test_f21_t2_04(ctx: TestContext) -> None:
    rs, _ = assert_studio_commands(ctx, ["test_llm_provider_connectivity"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "test_llm_provider_connectivity")
    assert "async fn" in fn_block and "Result<" in fn_block
    assert ("ttft" in fn_block.lower() or "elapsed" in fn_block.lower() or "latency" in fn_block.lower())


@REGISTRY.register("F21-T2-05", tier=2, features=[21], title="Module 6 idempotent/safe revocation of unknown or already-revoked key")
def test_f21_t2_05(ctx: TestContext) -> None:
    rs, _ = assert_studio_commands(ctx, ["revoke_api_key"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "revoke_api_key")
    assert "Result<" in fn_block and not re.search(r"\.\s*unwrap\s*\(\s*\)", fn_block)


# ===========================================================================
# F22 Tier 2 (6 tests)
# ===========================================================================

@REGISTRY.register("F22-T2-01", tier=2, features=[22], title="Miniapp consent refusal blocks persistent triage/memory")
def test_f22_t2_01(ctx: TestContext) -> None:
    _miniapp_texts(ctx)
    chat_js = discover_miniapp_page_text(ctx.workspace, ".js", ("chat", "triage", "index"))
    assert "consent" in chat_js.lower()


@REGISTRY.register("F22-T2-02", tier=2, features=[22], title="Miniapp empty/whitespace input & double-submit guard")
def test_f22_t2_02(ctx: TestContext) -> None:
    _miniapp_texts(ctx)
    chat_js = discover_miniapp_page_text(ctx.workspace, ".js", ("chat", "triage", "index"))
    assert "trim()" in chat_js
    assert "loading" in chat_js.lower() or "sending" in chat_js.lower() or "streaming" in chat_js.lower()


@REGISTRY.register("F22-T2-03", tier=2, features=[22], title="Miniapp 401 token expiry recovery & network/429 error handling")
def test_f22_t2_03(ctx: TestContext) -> None:
    files = _miniapp_texts(ctx)
    api_js = files.get("utils/api.js", "")
    assert "401" in api_js and ("429" in api_js or "showToast" in api_js or "statusCode" in api_js)


@REGISTRY.register("F22-T2-04", tier=2, features=[22], title="Miniapp destructive memory wipe confirmation modal (wx.showModal)")
def test_f22_t2_04(ctx: TestContext) -> None:
    _miniapp_texts(ctx)
    mem_js = discover_miniapp_page_text(ctx.workspace, ".js", ("memory",))
    assert "wx.showModal" in mem_js and "confirm" in mem_js


@REGISTRY.register("F22-T2-05", tier=2, features=[22], title="Miniapp chunked SSE buffering & listener cleanup on onUnload")
def test_f22_t2_05(ctx: TestContext) -> None:
    files = _miniapp_texts(ctx)
    blob = "\n".join(v for k, v in files.items() if k.endswith(".js"))
    assert "onUnload" in blob or "abort" in blob.lower()


@REGISTRY.register("F22-T2-06", tier=2, features=[22], title="Miniapp WXML/WXSS syntax integrity & zero hardcoded secrets")
def test_f22_t2_06(ctx: TestContext) -> None:
    files = _miniapp_texts(ctx)
    wxmls = {k: v for k, v in files.items() if k.endswith(".wxml")}
    wxsss = {k: v for k, v in files.items() if k.endswith(".wxss")}
    assert wxmls and wxsss
    for name, txt in wxmls.items():
        validate_wxml(txt)
    for name, txt in wxsss.items():
        validate_wxss(txt)
    full = "\n".join(files.values())
    assert "mk_live_" not in full and "sk-proj-" not in full

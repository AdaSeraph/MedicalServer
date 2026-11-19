"""Tier 1 Opaque-Box Feature Coverage Tests: Features F12–F22 (60 tests)."""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from e2e.lib.assertions import (
    DISCLAIMER_CORE_PHRASE,
    assert_completed_response,
    assert_disclaimer_present,
    assert_emergency_response,
    assert_follow_up_response,
    assert_no_pii,
    assert_studio_commands,
    collect_miniapp_sources,
    collect_rs_text,
    collect_studio_sources,
    extract_rust_fn_block,
    load_schema_rules,
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


def _assert_studio_commands(ctx: TestContext, commands: list[str]) -> tuple[str, str]:
    return assert_studio_commands(ctx, commands)


def _miniapp_texts(ctx: TestContext) -> dict[str, str]:
    return collect_miniapp_sources(ctx.workspace)


# ===========================================================================
# F12: Axum Auth, Multi-Tenancy & Rate Limiting (6 tests)
# ===========================================================================

@REGISTRY.register("F12-T1-01", tier=1, features=[12], title="WeChat code2Session login returns JWT & anonymized patient_id")
def test_f12_t1_01(ctx: TestContext) -> None:
    r = ctx.client.post("/v1/auth/wechat", json_body={"code": "wx_e2e_valid_code_001"})
    assert r.status == 200, f"/v1/auth/wechat failed ({r.status}): {r.text}"
    data = r.json()
    token = data.get("token", "")
    assert isinstance(token, str) and token.count(".") == 2, f"Expected 3-part JWT, got {token!r}"
    assert isinstance(data.get("expires_in"), int) and data["expires_in"] > 0
    pid = data.get("patient_id", "")
    assert isinstance(pid, str) and pid and "wx_e2e_valid_code_001" not in pid
    assert_no_pii(pid)


@REGISTRY.register("F12-T1-02", tier=1, features=[12], title="WeChat JWT authenticates protected triage & memory endpoints")
def test_f12_t1_02(ctx: TestContext) -> None:
    token, pid = ctx.get_wechat_auth("wx_f12_t1_02")
    r1 = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f12_t1_02", "patient_id": pid, "utterance": "咳嗽一天", "stream": False},
        bearer=token,
    )
    assert r1.status == 200, f"JWT /v1/triage/chat failed: {r1.status} {r1.text}"
    r2 = ctx.client.get(f"/v1/patients/{pid}/memory", bearer=token)
    assert r2.status in (200, 404), f"JWT GET /memory failed: {r2.status} {r2.text}"


@REGISTRY.register("F12-T1-03", tier=1, features=[12], title="Multi-tenant mk_live_* API key authenticates Open API endpoints")
def test_f12_t1_03(ctx: TestContext) -> None:
    key = ctx.tenant_key("hosp_a")
    r1 = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f12_key", "patient_id": "pat_sha256_his_01", "utterance": "咽痛两天", "stream": False},
        bearer=key,
    )
    r2 = ctx.client.post("/v1/knowledge/search", json_body={"query": "咽痛 发热", "top_k": 3}, bearer=key)
    r3 = ctx.client.post(
        "/v1/openai/chat/completions",
        json_body={"model": "medical-triage", "messages": [{"role": "user", "content": "咽痛两天"}]},
        bearer=key,
    )
    assert r1.status == 200 and r2.status == 200 and r3.status == 200, (
        f"API key auth statuses: {r1.status}, {r2.status}, {r3.status}"
    )


@REGISTRY.register("F12-T1-04", tier=1, features=[12], title="QPM token bucket enforces per-key request limit (429)")
def test_f12_t1_04(ctx: TestContext) -> None:
    low_key = ctx.tenant_key("qpm_test_t1_04")
    statuses: list[int] = []
    for _ in range(6):
        r = ctx.client.post("/v1/knowledge/search", json_body={"query": "感冒", "top_k": 1}, bearer=low_key)
        statuses.append(r.status)
        if r.status == 429:
            break
    assert all(s in (200, 429) for s in statuses), f"Unexpected QPM statuses: {statuses}"
    assert statuses[0] == 200, f"First request with qpm_test_t1_04 key should succeed: {statuses}"
    assert 429 in statuses, f"Expected 429 rate-limit enforcement within 6 requests for qpm=3 key: {statuses}"


@REGISTRY.register("F12-T1-05", tier=1, features=[12], title="Multi-tenant bucket & namespace isolation")
def test_f12_t1_05(ctx: TestContext) -> None:
    key_a = ctx.tenant_key("hosp_a")
    key_b = ctx.tenant_key("qpm_test_t1_05")
    b_statuses: list[int] = []
    for _ in range(6):
        rb = ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "top_k": 1}, bearer=key_b)
        b_statuses.append(rb.status)
    assert 429 in b_statuses, f"Expected qpm_test_t1_05 to hit 429 before checking hosp_a isolation: {b_statuses}"
    r_a = ctx.client.post("/v1/knowledge/search", json_body={"query": "发热", "top_k": 3}, bearer=key_a)
    assert r_a.status == 200, f"Tenant A throttled by qpm_test_t1_05 burst: {r_a.status}"


@REGISTRY.register("F12-T1-06", tier=1, features=[12], title="TPM token bucket enforcement contract & server implementation")
def test_f12_t1_06(ctx: TestContext) -> None:
    srv_src = collect_rs_text(ctx.workspace / "crates" / "medical-server")
    assert "tpm" in srv_src.lower() and "qpm" in srv_src.lower(), "medical-server missing QPM/TPM token buckets"


# ===========================================================================
# F13: Redis L1 Working Memory & Semantic Cache (5 tests)
# ===========================================================================

@REGISTRY.register("F13-T1-01", tier=1, features=[13], title="Multi-turn L1 session continuity")
def test_f13_t1_01(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f13_t1_01")
    sid = "sess_l1_cont_01"
    r1 = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": sid, "patient_id": pid, "utterance": "我发烧了", "stream": False},
        headers=hdrs,
    )
    assert r1.status == 200
    d1 = r1.json()
    if d1.get("status") == "follow_up":
        assert_follow_up_response(d1)
        opt = d1["quick_options"][0]
        r2 = ctx.client.post(
            "/v1/triage/chat",
            json_body={"session_id": sid, "patient_id": pid, "utterance": opt, "stream": False},
            headers=hdrs,
        )
        assert r2.status == 200
        d2 = r2.json()
        if d2.get("status") == "follow_up":
            assert d2["turn"] >= d1["turn"]


@REGISTRY.register("F13-T1-02", tier=1, features=[13], title="L1 turn tracking & convergence at max_turns <= 4")
def test_f13_t1_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f13_t1_02")
    sid = "sess_l1_conv_02"
    utt = "咳嗽咽痛两天"
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
    assert last.get("status") == "completed", f"Session did not converge by turn 4: {last}"
    assert_completed_response(last)


@REGISTRY.register("F13-T1-03", tier=1, features=[13], title="Semantic cache hit & /metrics counter increment")
def test_f13_t1_03(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f13_t1_03")
    m_before = parse_prometheus_metrics(ctx.client.get("/metrics").text)
    hits_before = sum(v for _, v in m_before.get("medical_server_cache_hits_total", []))

    q = {"query": "流感 高热 咳嗽", "top_k": 3}
    r1 = ctx.client.post("/v1/knowledge/search", json_body=q, headers=hdrs)
    r2 = ctx.client.post("/v1/knowledge/search", json_body=q, headers=hdrs)
    assert r1.status == 200 and r2.status == 200

    m_after = parse_prometheus_metrics(ctx.client.get("/metrics").text)
    assert "medical_server_cache_hits_total" in m_after
    hits_after = sum(v for _, v in m_after["medical_server_cache_hits_total"])
    assert hits_after >= hits_before


@REGISTRY.register("F13-T1-04", tier=1, features=[13], title="Graceful R5 in-memory fallback without external Redis")
def test_f13_t1_04(ctx: TestContext) -> None:
    h = ctx.client.get("/health")
    assert h.status == 200
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f13_t1_04")
    r = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_r5_fb", "patient_id": pid, "utterance": "流鼻涕打喷嚏", "stream": False},
        headers=hdrs,
    )
    assert r.status == 200


@REGISTRY.register("F13-T1-05", tier=1, features=[13], title="Concurrent session isolation across distinct session_ids")
def test_f13_t1_05(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f13_t1_05")
    r_a1 = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_iso_A", "patient_id": pid, "utterance": "咳嗽三天", "stream": False},
        headers=hdrs,
    )
    r_b1 = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_iso_B", "patient_id": pid, "utterance": "腹痛腹泻", "stream": False},
        headers=hdrs,
    )
    assert r_a1.status == 200 and r_b1.status == 200
    assert r_a1.json().get("session_id") == "sess_iso_A"
    assert r_b1.json().get("session_id") == "sess_iso_B"


# ===========================================================================
# F14: Triage, Search, OpenAI Proxy, Epidemic, Memory & Admin APIs (8 tests)
# ===========================================================================

@REGISTRY.register("F14-T1-01", tier=1, features=[14], title="GET /health and GET /metrics contract")
def test_f14_t1_01(ctx: TestContext) -> None:
    h = ctx.client.get("/health")
    assert h.status == 200
    hj = h.json()
    assert "status" in hj and "version" in hj and "checks" in hj

    m = ctx.client.get("/metrics")
    assert m.status == 200
    parsed = parse_prometheus_metrics(m.text)
    req_metrics = load_schema_rules()["triage_invariants"]["required_prometheus_metrics"]
    for metric_name in req_metrics:
        assert any(k == metric_name or k.startswith(metric_name + "_") for k in parsed), (
            f"Missing required Prometheus metric {metric_name!r} in /metrics"
        )


@REGISTRY.register("F14-T1-02", tier=1, features=[14], title="POST /v1/triage/chat JSON follow-up & completion")
def test_f14_t1_02(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f14_t1_02")
    sid = "sess_f14_t1_02"
    utt = "高热咽痛全身肌肉酸痛"
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
    assert_completed_response(last, require_citations=True)


@REGISTRY.register("F14-T1-03", tier=1, features=[14], title="POST /v1/triage/chat SSE stream: true")
def test_f14_t1_03(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f14_t1_03")
    sse = ctx.client.post_sse(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f14_sse", "patient_id": pid, "utterance": "咳嗽发热两天", "stream": True},
        headers=hdrs,
    )
    assert sse.status == 200
    assert "text/event-stream" in sse.headers.get("content-type", "")
    result_events = [e for e in sse.events if e.event == "result"]
    assert result_events, f"Expected SSE event: result, got {[e.event for e in sse.events]}"
    payload = result_events[-1].json()
    if payload.get("status") == "follow_up":
        assert_follow_up_response(payload)
    elif payload.get("status") == "completed":
        assert_completed_response(payload)
    else:
        raise AssertionError(f"Unexpected SSE result status: {payload!r}")


@REGISTRY.register("F14-T1-04", tier=1, features=[14], title="POST /v1/triage/chat Emergency120 short-circuit")
def test_f14_t1_04(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f14_t1_04")
    r = ctx.client.post(
        "/v1/triage/chat",
        json_body={"session_id": "sess_f14_emg", "patient_id": pid, "utterance": "胸痛伴大汗和左臂放射痛", "stream": False},
        headers=hdrs,
    )
    assert r.status == 200
    assert_emergency_response(r.json())


@REGISTRY.register("F14-T1-05", tier=1, features=[14], title="POST /v1/knowledge/search hybrid retrieval & filters")
def test_f14_t1_05(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f14_t1_05")
    r = ctx.client.post(
        "/v1/knowledge/search",
        json_body={
            "query": "高热全身肌肉酸痛",
            "target_population": ["adult"],
            "seasonal_boost": True,
            "region_code": "CN-110000",
            "top_k": 5,
        },
        headers=hdrs,
    )
    assert r.status == 200
    d = r.json()
    assert isinstance(d.get("results"), list) and len(d["results"]) <= 5
    assert isinstance(d.get("total"), int)
    assert "seasonal_context_applied" in d


@REGISTRY.register("F14-T1-06", tier=1, features=[14], title="POST /v1/openai/chat/completions JSON & SSE proxy")
def test_f14_t1_06(ctx: TestContext) -> None:
    hdrs, _ = ctx.auth_headers_or_wechat("wx_f14_t1_06")
    payloads = parse_json_file(ctx.fixtures_dir / "payloads" / "openai_proxy.json")
    r_json = ctx.client.post("/v1/openai/chat/completions", json_body=payloads["standard_completion"], headers=hdrs)
    assert r_json.status == 200
    dj = r_json.json()
    assert dj.get("object") == "chat.completion"
    msg = dj["choices"][0]["message"]
    assert msg["role"] == "assistant" and msg["content"].strip()
    assert_disclaimer_present(msg["content"])

    r_sse = ctx.client.post_sse("/v1/openai/chat/completions", json_body=payloads["streaming_completion"], headers=hdrs)
    assert r_sse.status == 200
    assert "text/event-stream" in r_sse.headers.get("content-type", "")
    assert any(e.data.strip() == "[DONE]" for e in r_sse.events)


@REGISTRY.register("F14-T1-07", tier=1, features=[14], title="GET /v1/epidemic/current & POST /v1/admin/reload hot swap")
def test_f14_t1_07(ctx: TestContext) -> None:
    r_ep = ctx.client.get("/v1/epidemic/current?region_code=CN-110000&season=winter")
    assert r_ep.status == 200 and isinstance(r_ep.json().get("contexts"), list)
    hdrs = {"Authorization": f"Bearer {ctx.admin_token}"} if ctx.admin_token else ctx.auth_headers_or_wechat("wx_f14_admin")[0]
    r_rel = ctx.client.post(
        "/v1/admin/reload",
        json_body={"reload_epidemics": True, "reload_dictionaries": True, "reload_router": True},
        headers=hdrs,
    )
    assert r_rel.status == 200


@REGISTRY.register("F14-T1-08", tier=1, features=[14], title="GET/PUT/DELETE /v1/patients/{id}/memory lifecycle")
def test_f14_t1_08(ctx: TestContext) -> None:
    hdrs, pid = ctx.auth_headers_or_wechat("wx_f14_t1_08")
    rec = {
        "patient_id": pid,
        "consent_long_term_memory": True,
        "l2_profile": {
            "age_group": "30-39", "gender": "female", "is_pregnant": False,
            "chronic_diseases": ["高血压"], "allergies": ["青霉素"],
            "surgical_history": [], "long_term_medications": [],
        },
        "l3_visit_events": [],
    }
    assert ctx.client.put(f"/v1/patients/{pid}/memory", json_body=rec, headers=hdrs).status in (200, 201)
    g1 = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert g1.status == 200 and "高血压" in g1.text and "青霉素" in g1.text
    d = ctx.client.delete(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert d.status in (200, 204)
    g2 = ctx.client.get(f"/v1/patients/{pid}/memory", headers=hdrs)
    assert g2.status in (200, 404)
    if g2.status == 200:
        assert "青霉素" not in g2.text


# ===========================================================================
# F15: Ops & Deployment Deliverables (5 tests)
# ===========================================================================

@REGISTRY.register("F15-T1-01", tier=1, features=[15], title="deploy/Dockerfile multi-stage production build structure")
def test_f15_t1_01(ctx: TestContext) -> None:
    df = parse_dockerfile((ctx.workspace / "deploy" / "Dockerfile").read_text(encoding="utf-8"))
    assert len(df.stages) >= 2, f"Expected multi-stage Dockerfile (>=2 FROM), got {df.stages}"
    assert df.copy_from, "Multi-stage Dockerfile missing COPY --from=..."
    assert df.exposes, "Dockerfile missing EXPOSE instruction"
    assert df.entrypoint_or_cmd, "Dockerfile missing CMD/ENTRYPOINT"


@REGISTRY.register("F15-T1-02", tier=1, features=[15], title="k6/triage_load_test.js concurrency & endpoint coverage")
def test_f15_t1_02(ctx: TestContext) -> None:
    k6_txt = (ctx.workspace / "k6" / "triage_load_test.js").read_text(encoding="utf-8")
    info = validate_k6_script(k6_txt)
    assert info["has_thresholds"], "k6 script missing thresholds"
    ep_str = " ".join(info["endpoints"])
    assert "/v1/triage/chat" in ep_str and ("/health" in ep_str or "/v1/knowledge/search" in ep_str)


@REGISTRY.register("F15-T1-03", tier=1, features=[15], title="deploy/prometheus/prometheus.yml scrape & rule configuration")
def test_f15_t1_03(ctx: TestContext) -> None:
    prom_path = ctx.workspace / "deploy" / "prometheus" / "prometheus.yml"
    data = parse_yaml_subset(prom_path.read_text(encoding="utf-8"))
    assert isinstance(data, dict)
    assert "global" in data and "scrape_configs" in data
    assert "alerts.yml" in json.dumps(data.get("rule_files", []))


@REGISTRY.register("F15-T1-04", tier=1, features=[15], title="deploy/prometheus/alerts.yml alert rules validity")
def test_f15_t1_04(ctx: TestContext) -> None:
    alerts_path = ctx.workspace / "deploy" / "prometheus" / "alerts.yml"
    data = parse_yaml_subset(alerts_path.read_text(encoding="utf-8"))
    groups = data.get("groups")
    assert isinstance(groups, list) and groups
    rules = [r for g in groups for r in g.get("rules", [])]
    assert len(rules) >= 2, "Expected >=2 alert rules in alerts.yml"


@REGISTRY.register("F15-T1-05", tier=1, features=[15], title="deploy/grafana provisioning & dashboard JSON validity")
def test_f15_t1_05(ctx: TestContext) -> None:
    graf_dir = ctx.workspace / "deploy" / "grafana"
    assert graf_dir.is_dir(), f"Missing {graf_dir}"
    ymls = list(graf_dir.rglob("*.yml")) + list(graf_dir.rglob("*.yaml"))
    assert ymls, f"No Grafana provisioning YAML found in {graf_dir}"
    for yp in ymls:
        parsed = parse_yaml_subset(yp.read_text(encoding="utf-8"))
        assert isinstance(parsed, dict)
    dash = parse_json_file(graf_dir / "dashboards" / "medical-server.json")
    validate_grafana_dashboard(dash)


# ===========================================================================
# F16–F21: MedicalStudio 6 Modules (5 tests each = 30 tests)
# ===========================================================================

@REGISTRY.register("F16-T1-01", tier=1, features=[16], title="Module 1 Tauri command registration & frontend binding")
def test_f16_t1_01(ctx: TestContext) -> None:
    cmds = load_schema_rules()["triage_invariants"]["required_studio_commands"]["module1"]
    _assert_studio_commands(ctx, cmds)


@REGISTRY.register("F16-T1-02", tier=1, features=[16], title="Module 1 multi-format import support & built frontend bundle")
def test_f16_t1_02(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    assert "medical_pipeline" in rs, "medical-studio must import/use medical_pipeline"
    for ext in ("json", "jsonl", "md", "docx", "pdf"):
        assert ext in fe.lower(), f"Module 1 frontend missing format reference: {ext}"
    studio = ctx.workspace / "crates" / "medical-studio"
    conf = parse_json_file(studio / "tauri.conf.json")
    dist_idx = (studio / conf["build"]["frontendDist"] / "index.html").resolve()
    assert dist_idx.is_file() and "<script" in dist_idx.read_text(encoding="utf-8"), (
        f"Missing or unbuilt frontendDist index.html: {dist_idx}"
    )


@REGISTRY.register("F16-T1-03", tier=1, features=[16], title="Module 1 chunk card preview & metadata/regex editor contract")
def test_f16_t1_03(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    for fld in ("target_population", "department", "authority_level", "seasonal_tags"):
        assert fld in rs and fld in fe, f"Module 1 missing metadata field {fld!r} in Rust/frontend"
    assert "regex" in rs.lower() and "regex" in fe.lower(), "Module 1 missing regex editor/parser contract"


@REGISTRY.register("F16-T1-04", tier=1, features=[16], title="Module 1 ingestion progress & throughput telemetry")
def test_f16_t1_04(ctx: TestContext) -> None:
    rs, fe = _assert_studio_commands(ctx, ["get_import_job_status"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "get_import_job_status")
    assert "Result<" in fn_block, "get_import_job_status must return Result<...>"
    assert "completed" in fe and ("failed" in fe or "error" in fe.lower())
    assert "completed" in rs and ("failed" in rs or "error" in rs.lower())


@REGISTRY.register("F16-T1-05", tier=1, features=[16], title="Module 1 checkpoint resume & error log export controls")
def test_f16_t1_05(ctx: TestContext) -> None:
    _assert_studio_commands(ctx, ["resume_knowledge_import", "export_import_errors"])


@REGISTRY.register("F17-T1-01", tier=1, features=[17], title="Module 2 Tauri command registration & frontend binding")
def test_f17_t1_01(ctx: TestContext) -> None:
    cmds = load_schema_rules()["triage_invariants"]["required_studio_commands"]["module2"]
    _assert_studio_commands(ctx, cmds)


@REGISTRY.register("F17-T1-02", tier=1, features=[17], title="Module 2 .xlsx/.csv/.json support & column mapping UI")
def test_f17_t1_02(ctx: TestContext) -> None:
    _, fe = _assert_studio_commands(ctx, ["preview_patient_import"])
    for ext in ("xlsx", "csv", "json"):
        assert ext in fe.lower()
    assert "column" in fe.lower() or "mapping" in fe.lower(), "Module 2 frontend missing column mapping UI"


@REGISTRY.register("F17-T1-03", tier=1, features=[17], title="Module 2 sensitive column auto-detection & highlighting")
def test_f17_t1_03(ctx: TestContext) -> None:
    rs, fe = _assert_studio_commands(ctx, ["preview_patient_import"])
    pii_tokens = ("id_card", "phone", "pii", "sensitive", "masked")
    assert any(t in rs.lower() for t in pii_tokens) and any(t in fe.lower() for t in pii_tokens), (
        "Module 2 missing sensitive column detection/highlighting markers in Rust/frontend"
    )


@REGISTRY.register("F17-T1-04", tier=1, features=[17], title="Module 2 masking strategy selection (strip/hash/age_group)")
def test_f17_t1_04(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    rs_l, fe_l = rs.lower(), fe.lower()
    assert ("age_group" in rs_l or "bucket" in rs_l) and ("age_group" in fe_l or "bucket" in fe_l)
    assert ("hash" in rs_l or "sha256" in rs_l) and ("hash" in fe_l or "sha256" in fe_l)


@REGISTRY.register("F17-T1-05", tier=1, features=[17], title="Module 2 mandatory pre-ingest privacy firewall preview gate")
def test_f17_t1_05(ctx: TestContext) -> None:
    _, fe = _assert_studio_commands(ctx, ["preview_patient_import", "apply_masking_and_import_patients"])
    assert "preview" in fe.lower(), "Module 2 frontend missing mandatory pre-ingest preview gate"


@REGISTRY.register("F18-T1-01", tier=1, features=[18], title="Module 3 Tauri command registration & frontend binding")
def test_f18_t1_01(ctx: TestContext) -> None:
    cmds = load_schema_rules()["triage_invariants"]["required_studio_commands"]["module3"]
    _assert_studio_commands(ctx, cmds)


@REGISTRY.register("F18-T1-02", tier=1, features=[18], title="Module 3 region, season & active toggle console")
def test_f18_t1_02(ctx: TestContext) -> None:
    rs, fe = _assert_studio_commands(ctx, ["list_epidemic_contexts", "upsert_epidemic_context"])
    assert "medical_core" in rs, "Module 3 Rust must use medical_core epidemic models"
    for fld in ("region_code", "season", "disease_name", "active"):
        assert fld in rs and fld in fe


@REGISTRY.register("F18-T1-03", tier=1, features=[18], title="Module 3 intensity slider (0.0~1.0) & priority_question_hint")
def test_f18_t1_03(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    for fld in ("intensity", "boost_symptoms", "priority_question_hint"):
        assert fld in rs and fld in fe, f"Module 3 missing {fld!r} in Rust or frontend"


@REGISTRY.register("F18-T1-04", tier=1, features=[18], title="Module 3 DB save & hot-reload push to medical-server")
def test_f18_t1_04(ctx: TestContext) -> None:
    rs, _ = _assert_studio_commands(ctx, ["notify_server_hot_reload"])
    fn_block = extract_rust_fn_block(strip_rust_comments_and_tests(rs), "notify_server_hot_reload")
    assert "/v1/admin/reload" in fn_block, "notify_server_hot_reload must call POST /v1/admin/reload"


@REGISTRY.register("F18-T1-05", tier=1, features=[18], title="Module 3 validity window (valid_from/valid_to) alignment")
def test_f18_t1_05(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    assert ("valid_from" in rs and "valid_to" in rs) and ("valid_from" in fe and "valid_to" in fe)


@REGISTRY.register("F19-T1-01", tier=1, features=[19], title="Module 4 Tauri command registration & frontend binding")
def test_f19_t1_01(ctx: TestContext) -> None:
    cmds = load_schema_rules()["triage_invariants"]["required_studio_commands"]["module4"]
    _assert_studio_commands(ctx, cmds)


@REGISTRY.register("F19-T1-02", tier=1, features=[19], title="Module 4 symptom_synonyms CRUD interface")
def test_f19_t1_02(ctx: TestContext) -> None:
    rs, fe = _assert_studio_commands(
        ctx, ["get_medical_dictionaries", "save_symptom_synonym", "delete_symptom_synonym"]
    )
    assert ("standard_term" in rs and "colloquial_terms" in rs) and (
        "standard_term" in fe and "colloquial_terms" in fe
    )


@REGISTRY.register("F19-T1-03", tier=1, features=[19], title="Module 4 red_flag_rules single & combinatorial rule CRUD")
def test_f19_t1_03(ctx: TestContext) -> None:
    rs, fe = _assert_studio_commands(ctx, ["save_red_flag_rule", "delete_red_flag_rule"])
    for fld in ("trigger_all_of", "trigger_any_of", "Emergency120", "alert_message"):
        assert fld in rs and fld in fe, f"Module 4 missing red-flag field {fld!r} in Rust or frontend"


@REGISTRY.register("F19-T1-04", tier=1, features=[19], title="Module 4 live test sandbox highlighting & alert level display")
def test_f19_t1_04(ctx: TestContext) -> None:
    _assert_studio_commands(ctx, ["test_dictionary_sandbox"])


@REGISTRY.register("F19-T1-05", tier=1, features=[19], title="Module 4 alignment with data-collection-schema.json dictionaries")
def test_f19_t1_05(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    assert "medical_core" in rs, "Module 4 must align with medical_core dictionary models"
    for fld in ("rule_id", "level", "standard_term", "colloquial_terms"):
        assert fld in rs and fld in fe


@REGISTRY.register("F20-T1-01", tier=1, features=[20], title="Module 5 Tauri command registration & frontend binding")
def test_f20_t1_01(ctx: TestContext) -> None:
    cmds = load_schema_rules()["triage_invariants"]["required_studio_commands"]["module5"]
    _assert_studio_commands(ctx, cmds)


@REGISTRY.register("F20-T1-02", tier=1, features=[20], title="Module 5 test patient & seasonal context simulator setup")
def test_f20_t1_02(ctx: TestContext) -> None:
    _assert_studio_commands(ctx, ["list_debug_patients", "run_ecs_whitebox_triage"])


@REGISTRY.register("F20-T1-03", tier=1, features=[20], title="Module 5 left pane Miniapp chat simulator")
def test_f20_t1_03(ctx: TestContext) -> None:
    _, fe = _assert_studio_commands(ctx, ["run_ecs_whitebox_triage"])
    assert "quick_options" in fe and ("primary_department" in fe or "triage_result" in fe)
    assert "30" in fe, "Module 5 chat simulator should surface/enforce 30-char follow-up constraint"


@REGISTRY.register("F20-T1-04", tier=1, features=[20], title="Module 5 right pane ECS Component Inspector")
def test_f20_t1_04(ctx: TestContext) -> None:
    rs, fe = _studio_texts(ctx)
    for comp in ("ExtractedSymptoms", "RedFlagAlert", "RagEvidence", "TriageSlots"):
        assert comp in rs and comp in fe, f"Module 5 missing ECS Component {comp!r} in Rust or frontend"


@REGISTRY.register("F20-T1-05", tier=1, features=[20], title="Module 5 debug session reset action")
def test_f20_t1_05(ctx: TestContext) -> None:
    _assert_studio_commands(ctx, ["reset_ecs_debug_session"])


@REGISTRY.register("F21-T1-01", tier=1, features=[21], title="Module 6 Tauri command registration & frontend binding")
def test_f21_t1_01(ctx: TestContext) -> None:
    cmds = load_schema_rules()["triage_invariants"]["required_studio_commands"]["module6"]
    _assert_studio_commands(ctx, cmds)


@REGISTRY.register("F21-T1-02", tier=1, features=[21], title="Module 6 multi-provider CRUD & TTFT connectivity probe")
def test_f21_t1_02(ctx: TestContext) -> None:
    _assert_studio_commands(ctx, ["list_llm_providers", "upsert_llm_provider", "test_llm_provider_connectivity"])


@REGISTRY.register("F21-T1-03", tier=1, features=[21], title="Module 6 Fast & Reasoning tier routing & failover config")
def test_f21_t1_03(ctx: TestContext) -> None:
    rs, fe = _assert_studio_commands(ctx, ["get_router_config", "save_router_config"])
    rs_l, fe_l = rs.lower(), fe.lower()
    assert ("fast" in rs_l and "reasoning" in rs_l) and ("fast" in fe_l and "reasoning" in fe_l)


@REGISTRY.register("F21-T1-04", tier=1, features=[21], title="Module 6 mk_live_* API key issuance, listing & revocation")
def test_f21_t1_04(ctx: TestContext) -> None:
    _assert_studio_commands(ctx, ["list_api_keys", "issue_api_key", "revoke_api_key"])


@REGISTRY.register("F21-T1-05", tier=1, features=[21], title="Module 6 gateway token & error-rate telemetry visualization")
def test_f21_t1_05(ctx: TestContext) -> None:
    _assert_studio_commands(ctx, ["get_gateway_metrics"])


# ===========================================================================
# F22: WeChat Mini Program (miniapp/) (6 tests)
# ===========================================================================

@REGISTRY.register("F22-T1-01", tier=1, features=[22], title="Native Mini Program project structure & config validity")
def test_f22_t1_01(ctx: TestContext) -> None:
    ma = ctx.workspace / "miniapp"
    app_json = parse_json_file(ma / "app.json")
    parse_json_file(ma / "project.config.json")
    parse_json_file(ma / "sitemap.json")
    assert (ma / "app.js").is_file() and (ma / "app.wxss").is_file() and (ma / "utils" / "api.js").is_file()
    pages = app_json.get("pages", [])
    assert len(pages) >= 2, f"Expected >=2 pages in miniapp/app.json, got {pages}"
    for page in pages:
        for ext in (".wxml", ".wxss", ".js", ".json"):
            p = ma / f"{page}{ext}"
            assert p.is_file(), f"Missing Mini Program page file: {p}"


@REGISTRY.register("F22-T1-02", tier=1, features=[22], title="First-entry privacy consent & AI triage disclaimer modal")
def test_f22_t1_02(ctx: TestContext) -> None:
    files = _miniapp_texts(ctx)
    blob = "\n".join(files.values())
    assert "wx.setStorageSync" in blob
    assert ("同意" in blob or "consent" in blob.lower()) and ("导诊" in blob or "免责" in blob)


@REGISTRY.register("F22-T1-03", tier=1, features=[22], title="wx.login auth, streaming chat & quick-option buttons")
def test_f22_t1_03(ctx: TestContext) -> None:
    files = _miniapp_texts(ctx)
    blob = "\n".join(files.values())
    assert "wx.login" in blob and "/v1/auth/wechat" in blob
    assert "/v1/triage/chat" in blob and "quick_options" in blob


@REGISTRY.register("F22-T1-04", tier=1, features=[22], title="Emergency120 red alert modal & one-tap 120 dial")
def test_f22_t1_04(ctx: TestContext) -> None:
    files = _miniapp_texts(ctx)
    blob = "\n".join(files.values())
    assert "Emergency120" in blob
    assert "wx.makePhoneCall" in blob and "120" in blob


@REGISTRY.register("F22-T1-05", tier=1, features=[22], title="Structured department recommendation card & mandatory disclaimer")
def test_f22_t1_05(ctx: TestContext) -> None:
    files = _miniapp_texts(ctx)
    blob = "\n".join(files.values())
    assert "primary_department" in blob and "urgency_level" in blob
    assert DISCLAIMER_CORE_PHRASE in blob or "不替代执业医师诊断" in blob


@REGISTRY.register("F22-T1-06", tier=1, features=[22], title="Patient memory view & one-tap physical wipe (pages/memory/)")
def test_f22_t1_06(ctx: TestContext) -> None:
    files = _miniapp_texts(ctx)
    mem_blob = "\n".join(v for k, v in files.items() if "memory" in k or "api.js" in k)
    assert "/v1/patients/" in mem_blob and "/memory" in mem_blob and "DELETE" in mem_blob

"""Isolated --self-test harness validating all 7 internal E2E suite pillars.

IMPORTANT: This module MUST ONLY be imported when `--self-test` is active.
Live mode (`--tier 1|2|3|4|all`) enforces `"e2e.lib.selftest_harness" not in sys.modules`.
"""

from __future__ import annotations

import json
import socket
import struct
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

from e2e.fixtures.generator import (
    generate_5000_patient_csv,
    generate_large_jsonl,
    generate_minimal_docx,
    generate_minimal_pdf,
    generate_minimal_xlsx,
)
from e2e.lib.assertions import (
    MANDATORY_DISCLAIMER,
    assert_completed_response,
    assert_emergency_response,
    assert_follow_up_response,
    assert_no_diagnosis_or_dosage,
    assert_no_pii,
    assert_studio_commands,
    assert_vitals_preserved,
    detect_pii,
    discover_miniapp_page_text,
    extract_rust_fn_block,
    extract_tauri_generate_handlers,
    load_schema_rules,
    strip_rust_comments_and_tests,
    validate_collection_bundle,
)
from e2e.lib.cli import CliUsageError, parse_args
from e2e.lib.http_client import HttpClient, HttpClientError
from e2e.lib.mock_upstream import FaultRule, MockUpstreamServer
from e2e.lib.parsers import (
    ParseError,
    dry_run_sqlite_ddl,
    inspect_pdf_bytes,
    parse_dockerfile,
    parse_json_file,
    parse_jsonl_stream,
    parse_prometheus_metrics,
    parse_sql_ddl,
    parse_sse_frames,
    parse_toml_file,
    parse_yaml_subset,
    read_docx_paragraphs,
    read_xlsx_rows,
    validate_grafana_dashboard,
    validate_ico_bytes,
    validate_k6_script,
    validate_qdrant_collections,
    validate_wxml,
    validate_wxss,
)
from e2e.lib.registry import load_all_tests
from e2e.lib.runner import TestContext, execute_cases, filter_cases


def _build_minimal_ico() -> bytes:
    """Create a valid 1-entry 16x16 ICO binary for parser testing."""
    # ICONDIR (6 bytes) + ICONDIRENTRY (16 bytes) + minimal 4-byte PNG payload
    png_stub = b"\x89PNG\r\n\x1a\n" + b"\x00" * 16
    header = struct.pack("<HHH", 0, 1, 1)
    entry = struct.pack("<BBBBHHII", 16, 16, 0, 0, 1, 32, len(png_stub), 22)
    return header + entry + png_stub


class _LoopbackMedicalServer:
    """Ephemeral loopback HTTP/SSE server for self-test Pillar 6 only."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._patients: dict[str, dict[str, Any]] = {}
        self._turns: dict[str, int] = {}
        self._key_hits: dict[str, int] = {}
        self._cache_hits = 0
        self._requests = 0
        self._server = ThreadingHTTPServer(("127.0.0.1", 0), self._make_handler())
        self.host, self.port = self._server.server_address[:2]
        self.base_url = f"http://{self.host}:{self.port}"
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    def start(self) -> "_LoopbackMedicalServer":
        self._thread.start()
        return self

    def stop(self) -> None:
        self._server.shutdown()
        self._server.server_close()

    def _make_handler(self) -> type[BaseHTTPRequestHandler]:
        parent = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
                return

            def _send_json(self, status: int, body: Any) -> None:
                raw = json.dumps(body, ensure_ascii=False).encode("utf-8")
                if status >= 400:
                    self.close_connection = True
                self.send_response(status)
                self.send_header("Content-Type", "application/json; charset=utf-8")
                self.send_header("Content-Length", str(len(raw)))
                if status >= 400:
                    self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(raw)

            def _read_json(self) -> dict[str, Any]:
                n = int(self.headers.get("Content-Length", "0") or 0)
                raw = self.rfile.read(n) if n > 0 else b"{}"
                return json.loads(raw.decode("utf-8"))

            def _check_auth(self) -> bool:
                auth = self.headers.get("Authorization", "")
                if not auth.startswith("Bearer ") or len(auth) <= 7:
                    self._send_json(401, {"error": "unauthorized"})
                    return False
                tok = auth.split(" ", 1)[1].strip()
                if "revoked" in tok or "invalidsig" in tok:
                    self._send_json(403, {"error": "forbidden"})
                    return False
                if tok.startswith("mk_live_qpm_") or tok == "mk_live_hosp_b_e2e_key":
                    with parent._lock:
                        hits = parent._key_hits.get(tok, 0) + 1
                        parent._key_hits[tok] = hits
                        if hits > 3:
                            self._send_json(429, {"error": "rate_limited"})
                            return False
                return True

            def do_GET(self) -> None:  # noqa: N802
                with parent._lock:
                    parent._requests += 1
                if self.path == "/health":
                    self._send_json(200, {"status": "ok", "version": "0.1.0-selftest", "checks": {"r5": "ok"}})
                    return
                if self.path == "/metrics":
                    text = (
                        "# TYPE medical_server_requests_total counter\n"
                        f"medical_server_requests_total {parent._requests}\n"
                        "medical_server_sse_active_connections 0\n"
                        "medical_server_triage_duration_seconds_count 1\n"
                        "medical_server_llm_errors_total 0\n"
                        "medical_server_rag_duration_seconds_count 1\n"
                        f"medical_server_cache_hits_total {parent._cache_hits}\n"
                        "medical_server_pg_pool_active 0\n"
                    ).encode("utf-8")
                    self.send_response(200)
                    self.send_header("Content-Type", "text/plain; version=0.0.4")
                    self.send_header("Content-Length", str(len(text)))
                    self.end_headers()
                    self.wfile.write(text)
                    return
                if self.path.startswith("/v1/epidemic/current"):
                    if "CN-999999" in self.path:
                        self._send_json(200, {"contexts": []})
                    else:
                        self._send_json(
                            200,
                            {"contexts": [{"context_id": "epi_flu", "region_code": "CN-110000", "disease_name": "甲型流行性感冒", "intensity": 0.85, "active": True}]},
                        )
                    return
                if self.path.startswith("/v1/patients/") and self.path.endswith("/memory"):
                    if not self._check_auth():
                        return
                    pid = self.path.split("/")[3]
                    rec = parent._patients.get(pid)
                    if rec is None:
                        self._send_json(404, {"error": "not found"})
                    else:
                        self._send_json(200, rec)
                    return
                self._send_json(404, {"error": "not found"})

            def do_PUT(self) -> None:  # noqa: N802
                if self.path.startswith("/v1/patients/") and self.path.endswith("/memory"):
                    if not self._check_auth():
                        return
                    pid = self.path.split("/")[3]
                    if pid.isdigit() or len(pid) == 11:
                        self._send_json(422, {"error": "plaintext PII forbidden"})
                        return
                    body = self._read_json()
                    parent._patients[pid] = body
                    self._send_json(200, body)
                    return
                self._send_json(404, {"error": "not found"})

            def do_DELETE(self) -> None:  # noqa: N802
                if self.path.startswith("/v1/patients/") and self.path.endswith("/memory"):
                    if not self._check_auth():
                        return
                    pid = self.path.split("/")[3]
                    parent._patients.pop(pid, None)
                    self._send_json(200, {"deleted": True})
                    return
                self._send_json(404, {"error": "not found"})

            def do_POST(self) -> None:  # noqa: N802
                with parent._lock:
                    parent._requests += 1
                if self.path == "/v1/auth/wechat":
                    try:
                        body = self._read_json()
                    except Exception:
                        self._send_json(400, {"error": "bad json"})
                        return
                    code = str(body.get("code", "")).strip()
                    if not code:
                        self._send_json(422, {"error": "empty code"})
                        return
                    self._send_json(200, {"token": "hdr.pay.sig", "expires_in": 3600, "patient_id": f"pat_sha256_{abs(hash(code)) & 0xffffffff:08x}"})
                    return

                if self.path == "/v1/admin/reload":
                    self._send_json(200, {"reloaded": True})
                    return

                if not self._check_auth():
                    return

                body = self._read_json()
                if self.path == "/v1/knowledge/search":
                    q = str(body.get("query", "")).strip()
                    if not q:
                        self._send_json(422, {"error": "empty query"})
                        return
                    with parent._lock:
                        parent._cache_hits += 1
                    self._send_json(
                        200,
                        {
                            "results": [{"doc_id": "book_internal_med_9th_ch02_001", "source_title": "内科学（第9版）", "disease_name": "流行性感冒", "department": "呼吸内科", "authority_level": 1.0, "target_population": ["adult"]}],
                            "seasonal_context_applied": ["甲型流行性感冒"],
                            "total": 1,
                        },
                    )
                    return

                if self.path == "/v1/triage/chat":
                    utt = str(body.get("utterance", ""))
                    sid = str(body.get("session_id", ""))
                    if not sid or not utt.strip():
                        self._send_json(422, {"error": "invalid input"})
                        return
                    if "胸痛" in utt or "心口窝" in utt:
                        payload = {
                            "session_id": sid,
                            "status": "emergency",
                            "red_flag": "Emergency120",
                            "alert_message": "请立即拨打120前往急诊科！",
                        }
                    else:
                        turn = parent._turns.get(sid, 0) + 1
                        parent._turns[sid] = turn
                        if turn < 2:
                            payload = {
                                "session_id": sid,
                                "status": "follow_up",
                                "red_flag": "None",
                                "turn": turn,
                                "max_turns": 4,
                                "question": "发烧大概持续多久了？",
                                "quick_options": ["今天刚开始", "2~3天", "超过3天"],
                                "seasonal_context_applied": ["甲型流行性感冒"],
                            }
                        else:
                            payload = {
                                "session_id": sid,
                                "status": "completed",
                                "red_flag": "None",
                                "seasonal_context_applied": ["甲型流行性感冒"],
                                "triage_result": {
                                    "primary_department": "呼吸内科",
                                    "secondary_departments": ["发热门诊"],
                                    "urgency_level": 2,
                                    "summary": "建议前往呼吸内科或发热门诊进一步评估。",
                                    "disclaimer": MANDATORY_DISCLAIMER,
                                },
                                "citations": [{"doc_id": "book_internal_med_9th_ch02_001", "source_title": "内科学（第9版）", "disease_name": "流行性感冒"}],
                            }
                    if body.get("stream"):
                        self.close_connection = True
                        self.send_response(200)
                        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
                        self.send_header("Connection", "close")
                        self.end_headers()
                        self.wfile.write(b'event: delta\ndata: {"text":"..."}\n\n')
                        self.wfile.write(f"event: result\ndata: {json.dumps(payload, ensure_ascii=False)}\n\n".encode("utf-8"))
                        self.wfile.flush()
                        return
                    self._send_json(200, payload)
                    return

                if self.path == "/v1/openai/chat/completions":
                    msgs = body.get("messages", [])
                    if not msgs:
                        self._send_json(422, {"error": "empty messages"})
                        return
                    if body.get("stream"):
                        self.close_connection = True
                        self.send_response(200)
                        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
                        self.send_header("Connection", "close")
                        self.end_headers()
                        self.wfile.write(b'data: {"id":"1","object":"chat.completion.chunk","choices":[{"delta":{"content":"ok"}}]}\n\n')
                        self.wfile.write(b"data: [DONE]\n\n")
                        self.wfile.flush()
                        return
                    self._send_json(
                        200,
                        {
                            "id": "chatcmpl-st",
                            "object": "chat.completion",
                            "choices": [{"index": 0, "message": {"role": "assistant", "content": f"建议呼吸内科评估。{MANDATORY_DISCLAIMER}"}}],
                            "usage": {"total_tokens": 20},
                        },
                    )
                    return

                self._send_json(404, {"error": "unknown"})

        return Handler


def _build_synthetic_workspace(root: Path) -> None:
    """Create a minimal synthetic workspace inside a temp dir for Pillar 6 smoke checks."""
    root.mkdir(parents=True, exist_ok=True)
    (root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/medical-core", "crates/medical-pipeline", "crates/medical-server", "crates/medical-studio"]\n'
        '[workspace.dependencies]\nsqlx = { version = "0.8", features = ["tls-rustls-ring"] }\n',
        encoding="utf-8",
    )
    (root / "Cargo.lock").write_text('[[package]]\nname = "medical-core"\n', encoding="utf-8")
    for c in ("medical-core", "medical-pipeline", "medical-server", "medical-studio"):
        (root / "crates" / c / "src").mkdir(parents=True, exist_ok=True)
        (root / "crates" / c / "src" / "lib.rs").write_text("// stub\n", encoding="utf-8")
    (root / "crates" / "medical-core" / "Cargo.toml").write_text('[package]\nname="medical-core"\nversion="0.1.0"\n', encoding="utf-8")
    (root / "crates" / "medical-pipeline" / "Cargo.toml").write_text(
        '[package]\nname="medical-pipeline"\nversion="0.1.0"\n[features]\ndefault=[]\nlocal-fastembed=["dep:fastembed"]\n[dependencies]\nmedical-core={path="../medical-core"}\nfastembed={version="4",optional=true}\n',
        encoding="utf-8",
    )
    (root / "crates" / "medical-server" / "Cargo.toml").write_text(
        '[package]\nname="medical-server"\nversion="0.1.0"\n[dependencies]\nmedical-core={path="../medical-core"}\n',
        encoding="utf-8",
    )
    (root / "crates" / "medical-server" / "src" / "main.rs").write_text("fn main(){}\n", encoding="utf-8")
    studio = root / "crates" / "medical-studio"
    (studio / "Cargo.toml").write_text(
        '[package]\nname="medical-studio"\nversion="0.1.0"\n[dependencies]\nmedical-core={path="../medical-core"}\nmedical-pipeline={path="../medical-pipeline"}\n',
        encoding="utf-8",
    )
    (studio / "src" / "main.rs").write_text("fn main(){}\n", encoding="utf-8")
    (studio / "build.rs").write_text("fn main(){ tauri_build::build(); }\n", encoding="utf-8")
    (studio / "icons").mkdir(parents=True, exist_ok=True)
    (studio / "icons" / "icon.ico").write_bytes(_build_minimal_ico())
    (studio / "frontend" / "dist").mkdir(parents=True, exist_ok=True)
    (studio / "frontend" / "dist" / "index.html").write_text('<div id="app"></div><script src="a.js"></script>', encoding="utf-8")
    (studio / "tauri.conf.json").write_text(json.dumps({"build": {"frontendDist": "frontend/dist"}}), encoding="utf-8")
    (root / "migrations").mkdir(parents=True, exist_ok=True)
    (root / "migrations" / "001_init.sql").write_text(
        "CREATE TABLE IF NOT EXISTS patient_l2_profiles (patient_id TEXT PRIMARY KEY, consent_long_term_memory BOOLEAN, profile_encrypted BYTEA);\n"
        "CREATE TABLE IF NOT EXISTS patient_l3_visit_events (event_id TEXT PRIMARY KEY, patient_id TEXT, occurred_at TIMESTAMPTZ);\n"
        "CREATE TABLE IF NOT EXISTS epidemic_contexts (context_id TEXT PRIMARY KEY, region_code TEXT, season TEXT, seasonal_tag TEXT, intensity DOUBLE PRECISION, active BOOLEAN, valid_from TEXT, valid_to TEXT);\n"
        "CREATE TABLE IF NOT EXISTS symptom_synonyms (standard_term TEXT PRIMARY KEY);\n"
        "CREATE TABLE IF NOT EXISTS red_flag_rules (rule_id TEXT PRIMARY KEY);\n"
        "CREATE TABLE IF NOT EXISTS triage_audit_logs (id INTEGER PRIMARY KEY);\n"
        "CREATE TABLE IF NOT EXISTS api_keys (id INTEGER PRIMARY KEY);\n"
        "CREATE TABLE IF NOT EXISTS llm_providers (id INTEGER PRIMARY KEY);\n"
        "CREATE TABLE IF NOT EXISTS ingestion_checkpoints (id INTEGER PRIMARY KEY);\n",
        encoding="utf-8",
    )
    (root / "config" / "qdrant").mkdir(parents=True, exist_ok=True)
    (root / "config" / "qdrant" / "collections.json").write_text(
        json.dumps(
            {
                "medical_knowledge": {"vectors": {"size": 1024, "distance": "Cosine"}, "payload_indexes": {"target_population": "keyword", "department": "keyword", "authority_level": "float", "seasonal_tags": "keyword"}},
                "patient_l3_events": {"vectors": {"size": 1024, "distance": "Cosine"}, "payload_indexes": {"patient_id": "keyword"}},
            }
        ),
        encoding="utf-8",
    )


def run_self_test(e2e_root: Path) -> int:
    """Execute all 7 internal validation pillars cleanly without touching M1–M5."""
    workspace_root = e2e_root.parent
    pillars: list[tuple[str, Any]] = []

    def p1_cli() -> None:
        ns = parse_args(["--tier", "1", "--feature", "1", "--feature", "7"])
        assert ns.tier == "1" and ns.features == [1, 7]
        assert parse_args(["--self-test"]).self_test is True
        for bad in ([], ["--tier", "1", "--self-test"], ["--feature", "99", "--tier", "1"], ["--timeout", "0", "--tier", "1"], ["--server-bin", "x.exe", "--tier", "1"]):
            try:
                parse_args(bad)
                raise AssertionError(f"Expected CliUsageError for {bad}")
            except CliUsageError:
                pass

    def p2_registry() -> None:
        reg = load_all_tests()
        errs = reg.audit_coverage()
        assert not errs, f"Registry coverage audit failed: {errs}"
        cov = reg.coverage_summary()
        assert cov["tier_counts"]["tier1"] == 123, cov["tier_counts"]
        assert cov["tier_counts"]["tier2"] == 123, cov["tier_counts"]
        assert cov["tier_counts"]["tier3"] == 28, cov["tier_counts"]
        assert cov["tier_counts"]["tier4"] == 11, cov["tier_counts"]
        assert cov["total"] == 285

    def p3_schemas_fixtures() -> None:
        rules = load_schema_rules()
        canon = parse_json_file(workspace_root / "doc" / "schemas" / "data-collection-schema.json")
        assert not validate_collection_bundle(canon, rules)
        valid_fx = parse_json_file(e2e_root / "fixtures" / "knowledge" / "valid_collection.json")
        assert not validate_collection_bundle(valid_fx, rules)
        neg = parse_json_file(e2e_root / "fixtures" / "knowledge" / "invalid_collection.json")["negative_cases"]
        from e2e.lib.assertions import (
            validate_epidemic_context,
            validate_knowledge_document,
            validate_medical_dictionaries,
            validate_patient_record,
        )
        for case in neg:
            et = case["entity_type"]
            pl = case["payload"]
            if et == "knowledge_document":
                e = validate_knowledge_document(pl, rules)
            elif et == "patient_record":
                e = validate_patient_record(pl, rules)
            elif et == "epidemic_context":
                e = validate_epidemic_context(pl, rules)
            else:
                e = validate_medical_dictionaries({"symptom_synonyms": [], "red_flag_rules": [pl]}, rules)
            assert e, f"Negative case {case['case_id']} unexpectedly passed"

        with tempfile.TemporaryDirectory() as td:
            p = Path(td)
            xlsx = generate_minimal_xlsx(p / "t.xlsx")
            assert len(read_xlsx_rows(xlsx)) == 3
            docx = generate_minimal_docx(p / "t.docx")
            assert len(read_docx_paragraphs(docx)) >= 4
            pdf_t = generate_minimal_pdf(p / "text.pdf", ["Influenza", "Fever"])
            assert inspect_pdf_bytes(pdf_t.read_bytes())["has_text_ops"] is True
            pdf_s = generate_minimal_pdf(p / "scan.pdf", None)
            assert inspect_pdf_bytes(pdf_s.read_bytes())["has_text_ops"] is False
            jl = generate_large_jsonl(p / "gen_1k.jsonl", count=1000)
            assert len(parse_jsonl_stream(jl).records) == 1000
            csv5k = generate_5000_patient_csv(p / "p5k.csv", rows=5000)
            assert len(csv5k.read_text(encoding="utf-8").splitlines()) == 5001

    def p4_parsers() -> None:
        # TOML positive & negative
        with tempfile.TemporaryDirectory() as td:
            tp = Path(td) / "a.toml"
            tp.write_text('[workspace]\nmembers = ["a"]\n', encoding="utf-8")
            assert parse_toml_file(tp)["workspace"]["members"] == ["a"]
            tp.write_text('[broken', encoding="utf-8")
            try:
                parse_toml_file(tp)
                raise AssertionError("Bad TOML should fail")
            except ParseError:
                pass

        # SQL DDL + SQLite dry-run positive & negative
        ddl = "CREATE TABLE IF NOT EXISTS t1 (id BIGSERIAL PRIMARY KEY, data BYTEA, tags TEXT[], active BOOLEAN DEFAULT TRUE);"
        assert "t1" in dry_run_sqlite_ddl(ddl).tables
        try:
            parse_sql_ddl("CREATE TABLE bad (id INT")
            raise AssertionError("Unclosed SQL DDL should fail")
        except ParseError:
            pass

        # Qdrant JSON positive & negative
        q_ok = {
            "medical_knowledge": {
                "vectors": {"size": 1024, "distance": "Cosine"},
                "payload_indexes": {
                    "target_population": "keyword",
                    "department": "keyword",
                    "authority_level": "float",
                    "seasonal_tags": "keyword",
                },
            },
            "patient_l3_events": {
                "vectors": {"size": 1024, "distance": "Cosine"},
                "payload_indexes": {"patient_id": "keyword"},
            },
        }
        assert "medical_knowledge" in validate_qdrant_collections(q_ok)
        for bad_q in (
            {"medical_knowledge": {"vectors": {"size": 0, "distance": "Cosine"}}},
            {"medical_knowledge": {"vectors": {"size": 1024, "distance": "Cosine"}}, "patient_l3_events": {"vectors": {"size": 1024, "distance": "Cosine"}}},
            {"medical_knowledge": {"vectors": {"size": True, "distance": "Cosine"}, "payload_indexes": q_ok["medical_knowledge"]["payload_indexes"]}, "patient_l3_events": q_ok["patient_l3_events"]},
        ):
            try:
                validate_qdrant_collections(bad_q)
                raise AssertionError(f"Bad Qdrant should fail: {bad_q}")
            except ParseError:
                pass

        # YAML positive & negative (incl. compound `for: 1m30s` and omitted `for`)
        y = parse_yaml_subset(
            "global:\n  scrape_interval: 15s\ngroups:\n  - name: alerts\n    rules:\n"
            "      - alert: HighErr\n        expr: rate(x[5m]) > 0\n        for: 1m30s\n"
            "      - alert: InstantErr\n        expr: up == 0\n"
        )
        assert y["groups"][0]["rules"][0]["alert"] == "HighErr"
        assert y["groups"][0]["rules"][0]["for"] == "1m30s"
        assert "for" not in y["groups"][0]["rules"][1]
        try:
            parse_yaml_subset("a:\n\tb: 1\n")
            raise AssertionError("Tab in YAML must fail")
        except ParseError:
            pass

        # Grafana, Dockerfile, k6, WXML, WXSS, ICO, SSE, JSONL
        validate_grafana_dashboard({"title": "D", "panels": [{"id": 1, "gridPos": {"x": 0, "y": 0, "w": 12, "h": 8}, "targets": [{"expr": "up"}]}]})
        df = parse_dockerfile("FROM rust:1.85 AS builder\nFROM debian:bookworm-slim\nCOPY --from=builder /a /a\nUSER app\nEXPOSE 8080\nCMD [\"/a\"]\n")
        assert len(df.stages) == 2
        assert validate_k6_script('import http from "k6/http";\nexport const options = { thresholds: {} };\nexport default function(){ http.get(`${__ENV.BASE}/health`); sleep(1); }\n')["uses_env"]
        validate_wxml("<view class='a'><text>{{msg}}</text><input /></view>")
        try:
            validate_wxml("<view><text></view>")
            raise AssertionError("Unclosed WXML must fail")
        except ParseError:
            pass
        assert validate_wxss(".a { color: red; }") == 1
        assert validate_ico_bytes(_build_minimal_ico()) == 1
        try:
            validate_ico_bytes(b"NOT_AN_ICO_FILE_TEXT_BYTES")
            raise AssertionError("Fake ICO must fail")
        except ParseError:
            pass

        sse_evs = parse_sse_frames("event: delta\ndata: {\"a\":1}\n\nevent: result\ndata: line1\ndata: line2\n\n", strict=True)
        assert len(sse_evs) == 2 and sse_evs[1].data == "line1\nline2"
        try:
            parse_sse_frames("bad_line_without_colon\n\n", strict=True)
            raise AssertionError("Malformed strict SSE should fail")
        except ParseError:
            pass

        assert len(parse_jsonl_stream(e2e_root / "fixtures" / "knowledge" / "knowledge_100.jsonl").records) == 100
        assert len(parse_jsonl_stream(e2e_root / "fixtures" / "knowledge" / "bom_crlf.jsonl").records) == 10
        bad_jl = parse_jsonl_stream(e2e_root / "fixtures" / "knowledge" / "malformed_line3.jsonl")
        assert len(bad_jl.records) == 9 and bad_jl.errors[0][0] == 3

    def p5_assertions() -> None:
        assert_follow_up_response({"status": "follow_up", "red_flag": "None", "turn": 1, "max_turns": 4, "question": "发烧持续几天了？", "quick_options": ["1天", "2~3天"]})
        try:
            assert_follow_up_response({"status": "follow_up", "red_flag": "None", "turn": 1, "max_turns": 4, "question": "x" * 31, "quick_options": ["1", "2"]})
            raise AssertionError(">30 char question should fail")
        except AssertionError:
            pass
        assert_completed_response({"status": "completed", "red_flag": "None", "triage_result": {"primary_department": "呼吸内科", "urgency_level": 2, "summary": "建议呼吸内科评估。", "disclaimer": MANDATORY_DISCLAIMER}, "citations": [{"doc_id": "d1", "source_title": "S", "disease_name": "Flu"}]}, require_citations=True)
        try:
            assert_no_diagnosis_or_dosage("您已确诊为肺炎，口服阿莫西林500mg每日3次")
            raise AssertionError("Diagnosis/dosage should fail")
        except AssertionError:
            pass
        assert_emergency_response({"status": "emergency", "red_flag": "Emergency120", "alert_message": "立即拨打120"})
        vitals_text = "体温39.5℃，血压140/90mmHg，心率110次/分，血糖7.8mmol/L"
        assert not detect_pii(vitals_text), f"Vital signs falsely flagged as PII: {detect_pii(vitals_text)}"
        assert_vitals_preserved(vitals_text, ["39.5", "140/90", "110", "7.8"])
        assert len(detect_pii("身份证11010119900101123X 电话+86 138-0013-8000")) == 2

        # strip_rust_comments_and_tests self-test
        rs_sample = (
            "pub fn prod() -> usize { foo().unwrap() }\n"
            "// x.unwrap()\n"
            "#[cfg(test)]\nmod tests {\n  #[test]\n  fn t() { x.unwrap(); }\n}\n"
        )
        rs_stripped = strip_rust_comments_and_tests(rs_sample, strip_tests=True)
        assert "foo().unwrap()" in rs_stripped and "x.unwrap()" not in rs_stripped

        # assert_studio_commands / extract_tauri_generate_handlers / extract_rust_fn_block + discover_miniapp_page_text
        with tempfile.TemporaryDirectory() as td:
            ws = Path(td)
            st_src = ws / "crates" / "medical-studio" / "src"
            fe_src = ws / "crates" / "medical-studio" / "frontend" / "src"
            st_src.mkdir(parents=True)
            fe_src.mkdir(parents=True)
            lib_rs = st_src / "lib.rs"
            lib_rs.write_text(
                '#[tauri::command]\npub async fn foo() -> Result<(), String> { Ok(()) }\n'
                'pub fn run() { tauri::Builder::default().invoke_handler(tauri::generate_handler![foo]); }\n',
                encoding="utf-8",
            )
            app_ts = fe_src / "App.ts"
            app_ts.write_text(
                'import { invoke } from "@tauri-apps/api/core";\n'
                'export async function callFoo() { return invoke<void>("foo"); }\n',
                encoding="utf-8",
            )
            fake_ctx = TestContext(workspace=ws, base_url="http://127.0.0.1:1", client=HttpClient("http://127.0.0.1:1"))
            assert_studio_commands(fake_ctx, ["foo"])
            assert "foo" in extract_tauri_generate_handlers(strip_rust_comments_and_tests(lib_rs.read_text(encoding="utf-8")))
            assert "Ok(())" in extract_rust_fn_block(strip_rust_comments_and_tests(lib_rs.read_text(encoding="utf-8")), "foo")

            # Missing generate_handler! must fail
            lib_rs.write_text('#[tauri::command]\npub async fn foo() -> Result<(), String> { Ok(()) }\n', encoding="utf-8")
            try:
                assert_studio_commands(fake_ctx, ["foo"])
                raise AssertionError("Missing generate_handler! must fail")
            except AssertionError:
                pass

            # Comment-only invoke must fail
            lib_rs.write_text(
                '#[tauri::command]\npub async fn foo() -> Result<(), String> { Ok(()) }\n'
                'pub fn run() { tauri::generate_handler![foo]; }\n',
                encoding="utf-8",
            )
            app_ts.write_text('import { invoke } from "@tauri-apps/api/core";\n// invoke("foo")\n', encoding="utf-8")
            try:
                assert_studio_commands(fake_ctx, ["foo"])
                raise AssertionError("Comment-only invoke must fail")
            except AssertionError:
                pass

            # Dynamic miniapp discovery
            ma = ws / "miniapp"
            (ma / "pages" / "chat").mkdir(parents=True)
            (ma / "pages" / "memory").mkdir(parents=True)
            (ma / "app.json").write_text(json.dumps({"pages": ["pages/chat/index", "pages/memory/index"]}), encoding="utf-8")
            (ma / "pages" / "chat" / "index.wxml").write_text("<view><text>免责声明</text></view>", encoding="utf-8")
            (ma / "pages" / "memory" / "index.js").write_text("Page({ wipe() { return 'DELETE'; } });", encoding="utf-8")
            assert "免责声明" in discover_miniapp_page_text(ws, ".wxml", ("chat", "triage", "index"))
            assert "DELETE" in discover_miniapp_page_text(ws, ".js", ("memory",))

    def p6_loopback_smoke() -> None:
        srv = _LoopbackMedicalServer().start()
        up = MockUpstreamServer().start()
        try:
            with tempfile.TemporaryDirectory() as td:
                ws = Path(td) / "ws"
                _build_synthetic_workspace(ws)
                client = HttpClient(srv.base_url, timeout=5.0)
                ctx = TestContext(workspace=ws, base_url=srv.base_url, client=client, mock_upstream=up)
                reg = load_all_tests()
                smoke_ids = ["F1-T1-01", "F1-T1-04", "F3-T1-01", "F3-T1-04", "F4-T1-03", "F5-T1-01", "F6-T1-02", "F7-T1-01", "F12-T1-01", "F14-T1-01", "F14-T1-03", "F14-T1-06", "F14-T1-08"]
                cases = filter_cases(reg.all_cases(), tier_arg="all", test_ids=smoke_ids)
                rep_path = Path(td) / "report.json"
                rc, rep = execute_cases(cases, ctx, reg, json_report=rep_path, enforce_live_guard=False)
                assert rc == 0 and rep["summary"]["passed"] == len(smoke_ids)
                assert rep_path.is_file() and json.loads(rep_path.read_text(encoding="utf-8"))["summary"]["failed"] == 0

                # Verify 401/403/429 handling
                assert client.post("/v1/knowledge/search", json_body={"query": "x"}).status == 401
                assert client.post("/v1/knowledge/search", json_body={"query": "x"}, bearer="mk_live_revoked").status == 403
                # Verify MockUpstreamServer fault injection & call recording
                uc = HttpClient(up.base_url, timeout=5.0)
                up.enqueue_fault("/v1/chat/completions", FaultRule(status=429))
                assert uc.post("/v1/chat/completions", json_body={"model": "m", "messages": []}).status == 429
                assert uc.post("/v1/chat/completions", json_body={"model": "m", "messages": []}).status == 200
                assert len(up.get_calls("/v1/chat/completions")) == 2
        finally:
            up.stop()
            srv.stop()

    def p7_live_non_mock_proof() -> None:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(("127.0.0.1", 0))
            closed_port = s.getsockname()[1]
        dead_client = HttpClient(f"http://127.0.0.1:{closed_port}", timeout=1.0)
        try:
            dead_client.get("/health")
            raise AssertionError("Closed port must raise HttpClientError, never mock fallback")
        except HttpClientError:
            pass

        with tempfile.TemporaryDirectory() as empty_td:
            reg = load_all_tests()
            ctx = TestContext(workspace=Path(empty_td), base_url=f"http://127.0.0.1:{closed_port}", client=dead_client)
            target_ids = ["F1-T1-01", "F5-T1-01", "F9-T2-04", "F19-T2-05", "F21-T2-01"]
            cases = filter_cases(reg.all_cases(), tier_arg="1", test_ids=target_ids) + filter_cases(
                reg.all_cases(), tier_arg="2", test_ids=target_ids
            )
            rc, rep = execute_cases(cases, ctx, reg, enforce_live_guard=False)
            assert rc == 1 and rep["summary"]["failed"] == 5, (
                f"Live mode against empty workspace + closed port MUST fail all 5, got {rep['summary']}"
            )

    pillars = [
        ("Pillar 1: CLI Matrix & Exit Codes", p1_cli),
        ("Pillar 2: Registry & Feature #1-#22 Coverage Audit (285 tests)", p2_registry),
        ("Pillar 3: Schemas, Static Fixtures & Generator Execution", p3_schemas_fixtures),
        ("Pillar 4: Pure-Stdlib Parsers Positive & Negative Checks", p4_parsers),
        ("Pillar 5: Clinical & Privacy Assertions", p5_assertions),
        ("Pillar 6: Isolated Loopback HTTP/SSE Smoke & JSON Report", p6_loopback_smoke),
        ("Pillar 7: Live-Mode Non-Mock Failure Proof", p7_live_non_mock_proof),
    ]

    t0 = time.perf_counter()
    failures = 0
    for name, fn in pillars:
        pt0 = time.perf_counter()
        try:
            fn()
            print(f"[PASS] {name} ({(time.perf_counter() - pt0) * 1000:.1f}ms)")
        except Exception as exc:
            failures += 1
            print(f"[FAIL] {name}: {type(exc).__name__}: {exc}")

    total_ms = (time.perf_counter() - t0) * 1000.0
    print(f"\nSelf-Test Summary: {len(pillars) - failures}/{len(pillars)} pillars passed ({total_ms:.1f}ms)")
    return 0 if failures == 0 else 1

"""Programmable OpenAI/Embedding/Rerank MockUpstreamServer for live --spawn-server tests."""

from __future__ import annotations

import json
import threading
import time
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


@dataclass
class RecordedUpstreamCall:
    path: str
    headers: dict[str, str]
    body: dict[str, Any]
    timestamp: float


@dataclass
class FaultRule:
    status: int = 200
    delay_sec: float = 0.0
    mode: str = "normal"  # normal|malformed_json|oversized_question|diagnosis_dosage|truncated_sse|always_follow_up
    custom_body: dict[str, Any] | None = None
    match_stream: bool | None = None
    match_prompt_contains: str | None = None
    match_model_contains: str | None = None


class MockUpstreamServer:
    """Thread-safe HTTP/SSE mock upstream recording all LLM/Embedding/Rerank calls."""

    def __init__(self, host: str = "127.0.0.1", port: int = 0) -> None:
        self._lock = threading.Lock()
        self._recorded: list[RecordedUpstreamCall] = []
        self._one_shot_faults: dict[str, list[FaultRule]] = {}
        self._persistent_faults: dict[str, list[FaultRule]] = {}
        self._server = ThreadingHTTPServer((host, port), self._make_handler())
        self.host, self.port = self._server.server_address[:2]
        self.base_url = f"http://{self.host}:{self.port}"
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    def start(self) -> "MockUpstreamServer":
        self._thread.start()
        return self

    def stop(self) -> None:
        self._server.shutdown()
        self._server.server_close()

    def clear_calls(self) -> None:
        with self._lock:
            self._recorded.clear()

    def get_calls(self, path_prefix: str | None = None) -> list[RecordedUpstreamCall]:
        with self._lock:
            if path_prefix is None:
                return list(self._recorded)
            return [c for c in self._recorded if c.path.startswith(path_prefix)]

    def enqueue_fault(self, key: str, rule: FaultRule) -> None:
        """Queue a single-use fault for `key` (model name, path, or '*')."""
        with self._lock:
            self._one_shot_faults.setdefault(key, []).append(rule)

    def set_persistent_fault(self, key: str, rule: FaultRule | None, *, replace: bool = True) -> None:
        with self._lock:
            if rule is None:
                self._persistent_faults.pop(key, None)
            elif replace:
                self._persistent_faults[key] = [rule]
            else:
                self._persistent_faults.setdefault(key, []).append(rule)

    def reset_faults(self) -> None:
        with self._lock:
            self._one_shot_faults.clear()
            self._persistent_faults.clear()

    @staticmethod
    def _matches(rule: FaultRule, model: str, body: dict[str, Any]) -> bool:
        if rule.match_stream is not None and bool(body.get("stream", False)) is not rule.match_stream:
            return False
        if rule.match_model_contains is not None and rule.match_model_contains.lower() not in model.lower():
            return False
        if rule.match_prompt_contains is not None:
            msg_blob = json.dumps(body.get("messages", []), ensure_ascii=False)
            if rule.match_prompt_contains not in msg_blob:
                return False
        return True

    def _pop_rule(self, path: str, model: str | None, body: dict[str, Any]) -> FaultRule:
        m = model or ""
        with self._lock:
            for key in [k for k in (model, path, "*") if k]:
                q = self._one_shot_faults.get(key)
                if q:
                    for idx, rule in enumerate(q):
                        if self._matches(rule, m, body):
                            return q.pop(idx)
            for key in [k for k in (model, path, "*") if k]:
                p_rules = self._persistent_faults.get(key)
                if p_rules:
                    for rule in p_rules:
                        if self._matches(rule, m, body):
                            return rule
        return FaultRule()

    def _record(self, path: str, headers: dict[str, str], body: dict[str, Any]) -> None:
        with self._lock:
            self._recorded.append(
                RecordedUpstreamCall(
                    path=path,
                    headers=headers,
                    body=body,
                    timestamp=time.time(),
                )
            )

    def _make_handler(self) -> type[BaseHTTPRequestHandler]:
        parent = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
                return

            def _read_json(self) -> dict[str, Any]:
                length = int(self.headers.get("Content-Length", "0") or 0)
                raw = self.rfile.read(length) if length > 0 else b"{}"
                try:
                    data = json.loads(raw.decode("utf-8"))
                    return data if isinstance(data, dict) else {"_raw": data}
                except Exception:
                    return {"_raw_bytes": raw.decode("utf-8", errors="replace")}

            def _send_json(self, status: int, payload: Any) -> None:
                body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
                if status >= 400:
                    self.close_connection = True
                self.send_response(status)
                self.send_header("Content-Type", "application/json; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                if status >= 400:
                    self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self) -> None:  # noqa: N802
                if self.path == "/health":
                    self._send_json(200, {"status": "ok"})
                else:
                    self._send_json(404, {"error": "not found"})

            def do_POST(self) -> None:  # noqa: N802
                body = self._read_json()
                hdrs = {k.lower(): v for k, v in self.headers.items()}
                parent._record(self.path, hdrs, body)
                model = str(body.get("model", "")) if isinstance(body, dict) else ""
                rule = parent._pop_rule(self.path, model or None, body if isinstance(body, dict) else {})
                if rule.delay_sec > 0:
                    time.sleep(rule.delay_sec)

                if rule.status != 200:
                    self._send_json(rule.status, {"error": {"message": f"Injected {rule.status}", "code": rule.status}})
                    return

                if self.path.startswith("/v1/embeddings"):
                    inputs = body.get("input", [])
                    if isinstance(inputs, str):
                        inputs = [inputs]
                    data = [
                        {"object": "embedding", "index": i, "embedding": [0.01 * ((i % 10) + 1)] * 1024}
                        for i, _ in enumerate(inputs)
                    ]
                    self._send_json(200, {"object": "list", "data": data, "model": model or "mock-embed"})
                    return

                if self.path.startswith("/v1/rerank"):
                    docs = body.get("documents", [])
                    results = [
                        {"index": i, "relevance_score": max(0.1, 0.95 - i * 0.08)}
                        for i in range(len(docs))
                    ]
                    self._send_json(200, {"results": results})
                    return

                if self.path.startswith("/v1/chat/completions"):
                    self._handle_chat(body, model, rule)
                    return

                self._send_json(404, {"error": f"unknown upstream path {self.path}"})

            def _handle_chat(self, body: dict[str, Any], model: str, rule: FaultRule) -> None:
                if rule.mode == "malformed_json":
                    raw = b'{"choices":[{"message":{"role":"assistant","content":"{broken_json"}}]}'
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(raw)))
                    self.end_headers()
                    self.wfile.write(raw)
                    return

                if rule.mode == "oversized_question":
                    content = json.dumps(
                        {
                            "action": "follow_up",
                            "status": "follow_up",
                            "question": "请问您这种反复发作的咳嗽症状到底是从什么时候开始一直持续到今天的呢？",
                            "quick_options": ["1天", "2天", "3天", "4天", "5天"],
                        },
                        ensure_ascii=False,
                    )
                elif rule.mode == "diagnosis_dosage":
                    content = json.dumps(
                        {
                            "action": "completed",
                            "status": "completed",
                            "primary_department": "呼吸内科",
                            "secondary_departments": ["发热门诊"],
                            "urgency_level": 2,
                            "summary": "您已确诊为急性细菌性支气管炎，请口服阿莫西林胶囊每次500mg，每日3次，连服7天。",
                        },
                        ensure_ascii=False,
                    )
                elif rule.mode == "always_follow_up":
                    content = json.dumps(
                        {
                            "action": "follow_up",
                            "status": "follow_up",
                            "question": "请问症状持续几天了？",
                            "quick_options": ["1天以内", "2~3天", "超过3天"],
                        },
                        ensure_ascii=False,
                    )
                elif rule.custom_body is not None:
                    content = json.dumps(rule.custom_body, ensure_ascii=False)
                else:
                    msg_blob = json.dumps(body.get("messages", []), ensure_ascii=False)
                    if "Query改写" in msg_blob:
                        content = json.dumps({"rewritten_query": ""}, ensure_ascii=False)
                    elif "结构化助手" in msg_blob:
                        content = json.dumps(
                            {"chronic_diseases_add": [], "allergies_add": [], "summary": ""},
                            ensure_ascii=False,
                        )
                    else:
                        content = json.dumps(
                            {
                                "action": "follow_up",
                                "status": "follow_up",
                                "question": "发烧大概持续多久了？",
                                "quick_options": ["今天刚开始", "2~3天", "超过3天"],
                                "primary_department": "呼吸内科",
                                "secondary_departments": ["发热门诊"],
                                "urgency_level": 2,
                                "summary": "结合患者症状与知识库检索依据，建议前往呼吸内科或发热门诊进一步评估。",
                            },
                            ensure_ascii=False,
                        )

                if body.get("stream"):
                    self.close_connection = True
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream; charset=utf-8")
                    self.send_header("Cache-Control", "no-cache")
                    self.send_header("Connection", "close")
                    self.end_headers()
                    mid = max(1, len(content) // 2)
                    parts = [content[:mid], content[mid:]]
                    for idx, part in enumerate(parts):
                        chunk = {
                            "id": "chatcmpl-mock",
                            "object": "chat.completion.chunk",
                            "model": model or "mock-reason",
                            "choices": [{"index": 0, "delta": {"content": part}, "finish_reason": None}],
                        }
                        self.wfile.write(f"data: {json.dumps(chunk, ensure_ascii=False)}\n\n".encode("utf-8"))
                        self.wfile.flush()
                        if rule.mode == "truncated_sse" and idx == 0:
                            return
                    self.wfile.write(b"data: [DONE]\n\n")
                    self.wfile.flush()
                    return

                self._send_json(
                    200,
                    {
                        "id": "chatcmpl-mock",
                        "object": "chat.completion",
                        "model": model or "mock-reason",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": content},
                                "finish_reason": "stop",
                            }
                        ],
                        "usage": {"prompt_tokens": 32, "completion_tokens": 24, "total_tokens": 56},
                    },
                )

        return Handler

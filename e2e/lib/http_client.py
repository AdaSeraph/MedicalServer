"""Pure stdlib HTTP/SSE client for MedicalServer E2E tests (zero mock fallback)."""

from __future__ import annotations

import http.client
import json
import socket
import time
import urllib.parse
from dataclasses import dataclass, field
from typing import Any

from e2e.lib.parsers import SseEvent, parse_sse_frames


class HttpClientError(RuntimeError):
    """Raised on network/connection/timeout failure against live server."""


@dataclass
class HttpResponse:
    status: int
    headers: dict[str, str]
    body_bytes: bytes
    elapsed_ms: float

    @property
    def text(self) -> str:
        return self.body_bytes.decode("utf-8", errors="replace")

    def json(self) -> Any:
        try:
            return json.loads(self.text)
        except json.JSONDecodeError as exc:
            raise AssertionError(
                f"Expected JSON response (status={self.status}), got: {self.text[:300]!r}"
            ) from exc


@dataclass
class SseResponse:
    status: int
    headers: dict[str, str]
    raw_text: str
    events: list[SseEvent] = field(default_factory=list)
    ttft_ms: float = 0.0
    total_ms: float = 0.0


class HttpClient:
    """Thread-safe stateless HTTP/SSE client built on http.client."""

    def __init__(
        self,
        base_url: str,
        *,
        timeout: float = 10.0,
        default_headers: dict[str, str] | None = None,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.default_headers = dict(default_headers or {})
        parsed = urllib.parse.urlparse(self.base_url)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError(f"Invalid base_url: {base_url!r}")
        self._scheme = parsed.scheme
        self._host = parsed.hostname
        self._port = parsed.port or (443 if self._scheme == "https" else 80)
        self._base_path = parsed.path.rstrip("/")

    def _connect(self, timeout: float | None = None) -> http.client.HTTPConnection:
        to = timeout if timeout is not None else self.timeout
        if self._scheme == "https":
            return http.client.HTTPSConnection(self._host, self._port, timeout=to)
        return http.client.HTTPConnection(self._host, self._port, timeout=to)

    def _build_path(self, path: str) -> str:
        if not path.startswith("/"):
            path = "/" + path
        return f"{self._base_path}{path}"

    def _merge_headers(
        self,
        headers: dict[str, str] | None,
        *,
        bearer: str | None = None,
        has_json: bool = False,
    ) -> dict[str, str]:
        by_lower: dict[str, tuple[str, str]] = {}
        for k, v in self.default_headers.items():
            by_lower[k.lower()] = (k, v)
        if has_json and "content-type" not in by_lower:
            by_lower["content-type"] = ("Content-Type", "application/json")
        if headers:
            for k, v in headers.items():
                by_lower[k.lower()] = (k, v)
        if bearer is not None:
            by_lower["authorization"] = ("Authorization", f"Bearer {bearer}")
        return {orig_k: val for orig_k, val in by_lower.values()}

    def request(
        self,
        method: str,
        path: str,
        *,
        json_body: Any = None,
        raw_body: bytes | str | None = None,
        headers: dict[str, str] | None = None,
        bearer: str | None = None,
        timeout: float | None = None,
    ) -> HttpResponse:
        if json_body is not None and raw_body is not None:
            raise ValueError("Pass either json_body or raw_body, not both")

        payload: bytes | None = None
        if json_body is not None:
            payload = json.dumps(json_body, ensure_ascii=False).encode("utf-8")
        elif isinstance(raw_body, str):
            payload = raw_body.encode("utf-8")
        elif isinstance(raw_body, bytes):
            payload = raw_body

        hdrs = self._merge_headers(headers, bearer=bearer, has_json=(json_body is not None))
        full_path = self._build_path(path)
        t0 = time.perf_counter()
        conn = self._connect(timeout=timeout)
        try:
            conn.request(method.upper(), full_path, body=payload, headers=hdrs)
            resp = conn.getresponse()
            body = resp.read()
            elapsed = (time.perf_counter() - t0) * 1000.0
            resp_headers = {k.lower(): v for k, v in resp.getheaders()}
            return HttpResponse(
                status=resp.status,
                headers=resp_headers,
                body_bytes=body,
                elapsed_ms=elapsed,
            )
        except (OSError, http.client.HTTPException, socket.timeout) as exc:
            raise HttpClientError(
                f"{method.upper()} {self.base_url}{path} failed: {type(exc).__name__}: {exc}"
            ) from exc
        finally:
            conn.close()

    def get(self, path: str, **kwargs: Any) -> HttpResponse:
        return self.request("GET", path, **kwargs)

    def post(self, path: str, **kwargs: Any) -> HttpResponse:
        return self.request("POST", path, **kwargs)

    def put(self, path: str, **kwargs: Any) -> HttpResponse:
        return self.request("PUT", path, **kwargs)

    def delete(self, path: str, **kwargs: Any) -> HttpResponse:
        return self.request("DELETE", path, **kwargs)

    def post_sse(
        self,
        path: str,
        *,
        json_body: Any,
        headers: dict[str, str] | None = None,
        bearer: str | None = None,
        timeout: float | None = None,
    ) -> SseResponse:
        payload = json.dumps(json_body, ensure_ascii=False).encode("utf-8")
        hdrs = self._merge_headers(headers, bearer=bearer, has_json=True)
        hdrs.setdefault("Accept", "text/event-stream")
        full_path = self._build_path(path)

        t0 = time.perf_counter()
        conn = self._connect(timeout=timeout)
        try:
            conn.request("POST", full_path, body=payload, headers=hdrs)
            resp = conn.getresponse()
            resp_headers = {k.lower(): v for k, v in resp.getheaders()}
            chunks: list[bytes] = []
            ttft_ms = 0.0
            while True:
                line = resp.readline()
                if not line:
                    break
                if ttft_ms == 0.0 and line.strip():
                    ttft_ms = (time.perf_counter() - t0) * 1000.0
                chunks.append(line)
            total_ms = (time.perf_counter() - t0) * 1000.0
            raw_text = b"".join(chunks).decode("utf-8", errors="replace")
            events = parse_sse_frames(raw_text, strict=False)
            return SseResponse(
                status=resp.status,
                headers=resp_headers,
                raw_text=raw_text,
                events=events,
                ttft_ms=ttft_ms or total_ms,
                total_ms=total_ms,
            )
        except (OSError, http.client.HTTPException, socket.timeout) as exc:
            raise HttpClientError(
                f"SSE POST {self.base_url}{path} failed: {type(exc).__name__}: {exc}"
            ) from exc
        finally:
            conn.close()

    def open_and_abort_sse(
        self,
        path: str,
        *,
        json_body: Any,
        bearer: str | None = None,
        timeout: float | None = None,
    ) -> tuple[int, bytes]:
        """Open SSE connection, read first non-empty bytes, and close socket abruptly."""
        payload = json.dumps(json_body, ensure_ascii=False).encode("utf-8")
        hdrs = self._merge_headers({"Accept": "text/event-stream"}, bearer=bearer, has_json=True)
        conn = self._connect(timeout=timeout)
        try:
            conn.request("POST", self._build_path(path), body=payload, headers=hdrs)
            resp = conn.getresponse()
            first = resp.readline()
            return resp.status, first
        except (OSError, http.client.HTTPException, socket.timeout) as exc:
            raise HttpClientError(f"SSE abort test failed: {exc}") from exc
        finally:
            conn.close()

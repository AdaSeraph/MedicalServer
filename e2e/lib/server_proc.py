"""Live medical-server subprocess lifecycle manager for --spawn-server."""

from __future__ import annotations

import os
import shutil
import socket
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any, TextIO

from e2e.lib.http_client import HttpClient, HttpClientError
from e2e.lib.mock_upstream import MockUpstreamServer


def _pick_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def locate_server_binary(workspace: Path, explicit_bin: Path | None = None) -> Path:
    if explicit_bin is not None:
        if not explicit_bin.is_file():
            raise FileNotFoundError(f"Explicit --server-bin not found: {explicit_bin}")
        return explicit_bin

    candidates = [
        workspace / "target" / "release" / "medical-server.exe",
        workspace / "target" / "debug" / "medical-server.exe",
        workspace / "target" / "release" / "medical-server",
        workspace / "target" / "debug" / "medical-server",
    ]
    for cand in candidates:
        if cand.is_file():
            return cand
    raise FileNotFoundError(
        "Compiled medical-server binary not found in target/{debug,release}/ "
        "(and --server-bin was not provided)."
    )


class ManagedServer:
    """Spawns a copied medical-server binary with MockUpstreamServer and cleans up cleanly."""

    def __init__(
        self,
        workspace: Path,
        *,
        server_bin: Path | None = None,
        startup_timeout: float = 20.0,
        admin_token: str | None = None,
        seed_tenants_file: Path | None = None,
    ) -> None:
        self.workspace = workspace
        self.source_bin = locate_server_binary(workspace, server_bin)
        self.startup_timeout = startup_timeout
        self.admin_token = admin_token
        self.seed_tenants_file = seed_tenants_file
        self.port = _pick_free_port()
        self.base_url = f"http://127.0.0.1:{self.port}"
        self.mock_upstream: MockUpstreamServer | None = None
        self._tmpdir: tempfile.TemporaryDirectory[str] | None = None
        self._proc: subprocess.Popen[str] | None = None
        self._log_fh: TextIO | None = None
        self.log_path: Path | None = None

    def __enter__(self) -> "ManagedServer":
        self.start()
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.stop()

    def start(self) -> None:
        try:
            self._tmpdir = tempfile.TemporaryDirectory(prefix="medsrv_e2e_")
            tmp_path = Path(self._tmpdir.name)
            copied_bin = tmp_path / self.source_bin.name
            # Copy binary to temp dir to prevent Windows EXE file locks on target/debug
            shutil.copy2(self.source_bin, copied_bin)

            self.mock_upstream = MockUpstreamServer().start()
            self.log_path = tmp_path / "medical-server.log"

            env = os.environ.copy()
            default_tenants = self.workspace / "e2e" / "fixtures" / "payloads" / "tenant_keys.json"
            tenants_path = self.seed_tenants_file or (default_tenants if default_tenants.is_file() else None)
            env_updates = {
                "PORT": str(self.port),
                "SERVER_ADDR": f"127.0.0.1:{self.port}",
                "MEDICAL_SERVER_ADDR": f"127.0.0.1:{self.port}",
                "MEDICAL_R5_OFFLINE": "true",
                "USE_IN_MEMORY_ADAPTERS": "true",
                "OPENAI_BASE_URL": f"{self.mock_upstream.base_url}/v1",
                "LLM_BASE_URL": f"{self.mock_upstream.base_url}/v1",
                "EMBEDDING_BASE_URL": f"{self.mock_upstream.base_url}/v1",
                "RERANK_BASE_URL": f"{self.mock_upstream.base_url}/v1",
                "OPENAI_API_KEY": "sk-e2e-mock-upstream",
                "MEDICAL_ADMIN_TOKEN": self.admin_token or env.get("MEDICAL_ADMIN_TOKEN") or "e2e-admin-secret",
                "RUST_LOG": env.get("RUST_LOG", "info"),
            }
            if tenants_path is not None:
                env_updates["MEDICAL_TENANT_KEYS_FILE"] = str(tenants_path)
            elif env.get("MEDICAL_TENANT_KEYS_FILE"):
                env_updates["MEDICAL_TENANT_KEYS_FILE"] = env["MEDICAL_TENANT_KEYS_FILE"]
            env.update(env_updates)

            self._log_fh = self.log_path.open("w", encoding="utf-8", errors="replace")
            self._proc = subprocess.Popen(
                [str(copied_bin)],
                cwd=str(self.workspace),
                env=env,
                stdout=self._log_fh,
                stderr=subprocess.STDOUT,
                text=True,
            )

            client = HttpClient(self.base_url, timeout=2.0)
            deadline = time.monotonic() + self.startup_timeout
            last_err: str = "no response"
            while time.monotonic() < deadline:
                if self._proc.poll() is not None:
                    self._log_fh.flush()
                    log_tail = self.log_path.read_text(encoding="utf-8", errors="replace")[-2000:]
                    raise RuntimeError(
                        f"medical-server exited prematurely (code={self._proc.returncode}):\n{log_tail}"
                    )
                try:
                    resp = client.get("/health")
                    if resp.status == 200:
                        return
                    last_err = f"HTTP {resp.status}: {resp.text[:200]}"
                except HttpClientError as exc:
                    last_err = str(exc)
                time.sleep(0.25)

            raise RuntimeError(f"medical-server failed /health within {self.startup_timeout}s: {last_err}")
        except Exception:
            self.stop()
            raise

    def stop(self) -> None:
        if self._proc is not None:
            try:
                if self._proc.poll() is None:
                    self._proc.terminate()
                    try:
                        self._proc.wait(timeout=5.0)
                    except subprocess.TimeoutExpired:
                        self._proc.kill()
                        self._proc.wait(timeout=3.0)
            finally:
                self._proc = None
        if self._log_fh is not None:
            try:
                self._log_fh.flush()
                self._log_fh.close()
            finally:
                self._log_fh = None
        if self.mock_upstream is not None:
            try:
                self.mock_upstream.stop()
            finally:
                self.mock_upstream = None
        if self._tmpdir is not None:
            try:
                self._tmpdir.cleanup()
            finally:
                self._tmpdir = None

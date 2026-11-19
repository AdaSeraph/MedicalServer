"""Test execution engine for MedicalServer E2E (live & loopback reporting)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import traceback
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Sequence

from e2e.lib.http_client import HttpClient
from e2e.lib.registry import TestCase, TestRegistry


@dataclass
class TestContext:
    workspace: Path
    base_url: str
    client: HttpClient
    api_key: str | None = None
    secondary_api_key: str | None = None
    low_qpm_api_key: str | None = None
    revoked_api_key: str | None = None
    admin_token: str | None = None
    timeout: float = 10.0
    with_cargo: bool = False
    mock_upstream: Any = None
    fixtures_dir: Path = field(
        default_factory=lambda: Path(__file__).resolve().parents[1] / "fixtures"
    )
    _cargo_cache: dict[str, tuple[int, str]] = field(default_factory=dict)
    _auth_cache: dict[str, tuple[str, str]] = field(default_factory=dict)

    def tenant_key(self, name: str) -> str:
        """Resolve tenant API key from CLI/env overrides or fixtures/payloads/tenant_keys.json."""
        if name == "hosp_a" and self.api_key:
            return self.api_key
        if name == "hosp_b" and self.secondary_api_key:
            return self.secondary_api_key
        if name.startswith("qpm_test") and self.low_qpm_api_key:
            return self.low_qpm_api_key
        if name == "revoked" and self.revoked_api_key:
            return self.revoked_api_key
        data = json.loads((self.fixtures_dir / "payloads" / "tenant_keys.json").read_text(encoding="utf-8"))
        return str(data["tenants"][name]["api_key"])

    def run_cargo(self, args: Sequence[str], *, strip_db_env: bool = False) -> tuple[int, str]:
        """Execute cargo command only when --with-cargo is enabled; otherwise fail clearly."""
        if not self.with_cargo:
            raise AssertionError(
                f"Cargo check requested ({' '.join(args)}) without --with-cargo flag. "
                "Re-run with --with-cargo when Cargo execution is permitted."
            )
        key = f"{'|'.join(args)}|{strip_db_env}"
        if key in self._cargo_cache:
            return self._cargo_cache[key]
        env = os.environ.copy()
        if strip_db_env:
            for k in ("DATABASE_URL", "REDIS_URL", "QDRANT_URL", "OPENAI_API_KEY", "SQLX_OFFLINE"):
                env.pop(k, None)
        proc = subprocess.run(
            ["cargo", *args],
            cwd=str(self.workspace),
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=300,
        )
        combined = (proc.stdout or "") + "\n" + (proc.stderr or "")
        res = (proc.returncode, combined)
        self._cargo_cache[key] = res
        return res

    def get_wechat_auth(self, code: str = "wx_e2e_default_user") -> tuple[str, str]:
        """Authenticate via POST /v1/auth/wechat and return (jwt_token, patient_id)."""
        if code in self._auth_cache:
            return self._auth_cache[code]
        resp = self.client.post("/v1/auth/wechat", json_body={"code": code})
        assert resp.status == 200, f"/v1/auth/wechat failed ({resp.status}): {resp.text}"
        data = resp.json()
        token = str(data.get("token", ""))
        pid = str(data.get("patient_id", ""))
        assert token and pid, f"Missing token/patient_id in /v1/auth/wechat response: {data}"
        self._auth_cache[code] = (token, pid)
        return token, pid

    def auth_headers_or_wechat(self, code: str = "wx_e2e_default_user") -> tuple[dict[str, str], str]:
        """Return auth headers (API key if configured else WeChat JWT) and a patient_id."""
        if self.api_key:
            _, pid = self.get_wechat_auth(code)
            return {"Authorization": f"Bearer {self.api_key}"}, pid
        token, pid = self.get_wechat_auth(code)
        return {"Authorization": f"Bearer {token}"}, pid


@dataclass
class TestResult:
    id: str
    tier: int
    features: list[int]
    title: str
    status: str  # PASS | FAIL
    duration_ms: float
    error: str | None = None
    evidence: str | None = None


def enforce_live_no_selftest_guard() -> None:
    if "e2e.lib.selftest_harness" in sys.modules:
        raise RuntimeError(
            "INTEGRITY VIOLATION: e2e.lib.selftest_harness must NEVER be loaded in live mode."
        )


def filter_cases(
    cases: Sequence[TestCase],
    *,
    tier_arg: str,
    features: Sequence[int] | None = None,
    test_ids: Sequence[str] | None = None,
) -> list[TestCase]:
    selected: list[TestCase] = []
    tiers = {1, 2, 3, 4} if tier_arg == "all" else {int(tier_arg)}
    feat_set = set(features) if features else None
    id_set = set(test_ids) if test_ids else None

    for tc in cases:
        if tc.tier not in tiers:
            continue
        if feat_set is not None and not (feat_set & set(tc.features)):
            continue
        if id_set is not None and tc.id not in id_set:
            continue
        selected.append(tc)
    return selected


def execute_cases(
    cases: Sequence[TestCase],
    ctx: TestContext,
    registry: TestRegistry,
    *,
    fail_fast: bool = False,
    verbose: bool = False,
    json_report: Path | None = None,
    enforce_live_guard: bool = True,
) -> tuple[int, dict[str, Any]]:
    if enforce_live_guard:
        enforce_live_no_selftest_guard()

    # Run --with-cargo workspace build/check BEFORE executing server tests if enabled
    if ctx.with_cargo and enforce_live_guard:
        rc, out = ctx.run_cargo(["check", "--workspace"], strip_db_env=True)
        if rc != 0:
            print(f"[PRE-FLIGHT] cargo check --workspace failed:\n{out[-1500:]}", file=sys.stderr)

    results: list[TestResult] = []
    t_start = time.perf_counter()

    for tc in cases:
        t0 = time.perf_counter()
        try:
            tc.fn(ctx)
            dur = (time.perf_counter() - t0) * 1000.0
            res = TestResult(
                id=tc.id,
                tier=tc.tier,
                features=list(tc.features),
                title=tc.title,
                status="PASS",
                duration_ms=round(dur, 2),
            )
            if verbose:
                print(f"[PASS] {tc.id} ({dur:.1f}ms) — {tc.title}")
        except Exception as exc:
            dur = (time.perf_counter() - t0) * 1000.0
            err_msg = f"{type(exc).__name__}: {exc}"
            tb = traceback.format_exc() if verbose else None
            res = TestResult(
                id=tc.id,
                tier=tc.tier,
                features=list(tc.features),
                title=tc.title,
                status="FAIL",
                duration_ms=round(dur, 2),
                error=err_msg,
                evidence=tb,
            )
            print(f"[FAIL] {tc.id} ({dur:.1f}ms) — {tc.title}\n       {err_msg}")
            if verbose and tb:
                print(tb)
            results.append(res)
            if fail_fast:
                break
            continue
        results.append(res)

    total_ms = round((time.perf_counter() - t_start) * 1000.0, 2)
    passed = sum(1 for r in results if r.status == "PASS")
    failed = sum(1 for r in results if r.status == "FAIL")
    cov = registry.coverage_summary()

    report: dict[str, Any] = {
        "summary": {
            "total_executed": len(results),
            "passed": passed,
            "failed": failed,
            "duration_ms": total_ms,
            "base_url": ctx.base_url,
            "workspace": str(ctx.workspace),
        },
        "tier_counts": cov["tier_counts"],
        "feature_coverage": cov["feature_matrix"],
        "results": [
            {
                "id": r.id,
                "tier": r.tier,
                "features": r.features,
                "title": r.title,
                "status": r.status,
                "duration_ms": r.duration_ms,
                "error": r.error,
                "evidence": r.evidence,
            }
            for r in results
        ],
    }

    if json_report is not None:
        json_report.parent.mkdir(parents=True, exist_ok=True)
        json_report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")

    print(
        f"\nE2E Summary: {passed}/{len(results)} passed, {failed} failed "
        f"({total_ms:.1f}ms)"
    )
    exit_code = 0 if (failed == 0 and len(results) > 0) else 1
    return exit_code, report

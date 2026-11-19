#!/usr/bin/env python3
"""MedicalServer Opaque-Box 4-Tier E2E Test Runner (Python 3.15 stdlib only)."""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Sequence

# Ensure repository root is on sys.path when invoked as `python e2e/run_e2e.py`
_E2E_DIR = Path(__file__).resolve().parent
_REPO_ROOT = _E2E_DIR.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from e2e.lib.cli import EXIT_FAIL, EXIT_PASS, EXIT_USAGE, CliUsageError, parse_args
from e2e.lib.http_client import HttpClient
from e2e.lib.registry import load_all_tests
from e2e.lib.runner import TestContext, enforce_live_no_selftest_guard, execute_cases, filter_cases
from e2e.lib.server_proc import ManagedServer


def main(argv: Sequence[str] | None = None) -> int:
    try:
        args = parse_args(argv)
    except CliUsageError as exc:
        print(f"CLI usage error: {exc}", file=sys.stderr)
        return EXIT_USAGE

    if args.self_test:
        from e2e.lib.selftest_harness import run_self_test

        return run_self_test(_E2E_DIR)

    # Strict Live-Mode Integrity Guard: selftest_harness MUST NEVER be loaded
    enforce_live_no_selftest_guard()

    registry = load_all_tests()
    cases = filter_cases(
        registry.all_cases(),
        tier_arg=args.tier,
        features=args.features,
        test_ids=args.test_ids,
    )
    if not cases:
        print("No tests matched the provided --tier / --feature / --test-id filters.", file=sys.stderr)
        return EXIT_USAGE

    workspace = args.workspace.resolve()
    if args.spawn_server:
        # Sequence --with-cargo checks BEFORE spawning medical-server.exe (Windows EXE lock safety)
        if args.with_cargo:
            pre_ctx = TestContext(
                workspace=workspace,
                base_url=args.base_url,
                client=HttpClient(args.base_url, timeout=args.timeout),
                with_cargo=True,
            )
            rc, out = pre_ctx.run_cargo(["build", "--workspace"], strip_db_env=True)
            if rc != 0:
                print(f"Pre-spawn cargo build --workspace failed (rc={rc}):\n{out[-2000:]}", file=sys.stderr)
                return EXIT_FAIL

        try:
            admin_tok = args.admin_token or "e2e-admin-secret"
            with ManagedServer(workspace, server_bin=args.server_bin, admin_token=admin_tok) as srv:
                client = HttpClient(srv.base_url, timeout=args.timeout)
                ctx = TestContext(
                    workspace=workspace,
                    base_url=srv.base_url,
                    client=client,
                    api_key=args.api_key,
                    secondary_api_key=args.secondary_api_key,
                    low_qpm_api_key=args.low_qpm_api_key,
                    revoked_api_key=args.revoked_api_key,
                    admin_token=admin_tok,
                    timeout=args.timeout,
                    with_cargo=args.with_cargo,
                    mock_upstream=srv.mock_upstream,
                )
                rc, _ = execute_cases(
                    cases,
                    ctx,
                    registry,
                    fail_fast=args.fail_fast,
                    verbose=args.verbose,
                    json_report=args.json_report,
                    enforce_live_guard=True,
                )
                return EXIT_PASS if rc == 0 else EXIT_FAIL
        except Exception as exc:
            print(f"Failed to spawn/run medical-server: {exc}", file=sys.stderr)
            return EXIT_FAIL

    client = HttpClient(args.base_url, timeout=args.timeout)
    ctx = TestContext(
        workspace=workspace,
        base_url=args.base_url,
        client=client,
        api_key=args.api_key,
        secondary_api_key=args.secondary_api_key,
        low_qpm_api_key=args.low_qpm_api_key,
        revoked_api_key=args.revoked_api_key,
        admin_token=args.admin_token,
        timeout=args.timeout,
        with_cargo=args.with_cargo,
        mock_upstream=None,
    )
    rc, _ = execute_cases(
        cases,
        ctx,
        registry,
        fail_fast=args.fail_fast,
        verbose=args.verbose,
        json_report=args.json_report,
        enforce_live_guard=True,
    )
    return EXIT_PASS if rc == 0 else EXIT_FAIL


if __name__ == "__main__":
    raise SystemExit(main())

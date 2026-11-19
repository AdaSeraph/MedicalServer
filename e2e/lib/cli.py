"""CLI parsing and exit code constants for e2e/run_e2e.py."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
from typing import Sequence

EXIT_PASS = 0
EXIT_FAIL = 1
EXIT_USAGE = 2

DEFAULT_WORKSPACE = Path(__file__).resolve().parents[2]
DEFAULT_BASE_URL = "http://127.0.0.1:8080"


class CliUsageError(Exception):
    """Raised when CLI arguments are invalid (maps to exit code 2)."""


class _E2EArgumentParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:  # pragma: no cover - tested via subprocess/CLI check
        raise CliUsageError(message)


def build_parser() -> argparse.ArgumentParser:
    parser = _E2EArgumentParser(
        prog="run_e2e.py",
        description="MedicalServer Opaque-Box 4-Tier E2E Test Runner (Python 3.15 stdlib only)",
    )
    parser.add_argument(
        "--tier",
        choices=["1", "2", "3", "4", "all"],
        help="Execution tier in live/workspace mode (mutually exclusive with --self-test).",
    )
    parser.add_argument(
        "--base-url",
        default=os.environ.get("MEDICAL_BASE_URL", DEFAULT_BASE_URL),
        help=f"Target live medical-server base URL (default: {DEFAULT_BASE_URL}).",
    )
    parser.add_argument(
        "--api-key",
        default=os.environ.get("MEDICAL_API_KEY"),
        help="Bearer mk_live_* API key for primary tenant (hosp_a).",
    )
    parser.add_argument(
        "--secondary-api-key",
        default=os.environ.get("MEDICAL_SECONDARY_API_KEY"),
        help="Optional Bearer mk_live_* API key for secondary tenant (hosp_b).",
    )
    parser.add_argument(
        "--low-qpm-api-key",
        default=os.environ.get("MEDICAL_LOW_QPM_API_KEY"),
        help="Optional Bearer mk_live_* API key with low QPM quota for throttling tests.",
    )
    parser.add_argument(
        "--revoked-api-key",
        default=os.environ.get("MEDICAL_REVOKED_API_KEY"),
        help="Optional revoked Bearer mk_live_* API key for revocation tests.",
    )
    parser.add_argument(
        "--admin-token",
        default=os.environ.get("MEDICAL_ADMIN_TOKEN"),
        help="Optional admin token for /v1/admin/reload endpoints.",
    )
    parser.add_argument(
        "--workspace",
        type=Path,
        default=DEFAULT_WORKSPACE,
        help="Workspace root containing Cargo.toml, migrations/, config/, crates/, miniapp/, deploy/, k6/.",
    )
    parser.add_argument(
        "--spawn-server",
        action="store_true",
        help="Spawn real medical-server binary cleanly and tear down on exit.",
    )
    parser.add_argument(
        "--server-bin",
        type=Path,
        default=None,
        help="Explicit path to compiled medical-server binary (used with --spawn-server).",
    )
    parser.add_argument(
        "--with-cargo",
        action="store_true",
        help="Run workspace cargo check/build/test assertions (sequenced BEFORE spawning server).",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=10.0,
        help="Per-HTTP/SSE request timeout in seconds (default: 10.0).",
    )
    parser.add_argument(
        "--feature",
        type=int,
        action="append",
        dest="features",
        default=None,
        help="Filter tests covering specific feature ID (1..22). Repeatable.",
    )
    parser.add_argument(
        "--test-id",
        action="append",
        dest="test_ids",
        default=None,
        help="Filter specific test ID (e.g., T3-07, F1-T1-01). Repeatable.",
    )
    parser.add_argument(
        "--json-report",
        type=Path,
        default=None,
        help="Write structured JSON test report to this path.",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Abort immediately on the first failed test.",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Print per-test diagnostic details.",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Run isolated internal E2E suite validation without requiring M1-M5.",
    )
    return parser


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = build_parser()
    args = parser.parse_args(list(argv) if argv is not None else None)

    if args.self_test and args.tier is not None:
        raise CliUsageError("--self-test and --tier are mutually exclusive.")
    if not args.self_test and args.tier is None:
        raise CliUsageError("Either --tier {1,2,3,4,all} or --self-test is required.")
    if args.self_test and args.spawn_server:
        raise CliUsageError("--spawn-server cannot be combined with --self-test.")
    if args.self_test and args.with_cargo:
        raise CliUsageError("--with-cargo cannot be combined with --self-test.")
    if args.timeout <= 0:
        raise CliUsageError("--timeout must be > 0.")
    if args.features:
        for fid in args.features:
            if not (1 <= fid <= 22):
                raise CliUsageError(f"--feature must be in 1..22, got {fid}.")
    if args.server_bin is not None and not args.spawn_server:
        raise CliUsageError("--server-bin requires --spawn-server.")

    return args

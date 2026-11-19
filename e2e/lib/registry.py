"""Declarative TestCase registry and Feature #1-#22 coverage auditor."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Sequence

ALL_FEATURES: tuple[int, ...] = tuple(range(1, 23))

FEATURE_NAMES: dict[int, str] = {
    1: "Cargo 4-Crate Workspace & Windows Build Config",
    2: "Core Domain Models & Schema Validation",
    3: "PostgreSQL DDL & Qdrant Collection Schemas",
    4: "AES-256-GCM Encryption & PII Scrubber",
    5: "Storage/Cache/Search Traits + Prod & Mock Adapters (R5)",
    6: "Unified AI Abstraction & ModelRouter",
    7: "bevy_ecs Triage Engine & Safety Short-Circuit",
    8: "sonic-rs JSON/JSONL Streaming Parser",
    9: "Medical Book Parsers & Semantic Chunker",
    10: "Patient Record Importer & rayon Privacy Masker",
    11: "Dual-Write Indexer (Qdrant+Tantivy) & Checkpoints",
    12: "Axum Server Auth, Multi-Tenancy & Rate Limiting",
    13: "Redis L1 Working Memory & Semantic Cache",
    14: "Triage, Search, OpenAI Proxy, Epidemic & Memory APIs",
    15: "Ops & Deployment Deliverables",
    16: "MedicalStudio Module 1: Knowledge Import Workshop",
    17: "MedicalStudio Module 2: Patient Import & Masking Center",
    18: "MedicalStudio Module 3: Seasonal Epidemic Console",
    19: "MedicalStudio Module 4: Dictionary & Red-Flag Manager",
    20: "MedicalStudio Module 5: ECS Triage Whitebox Debugger",
    21: "MedicalStudio Module 6: AI LLM & API Gateway Center",
    22: "WeChat Mini Program (miniapp/)",
}


@dataclass(frozen=True)
class TestCase:
    id: str
    tier: int
    features: tuple[int, ...]
    title: str
    fn: Callable[[Any], None]


class TestRegistry:
    def __init__(self) -> None:
        self._cases: list[TestCase] = []
        self._seen_ids: set[str] = set()

    def register(
        self,
        test_id: str,
        *,
        tier: int,
        features: Sequence[int],
        title: str,
    ) -> Callable[[Callable[[Any], None]], Callable[[Any], None]]:
        if test_id in self._seen_ids:
            raise ValueError(f"Duplicate test ID registered: {test_id}")
        if tier not in {1, 2, 3, 4}:
            raise ValueError(f"Invalid tier {tier} for {test_id}")
        f_tuple = tuple(sorted(set(features)))
        if not f_tuple or any(f not in FEATURE_NAMES for f in f_tuple):
            raise ValueError(f"Invalid feature list {features} for {test_id}")

        def decorator(fn: Callable[[Any], None]) -> Callable[[Any], None]:
            self._seen_ids.add(test_id)
            self._cases.append(
                TestCase(
                    id=test_id,
                    tier=tier,
                    features=f_tuple,
                    title=title,
                    fn=fn,
                )
            )
            return fn

        return decorator

    def all_cases(self) -> list[TestCase]:
        return list(self._cases)

    def coverage_summary(self) -> dict[str, Any]:
        by_tier: dict[int, list[TestCase]] = {1: [], 2: [], 3: [], 4: []}
        for c in self._cases:
            by_tier[c.tier].append(c)

        matrix: dict[int, dict[str, int]] = {}
        for fid in ALL_FEATURES:
            matrix[fid] = {
                "tier1": sum(1 for c in by_tier[1] if fid in c.features),
                "tier2": sum(1 for c in by_tier[2] if fid in c.features),
                "tier3": sum(1 for c in by_tier[3] if fid in c.features),
                "tier4": sum(1 for c in by_tier[4] if fid in c.features),
            }
        return {
            "total": len(self._cases),
            "tier_counts": {f"tier{t}": len(by_tier[t]) for t in (1, 2, 3, 4)},
            "feature_matrix": matrix,
        }

    def audit_coverage(self) -> list[str]:
        """Verify mandatory Tier 1-4 thresholds across all 22 features."""
        summary = self.coverage_summary()
        errors: list[str] = []
        tc = summary["tier_counts"]
        if tc["tier1"] < 110:
            errors.append(f"Tier 1 count {tc['tier1']} < 110")
        if tc["tier2"] < 110:
            errors.append(f"Tier 2 count {tc['tier2']} < 110")
        if tc["tier3"] < 25:
            errors.append(f"Tier 3 count {tc['tier3']} < 25")
        if tc["tier4"] < 8:
            errors.append(f"Tier 4 count {tc['tier4']} < 8")

        for fid, counts in summary["feature_matrix"].items():
            if counts["tier1"] < 5:
                errors.append(f"Feature F{fid} Tier 1 count {counts['tier1']} < 5")
            if counts["tier2"] < 5:
                errors.append(f"Feature F{fid} Tier 2 count {counts['tier2']} < 5")
            if counts["tier3"] < 1:
                errors.append(f"Feature F{fid} missing from Tier 3")
            if counts["tier4"] < 1:
                errors.append(f"Feature F{fid} missing from Tier 4")
        return errors


REGISTRY = TestRegistry()


def load_all_tests() -> TestRegistry:
    """Import all Tier 1-4 test modules so REGISTRY is populated."""
    import e2e.tests.tier1_core_pipeline  # noqa: F401
    import e2e.tests.tier1_server_clients  # noqa: F401
    import e2e.tests.tier2_core_pipeline  # noqa: F401
    import e2e.tests.tier2_server_clients  # noqa: F401
    import e2e.tests.tier3_cross_feature  # noqa: F401
    import e2e.tests.tier4_hospital_scenarios  # noqa: F401

    return REGISTRY

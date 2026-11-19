# MedicalServer Opaque-Box 4-Tier E2E Test Suite (`e2e/`)

Pure **Python 3.15 standard library** opaque-box end-to-end test suite covering all **22 Features (F1–F22)** across **285 tests**:
- **Tier 1 (123 tests)**: Feature coverage (`>= 5` tests per feature F1–F22)
- **Tier 2 (123 tests)**: Boundary, corner, negative, security & fault-injection cases (`>= 5` tests per feature F1–F22)
- **Tier 3 (28 tests)**: Pairwise cross-feature interactions (`T3-01`..`T3-28`, covering 100% of F1–F22)
- **Tier 4 (11 tests)**: Realistic hospital clinical & ops workflows (`T4-01`..`T4-11`, covering 100% of F1–F22)

Zero external `pip` dependencies are required.

## Usage

### 1. Self-Test (Validates E2E Suite Integrity Without M1–M5)
```powershell
python e2e/run_e2e.py --self-test
```
Runs 7 internal validation pillars:
1. CLI flag & exit-code matrix (`0` pass, `1` fail, `2` usage)
2. Registry & Feature #1–#22 coverage audit (285 tests total)
3. Schema validation against `doc/schemas/data-collection-schema.json` + temp-dir execution of `fixtures/generator.py` (`.xlsx`, `.docx`, `.pdf`, `.jsonl`, `.csv`)
4. Positive & negative checks for all pure-stdlib parsers (`tomllib`, SQL DDL + `sqlite3`, Qdrant JSON, YAML-subset, Grafana JSON, Dockerfile, k6 JS, WXML/WXSS, binary `.ico`, SSE frames)
5. Clinical & privacy assertion checks (`<=30` chars, `2~4` options, `max_turns<=4`, mandatory disclaimer, no diagnosis/dosage, PII detection, vital signs preservation)
6. Isolated loopback HTTP/SSE smoke test & `--json-report`
7. Live-mode non-mock failure proof against closed port + empty workspace

### 2. Live Mode Against Running `medical-server`
```powershell
python e2e/run_e2e.py --tier all --base-url http://127.0.0.1:8080 --json-report e2e-report.json
```

### 3. Live Mode Auto-Spawning `medical-server` + `MockUpstreamServer`
```powershell
python e2e/run_e2e.py --tier all --spawn-server --with-cargo --verbose --json-report e2e-report.json
```

### Filter Options
- `--tier 1|2|3|4|all`
- `--feature <1..22>` (repeatable)
- `--test-id <ID>` (repeatable, e.g., `--test-id T3-07 --test-id T4-01`)
- `--fail-fast`
- `--timeout <seconds>`

## Strict Live-vs-Self-Test Integrity
- Live mode (`--tier ...`) enforces `"e2e.lib.selftest_harness" not in sys.modules`.
- Live mode **never** falls back to mocks if the server or workspace deliverables are missing; it fails immediately with exit code `1`.

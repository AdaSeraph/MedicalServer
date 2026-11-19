"""Clinical, privacy, schema, and API contract assertions for MedicalServer E2E."""

from __future__ import annotations

import json
import re
from datetime import date
from pathlib import Path
from typing import Any, Iterable, Sequence

MANDATORY_DISCLAIMER = "本建议由 AI 智能导诊结合医学知识库生成，仅供参考，不替代执业医师诊断。"
DISCLAIMER_CORE_PHRASE = "仅供参考，不替代执业医师诊断"

DEFAULT_SCHEMA_RULES_PATH = (
    Path(__file__).resolve().parents[1] / "fixtures" / "schemas" / "schema_rules.json"
)

# PII patterns: 18-digit / 15-digit Chinese ID card & 11-digit Chinese mobile (incl. +86 / dashes / 3-3-5)
_RE_ID_CARD_18 = re.compile(r"(?<!\d)[1-9]\d{5}(?:19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dXx](?!\d)")
_RE_ID_CARD_15 = re.compile(r"(?<!\d)[1-9]\d{5}\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}(?!\d)")
_RE_MOBILE = re.compile(
    r"(?<!\d)(?:\+?86[\s-]?)?1[3-9]\d(?:[\s-]?\d{4}[\s-]?\d{4}|[\s-]?\d{3}[\s-]?\d{5})(?!\d)"
)

# Definitive diagnosis & concrete prescription dosage patterns forbidden in clinical output
_RE_DIAGNOSIS = re.compile(
    r"(?:(?<!未)(?<!尚未)(?<!不能)(?<!无法)(?<!非)(?<!不是)"
    r"(?:确诊为|确诊是|确诊您患有)|"
    r"明确诊断为|可以诊断为|诊断(?:结论|结果)[:：为是]\s*(?:您患有|为)?|"
    r"(?<!不替代)(?<!不能)您患有)"
)
_NUM_CN = r"(?:\d+(?:\.\d+)?|[一二两三四五六七八九十]+)"
_UNIT_DOSE = r"(?:mg|毫克|(?<![A-Za-z])g(?![A-Za-z])|克|m[lL]|毫升|片|粒|支|袋)"
_FREQ = rf"(?:每日|每天|一日|一天|每\s*\d+\s*小时)\s*{_NUM_CN}\s*次"
_DOSE = rf"(?:(?:每次|一次)\s*)?\d+(?:\.\d+)?\s*{_UNIT_DOSE}"
_RE_DOSAGE = re.compile(
    rf"(?:{_DOSE}\s*[,，/／\s]*{_FREQ}|"
    rf"{_FREQ}\s*[,，/／\s]*(?:每次|一次)?\s*\d+(?:\.\d+)?\s*{_UNIT_DOSE}|"
    rf"(?:口服|静滴|静脉注射|肌注|服用)[^。\n]{{0,16}}(?:每次|一次)\s*\d+(?:\.\d+)?\s*{_UNIT_DOSE})"
)


def load_schema_rules(path: Path | None = None) -> dict[str, Any]:
    target = path or DEFAULT_SCHEMA_RULES_PATH
    return json.loads(target.read_text(encoding="utf-8"))


def is_plaintext_pii_patient_id(patient_id: Any) -> bool:
    if not isinstance(patient_id, str):
        return False
    s = patient_id.strip()
    if re.fullmatch(r"\d{17}[\dXx]", s) or re.fullmatch(r"\d{15}", s):
        return True
    if _RE_MOBILE.fullmatch(s):
        return True
    return False


def detect_pii(text: str) -> list[str]:
    """Return list of PII findings in `text`. Must NOT false-positive on vital signs."""
    findings: list[str] = []
    for m in _RE_ID_CARD_18.finditer(text):
        findings.append(f"id_card_18:{m.group(0)}")
    for m in _RE_ID_CARD_15.finditer(text):
        findings.append(f"id_card_15:{m.group(0)}")
    for m in _RE_MOBILE.finditer(text):
        findings.append(f"mobile:{m.group(0)}")
    return findings


def assert_no_pii(text: str, extra_forbidden: Sequence[str] = ()) -> None:
    findings = detect_pii(text)
    assert not findings, f"Detected unmasked PII in text: {findings} | text={text[:160]!r}"
    for item in extra_forbidden:
        if item and item in text:
            raise AssertionError(f"Forbidden raw PII token {item!r} leaked in text: {text[:160]!r}")


def assert_vitals_preserved(text: str, vitals: Sequence[str]) -> None:
    for v in vitals:
        assert v in text, f"Clinical vital sign {v!r} was unexpectedly stripped from: {text!r}"


def assert_disclaimer_present(text: str) -> None:
    assert MANDATORY_DISCLAIMER in text or DISCLAIMER_CORE_PHRASE in text, (
        f"Missing mandatory medical disclaimer in: {text!r}"
    )


def assert_no_diagnosis_or_dosage(text: str) -> None:
    if _RE_DIAGNOSIS.search(text):
        raise AssertionError(f"Forbidden definitive diagnosis statement found in: {text!r}")
    if _RE_DOSAGE.search(text):
        raise AssertionError(f"Forbidden prescription drug dosage found in: {text!r}")


def validate_knowledge_document(doc: dict[str, Any], rules: dict[str, Any] | None = None) -> list[str]:
    if not isinstance(doc, dict):
        return ["knowledge_document must be an object"]
    rules = rules or load_schema_rules()
    kd = rules["knowledge_document"]
    errors: list[str] = []

    for req in kd["required"]:
        if req not in doc or doc[req] is None:
            errors.append(f"missing required field '{req}'")
    if errors:
        return errors

    for s_field in ("doc_id", "source_title", "disease_name", "department", "content"):
        if not isinstance(doc.get(s_field), str) or not doc[s_field].strip():
            errors.append(f"field '{s_field}' must be non-empty string")

    st = doc["source_type"]
    if not isinstance(st, str) or st not in kd["enums"]["source_type"]:
        errors.append(f"invalid source_type: {st!r}")
    sect = doc["section_type"]
    if not isinstance(sect, str) or sect not in kd["enums"]["section_type"]:
        errors.append(f"invalid section_type: {sect!r}")

    auth = doc["authority_level"]
    lo, hi = kd["ranges"]["authority_level"]
    if not isinstance(auth, (int, float)) or isinstance(auth, bool) or not (lo <= float(auth) <= hi):
        errors.append(f"authority_level out of range [{lo}, {hi}]: {auth!r}")

    tp = doc["target_population"]
    valid_tp = set(kd["enums"]["target_population"])
    if (
        not isinstance(tp, list)
        or len(tp) == 0
        or any(not isinstance(item, str) or item not in valid_tp for item in tp)
    ):
        errors.append(f"invalid target_population: {tp!r}")

    ks = doc["key_symptoms"]
    if (
        not isinstance(ks, list)
        or len(ks) == 0
        or any(not isinstance(item, str) or not item.strip() for item in ks)
    ):
        errors.append("key_symptoms must be a non-empty list of non-empty strings")

    slots = doc.get("suggested_follow_up_slots")
    if slots is not None:
        if not isinstance(slots, list):
            errors.append("suggested_follow_up_slots must be a list")
        else:
            max_q = kd["slot_constraints"]["max_question_chars"]
            min_o = kd["slot_constraints"]["min_quick_options"]
            max_o = kd["slot_constraints"]["max_quick_options"]
            for idx, slot in enumerate(slots):
                if not isinstance(slot, dict):
                    errors.append(f"slot[{idx}] must be an object")
                    continue
                q = slot.get("short_question", "")
                opts = slot.get("quick_options", [])
                if not isinstance(q, str) or not q.strip() or len(q) > max_q:
                    q_len = len(q) if isinstance(q, str) else f"type={type(q).__name__}"
                    errors.append(f"slot[{idx}].short_question length {q_len} violates 1..={max_q}")
                if not isinstance(opts, list) or not (min_o <= len(opts) <= max_o):
                    errors.append(
                        f"slot[{idx}].quick_options count {len(opts) if isinstance(opts, list) else 'N/A'} violates {min_o}..={max_o}"
                    )
    return errors


def validate_patient_record(rec: dict[str, Any], rules: dict[str, Any] | None = None) -> list[str]:
    if not isinstance(rec, dict):
        return ["patient_record must be an object"]
    rules = rules or load_schema_rules()
    pr = rules["patient_record"]
    errors: list[str] = []

    for req in pr["required"]:
        if req not in rec or rec[req] is None:
            errors.append(f"missing required field '{req}'")
    if errors:
        return errors

    pid = rec["patient_id"]
    if not isinstance(pid, str) or not pid.strip() or is_plaintext_pii_patient_id(pid):
        errors.append(f"patient_id invalid or contains plaintext PII: {pid!r}")
    if not isinstance(rec["consent_long_term_memory"], bool):
        errors.append("consent_long_term_memory must be boolean")

    l2 = rec["l2_profile"]
    if not isinstance(l2, dict):
        errors.append("l2_profile must be an object")
    else:
        for req in pr["l2_profile_required"]:
            if req not in l2:
                errors.append(f"l2_profile missing '{req}'")

    l3 = rec["l3_visit_events"]
    if not isinstance(l3, list):
        errors.append("l3_visit_events must be a list")
    else:
        u_lo, u_hi = pr["ranges"]["urgency_level"]
        for idx, evt in enumerate(l3):
            if not isinstance(evt, dict):
                errors.append(f"l3_visit_events[{idx}] must be an object")
                continue
            for req in pr["l3_visit_event_required"]:
                if req not in evt:
                    errors.append(f"l3_visit_events[{idx}] missing '{req}'")
            urg = evt.get("urgency_level")
            if not isinstance(urg, int) or isinstance(urg, bool) or not (u_lo <= urg <= u_hi):
                errors.append(f"l3_visit_events[{idx}].urgency_level out of range {u_lo}..={u_hi}: {urg!r}")
    return errors


def validate_epidemic_context(ctx: dict[str, Any], rules: dict[str, Any] | None = None) -> list[str]:
    if not isinstance(ctx, dict):
        return ["epidemic_context must be an object"]
    rules = rules or load_schema_rules()
    ec = rules["epidemic_context"]
    errors: list[str] = []
    for req in ec["required"]:
        if req not in ctx or ctx[req] is None:
            errors.append(f"missing required field '{req}'")
    if errors:
        return errors

    for s_field in ("context_id", "region_code", "disease_name", "seasonal_tag"):
        if not isinstance(ctx[s_field], str) or not ctx[s_field].strip():
            errors.append(f"field '{s_field}' must be non-empty string")

    intensity = ctx["intensity"]
    lo, hi = ec["ranges"]["intensity"]
    if not isinstance(intensity, (int, float)) or isinstance(intensity, bool) or not (lo <= float(intensity) <= hi):
        errors.append(f"intensity out of range [{lo}, {hi}]: {intensity!r}")

    try:
        vf = date.fromisoformat(str(ctx["valid_from"]))
        vt = date.fromisoformat(str(ctx["valid_to"]))
        if vf > vt:
            errors.append(f"valid_from ({vf}) > valid_to ({vt})")
    except ValueError as exc:
        errors.append(f"invalid valid_from/valid_to date: {exc}")
    return errors


def validate_medical_dictionaries(dicts: dict[str, Any], rules: dict[str, Any] | None = None) -> list[str]:
    if not isinstance(dicts, dict):
        return ["medical_dictionaries must be an object"]
    rules = rules or load_schema_rules()
    md = rules["medical_dictionaries"]
    errors: list[str] = []

    synonyms = dicts.get("symptom_synonyms", [])
    if synonyms is None:
        synonyms = []
    if not isinstance(synonyms, list):
        errors.append("symptom_synonyms must be a list")
        synonyms = []

    for idx, syn in enumerate(synonyms):
        if not isinstance(syn, dict):
            errors.append(f"symptom_synonyms[{idx}] must be an object")
            continue
        for req in md["synonym_required"]:
            if req not in syn:
                errors.append(f"symptom_synonyms[{idx}] missing '{req}'")
        sterm = syn.get("standard_term")
        if not isinstance(sterm, str) or not sterm.strip():
            errors.append(f"symptom_synonyms[{idx}].standard_term is empty")
        cterms = syn.get("colloquial_terms")
        if not isinstance(cterms, list) or not cterms:
            errors.append(f"symptom_synonyms[{idx}].colloquial_terms must be non-empty list")

    valid_levels = set(md["enums"]["red_flag_level"])
    rf_rules = dicts.get("red_flag_rules", [])
    if rf_rules is None:
        rf_rules = []
    if not isinstance(rf_rules, list):
        errors.append("red_flag_rules must be a list")
        rf_rules = []

    for idx, rf in enumerate(rf_rules):
        if not isinstance(rf, dict):
            errors.append(f"red_flag_rules[{idx}] must be an object")
            continue
        for req in md["red_flag_required"]:
            if req not in rf:
                errors.append(f"red_flag_rules[{idx}] missing '{req}'")
        lvl = rf.get("level")
        if not isinstance(lvl, str) or lvl not in valid_levels:
            errors.append(f"red_flag_rules[{idx}].level invalid: {lvl!r}")
        t_all = rf.get("trigger_all_of", [])
        t_any = rf.get("trigger_any_of", [])
        if (t_all is not None and not isinstance(t_all, list)) or (
            t_any is not None and not isinstance(t_any, list)
        ):
            errors.append(f"red_flag_rules[{idx}] trigger_all_of/trigger_any_of must be lists")
        elif not t_all and not t_any:
            errors.append(f"red_flag_rules[{idx}] trigger_all_of and trigger_any_of cannot both be empty")
    return errors


def validate_collection_bundle(bundle: dict[str, Any], rules: dict[str, Any] | None = None) -> list[str]:
    if not isinstance(bundle, dict):
        return ["collection bundle must be an object"]
    rules = rules or load_schema_rules()
    errors: list[str] = []
    for key, validator in (
        ("knowledge_documents", validate_knowledge_document),
        ("patient_records", validate_patient_record),
        ("epidemic_contexts", validate_epidemic_context),
    ):
        items = bundle.get(key, [])
        if items is None:
            continue
        if not isinstance(items, list):
            errors.append(f"{key} must be a list")
            continue
        for i, item in enumerate(items):
            for err in validator(item, rules):
                errors.append(f"{key}[{i}]: {err}")
    if "medical_dictionaries" in bundle and bundle["medical_dictionaries"] is not None:
        if not isinstance(bundle["medical_dictionaries"], dict):
            errors.append("medical_dictionaries must be an object")
        else:
            for err in validate_medical_dictionaries(bundle["medical_dictionaries"], rules):
                errors.append(f"medical_dictionaries: {err}")
    return errors


def assert_follow_up_response(resp: dict[str, Any]) -> None:
    assert resp.get("status") == "follow_up", f"Expected status='follow_up', got {resp!r}"
    assert resp.get("red_flag", "None") == "None", f"Unexpected red_flag in follow_up: {resp!r}"
    turn = resp.get("turn")
    max_turns = resp.get("max_turns")
    assert isinstance(turn, int) and not isinstance(turn, bool) and turn >= 1, f"Invalid turn: {turn!r}"
    assert (
        isinstance(max_turns, int) and not isinstance(max_turns, bool) and 1 <= max_turns <= 4
    ), f"max_turns violates <=4: {max_turns!r}"
    assert turn <= max_turns, f"turn {turn} exceeded max_turns {max_turns}"
    question = resp.get("question", "")
    assert isinstance(question, str) and question.strip() and 1 <= len(question) <= 30, (
        f"Follow-up question violates non-empty 1..=30 chars (got {question!r}): {resp!r}"
    )
    opts = resp.get("quick_options")
    assert isinstance(opts, list) and 2 <= len(opts) <= 4, (
        f"quick_options must have 2..=4 items, got {opts!r}"
    )
    for opt in opts:
        assert isinstance(opt, str) and opt.strip(), f"Empty quick_option in {opts!r}"
    assert_no_diagnosis_or_dosage(question)


def assert_completed_response(resp: dict[str, Any], *, require_citations: bool = False) -> None:
    assert resp.get("status") == "completed", f"Expected status='completed', got {resp!r}"
    assert resp.get("red_flag", "None") == "None", f"Unexpected red_flag in completed: {resp!r}"
    tr = resp.get("triage_result")
    assert isinstance(tr, dict), f"Missing triage_result object in {resp!r}"
    p_dept = tr.get("primary_department", "")
    assert isinstance(p_dept, str) and p_dept.strip(), f"Missing primary_department in {tr!r}"
    urg = tr.get("urgency_level")
    assert isinstance(urg, int) and not isinstance(urg, bool) and 1 <= urg <= 4, (
        f"urgency_level out of 1..=4: {urg!r}"
    )
    summary = tr.get("summary", "")
    assert isinstance(summary, str) and summary.strip(), f"Empty triage summary in {tr!r}"
    disclaimer = tr.get("disclaimer", "")
    assert_disclaimer_present(disclaimer)
    assert_no_diagnosis_or_dosage(summary)
    if require_citations:
        cits = resp.get("citations")
        assert isinstance(cits, list) and len(cits) >= 1, f"Expected >=1 citation, got {cits!r}"
        for c in cits:
            assert isinstance(c, dict) and c.get("doc_id") and c.get("source_title") and c.get("disease_name"), (
                f"Malformed citation object: {c!r}"
            )


def assert_emergency_response(resp: dict[str, Any]) -> None:
    assert resp.get("status") == "emergency", f"Expected status='emergency', got {resp!r}"
    assert resp.get("red_flag") == "Emergency120", f"Expected red_flag='Emergency120', got {resp!r}"
    blob = json.dumps(resp, ensure_ascii=False)
    assert "120" in blob or "急诊" in blob, f"Emergency response missing 120/ER instruction: {resp!r}"


# ---------------------------------------------------------------------------
# Workspace Source Collectors & Structural Code Checkers (Zero Vacuous Globs)
# ---------------------------------------------------------------------------

def collect_rs_sources(src_dir: Path) -> dict[Path, str]:
    assert src_dir.is_dir(), f"Missing Rust src directory: {src_dir}"
    files = sorted(src_dir.rglob("*.rs"))
    assert files, f"No .rs files found in {src_dir}"
    out: dict[Path, str] = {}
    for p in files:
        out[p] = p.read_text(encoding="utf-8", errors="replace")
    assert any(v.strip() for v in out.values()), f"All .rs files in {src_dir} are empty"
    return out


def collect_rs_text(crate_dir: Path) -> str:
    sources = collect_rs_sources(crate_dir / "src")
    return "\n".join(sources.values())


def collect_studio_sources(workspace: Path) -> tuple[str, str]:
    studio = workspace / "crates" / "medical-studio"
    rs_dir = studio / "src"
    rs_sources = collect_rs_sources(rs_dir)
    fe_src = studio / "frontend" / "src"
    assert fe_src.is_dir(), f"Missing MedicalStudio frontend/src: {fe_src}"
    fe_files = [
        p
        for p in sorted(fe_src.rglob("*"))
        if p.is_file() and p.suffix in {".ts", ".tsx", ".js", ".jsx", ".vue"}
    ]
    assert fe_files, f"No frontend source files found in {fe_src}"
    fe_parts = [p.read_text(encoding="utf-8", errors="replace") for p in fe_files]
    rs_joined = "\n".join(rs_sources.values())
    fe_joined = "\n".join(fe_parts)
    assert fe_joined.strip(), f"MedicalStudio frontend/src files are empty: {fe_src}"
    return rs_joined, fe_joined


def collect_miniapp_sources(workspace: Path) -> dict[str, str]:
    ma = workspace / "miniapp"
    assert ma.is_dir(), f"Missing miniapp directory: {ma}"
    files: dict[str, str] = {}
    for p in sorted(ma.rglob("*")):
        if p.is_file() and p.suffix in {".json", ".js", ".wxml", ".wxss"}:
            files[p.relative_to(ma).as_posix()] = p.read_text(encoding="utf-8", errors="replace")
    assert files, f"No miniapp source files (.json/.js/.wxml/.wxss) found in {ma}"
    return files


def discover_miniapp_page_text(
    workspace: Path,
    ext: str,
    keywords: Sequence[str],
) -> str:
    ma = workspace / "miniapp"
    app_json_path = ma / "app.json"
    assert app_json_path.is_file(), f"Missing miniapp/app.json: {app_json_path}"
    try:
        app_json = json.loads(app_json_path.read_text(encoding="utf-8-sig"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise AssertionError(f"Invalid miniapp/app.json: {exc}") from exc

    pages: list[str] = []
    for p in app_json.get("pages", []):
        if isinstance(p, str) and p.strip():
            pages.append(p.strip().lstrip("/"))
    for sub_key in ("subpackages", "subPackages"):
        subs = app_json.get(sub_key)
        if isinstance(subs, list):
            for sub in subs:
                if isinstance(sub, dict):
                    root = str(sub.get("root", "")).strip("/")
                    for sp in sub.get("pages", []):
                        if isinstance(sp, str) and sp.strip():
                            full = f"{root}/{sp.strip().lstrip('/')}" if root else sp.strip().lstrip("/")
                            pages.append(full)
    assert pages, "miniapp/app.json defines no pages"

    kw_lower = tuple(k.lower() for k in keywords)
    matched = [p for p in pages if any(k in p.lower() for k in kw_lower)]
    candidates = matched if matched else pages

    texts: list[str] = []
    for page in candidates:
        fp = ma / f"{page}{ext}"
        if fp.is_file():
            texts.append(fp.read_text(encoding="utf-8", errors="replace"))
    if not texts and matched:
        for page in pages:
            fp = ma / f"{page}{ext}"
            if fp.is_file():
                texts.append(fp.read_text(encoding="utf-8", errors="replace"))
    assert texts, f"No miniapp {ext} page files found for keywords={keywords} in pages={pages}"
    return "\n".join(texts)


def _skip_rust_raw_string(src: str, i: int) -> int | None:
    """If src[i:] starts a Rust raw string (r#*"..."#* or br#*"..."#* or cr#*"..."#*), return end index."""
    n = len(src)
    j = i
    if src[j] in ("b", "c") and j + 1 < n and src[j + 1] == "r":
        j += 2
    elif src[j] == "r":
        j += 1
    else:
        return None
    hashes = 0
    while j < n and src[j] == "#":
        hashes += 1
        j += 1
    if j >= n or src[j] != '"':
        return None
    j += 1
    closing = '"' + "#" * hashes
    end = src.find(closing, j)
    return n if end == -1 else end + len(closing)


def _skip_rust_string_or_char(src: str, i: int) -> int | None:
    """Return index after Rust string/raw-string/char literal if one starts at i, else None."""
    n = len(src)
    raw_end = _skip_rust_raw_string(src, i)
    if raw_end is not None:
        return raw_end
    ch = src[i]
    # b"..." / c"..." / "..."
    if ch in ("b", "c") and i + 1 < n and src[i + 1] == '"':
        j = i + 2
        while j < n:
            if src[j] == "\\":
                j += 2
                continue
            if src[j] == '"':
                return j + 1
            j += 1
        return n
    if ch == '"':
        j = i + 1
        while j < n:
            if src[j] == "\\":
                j += 2
                continue
            if src[j] == '"':
                return j + 1
            j += 1
        return n
    # b'x' or 'x' char literal (avoid confusing lifetimes like 'a or 'static)
    if ch == "b" and i + 1 < n and src[i + 1] == "'":
        if i + 2 < n and src[i + 2] == "\\":
            end = src.find("'", i + 4)
            return end + 1 if end != -1 and end <= i + 8 else None
        if i + 3 < n and src[i + 3] == "'":
            return i + 4
        return None
    if ch == "'":
        if i + 1 < n and src[i + 1] == "\\":
            end = src.find("'", i + 3)
            return end + 1 if end != -1 and end <= i + 8 else None
        if i + 2 < n and src[i + 2] == "'" and src[i + 1] not in ("\n", "\r", "'"):
            return i + 3
    return None


def strip_rust_comments_and_tests(src: str, *, strip_tests: bool = True) -> str:
    """Strip Rust // and nested /* */ comments (preserving strings), and optionally #[cfg(test)] items."""
    out: list[str] = []
    i = 0
    n = len(src)
    while i < n:
        lit_end = _skip_rust_string_or_char(src, i)
        if lit_end is not None:
            out.append(src[i:lit_end])
            i = lit_end
            continue
        if src[i : i + 2] == "//":
            nl = src.find("\n", i + 2)
            if nl == -1:
                break
            out.append("\n")
            i = nl + 1
            continue
        if src[i : i + 2] == "/*":
            depth = 1
            i += 2
            while i < n and depth > 0:
                if src[i : i + 2] == "/*":
                    depth += 1
                    i += 2
                elif src[i : i + 2] == "*/":
                    depth -= 1
                    i += 2
                else:
                    if src[i] == "\n":
                        out.append("\n")
                    i += 1
            out.append(" ")
            continue
        out.append(src[i])
        i += 1

    cleaned = "".join(out)
    if not strip_tests:
        return cleaned

    cfg_test_re = re.compile(r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]")
    while True:
        m = cfg_test_re.search(cleaned)
        if not m:
            break
        start = m.start()
        j = m.end()
        nc = len(cleaned)
        # Advance through subsequent attributes #[...] and item header to '{' or ';'
        while j < nc:
            lit_end = _skip_rust_string_or_char(cleaned, j)
            if lit_end is not None:
                j = lit_end
                continue
            if cleaned[j : j + 2] == "#[":
                # Skip attribute brackets
                b_depth = 1
                j += 2
                while j < nc and b_depth > 0:
                    le = _skip_rust_string_or_char(cleaned, j)
                    if le is not None:
                        j = le
                        continue
                    if cleaned[j] == "[":
                        b_depth += 1
                    elif cleaned[j] == "]":
                        b_depth -= 1
                    j += 1
                continue
            if cleaned[j] == ";":
                j += 1
                break
            if cleaned[j] == "{":
                depth = 1
                j += 1
                while j < nc and depth > 0:
                    le = _skip_rust_string_or_char(cleaned, j)
                    if le is not None:
                        j = le
                        continue
                    if cleaned[j] == "{":
                        depth += 1
                    elif cleaned[j] == "}":
                        depth -= 1
                    j += 1
                break
            j += 1
        cleaned = cleaned[:start] + "\n" + cleaned[j:]
    return cleaned


def extract_rust_fn_block(rs_clean: str, fn_name: str) -> str:
    """Extract signature + brace-balanced body of `fn <fn_name>(...) { ... }` from clean Rust."""
    fn_re = re.compile(
        rf"(?:^|\s)((?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+{re.escape(fn_name)}\s*(?:<[^;{{}}]*>)?\s*\()",
        re.MULTILINE,
    )
    m = fn_re.search(rs_clean)
    assert m, f"Function `fn {fn_name}(...)` not found in Rust source"
    start = m.start(1)
    n = len(rs_clean)
    j = m.end(1) - 1  # points to '('
    paren_depth = 0
    bracket_depth = 0
    brace_start = -1
    while j < n:
        le = _skip_rust_string_or_char(rs_clean, j)
        if le is not None:
            j = le
            continue
        ch = rs_clean[j]
        if ch == "(":
            paren_depth += 1
        elif ch == ")":
            paren_depth = max(0, paren_depth - 1)
        elif ch == "[":
            bracket_depth += 1
        elif ch == "]":
            bracket_depth = max(0, bracket_depth - 1)
        elif ch == "{" and paren_depth == 0 and bracket_depth == 0:
            brace_start = j
            break
        elif ch == ";" and paren_depth == 0 and bracket_depth == 0:
            raise AssertionError(f"Function `fn {fn_name}` has no body (declaration only)")
        j += 1
    assert brace_start != -1, f"Missing opening '{{' for `fn {fn_name}`"

    depth = 1
    k = brace_start + 1
    while k < n and depth > 0:
        le = _skip_rust_string_or_char(rs_clean, k)
        if le is not None:
            k = le
            continue
        if rs_clean[k] == "{":
            depth += 1
        elif rs_clean[k] == "}":
            depth -= 1
        k += 1
    assert depth == 0, f"Unbalanced braces in `fn {fn_name}`"
    return rs_clean[start:k]


def extract_tauri_generate_handlers(rs_clean: str) -> str:
    """Extract all `(?:tauri::)?generate_handler![...]` blocks from clean Rust source."""
    gen_re = re.compile(r"(?:tauri\s*::\s*)?generate_handler!\s*\[")
    blocks: list[str] = []
    n = len(rs_clean)
    for m in gen_re.finditer(rs_clean):
        j = m.end()
        depth = 1
        while j < n and depth > 0:
            le = _skip_rust_string_or_char(rs_clean, j)
            if le is not None:
                j = le
                continue
            if rs_clean[j] == "[":
                depth += 1
            elif rs_clean[j] == "]":
                depth -= 1
            j += 1
        if depth == 0:
            blocks.append(rs_clean[m.end() : j - 1])
    assert blocks, "Missing `tauri::generate_handler![...]` macro in crates/medical-studio/src"
    return "\n,".join(blocks)


def _strip_fe_comments(fe: str) -> str:
    """Strip //, /* */, and <!-- --> comments from TS/JS/Vue sources while preserving strings."""
    out: list[str] = []
    i = 0
    n = len(fe)
    in_sq = in_dq = in_bt = False
    while i < n:
        ch = fe[i]
        nxt = fe[i + 1] if i + 1 < n else ""
        if in_sq:
            out.append(ch)
            if ch == "\\" and i + 1 < n:
                out.append(fe[i + 1])
                i += 2
                continue
            if ch == "'":
                in_sq = False
            i += 1
            continue
        if in_dq:
            out.append(ch)
            if ch == "\\" and i + 1 < n:
                out.append(fe[i + 1])
                i += 2
                continue
            if ch == '"':
                in_dq = False
            i += 1
            continue
        if in_bt:
            out.append(ch)
            if ch == "\\" and i + 1 < n:
                out.append(fe[i + 1])
                i += 2
                continue
            if ch == "`":
                in_bt = False
            i += 1
            continue

        if fe[i : i + 4] == "<!--":
            end = fe.find("-->", i + 4)
            if end == -1:
                break
            i = end + 3
            out.append(" ")
            continue
        if ch == "/" and nxt == "/":
            nl = fe.find("\n", i + 2)
            if nl == -1:
                break
            out.append("\n")
            i = nl + 1
            continue
        if ch == "/" and nxt == "*":
            end = fe.find("*/", i + 2)
            if end == -1:
                break
            i = end + 2
            out.append(" ")
            continue
        if ch == "'":
            in_sq = True
        elif ch == '"':
            in_dq = True
        elif ch == "`":
            in_bt = True
        out.append(ch)
        i += 1
    return "".join(out)


def assert_studio_commands(ctx: Any, commands: Sequence[str]) -> tuple[str, str]:
    """Verify Rust #[tauri::command], generate_handler![...], non-stub bodies, frontend invoke, & cargo tests."""
    rs, fe = collect_studio_sources(ctx.workspace)
    rs_clean = strip_rust_comments_and_tests(rs, strip_tests=True)
    handlers_blob = extract_tauri_generate_handlers(rs_clean)
    fe_clean = _strip_fe_comments(fe)

    assert re.search(
        r"""import\s*\{[^}]*\binvoke\b[^}]*\}\s*from\s*['"]@tauri-apps/api(?:/core|/tauri)?['"]""",
        fe_clean,
    ), "MedicalStudio frontend/src missing `invoke` import from @tauri-apps/api"

    for cmd in commands:
        cmd_def_re = re.compile(
            rf"#\[\s*tauri::command(?:\s*\([^)]*\))?\s*\]"
            rf"(?:\s*#\[[^\]]*\])*"
            rf"\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+{re.escape(cmd)}\s*\(",
            re.MULTILINE,
        )
        assert cmd_def_re.search(rs_clean), (
            f"Missing `#[tauri::command] fn {cmd}(...)` in crates/medical-studio/src"
        )
        assert re.search(
            rf"(?:^|[\s,:]){re.escape(cmd)}(?:\s*(?:,|$))",
            handlers_blob,
            re.MULTILINE,
        ), f"Tauri command {cmd!r} not registered in tauri::generate_handler![...]"
        fn_block = extract_rust_fn_block(rs_clean, cmd)
        assert "todo!(" not in fn_block and "unimplemented!(" not in fn_block, (
            f"Tauri command {cmd!r} contains todo!()/unimplemented!() stub"
        )
        fe_call_re = re.compile(
            rf"""\b(?:\w*invoke\w*|call\w*|tauri\w*)\s*(?:<[^;{{}}()]*>)?\s*\(\s*['"`]{re.escape(cmd)}['"`]""",
            re.DOTALL | re.IGNORECASE,
        )
        assert fe_call_re.search(fe_clean), (
            f"Missing frontend invoke({cmd!r}) call in crates/medical-studio/frontend/src"
        )

    if getattr(ctx, "with_cargo", False):
        rc, out = ctx.run_cargo(["test", "-p", "medical-studio"], strip_db_env=True)
        assert rc == 0, f"cargo test -p medical-studio failed:\n{out[-1500:]}"

    return rs, fe

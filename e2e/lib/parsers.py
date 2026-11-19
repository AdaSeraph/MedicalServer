"""Pure Python 3.15 stdlib parsers and structural validators for MedicalServer E2E."""

from __future__ import annotations

import json
import re
import sqlite3
import tomllib
import xml.etree.ElementTree as ET
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable


class ParseError(ValueError):
    """Raised when an artifact or stream fails structural parsing."""


# ---------------------------------------------------------------------------
# TOML & JSON / JSONL
# ---------------------------------------------------------------------------

def parse_toml_file(path: Path | str) -> dict[str, Any]:
    p = Path(path)
    if not p.is_file():
        raise ParseError(f"TOML file not found: {p}")
    try:
        with p.open("rb") as fh:
            return tomllib.load(fh)
    except tomllib.TOMLDecodeError as exc:
        raise ParseError(f"Invalid TOML in {p}: {exc}") from exc


def parse_json_file(path: Path | str) -> Any:
    p = Path(path)
    if not p.is_file():
        raise ParseError(f"JSON file not found: {p}")
    try:
        return json.loads(p.read_text(encoding="utf-8-sig"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ParseError(f"Invalid JSON in {p}: {exc}") from exc


@dataclass
class JsonlParseResult:
    records: list[dict[str, Any]] = field(default_factory=list)
    errors: list[tuple[int, str]] = field(default_factory=list)
    total_lines: int = 0


def parse_jsonl_stream(
    path: Path | str,
    validator_fn: Callable[[dict[str, Any]], list[str]] | None = None,
) -> JsonlParseResult:
    """Line-by-line streaming JSONL parser supporting UTF-8 BOM, CRLF, and blank lines."""
    p = Path(path)
    if not p.is_file():
        raise ParseError(f"JSONL file not found: {p}")

    res = JsonlParseResult()
    with p.open("r", encoding="utf-8-sig", newline=None) as fh:
        for line_no, raw_line in enumerate(fh, start=1):
            res.total_lines += 1
            stripped = raw_line.strip()
            if not stripped:
                continue
            try:
                obj = json.loads(stripped)
            except json.JSONDecodeError as exc:
                res.errors.append((line_no, f"JSON syntax error: {exc}"))
                continue
            if not isinstance(obj, dict):
                res.errors.append((line_no, "JSONL line must decode to a JSON object"))
                continue
            if validator_fn is not None:
                errs = validator_fn(obj)
                if errs:
                    res.errors.append((line_no, "; ".join(errs)))
                    continue
            res.records.append(obj)
    return res


# ---------------------------------------------------------------------------
# SQL DDL Parser & SQLite Dry-Run
# ---------------------------------------------------------------------------

@dataclass
class SqlDdlInfo:
    tables: dict[str, list[str]] = field(default_factory=dict)
    table_defs: dict[str, str] = field(default_factory=dict)
    create_table_statements: list[str] = field(default_factory=list)
    create_index_statements: list[str] = field(default_factory=list)


def strip_sql_comments(sql: str) -> str:
    # Remove /* ... */ block comments and -- line comments
    no_block = re.sub(r"/\*.*?\*/", " ", sql, flags=re.DOTALL)
    no_line = re.sub(r"--[^\r\n]*", "", no_block)
    return no_line


def _split_top_level_commas(body: str) -> list[str]:
    parts: list[str] = []
    depth = 0
    in_sq = False
    buf: list[str] = []
    i = 0
    while i < len(body):
        ch = body[i]
        if ch == "'":
            if in_sq and i + 1 < len(body) and body[i + 1] == "'":
                buf.append("''")
                i += 2
                continue
            if not (i > 0 and body[i - 1] == "\\"):
                in_sq = not in_sq
            buf.append(ch)
        elif not in_sq:
            if ch == "(":
                depth += 1
                buf.append(ch)
            elif ch == ")":
                depth = max(0, depth - 1)
                buf.append(ch)
            elif ch == "," and depth == 0:
                item = "".join(buf).strip()
                if item:
                    parts.append(item)
                buf.clear()
                i += 1
                continue
            else:
                buf.append(ch)
        else:
            buf.append(ch)
        i += 1
    tail = "".join(buf).strip()
    if tail:
        parts.append(tail)
    return parts


def _split_sql_table_items(tbody: str) -> list[str]:
    """Split CREATE TABLE body by top-level commas, rejecting empty/trailing items."""
    if not tbody or not tbody.strip():
        raise ParseError("Empty column/constraint list in CREATE TABLE")
    parts: list[str] = []
    depth = 0
    in_sq = False
    buf: list[str] = []
    i = 0
    while i < len(tbody):
        ch = tbody[i]
        if ch == "'":
            if in_sq and i + 1 < len(tbody) and tbody[i + 1] == "'":
                buf.append("''")
                i += 2
                continue
            if not (in_sq and i > 0 and tbody[i - 1] == "\\"):
                in_sq = not in_sq
            buf.append(ch)
        elif not in_sq:
            if ch == "(":
                depth += 1
                buf.append(ch)
            elif ch == ")":
                depth = max(0, depth - 1)
                buf.append(ch)
            elif ch == "," and depth == 0:
                item = "".join(buf).strip()
                if not item:
                    raise ParseError("Empty column or trailing/double comma in CREATE TABLE")
                parts.append(item)
                buf.clear()
                i += 1
                continue
            else:
                buf.append(ch)
        else:
            buf.append(ch)
        i += 1
    tail = "".join(buf).strip()
    if not tail:
        raise ParseError("Trailing comma in CREATE TABLE")
    parts.append(tail)
    return parts


def _split_sql_statements(sql: str) -> list[str]:
    """Split SQL statements on ';' only outside single-quoted string literals."""
    stmts: list[str] = []
    buf: list[str] = []
    in_sq = False
    i = 0
    n = len(sql)
    while i < n:
        ch = sql[i]
        if ch == "'":
            if in_sq and i + 1 < n and sql[i + 1] == "'":
                buf.append("''")
                i += 2
                continue
            if not (in_sq and i > 0 and sql[i - 1] == "\\"):
                in_sq = not in_sq
            buf.append(ch)
        elif ch == ";" and not in_sq:
            s = "".join(buf).strip()
            if s:
                stmts.append(s)
            buf.clear()
        else:
            buf.append(ch)
        i += 1
    tail = "".join(buf).strip()
    if tail:
        stmts.append(tail)
    return stmts


def _pg_jsonb_array_length(val: Any) -> int:
    if val is None:
        return 0
    if isinstance(val, (bytes, bytearray)):
        val = val.decode("utf-8", errors="replace")
    parsed = json.loads(str(val))
    if not isinstance(parsed, list):
        raise ValueError("jsonb_array_length expects a JSON array")
    return len(parsed)


def _pg_octet_length(val: Any) -> int:
    if val is None:
        return 0
    if isinstance(val, (bytes, bytearray, memoryview)):
        return len(val)
    return len(str(val).encode("utf-8"))


def parse_sql_ddl(sql_text: str) -> SqlDdlInfo:
    if not sql_text or not sql_text.strip():
        raise ParseError("SQL DDL text is empty")
    cleaned = strip_sql_comments(sql_text)

    # Balance check for parentheses outside single-quoted strings
    depth = 0
    in_sq = False
    i = 0
    while i < len(cleaned):
        ch = cleaned[i]
        if ch == "'":
            if in_sq and i + 1 < len(cleaned) and cleaned[i + 1] == "'":
                i += 2
                continue
            if not (in_sq and i > 0 and cleaned[i - 1] == "\\"):
                in_sq = not in_sq
        elif not in_sq:
            if ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
                if depth < 0:
                    raise ParseError("Unmatched ')' in SQL DDL")
        i += 1
    if depth != 0:
        raise ParseError("Unclosed '(' in SQL DDL")
    if in_sq:
        raise ParseError("Unterminated string literal in SQL DDL")

    info = SqlDdlInfo()
    statements = _split_sql_statements(cleaned)
    for stmt in statements:
        upper = re.sub(r"\s+", " ", stmt).strip().upper()
        if upper.startswith("CREATE TABLE"):
            info.create_table_statements.append(stmt)
            m = re.match(
                r"CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:[\w]+\.)?\"?([A-Za-z_][A-Za-z0-9_]*)\"?\s*\((.*)\)\s*$",
                stmt,
                flags=re.IGNORECASE | re.DOTALL,
            )
            if not m:
                raise ParseError(f"Malformed CREATE TABLE statement: {stmt[:80]}")
            tname = m.group(1).lower()
            tbody = m.group(2)
            info.table_defs[tname] = tbody
            cols: list[str] = []
            seen_cols: set[str] = set()
            for item in _split_sql_table_items(tbody):
                stripped_item = item.strip()
                head_word = stripped_item.split()[0].upper() if stripped_item else ""
                if head_word in {"PRIMARY", "FOREIGN", "UNIQUE", "CHECK", "CONSTRAINT"}:
                    continue
                cm = re.match(r'^"?([A-Za-z_][A-Za-z0-9_]*)"?\s+\S+', stripped_item)
                if not cm:
                    raise ParseError(f"Malformed column definition in {tname}: {stripped_item[:60]}")
                cname = cm.group(1).lower()
                if cname in seen_cols:
                    raise ParseError(f"Duplicate column '{cname}' in CREATE TABLE {tname}")
                seen_cols.add(cname)
                cols.append(cname)
            if not cols:
                raise ParseError(f"CREATE TABLE {tname} defines zero columns")
            info.tables[tname] = cols
        elif re.match(r"CREATE\s+(?:UNIQUE\s+)?INDEX\b", upper):
            info.create_index_statements.append(stmt)
        elif re.match(r"(?:CREATE\s+(?:EXTENSION|TYPE)\b|ALTER\s+TABLE\b|COMMENT\s+ON\b|SET\s+)", upper):
            continue
        else:
            raise ParseError(f"Unrecognized SQL statement: {stmt[:80]}")
    if not info.tables:
        raise ParseError("No CREATE TABLE definitions found in SQL DDL")
    return info


def dry_run_sqlite_ddl(sql_text: str) -> SqlDdlInfo:
    """Translate PG DDL subset to SQLite-compatible DDL and execute in :memory:."""
    info = parse_sql_ddl(sql_text)
    cleaned = strip_sql_comments(sql_text)
    stmts = _split_sql_statements(cleaned)

    sqlite_stmts: list[str] = []
    for stmt in stmts:
        upper = re.sub(r"\s+", " ", stmt).upper()
        is_table = upper.startswith("CREATE TABLE")
        is_index = bool(re.match(r"CREATE\s+(?:UNIQUE\s+)?INDEX\b", upper))
        if not (is_table or is_index):
            continue
        s = stmt
        s = re.sub(r"\bIF\s+NOT\s+EXISTS\b", "IF NOT EXISTS", s, flags=re.IGNORECASE)
        if is_index:
            s = re.sub(r"\bUSING\s+(GIN|GIST|BTREE|HASH)\b", "", s, flags=re.IGNORECASE)
            s = re.sub(
                r"\b(?:jsonb_path_ops|jsonb_ops|gin_trgm_ops|text_pattern_ops|varchar_pattern_ops)\b",
                "",
                s,
                flags=re.IGNORECASE,
            )
            s = re.sub(r"\bINCLUDE\s*\([^)]*\)", "", s, flags=re.IGNORECASE)
            s = re.sub(r"\bWHERE\s+.*$", "", s, flags=re.IGNORECASE | re.DOTALL)
        s = re.sub(r"\bTEXT\s*\[\s*\]", "TEXT", s, flags=re.IGNORECASE)
        s = re.sub(r"\bVARCHAR\s*\(\s*\d+\s*\)\s*\[\s*\]", "TEXT", s, flags=re.IGNORECASE)
        s = re.sub(r"\bBIGSERIAL\b", "INTEGER", s, flags=re.IGNORECASE)
        s = re.sub(r"\bSERIAL\b", "INTEGER", s, flags=re.IGNORECASE)
        s = re.sub(r"\bUUID\b", "TEXT", s, flags=re.IGNORECASE)
        s = re.sub(r"\bJSONB?\b", "TEXT", s, flags=re.IGNORECASE)
        s = re.sub(r"\bBYTEA\b", "BLOB", s, flags=re.IGNORECASE)
        s = re.sub(r"\bTIMESTAMPTZ\b", "TEXT", s, flags=re.IGNORECASE)
        s = re.sub(r"\bTIMESTAMP\s+WITH\s+TIME\s+ZONE\b", "TEXT", s, flags=re.IGNORECASE)
        s = re.sub(r"\bTIMESTAMP\b", "TEXT", s, flags=re.IGNORECASE)
        s = re.sub(r"\bDOUBLE\s+PRECISION\b", "REAL", s, flags=re.IGNORECASE)
        s = re.sub(r"\bBOOLEAN\b", "INTEGER", s, flags=re.IGNORECASE)
        s = re.sub(r"\bDEFAULT\s+NOW\s*\(\s*\)", "DEFAULT CURRENT_TIMESTAMP", s, flags=re.IGNORECASE)
        s = re.sub(r"\bDEFAULT\s+gen_random_uuid\s*\(\s*\)", "DEFAULT ''", s, flags=re.IGNORECASE)
        s = re.sub(r"\bDEFAULT\s+TRUE\b", "DEFAULT 1", s, flags=re.IGNORECASE)
        s = re.sub(r"\bDEFAULT\s+FALSE\b", "DEFAULT 0", s, flags=re.IGNORECASE)
        s = re.sub(r"::[A-Za-z_][A-Za-z0-9_\[\]]*", "", s)
        sqlite_stmts.append(s)

    conn = sqlite3.connect(":memory:")
    try:
        conn.create_function("jsonb_array_length", 1, _pg_jsonb_array_length)
        conn.create_function("octet_length", 1, _pg_octet_length)
        cur = conn.cursor()
        for s in sqlite_stmts:
            try:
                cur.execute(s)
            except sqlite3.Error as exc:
                raise ParseError(f"SQLite DDL dry-run failed on `{s[:90]}...`: {exc}") from exc
    finally:
        conn.close()
    return info


# ---------------------------------------------------------------------------
# Qdrant Collection Schema
# ---------------------------------------------------------------------------

_VALID_QDRANT_FIELD_TYPES = {
    "keyword",
    "integer",
    "float",
    "bool",
    "boolean",
    "geo",
    "datetime",
    "text",
    "uuid",
}


def _extract_qdrant_payload_indexes(cname: str, cfg: dict[str, Any]) -> dict[str, str]:
    raw_idx = cfg.get("payload_indexes")
    if raw_idx is None:
        raw_idx = cfg.get("payload_schema")
    if raw_idx is None:
        raise ParseError(f"Collection '{cname}' missing required 'payload_indexes' / 'payload_schema'")

    parsed: dict[str, str] = {}
    if isinstance(raw_idx, list):
        if not raw_idx:
            raise ParseError(f"Collection '{cname}' payload_indexes list is empty")
        for item in raw_idx:
            if not isinstance(item, dict):
                raise ParseError(f"Collection '{cname}' payload_indexes entry must be object: {item!r}")
            fname = item.get("field_name")
            fschema = item.get("field_schema") if "field_schema" in item else item.get("type")
            if not isinstance(fname, str) or not fname.strip():
                raise ParseError(f"Collection '{cname}' payload_indexes entry missing field_name: {item!r}")
            if not isinstance(fschema, str) or fschema.lower() not in _VALID_QDRANT_FIELD_TYPES:
                raise ParseError(
                    f"Collection '{cname}' payload index '{fname}' invalid schema type: {fschema!r}"
                )
            parsed[fname] = fschema.lower()
    elif isinstance(raw_idx, dict):
        if not raw_idx:
            raise ParseError(f"Collection '{cname}' payload_indexes dict is empty")
        for fname, val in raw_idx.items():
            if not isinstance(fname, str) or not fname.strip():
                raise ParseError(f"Collection '{cname}' invalid payload index field name: {fname!r}")
            if isinstance(val, dict):
                fschema = val.get("type") if "type" in val else val.get("field_schema")
            else:
                fschema = val
            if not isinstance(fschema, str) or fschema.lower() not in _VALID_QDRANT_FIELD_TYPES:
                raise ParseError(
                    f"Collection '{cname}' payload index '{fname}' invalid schema type: {fschema!r}"
                )
            parsed[fname] = fschema.lower()
    else:
        raise ParseError(f"Collection '{cname}' payload_indexes must be list or dict, got {type(raw_idx).__name__}")
    return parsed


def validate_qdrant_collections(data: Any) -> dict[str, dict[str, Any]]:
    """Normalize & validate Qdrant collections.json (list or dict keyed by name)."""
    if not isinstance(data, dict):
        raise ParseError("Qdrant collections JSON must be an object")

    raw_cols = data.get("collections", data)
    collections: dict[str, dict[str, Any]] = {}
    if isinstance(raw_cols, list):
        for item in raw_cols:
            if not isinstance(item, dict) or "name" not in item:
                raise ParseError("Each item in collections list must have 'name'")
            collections[str(item["name"])] = item
    elif isinstance(raw_cols, dict):
        for k, v in raw_cols.items():
            if k in {"$schema", "description", "version"}:
                continue
            if isinstance(v, dict):
                collections[str(k)] = v
    else:
        raise ParseError("Invalid collections structure in Qdrant JSON")

    for req_name in ("medical_knowledge", "patient_l3_events"):
        if req_name not in collections:
            raise ParseError(f"Missing required Qdrant collection: {req_name}")

    for cname, cfg in collections.items():
        vec = cfg.get("vectors") or cfg.get("vector_config") or cfg
        size = vec.get("size") if isinstance(vec, dict) else None
        distance = vec.get("distance") if isinstance(vec, dict) else None
        if isinstance(size, bool) or not isinstance(size, int) or size <= 0:
            raise ParseError(f"Collection '{cname}' vectors.size must be positive int, got {size!r}")
        if not isinstance(distance, str) or distance.lower() not in {"cosine", "dot", "euclid"}:
            raise ParseError(f"Collection '{cname}' vectors.distance invalid: {distance!r}")
        idx_map = _extract_qdrant_payload_indexes(cname, cfg)
        if cname == "medical_knowledge":
            for req_idx in ("target_population", "department", "authority_level", "seasonal_tags"):
                if req_idx not in idx_map:
                    raise ParseError(f"Collection 'medical_knowledge' missing required payload index '{req_idx}'")
            if idx_map["authority_level"] not in {"float", "integer"}:
                raise ParseError(
                    f"Collection 'medical_knowledge' authority_level must be float/integer, got {idx_map['authority_level']!r}"
                )
        elif cname == "patient_l3_events":
            if "patient_id" not in idx_map:
                raise ParseError("Collection 'patient_l3_events' missing required payload index 'patient_id'")
            if idx_map["patient_id"] != "keyword":
                raise ParseError(
                    f"Collection 'patient_l3_events' patient_id must be keyword, got {idx_map['patient_id']!r}"
                )
    return collections


# ---------------------------------------------------------------------------
# Pure-Stdlib Structural YAML-Subset Parser (Prometheus / Alerts / Grafana)
# ---------------------------------------------------------------------------

def _strip_yaml_comment(line: str) -> str:
    in_sq = False
    in_dq = False
    for idx, ch in enumerate(line):
        if ch == "'" and not in_dq:
            in_sq = not in_sq
        elif ch == '"' and not in_sq and (idx == 0 or line[idx - 1] != "\\"):
            in_dq = not in_dq
        elif ch == "#" and not in_sq and not in_dq:
            if idx == 0 or line[idx - 1].isspace():
                return line[:idx]
    return line


def _split_yaml_flow_commas(inner: str) -> list[str]:
    """Split YAML flow sequence/mapping items by commas outside [], {}, and quotes."""
    parts: list[str] = []
    bracket_depth = 0
    brace_depth = 0
    in_sq = False
    in_dq = False
    buf: list[str] = []
    i = 0
    n = len(inner)
    while i < n:
        ch = inner[i]
        if in_dq:
            if ch == "\\" and i + 1 < n:
                buf.append(ch)
                buf.append(inner[i + 1])
                i += 2
                continue
            if ch == '"':
                in_dq = False
            buf.append(ch)
        elif in_sq:
            if ch == "'" and i + 1 < n and inner[i + 1] == "'":
                buf.append("''")
                i += 2
                continue
            if ch == "'":
                in_sq = False
            buf.append(ch)
        else:
            if ch == '"':
                in_dq = True
                buf.append(ch)
            elif ch == "'":
                in_sq = True
                buf.append(ch)
            elif ch == "[":
                bracket_depth += 1
                buf.append(ch)
            elif ch == "]":
                bracket_depth -= 1
                if bracket_depth < 0:
                    raise ParseError(f"Unmatched ']' in YAML flow value: {inner}")
                buf.append(ch)
            elif ch == "{":
                brace_depth += 1
                buf.append(ch)
            elif ch == "}":
                brace_depth -= 1
                if brace_depth < 0:
                    raise ParseError(f"Unmatched '}}' in YAML flow value: {inner}")
                buf.append(ch)
            elif ch == "," and bracket_depth == 0 and brace_depth == 0:
                item = "".join(buf).strip()
                if not item:
                    raise ParseError(f"Empty item in YAML flow collection: {inner}")
                parts.append(item)
                buf.clear()
                i += 1
                continue
            else:
                buf.append(ch)
        i += 1
    if in_sq or in_dq or bracket_depth != 0 or brace_depth != 0:
        raise ParseError(f"Unclosed delimiter/quote in YAML flow collection: {inner}")
    tail = "".join(buf).strip()
    if tail:
        parts.append(tail)
    return parts


def _split_yaml_kv(line: str, *, require_space_or_eol: bool = True) -> tuple[str, str] | None:
    """Find first YAML key:value separator outside quotes, [], and {}."""
    in_sq = False
    in_dq = False
    bracket_depth = 0
    brace_depth = 0
    i = 0
    n = len(line)
    while i < n:
        ch = line[i]
        if in_dq:
            if ch == "\\" and i + 1 < n:
                i += 2
                continue
            if ch == '"':
                in_dq = False
        elif in_sq:
            if ch == "'" and i + 1 < n and line[i + 1] == "'":
                i += 2
                continue
            if ch == "'":
                in_sq = False
        else:
            if ch == '"':
                in_dq = True
            elif ch == "'":
                in_sq = True
            elif ch == "[":
                bracket_depth += 1
            elif ch == "]":
                bracket_depth = max(0, bracket_depth - 1)
            elif ch == "{":
                brace_depth += 1
            elif ch == "}":
                brace_depth = max(0, brace_depth - 1)
            elif ch == ":" and bracket_depth == 0 and brace_depth == 0:
                if not require_space_or_eol or i == n - 1 or line[i + 1].isspace():
                    return line[:i], line[i + 1 :]
        i += 1
    return None


def _parse_yaml_scalar(val: str) -> Any:
    s = val.strip()
    if s == "" or s in {"null", "~"}:
        return None
    if s in {"true", "True"}:
        return True
    if s in {"false", "False"}:
        return False
    if s.startswith('"') or s.startswith("'"):
        q = s[0]
        if len(s) < 2 or not s.endswith(q):
            raise ParseError(f"Unterminated YAML quote: {s}")
        return s[1:-1]
    if s.startswith("["):
        if not s.endswith("]"):
            raise ParseError(f"Unclosed YAML flow sequence: {s}")
        inner = s[1:-1].strip()
        if not inner:
            return []
        return [_parse_yaml_scalar(part) for part in _split_yaml_flow_commas(inner)]
    if s.startswith("{"):
        if not s.endswith("}"):
            raise ParseError(f"Unclosed YAML flow mapping: {s}")
        inner = s[1:-1].strip()
        if not inner:
            return {}
        out: dict[str, Any] = {}
        for part in _split_yaml_flow_commas(inner):
            kv = _split_yaml_kv(part, require_space_or_eol=False)
            if kv is None:
                raise ParseError(f"Invalid inline YAML mapping entry: {part}")
            k, v = kv
            out[k.strip().strip("'\"")] = _parse_yaml_scalar(v)
        return out
    if re.fullmatch(r"-?\d+", s):
        return int(s)
    if re.fullmatch(r"-?\d+\.\d+", s):
        return float(s)
    # Check unbalanced quotes in unquoted scalar
    if s.count('"') % 2 != 0 or s.count("'") % 2 != 0:
        raise ParseError(f"Unbalanced quotes in YAML value: {s}")
    return s


def parse_yaml_subset(text: str) -> Any:
    """Deterministic stdlib YAML-subset parser for Prometheus/Grafana config files."""
    if not text or not text.strip():
        raise ParseError("YAML content is empty")

    tokens: list[tuple[int, str]] = []
    for lineno, raw in enumerate(text.splitlines(), start=1):
        if "\t" in raw[: len(raw) - len(raw.lstrip(" \t"))]:
            raise ParseError(f"Tab indentation forbidden in YAML (line {lineno})")
        stripped_comment = _strip_yaml_comment(raw).rstrip()
        if not stripped_comment.strip():
            continue
        if stripped_comment.strip() in {"---", "..."}:
            continue
        indent = len(stripped_comment) - len(stripped_comment.lstrip(" "))
        tokens.append((indent, stripped_comment.lstrip(" ")))

    if not tokens:
        raise ParseError("YAML contains no data nodes")

    def parse_block(idx: int, min_indent: int) -> tuple[Any, int]:
        indent0, line0 = tokens[idx]
        if line0.startswith("- ") or line0 == "-":
            items: list[Any] = []
            while idx < len(tokens):
                ind, ln = tokens[idx]
                if ind < min_indent:
                    break
                if ind == indent0 and not (ln.startswith("- ") or ln == "-"):
                    break
                if ind != indent0 or not (ln.startswith("- ") or ln == "-"):
                    if ind < indent0:
                        break
                    raise ParseError(f"Unexpected YAML list indentation near: {ln}")
                rest = ln[1:].lstrip(" ")
                if not rest:
                    idx += 1
                    if idx < len(tokens) and tokens[idx][0] > indent0:
                        child, idx = parse_block(idx, tokens[idx][0])
                        items.append(child)
                    else:
                        items.append(None)
                else:
                    rest_kv = (
                        None
                        if rest.startswith(('"', "'", "[", "{"))
                        else _split_yaml_kv(rest, require_space_or_eol=True)
                    )
                    if rest_kv is not None:
                        # Inline mapping start after `- ` (e.g. `- alert: Foo`)
                        item_dict: dict[str, Any] = {}
                        k, v = rest_kv
                        k = k.strip().strip("'\"")
                        v_str = v.strip()
                        item_indent = indent0 + 2
                        idx += 1
                        if re.fullmatch(r"[|>][+-]?\d*", v_str):
                            block_lines: list[str] = []
                            while idx < len(tokens) and tokens[idx][0] > indent0:
                                block_lines.append(tokens[idx][1])
                                idx += 1
                            joiner = "\n" if v_str.startswith("|") else " "
                            item_dict[k] = joiner.join(block_lines)
                        elif v_str == "":
                            if idx < len(tokens) and tokens[idx][0] > item_indent:
                                child, idx = parse_block(idx, tokens[idx][0])
                                item_dict[k] = child
                            elif idx < len(tokens) and tokens[idx][0] == item_indent and (tokens[idx][1].startswith("- ") or tokens[idx][1] == "-"):
                                child, idx = parse_block(idx, item_indent)
                                item_dict[k] = child
                            else:
                                item_dict[k] = None
                        else:
                            item_dict[k] = _parse_yaml_scalar(v_str)

                        while idx < len(tokens):
                            n_ind, n_ln = tokens[idx]
                            if n_ind < item_indent:
                                break
                            if n_ind == indent0 and (n_ln.startswith("- ") or n_ln == "-"):
                                break
                            n_kv = _split_yaml_kv(n_ln, require_space_or_eol=True)
                            if n_kv is None:
                                raise ParseError(f"Expected key:value in list item near: {n_ln}")
                            nk, nv = n_kv
                            nk = nk.strip().strip("'\"")
                            nv_str = nv.strip()
                            idx += 1
                            if re.fullmatch(r"[|>][+-]?\d*", nv_str):
                                block_lines = []
                                while idx < len(tokens) and tokens[idx][0] > n_ind:
                                    block_lines.append(tokens[idx][1])
                                    idx += 1
                                joiner = "\n" if nv_str.startswith("|") else " "
                                item_dict[nk] = joiner.join(block_lines)
                            elif nv_str == "":
                                if idx < len(tokens) and tokens[idx][0] > n_ind:
                                    child, idx = parse_block(idx, tokens[idx][0])
                                    item_dict[nk] = child
                                elif idx < len(tokens) and tokens[idx][0] == n_ind and (tokens[idx][1].startswith("- ") or tokens[idx][1] == "-"):
                                    child, idx = parse_block(idx, n_ind)
                                    item_dict[nk] = child
                                else:
                                    item_dict[nk] = None
                            else:
                                item_dict[nk] = _parse_yaml_scalar(nv_str)
                        items.append(item_dict)
                    else:
                        items.append(_parse_yaml_scalar(rest))
                        idx += 1
            return items, idx

        mapping: dict[str, Any] = {}
        while idx < len(tokens):
            ind, ln = tokens[idx]
            if ind < min_indent:
                break
            if ind != indent0:
                raise ParseError(f"Inconsistent YAML mapping indentation near: {ln}")
            if ln.startswith("- ") or ln == "-":
                break
            kv = _split_yaml_kv(ln, require_space_or_eol=True)
            if kv is None:
                raise ParseError(f"Invalid YAML mapping line (missing ':'): {ln}")
            k, v = kv
            k = k.strip().strip("'\"")
            v_str = v.strip()
            idx += 1
            if re.fullmatch(r"[|>][+-]?\d*", v_str):
                block_lines = []
                while idx < len(tokens) and tokens[idx][0] > indent0:
                    block_lines.append(tokens[idx][1])
                    idx += 1
                joiner = "\n" if v_str.startswith("|") else " "
                mapping[k] = joiner.join(block_lines)
            elif v_str == "":
                if idx < len(tokens) and tokens[idx][0] > indent0:
                    child, idx = parse_block(idx, tokens[idx][0])
                    mapping[k] = child
                elif idx < len(tokens) and tokens[idx][0] == indent0 and (tokens[idx][1].startswith("- ") or tokens[idx][1] == "-"):
                    child, idx = parse_block(idx, indent0)
                    mapping[k] = child
                else:
                    mapping[k] = None
            else:
                mapping[k] = _parse_yaml_scalar(v_str)
        return mapping, idx

    result, next_idx = parse_block(0, tokens[0][0])
    if next_idx < len(tokens):
        raise ParseError(f"Trailing unparsed YAML near: {tokens[next_idx][1]}")
    return result


# ---------------------------------------------------------------------------
# Grafana Dashboard, Dockerfile, k6 JS, Miniapp WXML/WXSS, ICO
# ---------------------------------------------------------------------------

def validate_grafana_dashboard(data: Any) -> dict[str, Any]:
    if not isinstance(data, dict):
        raise ParseError("Grafana dashboard JSON must be an object")
    title = data.get("title")
    if not isinstance(title, str) or not title.strip():
        raise ParseError("Grafana dashboard missing non-empty 'title'")
    panels = data.get("panels")
    if not isinstance(panels, list) or not panels:
        raise ParseError("Grafana dashboard missing non-empty 'panels' array")

    seen_ids: set[Any] = set()
    expr_count = 0
    for panel in panels:
        if not isinstance(panel, dict):
            raise ParseError("Grafana panel must be a JSON object")
        pid = panel.get("id")
        if pid is None or pid in seen_ids:
            raise ParseError(f"Duplicate or missing Grafana panel id: {pid!r}")
        seen_ids.add(pid)
        gp = panel.get("gridPos")
        if not isinstance(gp, dict):
            raise ParseError(f"Panel {pid} missing gridPos")
        x, w, h = gp.get("x"), gp.get("w"), gp.get("h")
        if not all(isinstance(v, (int, float)) for v in (x, w, h)):
            raise ParseError(f"Panel {pid} invalid gridPos types: {gp}")
        if not (0 <= x < 24 and w > 0 and h > 0 and x + w <= 24):
            raise ParseError(f"Panel {pid} gridPos out of 24-column bounds: {gp}")
        for tgt in panel.get("targets", []):
            if isinstance(tgt, dict) and isinstance(tgt.get("expr"), str) and tgt["expr"].strip():
                expr_count += 1
    if expr_count == 0:
        raise ParseError("Grafana dashboard contains zero PromQL targets[].expr")
    return data


@dataclass
class DockerfileInfo:
    stages: list[str]
    copy_from: list[str]
    exposes: list[int]
    users: list[str]
    entrypoint_or_cmd: list[str]
    raw_lines: list[str]


def _join_dockerfile_continuations(text: str) -> list[str]:
    logical: list[str] = []
    acc: list[str] = []
    for raw in text.splitlines():
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            if not acc:
                continue
            # Comment inside continuation block is ignored in Dockerfiles
            continue
        if stripped.endswith("\\") and not stripped.endswith("\\\\"):
            acc.append(stripped[:-1].rstrip())
        else:
            acc.append(stripped)
            logical.append(" ".join(p for p in acc if p))
            acc.clear()
    if acc:
        logical.append(" ".join(p for p in acc if p))
    return logical


def parse_dockerfile(text: str) -> DockerfileInfo:
    if not text or not text.strip():
        raise ParseError("Dockerfile is empty")
    stages: list[str] = []
    stage_aliases: set[str] = set()
    copy_from: list[str] = []
    exposes: list[int] = []
    users: list[str] = []
    cmds: list[str] = []
    raw_lines = text.splitlines()
    logical_lines = _join_dockerfile_continuations(text)
    for line in logical_lines:
        m_from = re.match(r"^FROM\s+(.+)$", line, flags=re.IGNORECASE)
        if m_from:
            from_arg = m_from.group(1).strip()
            stages.append(from_arg)
            m_alias = re.search(r"\bAS\s+([A-Za-z0-9_.-]+)\s*$", from_arg, flags=re.IGNORECASE)
            if m_alias:
                stage_aliases.add(m_alias.group(1).lower())
        m_cf = re.match(r"^COPY\b.*?--from=([^\s]+)", line, flags=re.IGNORECASE)
        if m_cf:
            src_stage = m_cf.group(1)
            copy_from.append(src_stage)
            if not src_stage.isdigit() and "/" not in src_stage and ":" not in src_stage:
                if src_stage.lower() not in stage_aliases:
                    raise ParseError(
                        f"COPY --from={src_stage} references unknown stage alias (known={sorted(stage_aliases)})"
                    )
        m_exp = re.match(r"^EXPOSE\s+(\d+)", line, flags=re.IGNORECASE)
        if m_exp:
            exposes.append(int(m_exp.group(1)))
        m_usr = re.match(r"^USER\s+([^\s]+)", line, flags=re.IGNORECASE)
        if m_usr:
            users.append(m_usr.group(1))
        if re.match(r"^(ENTRYPOINT|CMD)\b", line, flags=re.IGNORECASE):
            cmds.append(line)
    if not stages:
        raise ParseError("Dockerfile contains no FROM instruction")
    return DockerfileInfo(
        stages=stages,
        copy_from=copy_from,
        exposes=exposes,
        users=users,
        entrypoint_or_cmd=cmds,
        raw_lines=raw_lines,
    )


_JS_REGEX_PREV_KEYWORDS = {
    "return",
    "case",
    "throw",
    "typeof",
    "void",
    "delete",
    "in",
    "instanceof",
    "new",
    "else",
    "yield",
    "await",
    "of",
}


def _check_js_balanced_delimiters(js_text: str) -> str:
    """Validate balanced (), {}, [] and return comment-stripped JS text."""
    stack: list[str] = []
    template_expr_stack: list[int] = []
    out: list[str] = []
    pairs = {"(": ")", "{": "}", "[": "]"}
    closing = {")", "}", "]"}
    i = 0
    n = len(js_text)
    in_sq = in_dq = in_bt = False
    in_line_comment = in_block_comment = False
    in_regex = in_regex_class = False
    last_sig_ch = ""
    last_ident = ""
    ident_buf: list[str] = []

    def flush_ident() -> None:
        nonlocal last_ident
        if ident_buf:
            last_ident = "".join(ident_buf)
            ident_buf.clear()

    while i < n:
        ch = js_text[i]
        nxt = js_text[i + 1] if i + 1 < n else ""

        if in_line_comment:
            if ch == "\n":
                in_line_comment = False
                out.append("\n")
            i += 1
            continue

        if in_block_comment:
            if ch == "*" and nxt == "/":
                in_block_comment = False
                out.append(" ")
                i += 2
                continue
            if ch == "\n":
                out.append("\n")
            i += 1
            continue

        if in_sq:
            out.append(ch)
            if ch == "\\":
                if i + 1 < n:
                    out.append(js_text[i + 1])
                    i += 2
                    continue
            elif ch == "'":
                in_sq = False
                last_sig_ch = "'"
                last_ident = ""
            i += 1
            continue

        if in_dq:
            out.append(ch)
            if ch == "\\":
                if i + 1 < n:
                    out.append(js_text[i + 1])
                    i += 2
                    continue
            elif ch == '"':
                in_dq = False
                last_sig_ch = '"'
                last_ident = ""
            i += 1
            continue

        if in_regex:
            out.append(ch)
            if ch == "\\":
                if i + 1 < n:
                    out.append(js_text[i + 1])
                    i += 2
                    continue
            elif ch == "[" and not in_regex_class:
                in_regex_class = True
            elif ch == "]" and in_regex_class:
                in_regex_class = False
            elif ch == "/" and not in_regex_class:
                in_regex = False
                last_sig_ch = "/"
                last_ident = ""
                i += 1
                while i < n and js_text[i].isalpha():
                    out.append(js_text[i])
                    i += 1
                continue
            i += 1
            continue

        if in_bt:
            out.append(ch)
            if ch == "\\":
                if i + 1 < n:
                    out.append(js_text[i + 1])
                    i += 2
                    continue
            elif ch == "$" and nxt == "{":
                out.append("{")
                template_expr_stack.append(len(stack))
                in_bt = False
                last_sig_ch = "{"
                last_ident = ""
                i += 2
                continue
            elif ch == "`":
                in_bt = False
                last_sig_ch = "`"
                last_ident = ""
            i += 1
            continue

        # Normal JS code
        if ch == "/" and nxt == "/":
            flush_ident()
            in_line_comment = True
            i += 2
            continue
        if ch == "/" and nxt == "*":
            flush_ident()
            in_block_comment = True
            i += 2
            continue

        if ch == "/":
            flush_ident()
            is_regex_start = (
                last_sig_ch in {"", "(", "[", "{", ",", ";", ":", "=", "!", "&", "|", "?", "~", "+", "-", "*", "%", "^", "<", ">"}
                or last_ident in _JS_REGEX_PREV_KEYWORDS
            )
            if is_regex_start:
                in_regex = True
                in_regex_class = False
                out.append(ch)
                i += 1
                continue

        if ch.isalnum() or ch in {"_", "$"}:
            ident_buf.append(ch)
            last_sig_ch = ch
            out.append(ch)
            i += 1
            continue

        flush_ident()
        if not ch.isspace():
            if ch not in {"'", '"', "`"}:
                last_ident = ""
            last_sig_ch = ch

        out.append(ch)
        if ch == "'":
            in_sq = True
        elif ch == '"':
            in_dq = True
        elif ch == "`":
            in_bt = True
        elif ch in pairs:
            stack.append(pairs[ch])
        elif ch in closing:
            if ch == "}" and template_expr_stack and len(stack) == template_expr_stack[-1]:
                template_expr_stack.pop()
                in_bt = True
            else:
                if not stack or stack[-1] != ch:
                    raise ParseError(f"Unbalanced JS delimiter '{ch}' at index {i}")
                stack.pop()
        i += 1

    if in_sq or in_dq or in_bt or template_expr_stack:
        raise ParseError("Unterminated string/template literal in JS")
    if in_regex:
        raise ParseError("Unterminated regex literal in JS")
    if stack:
        raise ParseError(f"Unclosed JS delimiter(s): {''.join(stack)}")
    return "".join(out)


def validate_k6_script(js_text: str) -> dict[str, Any]:
    if not js_text or not js_text.strip():
        raise ParseError("k6 script is empty")
    stripped = _check_js_balanced_delimiters(js_text)
    has_k6_http = bool(re.search(r"['\"]k6/http['\"]", stripped))
    has_options = bool(re.search(r"\bexport\s+(?:const|let)\s+options\b", stripped))
    has_default_fn = bool(re.search(r"\bexport\s+default\s+(?:async\s+)?function\b", stripped))
    if not has_k6_http:
        raise ParseError("k6 script missing `k6/http` import")
    if not has_options:
        raise ParseError("k6 script missing `export const options`")
    if not has_default_fn:
        raise ParseError("k6 script missing `export default function`")
    endpoints = sorted(set(re.findall(r"/(?:health|metrics|v1/[a-zA-Z0-9_/{}-]+)", stripped)))
    return {
        "endpoints": endpoints,
        "uses_env": "__ENV" in stripped,
        "uses_sleep": "sleep(" in stripped,
        "has_thresholds": "thresholds" in stripped,
    }


def validate_wxml(wxml_text: str) -> list[str]:
    """Verify balanced WXML tags (ignoring mustache {{ ... }} inside attributes/text)."""
    if not wxml_text or not wxml_text.strip():
        raise ParseError("WXML content is empty")
    cleaned = re.sub(r"<!--.*?-->", "", wxml_text, flags=re.DOTALL)

    # Validate mustache {{ ... }} balance before replacing
    pos = 0
    while pos < len(cleaned):
        nxt_open = cleaned.find("{{", pos)
        nxt_close = cleaned.find("}}", pos)
        if nxt_close != -1 and (nxt_open == -1 or nxt_close < nxt_open):
            raise ParseError("Stray '}}' mustache delimiter in WXML")
        if nxt_open == -1:
            break
        end_close = cleaned.find("}}", nxt_open + 2)
        if end_close == -1:
            raise ParseError("Unclosed '{{' mustache delimiter in WXML")
        pos = end_close + 2

    # Strip mustache blocks before scanning `<` `>` inside attribute values
    no_mustache = re.sub(r"\{\{.*?\}\}", "EXPR", cleaned, flags=re.DOTALL)
    tag_re = re.compile(r"<(/?)([A-Za-z][A-Za-z0-9_-]*)([^>]*)>", flags=re.DOTALL)

    non_tag = tag_re.sub("", no_mustache)
    if "<" in non_tag or ">" in non_tag:
        raise ParseError("Malformed or unclosed '<'/'>' tag fragment in WXML")

    stack: list[str] = []
    tags_seen: list[str] = []
    for m in tag_re.finditer(no_mustache):
        is_close = m.group(1) == "/"
        tag = m.group(2)
        attrs = m.group(3)
        if "<" in attrs:
            raise ParseError(f"Unclosed '<' inside tag <{tag}> attributes")
        self_close = attrs.rstrip().endswith("/")
        tags_seen.append(tag)
        if is_close:
            if attrs.strip():
                raise ParseError(f"Closing tag </{tag}> must not have attributes: {attrs!r}")
            if not stack or stack[-1] != tag:
                raise ParseError(f"Mismatched closing WXML tag </{tag}>; stack={stack}")
            stack.pop()
        elif not self_close:
            stack.append(tag)
    if stack:
        raise ParseError(f"Unclosed WXML tag(s): {stack}")
    if not tags_seen:
        raise ParseError("No WXML tags found")
    return tags_seen


def validate_wxss(wxss_text: str) -> int:
    if not wxss_text or not wxss_text.strip():
        raise ParseError("WXSS content is empty")
    cleaned = re.sub(r"/\*.*?\*/", "", wxss_text, flags=re.DOTALL)
    depth = 0
    for ch in cleaned:
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth < 0:
                raise ParseError("Unmatched '}' in WXSS")
    if depth != 0:
        raise ParseError("Unclosed '{' in WXSS")
    return cleaned.count("{")


def validate_ico_bytes(data: bytes) -> int:
    """Validate Windows .ico header, directory bounds, and PNG/BMP image payloads."""
    if len(data) < 22:
        raise ParseError(f"ICO too short ({len(data)} < 22 bytes)")
    if data[:4] != b"\x00\x00\x01\x00":
        raise ParseError(f"Invalid ICO magic header: {data[:4]!r}")
    count = int.from_bytes(data[4:6], "little")
    if count < 1:
        raise ParseError("ICO directory contains 0 images")
    header_end = 6 + 16 * count
    if len(data) < header_end:
        raise ParseError("ICO directory truncated")
    for idx in range(count):
        entry = data[6 + 16 * idx : 22 + 16 * idx]
        img_size = int.from_bytes(entry[8:12], "little")
        img_offset = int.from_bytes(entry[12:16], "little")
        if img_size == 0 or img_offset < header_end or img_offset + img_size > len(data):
            raise ParseError(
                f"ICO entry {idx} invalid offset/size ({img_offset}+{img_size} > {len(data)})"
            )
        payload = data[img_offset : img_offset + img_size]
        is_png = payload.startswith(b"\x89PNG\r\n\x1a\n") and len(payload) >= 16
        is_bmp = (
            len(payload) >= 40
            and int.from_bytes(payload[:4], "little") in {12, 40, 52, 56, 108, 124}
        )
        if not (is_png or is_bmp):
            raise ParseError(f"ICO entry {idx} payload is neither valid PNG nor BMP/DIB")
    return count


# ---------------------------------------------------------------------------
# SSE Frame Parser & Prometheus Metrics Parser
# ---------------------------------------------------------------------------

@dataclass
class SseEvent:
    event: str
    data: str
    id: str | None = None

    def json(self) -> Any:
        return json.loads(self.data)


def parse_sse_frames(raw_text: str, *, strict: bool = False) -> list[SseEvent]:
    """Parse SSE text/event-stream payload into SseEvent objects (ASCII CR/LF only)."""
    events: list[SseEvent] = []
    cur_event = "message"
    cur_id: str | None = None
    data_lines: list[str] = []

    def flush() -> None:
        nonlocal cur_event, cur_id, data_lines
        if data_lines or cur_id is not None or cur_event != "message":
            events.append(SseEvent(event=cur_event, data="\n".join(data_lines), id=cur_id))
        cur_event = "message"
        cur_id = None
        data_lines = []

    for line in re.split(r"\r\n|\n|\r", raw_text):
        if line == "":
            flush()
            continue
        if line.startswith(":"):
            continue
        if ":" in line:
            field_name, rest = line.split(":", 1)
            val = rest[1:] if rest.startswith(" ") else rest
        else:
            if strict:
                raise ParseError(f"Malformed SSE line without colon: {line!r}")
            field_name, val = line, ""

        if field_name == "event":
            cur_event = val
        elif field_name == "data":
            data_lines.append(val)
        elif field_name == "id":
            cur_id = val
        elif strict and field_name not in {"retry"}:
            raise ParseError(f"Unknown SSE field {field_name!r}")

    flush()
    return events


def parse_prometheus_metrics(text: str) -> dict[str, list[tuple[str, float]]]:
    """Parse Prometheus text exposition format into base metric series."""
    metrics: dict[str, list[tuple[str, float]]] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        m = re.match(
            r"^([a-zA-Z_:][a-zA-Z0-9_:]*)(\{[^}]*\})?\s+([+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?|NaN|[+-]?Inf)(?:\s+\d+)?$",
            line,
        )
        if not m:
            continue
        name = m.group(1)
        labels = m.group(2) or ""
        try:
            val = float(m.group(3))
        except ValueError:
            continue
        metrics.setdefault(name, []).append((labels, val))
    return metrics


def read_xlsx_rows(path: Path | str) -> list[list[str]]:
    """Inspect an OOXML .xlsx sheet1.xml using stdlib zipfile + ElementTree."""
    with zipfile.ZipFile(path, "r") as zf:
        xml_bytes = zf.read("xl/worksheets/sheet1.xml")
    root = ET.fromstring(xml_bytes)
    ns = {"s": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
    rows: list[list[str]] = []
    for row_el in root.findall(".//s:sheetData/s:row", ns):
        vals: list[str] = []
        for c_el in row_el.findall("s:c", ns):
            t_el = c_el.find("s:is/s:t", ns)
            v_el = c_el.find("s:v", ns)
            if t_el is not None:
                vals.append(t_el.text or "")
            elif v_el is not None:
                vals.append(v_el.text or "")
            else:
                vals.append("")
        rows.append(vals)
    return rows


def read_docx_paragraphs(path: Path | str) -> list[str]:
    """Inspect an OOXML .docx word/document.xml using stdlib zipfile + ElementTree."""
    with zipfile.ZipFile(path, "r") as zf:
        xml_bytes = zf.read("word/document.xml")
    root = ET.fromstring(xml_bytes)
    ns = {"w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main"}
    paras: list[str] = []
    for p_el in root.findall(".//w:body/w:p", ns):
        texts = [t.text or "" for t in p_el.findall(".//w:t", ns)]
        joined = "".join(texts).strip()
        if joined:
            paras.append(joined)
    return paras


def inspect_pdf_bytes(data: bytes) -> dict[str, Any]:
    if not data.startswith(b"%PDF-"):
        raise ParseError("Missing %PDF- header")
    if b"xref" not in data or b"%%EOF" not in data:
        raise ParseError("Missing xref or %%EOF trailer in PDF")
    has_text_ops = b"Tj" in data or b"TJ" in data
    return {"size": len(data), "has_text_ops": has_text_ops}

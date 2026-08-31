#!/usr/bin/env python3
"""Sylvode Flow REST/MCP/CLI/UI surface-coverage parser and cross-checker.

Contract: /opt/working/sylvode-flow/gates/gate-commands.md, "Surface coverage
verifier (v0.4-v1.0 共用)" section, and
/opt/working/sylvode-flow/contracts/surface-coverage-v1.md.

This module parses the five frozen markdown contract files and recomputes
their internal cross-reference matrix.  The shell verifier then combines
this result with scripts/lib/flow_surface_implementation.py, which checks
the shipped MCP/CLI binaries and the API route registrations.

Inputs (paths under --contracts-root):
  contracts/rest-api-v1.md
  contracts/mcp-surface-v1.md
  contracts/cli-surface-v1.md
  contracts/ui-surface-v1.md
  contracts/surface-coverage-v1.md

Output: a dict matching docs/schemas/sylvode-flow-surface-coverage-result-v1.schema.json
(minus schema_version/release/source_head/generated_at, which the caller
fills in).
"""
from __future__ import annotations

import hashlib
import re
import sys
from dataclasses import dataclass, field


def sha256_file(path: str) -> str:
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def normalize_rest_identity(method: str, path: str) -> str:
    """Normalized primary key: METHOD + path with query values and the
    /api/v1 base stripped (surface-coverage-v1.md "主键" note)."""
    p = path.strip()
    p = re.sub(r"^/api/v1", "", p)
    # Drop query string entirely (e.g. "?ticket=..." -> ""), but keep the
    # bare "?" removed too so "GET /collab/ws" and "GET /collab/ws?ticket=..."
    # normalize to the same identity.
    p = p.split("?", 1)[0]
    p = p.rstrip("/")
    return f"{method.strip().upper()} {p}"


VERSION_HEADER_RE = re.compile(r"^##\s+v(\d+\.\d+)\b")


def split_table_row(line: str) -> list[str] | None:
    """Split a markdown table row on top-level '|' only (not inside
    backtick spans), stripping outer empty cells from the leading/trailing
    pipe. Returns None if the line is not a table row."""
    line = line.rstrip("\n")
    if not line.startswith("|"):
        return None
    cells: list[str] = []
    buf: list[str] = []
    in_backtick = False
    for ch in line:
        if ch == "`":
            in_backtick = not in_backtick
            buf.append(ch)
        elif ch == "|" and not in_backtick:
            cells.append("".join(buf).strip())
            buf = []
        else:
            buf.append(ch)
    cells.append("".join(buf).strip())
    # A well-formed "| a | b | c |" row has leading/trailing empty cells.
    if cells and cells[0] == "":
        cells = cells[1:]
    if cells and cells[-1] == "":
        cells = cells[:-1]
    if not cells:
        return None
    return cells


def is_separator_row(cells: list[str]) -> bool:
    return all(re.fullmatch(r":?-+:?", c) for c in cells)


@dataclass
class RestRow:
    identity: str
    version: str  # e.g. "0.4"
    conditional: bool = False


def parse_rest_table(path: str) -> list[RestRow]:
    rows: list[RestRow] = []
    current_version = None
    with open(path, encoding="utf-8") as f:
        for line in f:
            m = VERSION_HEADER_RE.match(line)
            if m:
                current_version = m.group(1)
                continue
            cells = split_table_row(line)
            if not cells or is_separator_row(cells):
                continue
            if cells[0] in ("Method / path",):
                continue
            m2 = re.match(r"`([A-Z]+)\s+([^`]+)`", cells[0])
            if not m2:
                continue
            if current_version is None:
                continue
            identity = normalize_rest_identity(m2.group(1), m2.group(2))
            # ADR-0003's legacy table is explicitly conditional on a nonzero
            # inventory; the frozen zero-row v0.4 branch has no route surface.
            conditional = "/legacy-pages/" in identity
            rows.append(RestRow(identity=identity, version=current_version, conditional=conditional))
    return rows


MCP_TOKEN_RE = re.compile(r"`((?:tool|resource):([^@`]+)(?:@(\d+\.\d+))?|not_exposed:([a-z0-9_]+))`")
CLI_TOKEN_RE = re.compile(r"`(cli:([^@`]+)(?:@(\d+\.\d+))?|not_exposed:([a-z0-9_]+))`")
UI_TOKEN_RE = re.compile(r"`adapter:([A-Za-z0-9_]+)`")


@dataclass
class MatrixRow:
    identity: str
    version: str
    conditional: bool
    mcp_cell: str
    cli_cell: str
    ui_cell: str
    reason_cell: str
    line_no: int


def parse_matrix(path: str) -> list[MatrixRow]:
    rows: list[MatrixRow] = []
    in_matrix = False
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f, start=1):
            if line.startswith("## 49-endpoint coverage matrix"):
                in_matrix = True
                continue
            if in_matrix and line.startswith("## "):
                break
            if not in_matrix:
                continue
            cells = split_table_row(line)
            if not cells or is_separator_row(cells):
                continue
            if cells[0] == "REST endpoint":
                continue
            if len(cells) < 6:
                continue
            m = re.match(r"`([A-Z]+)\s+([^`]+)`", cells[0])
            if not m:
                continue
            identity = normalize_rest_identity(m.group(1), m.group(2))
            version_raw = cells[1].strip()
            conditional = "conditional" in version_raw
            version = version_raw.split()[0]
            rows.append(
                MatrixRow(
                    identity=identity,
                    version=version,
                    conditional=conditional,
                    mcp_cell=cells[2],
                    cli_cell=cells[3],
                    ui_cell=cells[4],
                    reason_cell=cells[5],
                    line_no=i,
                )
            )
    return rows


@dataclass
class McpTool:
    name: str
    version: str
    conditional: bool = False


def parse_mcp_live(path: str) -> tuple[list[McpTool], list[McpTool]]:
    tools: list[McpTool] = []
    resources: list[McpTool] = []
    section = None
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.startswith("## Tools"):
                section = "tools"
                continue
            if line.startswith("## Resources"):
                section = "resources"
                continue
            if line.startswith("## ") and section in ("tools", "resources"):
                section = None
                continue
            if section is None:
                continue
            cells = split_table_row(line)
            if not cells or is_separator_row(cells):
                continue
            if cells[0] in ("Tool", "Template"):
                continue
            m = re.match(r"`([^`]+)`", cells[0])
            if not m:
                continue
            name = m.group(1)
            version_raw = cells[1].strip() if len(cells) > 1 else ""
            conditional = "conditional" in version_raw
            version = version_raw.split()[0] if version_raw else ""
            if section == "tools":
                tools.append(McpTool(name=name, version=version, conditional=conditional))
            else:
                resources.append(McpTool(name=name, version=version, conditional=conditional))
    return tools, resources


SUFFIX_VOCAB = ["deep", "dry_run", "execute", "shallow"]


def cli_base_key(command_cell: str) -> list[str]:
    """Tokenize a CLI command cell (the raw text between backticks) into
    the leading literal subcommand words, stopping at the first
    placeholder/flag/bracket/paren token."""
    m = re.match(r"`([^`]+)`", command_cell)
    text = m.group(1) if m else command_cell
    tokens = text.split()
    words: list[str] = []
    for t in tokens[1:]:  # skip leading "sylvode"
        if t[0] in "<[(-":
            break
        words.append(t)
    return words


@dataclass
class CliCommand:
    key: str
    version: str
    raw: str
    data_cell: str
    conditional: bool = False


def parse_cli_live(path: str) -> list[CliCommand]:
    raw_rows: list[tuple[list[str], str, str, str]] = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            cells = split_table_row(line)
            if not cells or is_separator_row(cells):
                continue
            if cells[0] == "命令":
                continue
            if not cells[0].startswith("`sylvode"):
                continue
            base = cli_base_key(cells[0])
            version_raw = cells[1].strip() if len(cells) > 1 else ""
            conditional = "conditional" in version_raw
            version = version_raw.split()[0] if version_raw else ""
            data_cell = cells[3] if len(cells) > 3 else ""
            raw_rows.append((base, cells[0], version, data_cell, conditional))

    # Group by base key text to detect duplicates needing a suffix.
    groups: dict[str, list[int]] = {}
    for idx, (base, _raw, _v, _d, _conditional) in enumerate(raw_rows):
        groups.setdefault(".".join(base), []).append(idx)

    out: list[CliCommand] = []
    for base_str, idxs in groups.items():
        if len(idxs) == 1:
            i = idxs[0]
            _base, raw, version, _data, conditional = raw_rows[i]
            out.append(CliCommand(key=base_str, version=version, raw=raw, data_cell=_data, conditional=conditional))
            continue
        for i in idxs:
            _base, raw, version, data, conditional = raw_rows[i]
            suffix = None
            # Priority 1: a literal boolean flag matching the closed
            # suffix vocabulary (surface-coverage-v1.md "记法": only
            # page_navigator|collection|shallow|deep|dry_run|execute are
            # legal suffixes).
            for candidate in ("deep", "dry-run", "execute"):
                if re.search(rf"--{candidate}\b", raw):
                    suffix = candidate.replace("-", "_")
                    break
            if suffix is None:
                tm = re.search(r"--type\s+([\w|]+)", raw)
                if tm:
                    value = tm.group(1)
                    suffix = value.replace("|", "_") if "|" in value else value
            if suffix is None:
                # Fall back to scanning this row's own data/description
                # column for one of the closed vocabulary words appearing
                # as literal text (e.g. "v0.4 shallow" in cli-surface-v1.md).
                for word in SUFFIX_VOCAB:
                    if re.search(rf"\b{word}\b", data):
                        suffix = word
                        break
            key = f"{base_str}#{suffix}" if suffix else f"{base_str}#UNRESOLVED{i}"
            out.append(CliCommand(key=key, version=version, raw=raw, data_cell=data, conditional=conditional))
    return out


def parse_ui_live_adapters(path: str) -> set[str]:
    adapters = set()
    with open(path, encoding="utf-8") as f:
        for line in f:
            m = re.match(r"\s*export interface (\w+)", line)
            if m:
                adapters.add(m.group(1))
    return adapters


REASON_TABLE_RE = re.compile(r"^\|\s*`([a-z0-9_]+)`\s*\|\s*(.+?)\s*\|\s*$")


def parse_reason_table(path: str) -> dict[str, str]:
    """Returns {reason_code: scope_hint} where scope_hint is the raw
    Chinese "稳定含义" text (its leading token before the first Chinese
    punctuation tells us MCP-only / CLI-only / MCP+CLI)."""
    reasons: dict[str, str] = {}
    in_table = False
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.startswith("| Reason code"):
                in_table = True
                continue
            if in_table:
                if not line.startswith("|"):
                    break
                if re.match(r"^\|---", line):
                    continue
                m = REASON_TABLE_RE.match(line)
                if m:
                    reasons[m.group(1)] = m.group(2)
    return reasons


def version_tuple(v: str) -> tuple[int, int]:
    a, b = v.split(".")
    return (int(a), int(b))


@dataclass
class CoverageResult:
    counts: dict = field(default_factory=dict)
    violations: dict = field(default_factory=dict)
    passed: bool = False
    contracts: dict = field(default_factory=dict)


VIOLATION_KEYS = [
    "missing_rest_rows", "duplicate_rest_rows", "unknown_matrix_rest_rows",
    "orphan_mcp_tools", "orphan_mcp_resources", "orphan_cli_commands",
    "unknown_mcp_refs", "unknown_cli_refs", "unknown_ui_consumers",
    "blank_cells", "invalid_reason_codes", "reason_column_mismatches",
    "mcp_exception_endpoint_mismatches", "mcp_exception_without_authority",
    "version_inversions", "future_exposure_counted_as_shipped",
]

MCP_ALLOWED_REASONS = {
    "adr0007_bot_ticket_excluded",
    "adr0007_direct_ws_transport",
    "tm_crdt_bytes_semantic_boundary",
}
CLI_ALLOWED_REASONS = {
    "adr0007_direct_ws_transport",
    "cli_interactive_collab_session",
    "cli_query_surface_preferred",
}
MCP_EXPECTED_ENDPOINT_REASON = {
    "tm_crdt_bytes_semantic_boundary": "/bootstrap",
    "adr0007_bot_ticket_excluded": "POST /collab/tickets",
    "adr0007_direct_ws_transport": "GET /collab/ws",
}


def run(contracts_root: str, release: str) -> CoverageResult:
    rest_path = f"{contracts_root}/contracts/rest-api-v1.md"
    matrix_path = f"{contracts_root}/contracts/surface-coverage-v1.md"
    mcp_path = f"{contracts_root}/contracts/mcp-surface-v1.md"
    cli_path = f"{contracts_root}/contracts/cli-surface-v1.md"
    ui_path = f"{contracts_root}/contracts/ui-surface-v1.md"

    v: dict[str, list[str]] = {k: [] for k in VIOLATION_KEYS}

    rest_rows = parse_rest_table(rest_path)
    matrix_rows = parse_matrix(matrix_path)
    mcp_tools, mcp_resources = parse_mcp_live(mcp_path)
    cli_commands = parse_cli_live(cli_path)
    ui_adapters = parse_ui_live_adapters(ui_path)
    reason_table = parse_reason_table(matrix_path)

    # ---- rule 1: full REST contract vs matrix, exact 1:1 ----
    rest_ids = [r.identity for r in rest_rows]
    rest_id_set = set(rest_ids)
    for rid in rest_ids:
        if rest_ids.count(rid) > 1 and rid not in v["duplicate_rest_rows"]:
            v["duplicate_rest_rows"].append(rid)

    matrix_ids = [m.identity for m in matrix_rows]
    for mid in matrix_ids:
        if matrix_ids.count(mid) > 1 and mid not in v["duplicate_rest_rows"]:
            v["duplicate_rest_rows"].append(mid)

    matrix_id_set = set(matrix_ids)
    v["missing_rest_rows"] = sorted(rest_id_set - matrix_id_set)
    v["unknown_matrix_rest_rows"] = sorted(matrix_id_set - rest_id_set)

    # REST endpoint's own version, by identity (for version-inversion checks)
    rest_version_by_id = {r.identity: r.version for r in rest_rows}

    # ---- live registries as name sets ----
    mcp_tool_names = {t.name for t in mcp_tools}
    mcp_resource_names = {t.name for t in mcp_resources}
    cli_keys = {c.key for c in cli_commands}
    mcp_tool_version = {t.name: t.version for t in mcp_tools}
    mcp_resource_version = {t.name: t.version for t in mcp_resources}
    cli_key_version = {c.key: c.version for c in cli_commands}

    referenced_mcp_tools: set[str] = set()
    referenced_mcp_resources: set[str] = set()
    referenced_cli_keys: set[str] = set()

    mcp_not_exposed_rows = []
    cli_not_exposed_count = 0

    for row in matrix_rows:
        rest_version = rest_version_by_id.get(row.identity, row.version)

        # --- blank cell check ---
        if not row.mcp_cell.strip():
            v["blank_cells"].append(f"{row.identity}: mcp cell blank")
        if not row.cli_cell.strip():
            v["blank_cells"].append(f"{row.identity}: cli cell blank")
        if not row.ui_cell.strip():
            v["blank_cells"].append(f"{row.identity}: ui cell blank")

        # --- MCP cell ---
        mcp_reasons_here = []
        for tok in MCP_TOKEN_RE.finditer(row.mcp_cell):
            full, name_at, tver, not_exposed_reason = tok.groups()
            if not_exposed_reason:
                mcp_reasons_here.append(not_exposed_reason)
                if not_exposed_reason not in MCP_ALLOWED_REASONS:
                    v["invalid_reason_codes"].append(f"{row.identity}: mcp not_exposed reason '{not_exposed_reason}' not in allowed set")
                else:
                    expected_endpoint = MCP_EXPECTED_ENDPOINT_REASON.get(not_exposed_reason)
                    if expected_endpoint and expected_endpoint not in row.identity:
                        v["mcp_exception_endpoint_mismatches"].append(
                            f"{row.identity}: uses reason '{not_exposed_reason}' expected for endpoint containing '{expected_endpoint}'"
                        )
                mcp_not_exposed_rows.append(row.identity)
                continue
            is_resource = full.startswith("resource:")
            name = name_at
            effective_version = tver or rest_version
            if is_resource:
                referenced_mcp_resources.add(name)
                if name not in mcp_resource_names:
                    v["unknown_mcp_refs"].append(f"{row.identity}: resource '{name}' not found in mcp-surface-v1.md Resources table")
            else:
                referenced_mcp_tools.add(name)
                if name not in mcp_tool_names:
                    v["unknown_mcp_refs"].append(f"{row.identity}: tool '{name}' not found in mcp-surface-v1.md Tools table")
            try:
                if version_tuple(effective_version) < version_tuple(rest_version):
                    v["version_inversions"].append(f"{row.identity}: mcp ref '{name}'@{effective_version} ships before REST endpoint itself ({rest_version})")
            except ValueError:
                pass

        # --- CLI cell ---
        cli_reasons_here = []
        for tok in CLI_TOKEN_RE.finditer(row.cli_cell):
            full, key_at, tver, not_exposed_reason = tok.groups()
            if not_exposed_reason:
                cli_reasons_here.append(not_exposed_reason)
                cli_not_exposed_count += 1
                if not_exposed_reason not in CLI_ALLOWED_REASONS:
                    v["invalid_reason_codes"].append(f"{row.identity}: cli not_exposed reason '{not_exposed_reason}' not in allowed set")
                continue
            key = key_at
            referenced_cli_keys.add(key)
            effective_version = tver or rest_version
            if key not in cli_keys:
                v["unknown_cli_refs"].append(f"{row.identity}: cli key '{key}' not found in cli-surface-v1.md (derived key)")
            try:
                if version_tuple(effective_version) < version_tuple(rest_version):
                    v["version_inversions"].append(f"{row.identity}: cli ref '{key}'@{effective_version} ships before REST endpoint itself ({rest_version})")
            except ValueError:
                pass

        # --- UI cell ---
        for name in UI_TOKEN_RE.findall(row.ui_cell):
            if name not in ui_adapters:
                v["unknown_ui_consumers"].append(f"{row.identity}: adapter '{name}' not declared in ui-surface-v1.md")

        # --- reason column consistency ---
        expected_tokens = set()
        for r in mcp_reasons_here:
            expected_tokens.add(f"mcp:{r}")
        for r in cli_reasons_here:
            expected_tokens.add(f"cli:{r}")
        reason_text = row.reason_cell.strip()
        if not expected_tokens:
            if reason_text not in ("none",):
                v["reason_column_mismatches"].append(f"{row.identity}: reason column is '{reason_text}' but no not_exposed markers present (expected 'none')")
        else:
            for tok in expected_tokens:
                code = tok.split(":", 1)[1]
                if f"`{code}`" not in reason_text:
                    v["reason_column_mismatches"].append(f"{row.identity}: reason column '{reason_text}' does not mention '{code}'")
            if reason_text.strip().lower() in ("n/a", "none", "-", ""):
                v["reason_column_mismatches"].append(f"{row.identity}: reason column illegally empty/N/A while not_exposed markers are present")

    # ---- orphans: live identities never referenced by the matrix ----
    v["orphan_mcp_tools"] = sorted(mcp_tool_names - referenced_mcp_tools)
    v["orphan_mcp_resources"] = sorted(mcp_resource_names - referenced_mcp_resources)
    v["orphan_cli_commands"] = sorted(cli_keys - referenced_cli_keys)

    # ---- MCP exception cardinality: exactly 3 allowed not_exposed rows ----
    if len(mcp_not_exposed_rows) > 3:
        extra = mcp_not_exposed_rows[3:]
        for e in extra:
            v["mcp_exception_without_authority"].append(f"{e}: mcp not_exposed row beyond the 3 allowlisted exceptions")

    # ---- counts ----
    try:
        release_tuple = version_tuple(release)
    except ValueError:
        release_tuple = None
    rest_in_release = 0
    if release_tuple is not None:
        for r in rest_rows:
            try:
                if version_tuple(r.version) <= release_tuple:
                    rest_in_release += 1
            except ValueError:
                pass

    result = CoverageResult()
    result.counts = {
        "rest_total": len(rest_id_set),
        "rest_in_release": rest_in_release,
        "matrix_rows": len(matrix_rows),
        "mcp_tools_total": len(mcp_tool_names),
        "mcp_resources_total": len(mcp_resource_names),
        "cli_commands_total": len(cli_keys),
        "not_exposed": {
            "mcp": len(mcp_not_exposed_rows),
            "cli": cli_not_exposed_count,
            "ui": 0,
        },
    }
    result.violations = {k: sorted(set(vv)) for k, vv in v.items()}
    result.passed = all(len(vv) == 0 for vv in result.violations.values())
    result.contracts = {
        "rest_sha256": sha256_file(rest_path),
        "mcp_sha256": sha256_file(mcp_path),
        "cli_sha256": sha256_file(cli_path),
        "ui_sha256": sha256_file(ui_path),
        "matrix_sha256": sha256_file(matrix_path),
    }
    return result


def main() -> int:
    import argparse
    import json

    ap = argparse.ArgumentParser()
    ap.add_argument("--contracts-root", required=True)
    ap.add_argument("--release", required=True)
    args = ap.parse_args()
    result = run(args.contracts_root, args.release)
    json.dump(
        {
            "counts": result.counts,
            "violations": result.violations,
            "passed": result.passed,
            "contracts": result.contracts,
        },
        sys.stdout,
        indent=2,
        sort_keys=True,
    )
    print()
    return 0 if result.passed else 1


if __name__ == "__main__":
    sys.exit(main())

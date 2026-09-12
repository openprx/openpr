#!/usr/bin/env python3
"""Source parsers shared by v0.4 verifiers and their mutation tests."""

from __future__ import annotations

import json
import re
import sys


def projection_parent_alias_result(source: str) -> dict:
    violations: list[str] = []
    aliases: set[str] = set()
    # Scope alias checks to individual raw SQL literals. A file-wide grep
    # confuses the common `p` alias for flow_objects in one query with `p` for
    # flow_object_projections in another query.
    raw_sql_literals = re.findall(r'r(?P<h>#{0,})"(?P<body>.*?)"(?P=h)', source, re.S)
    for _hashes, sql in raw_sql_literals:
        if "flow_object_projections" not in sql:
            continue
        # PostgreSQL permits the same short alias in distinct CTEs. Split at
        # sibling CTE boundaries before relating an alias declaration to a
        # field reference, otherwise `p` for flow_objects in `chain` is
        # confused with `p` for flow_object_projections in `lags`.
        scopes = re.split(r"\)\s*,\s*[A-Za-z_][A-Za-z0-9_]*\s+AS\s*\(", sql, flags=re.I)
        for scope in scopes:
            local_aliases = set(
                re.findall(
                    r"\b(?:FROM|JOIN)\s+flow_object_projections\s+(?:AS\s+)?([A-Za-z_][A-Za-z0-9_]*)",
                    scope,
                    re.I,
                )
            )
            aliases.update(local_aliases)
            for alias in local_aliases:
                if re.search(rf"\b{re.escape(alias)}\.parent_id\b", scope):
                    violations.append(
                        f"projection alias {alias!r} references parent_id inside its SQL scope"
                    )
    return {"aliases": sorted(aliases), "violations": violations, "passed": not violations}


def tool_policy_scopes_result(source: str) -> dict:
    fixed = re.search(
        r"const\s+TOOL_POLICY_SCOPES:\s*\[\(&str,\s*PolicyScope\);\s*(\d+)\]\s*=\s*\[(.*?)\n\];",
        source,
        re.S,
    )
    sliced = re.search(
        r"const\s+TOOL_POLICY_SCOPES:\s*&\[\(&str,\s*PolicyScope\)\]\s*=\s*&\[(.*?)\n\];",
        source,
        re.S,
    )
    if fixed:
        declared_len = int(fixed.group(1))
        body = fixed.group(2)
        representation = "fixed_array"
    elif sliced:
        body = sliced.group(1)
        declared_len = None
        representation = "slice"
    else:
        raise ValueError("TOOL_POLICY_SCOPES array or slice not found")
    entries = re.findall(
        r'\(\s*"([A-Za-z][A-Za-z0-9_.]*)"\s*,\s*PolicyScope::', body
    )
    return {
        "representation": representation,
        "declared_len": declared_len,
        "entry_count": len(entries),
        "names": sorted(set(entries)),
        "unique_entry_count": len(set(entries)),
    }


def main() -> int:
    if len(sys.argv) != 3 or sys.argv[1] not in {
        "projection-parent-aliases",
        "tool-policy-scopes",
    }:
        print(
            "usage: flow_v0_4_verifier_source_checks.py "
            "{projection-parent-aliases|tool-policy-scopes} SOURCE",
            file=sys.stderr,
        )
        return 2
    try:
        with open(sys.argv[2], encoding="utf-8") as handle:
            source = handle.read()
        if sys.argv[1] == "projection-parent-aliases":
            result = projection_parent_alias_result(source)
        else:
            result = tool_policy_scopes_result(source)
    except (OSError, ValueError) as exc:
        print(json.dumps({"error": str(exc)}))
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

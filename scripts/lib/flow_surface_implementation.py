#!/usr/bin/env python3
"""Compare release-applicable Flow contracts with shipped implementation surfaces."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(__file__))
from flow_surface_coverage import (  # noqa: E402
    normalize_rest_identity,
    parse_cli_live,
    parse_mcp_live,
    parse_rest_table,
)


def parse_live_mcp(text: str) -> list[str]:
    # list-tools renders every name on a two-space-indented line followed by
    # an indented description. Tool names are the only such lines containing
    # no whitespace after trim and matching the registry identifier grammar.
    return sorted(set(re.findall(r"^  ([a-z][a-z0-9_.-]+)\s*$", text, re.M)))


def balanced_route_calls(text: str):
    marker = ".route("
    pos = 0
    while True:
        start = text.find(marker, pos)
        if start < 0:
            return
        i = start + len(marker)
        depth, quote, escaped = 1, None, False
        while i < len(text) and depth:
            ch = text[i]
            if quote:
                if escaped:
                    escaped = False
                elif ch == "\\":
                    escaped = True
                elif ch == quote:
                    quote = None
            elif ch in ('"', "'"):
                quote = ch
            elif ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
            i += 1
        yield text[start + len(marker): i - 1]
        pos = i


def live_rest(main_rs: str) -> tuple[list[str], str]:
    raw = open(main_rs, encoding="utf-8").read()
    found: set[str] = set()
    for body in balanced_route_calls(raw):
        path_match = re.match(r'\s*"([^"]+)"\s*,', body)
        if not path_match:
            continue
        path = path_match.group(1)
        if "/flow/" not in path and "/collab/" not in path and not path.endswith("/features/flow"):
            continue
        handler = body[path_match.end():]
        for method in re.findall(r"(?:^|[.\s])(?:axum::routing::)?(get|post|put|patch|delete)\s*\(", handler):
            found.add(normalize_rest_identity(method.upper(), path))
    return sorted(found), hashlib.sha256(raw.encode()).hexdigest()


def cli_command_words(raw: str) -> list[str]:
    text = re.match(r"`([^`]+)`", raw).group(1)
    words = []
    for token in text.split()[1:]:
        if token[0] in "<[(-" or token.startswith("--"):
            break
        words.append(token)
    return words


def live_cli(binary: str, commands) -> tuple[list[str], list[dict]]:
    present, probes = [], []
    for command in commands:
        words = cli_command_words(command.raw)
        proc = subprocess.run([binary, *words, "--help"], text=True, capture_output=True, check=False)
        help_text = proc.stdout + proc.stderr
        required_flags = sorted(set(re.findall(r"--[a-z][a-z0-9-]*", command.raw)))
        missing_flags = [flag for flag in required_flags if flag not in help_text]
        ok = proc.returncode == 0 and not missing_flags
        probes.append({
            "key": command.key,
            "argv": words + ["--help"],
            "exit_code": proc.returncode,
            "required_flags": required_flags,
            "missing_flags": missing_flags,
            "present": ok,
        })
        if ok:
            present.append(command.key)
    return sorted(set(present)), probes


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--contracts-root", required=True)
    ap.add_argument("--repo-root", required=True)
    ap.add_argument("--release", required=True)
    ap.add_argument("--mcp-output", required=True)
    ap.add_argument("--cli-binary", required=True)
    args = ap.parse_args()

    contract_rest_all = parse_rest_table(f"{args.contracts_root}/contracts/rest-api-v1.md")
    contract_mcp_all, _ = parse_mcp_live(f"{args.contracts_root}/contracts/mcp-surface-v1.md")
    contract_cli_all = parse_cli_live(f"{args.contracts_root}/contracts/cli-surface-v1.md")
    # a92e3e9 deliberately says every contract declaration, not merely the
    # rows whose version is <= the requested release.  `release` remains
    # provenance on the shared result; it is not authority to weaken this
    # implementation-parity subset check.
    contract_rest = sorted({r.identity for r in contract_rest_all})
    contract_mcp = sorted({t.name for t in contract_mcp_all})
    contract_cli = contract_cli_all

    mcp_text = open(args.mcp_output, encoding="utf-8").read()
    mcp_live = parse_live_mcp(mcp_text)
    rest_live, rest_sha = live_rest(f"{args.repo_root}/apps/api/src/main.rs")
    cli_live, cli_probes = live_cli(args.cli_binary, contract_cli)

    def dimension(contract, implementation, proof, **extra):
        contract_set, implementation_set = set(contract), set(implementation)
        return {
            "proof": proof,
            "contract_required": sorted(contract_set),
            "implementation": sorted(implementation_set),
            "contract_missing_in_implementation": sorted(contract_set - implementation_set),
            "not_in_flow_contract": sorted(implementation_set - contract_set),
            "counts": {
                "contract_required": len(contract_set),
                "implementation": len(implementation_set),
                "contract_missing_in_implementation": len(contract_set - implementation_set),
                "not_in_flow_contract": len(implementation_set - contract_set),
            },
            "passed": contract_set <= implementation_set,
            **extra,
        }

    result = {
        "scope": "all declarations in the frozen contracts; release metadata does not filter the implementation subset check",
        "mcp": dimension(contract_mcp, mcp_live, "executed shipped list-tools binary"),
        "rest": dimension(contract_rest, rest_live, "Axum route registrations assembled by apps/api/src/main.rs", route_source_sha256=rest_sha),
        "cli": dimension([c.key for c in contract_cli], cli_live, "executed shipped sylvode command tree with per-command --help and required-flag presence", probes=cli_probes),
    }
    result["passed"] = all(result[name]["passed"] for name in ("mcp", "rest", "cli"))
    json.dump(result, sys.stdout, indent=2, sort_keys=True)
    print()
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

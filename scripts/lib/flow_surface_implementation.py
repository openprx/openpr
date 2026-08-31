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
    version_tuple,
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
    release = version_tuple(args.release)

    def classify(rows, identity):
        declared, required, future, conditional = [], [], [], []
        for row in rows:
            name = identity(row)
            declared.append(name)
            entry = {"name": name, "version": row.version}
            if version_tuple(row.version) > release:
                future.append(entry)
            elif row.conditional:
                conditional.append(entry)
            else:
                required.append(name)
        return sorted(set(declared)), sorted(set(required)), sorted(
            future, key=lambda item: (version_tuple(item["version"]), item["name"])
        ), sorted(conditional, key=lambda item: item["name"])

    rest_declared, contract_rest, rest_future, rest_conditional = classify(
        contract_rest_all, lambda row: row.identity
    )
    mcp_declared, contract_mcp, mcp_future, mcp_conditional = classify(
        contract_mcp_all, lambda row: row.name
    )
    cli_declared, contract_cli_keys, cli_future, cli_conditional = classify(
        contract_cli_all, lambda row: row.key
    )

    mcp_text = open(args.mcp_output, encoding="utf-8").read()
    mcp_live = parse_live_mcp(mcp_text)
    rest_live, rest_sha = live_rest(f"{args.repo_root}/apps/api/src/main.rs")
    # Probe the whole command contract so evidence can distinguish a future
    # command that happens to exist from one that correctly does not exist yet.
    # Only contract_cli_keys participates in the current-release subset check.
    cli_live, cli_probes = live_cli(args.cli_binary, contract_cli_all)

    def dimension(declared, contract, future, conditional, implementation, proof, **extra):
        declared_set = set(declared)
        contract_set, implementation_set = set(contract), set(implementation)
        future_with_presence = [
            {**item, "implementation_present": item["name"] in implementation_set}
            for item in future
        ]
        conditional_with_presence = [
            {
                **item,
                "implementation_present": item["name"] in implementation_set,
                "reason": "conditional_surface_requires_activation",
            }
            for item in conditional
        ]
        return {
            "proof": proof,
            "contract_declared": sorted(declared_set),
            "contract_required": sorted(contract_set),
            "required_set_non_empty": bool(contract_set),
            "not_yet_in_release": future_with_presence,
            "conditional_not_applicable": conditional_with_presence,
            "implementation": sorted(implementation_set),
            "contract_missing_in_implementation": sorted(contract_set - implementation_set),
            "not_in_flow_contract": sorted(implementation_set - declared_set),
            "counts": {
                "contract_declared": len(declared_set),
                "contract_required": len(contract_set),
                "not_yet_in_release": len(future_with_presence),
                "not_yet_in_release_absent": sum(
                    not item["implementation_present"] for item in future_with_presence
                ),
                "conditional_not_applicable": len(conditional_with_presence),
                "implementation": len(implementation_set),
                "contract_missing_in_implementation": len(contract_set - implementation_set),
                "not_in_flow_contract": len(implementation_set - declared_set),
            },
            "passed": bool(contract_set) and contract_set <= implementation_set,
            **extra,
        }

    result = {
        "scope": {
            "release": args.release,
            "rule": "only non-conditional contract entries with version <= release are required in the shipped implementation",
            "future_entry_disposition": "not_yet_in_release entries are diagnostic and never counted as missing",
            "conditional_entry_disposition": "conditional entries require their contract activation condition before becoming required",
        },
        "mcp": dimension(mcp_declared, contract_mcp, mcp_future, mcp_conditional, mcp_live, "executed shipped list-tools binary"),
        "rest": dimension(rest_declared, contract_rest, rest_future, rest_conditional, rest_live, "Axum route registrations assembled by apps/api/src/main.rs", route_source_sha256=rest_sha),
        "cli": dimension(cli_declared, contract_cli_keys, cli_future, cli_conditional, cli_live, "executed shipped sylvode command tree with per-command --help and required-flag presence", probes=cli_probes),
    }
    result["version_scope_diagnostic"] = {
        "code": "future_contract_entries_excluded_from_current_release_failure",
        "classification": "verifier_criterion_version_scope",
        "message": (
            "The frozen contracts span later releases. Entries newer than the requested "
            "release are recorded as not_yet_in_release; reporting them as ordinary missing "
            "would be a verifier criterion error, not an implementation failure."
        ),
        "release": args.release,
        "not_yet_in_release_counts": {
            name: result[name]["counts"]["not_yet_in_release"]
            for name in ("mcp", "rest", "cli")
        },
        "future_absent_but_non_failing_counts": {
            name: result[name]["counts"]["not_yet_in_release_absent"]
            for name in ("mcp", "rest", "cli")
        },
    }
    required_total = sum(result[name]["counts"]["contract_required"] for name in ("mcp", "rest", "cli"))
    declared_total = sum(result[name]["counts"]["contract_declared"] for name in ("mcp", "rest", "cli"))
    result["proof_limitations"] = {
        "rest": {
            "proof_kind": "source_route_registration_parser",
            "proves": "matching method/path registrations were found by parsing balanced .route(...) calls in apps/api/src/main.rs",
            "does_not_prove": "the routes are present in a running runtime router",
        },
        "cli": {
            "proof_kind": "shipped_help_probe",
            "proves": "the shipped command path exits zero for --help and its help text contains contract-required flag strings",
            "does_not_prove": "the command can successfully perform its operation",
        },
        "release_scope": {
            "required_entries_compared": required_total,
            "contract_entries_declared": declared_total,
            "ratio": f"{required_total}/{declared_total}",
            "excluded_entries": declared_total - required_total,
            "excluded_disposition": "not_yet_in_release or conditional_not_applicable",
        },
    }
    result["conditional_surface_observation"] = {
        "code": "conditional_legacy_pages_surface_asymmetry",
        "non_failing": True,
        "reason": "conditional surfaces do not become required until their activation condition is satisfied",
        **{
            f"{surface}_{presence}": sorted(
                item["name"]
                for item in result[surface]["conditional_not_applicable"]
                if item["implementation_present"] == (presence == "present")
            )
            for surface in ("mcp", "rest", "cli")
            for presence in ("present", "absent")
        },
    }
    result["empty_contract_required_surfaces"] = sorted(
        name for name in ("mcp", "rest", "cli") if not result[name]["required_set_non_empty"]
    )
    result["passed"] = all(result[name]["passed"] for name in ("mcp", "rest", "cli"))
    json.dump(result, sys.stdout, indent=2, sort_keys=True)
    print()
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

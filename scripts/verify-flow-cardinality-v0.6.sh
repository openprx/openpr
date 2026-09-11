#!/usr/bin/env bash
set -euo pipefail

# Flow v0.6 per-release Collection-command cardinality verifier.
# It derives this release's delta from the Collection enum/parse/registry/cardinality
# declarations at the committed source HEAD; it never extends the v0.5 list.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.6"
ADR_PATH=""
SINCE_RELEASE=""
JSON_MODE=0
TEST_DROP_DECLARATION=""
TEST_PARSER_ERROR=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-cardinality-v0.6.sh --adr PATH --since-release 0.5 --json [OPTIONS]

Options:
  --adr PATH                    ADR-0013 path (absolute, cwd-relative, or contracts-root-relative).
  --since-release 0.5          Required exact predecessor; no v0.5 extrapolation is accepted.
  --contracts-root DIR         Default: /opt/working/sylvode-flow
  --evidence-root DIR          Default: <contracts-root>/evidence/v0.6
  --repo-root DIR              Default: this checkout
  --json                       Required; emit JSON and write cardinality-result.json
  --test-drop-declaration NAME Test-only falsification hook for one v0.6 wire command
  --test-parser-error          Test-only parser failure; can never pass
  -h, --help                   Show help

Exit: 0 verified, 1 assertion failed, 2 usage/tool/preflight failure.
EOF
}

require_value() {
  if [[ $# -lt 2 || -z "$2" || "$2" == -* ]]; then
    echo "FAIL: $1 requires a value" >&2
    exit 2
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) require_value "$@"; ADR_PATH="$2"; shift 2 ;;
    --since-release) require_value "$@"; SINCE_RELEASE="$2"; shift 2 ;;
    --contracts-root) require_value "$@"; CONTRACTS_ROOT="$2"; shift 2 ;;
    --evidence-root) require_value "$@"; EVIDENCE_ROOT="$2"; shift 2 ;;
    --repo-root) require_value "$@"; REPO_ROOT="$2"; shift 2 ;;
    --test-drop-declaration) require_value "$@"; TEST_DROP_DECLARATION="$2"; shift 2 ;;
    --test-parser-error) TEST_PARSER_ERROR=1; shift ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$ADR_PATH" ]] || { echo "FAIL: --adr is required" >&2; exit 2; }
[[ "$SINCE_RELEASE" == "0.5" ]] || { echo "FAIL: --since-release must be exactly 0.5" >&2; exit 2; }
[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
for tool in git python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
[[ -d "$REPO_ROOT/.git" ]] || { echo "FAIL: --repo-root is not a git checkout: $REPO_ROOT" >&2; exit 2; }

if [[ "$ADR_PATH" != /* ]]; then
  if [[ -f "$ADR_PATH" ]]; then
    ADR_PATH="$(cd "$(dirname "$ADR_PATH")" && pwd)/$(basename "$ADR_PATH")"
  elif [[ -f "$CONTRACTS_ROOT/$ADR_PATH" ]]; then
    ADR_PATH="$CONTRACTS_ROOT/$ADR_PATH"
  fi
fi
[[ -f "$ADR_PATH" ]] || { echo "FAIL: ADR not found: $ADR_PATH" >&2; exit 2; }
case "$TEST_DROP_DECLARATION" in
  ""|field_create|field_update|field_archive|field_reorder|view_create|view_update|view_reorder|record_create|record_patch|record_archive|record_query|create_collection_embed) ;;
  *) echo "FAIL: unsupported test command: $TEST_DROP_DECLARATION" >&2; exit 2 ;;
esac
mkdir -p "$EVIDENCE_ROOT"

python3 - "$REPO_ROOT" "$ADR_PATH" "$EVIDENCE_ROOT" "$SINCE_RELEASE" "$TEST_DROP_DECLARATION" "$TEST_PARSER_ERROR" <<'PY'
import datetime as dt
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

repo = pathlib.Path(sys.argv[1]).resolve()
adr = pathlib.Path(sys.argv[2]).resolve()
evidence = pathlib.Path(sys.argv[3]).resolve()
since, drop, parser_error = sys.argv[4:]
head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
source_path = "apps/api/src/flow/collections.rs"
source = subprocess.check_output(["git", "-C", str(repo), "show", f"{head}:{source_path}"], text=True)
dirty_lines = subprocess.run(
    ["git", "-C", str(repo), "status", "--porcelain=v1", "--untracked-files=all", "--",
     "apps", "crates", "spikes", "migrations", ".cargo", "Cargo.toml", "Cargo.lock"],
    text=True, stdout=subprocess.PIPE, check=True,
).stdout.splitlines()

wire_to_variant = {
    "field_create": "FieldCreate",
    "field_update": "FieldUpdate",
    "field_archive": "FieldArchive",
    "field_reorder": "FieldReorder",
    "view_create": "ViewCreate",
    "view_update": "ViewUpdate",
    "view_reorder": "ViewReorder",
    "record_create": "RecordCreate",
    "record_patch": "RecordPatch",
    "record_archive": "RecordArchive",
    "record_query": "RecordQuery",
    "create_collection_embed": "CreateCollectionEmbed",
}
expected_cardinality = {
    **{name: "one" for name in ["field_create", "field_update", "field_archive", "field_reorder",
                                  "view_create", "view_update", "view_reorder", "record_patch",
                                  "create_collection_embed"]},
    **{name: "zero" for name in ["record_create", "record_archive", "record_query"]},
}
checks = []
def check(check_id, passed, detail):
    checks.append({"id": check_id, "status": "passed" if passed else "failed", "detail": detail})

try:
    if parser_error == "1":
        raise ValueError("test-only parser failure")
    enum_match = re.search(r"pub enum CollectionCommandType\s*\{(?P<body>.*?)\n\}", source, re.S)
    if not enum_match:
        raise ValueError("CollectionCommandType enum not found")
    variants = set(re.findall(r"^\s*([A-Z][A-Za-z0-9_]*)\s*,", enum_match.group("body"), re.M))
    check("release_delta_exact", variants == set(wire_to_variant.values()),
          {"expected": sorted(wire_to_variant.values()), "observed": sorted(variants)})

    parse_map = {wire: variant for wire, variant in re.findall(r'"([a-z0-9_]+)"\s*=>\s*Some\(Self::([A-Za-z0-9_]+)\)', source)}
    wire_map = {wire: variant for variant, wire in re.findall(r'Self::([A-Za-z0-9_]+)\s*=>\s*"([a-z0-9_]+)"', source)}
    if drop:
        parse_map.pop(drop, None)
    check("parse_and_wire_names_exact", parse_map == wire_to_variant and wire_map == wire_to_variant,
          {"parse": parse_map, "wire": wire_map})

    registry_match = re.search(
        r"pub fn v0_6_command_cardinality_registry\(\).*?\[(?P<body>.*?)\]\s*\.into_iter\(\)", source, re.S
    )
    if not registry_match:
        raise ValueError("v0_6 cardinality registry not found")
    registry_variants = re.findall(r"CollectionCommandType::([A-Za-z0-9_]+)", registry_match.group("body"))
    check("registry_nonempty", bool(registry_variants), {"count": len(registry_variants)})
    check("registry_exact", set(registry_variants) == set(wire_to_variant.values()) and len(registry_variants) == 12,
          {"variants": registry_variants})

    cardinality_match = re.search(
        r"pub const fn existing_document_cardinality\(self\).*?match self\s*\{(?P<body>.*?)\n\s*\}\n\s*\}", source, re.S
    )
    if not cardinality_match:
        raise ValueError("existing_document_cardinality match not found")
    observed_by_variant = {}
    for arm, kind in re.findall(
        r"(?P<arm>(?:\s*Self::[A-Za-z0-9_]+\s*\|?)+)\s*=>\s*ExistingDocumentCardinality::(?P<kind>One|Zero|BoundedMany)",
        cardinality_match.group("body"), re.S,
    ):
        for variant in re.findall(r"Self::([A-Za-z0-9_]+)", arm):
            observed_by_variant[variant] = kind.lower()
    observed = {wire: observed_by_variant.get(variant) for wire, variant in wire_to_variant.items()}
    check("declarations_complete_and_exact", observed == expected_cardinality,
          {"expected": expected_cardinality, "observed": observed})

    bounded_many = [wire for wire, kind in observed.items() if kind == "boundedmany"]
    check("bounded_many_rules", not bounded_many,
          {"commands": bounded_many, "reason": "no cross-record batch command ships in v0.6"})
    adr_text = adr.read_text(encoding="utf-8")
    check("adr_0013_lock_contract_present", "bounded_many" in adr_text and "document_id" in adr_text,
          {"adr": str(adr)})
except Exception as exc:
    check("parser", False, str(exc))

check("source_clean", not dirty_lines, {"dirty_entries": dirty_lines})
passed = all(item["status"] == "passed" for item in checks) and not drop and parser_error != "1"
result = {
    "schema_version": "sylvode.flow.cardinality-result.v1",
    "release": "0.6.0",
    "since_release": since,
    "source_head": head,
    "source_path": source_path,
    "source_dirty": bool(dirty_lines),
    "commands_found": sorted(wire_to_variant),
    "new_commands_found": len(wire_to_variant),
    "bounded_many_commands": [],
    "test_fault": drop or ("parser_error" if parser_error == "1" else None),
    "checks": checks,
    "unresolved": sum(item["status"] != "passed" for item in checks),
    "passed": passed,
    "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
}
out = evidence / "cardinality-result.json"
fd, temp_name = tempfile.mkstemp(prefix=".cardinality-result.", suffix=".json", dir=evidence)
with os.fdopen(fd, "w", encoding="utf-8") as handle:
    json.dump(result, handle, ensure_ascii=False, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temp_name, out)
print(json.dumps(result, ensure_ascii=False, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY

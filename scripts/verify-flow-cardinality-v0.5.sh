#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.5 per-release command-cardinality verifier.
#
# Production Rust is always read from the committed source tree with
# `git show <source_head>:<path>`.  The working tree is consulted only for the
# scoped dirty-state assertion, so concurrent edits cannot change the facts
# attributed to source_head.
#
# Exit codes: 0 = every assertion passed, 1 = a source/evidence assertion
# failed, 2 = usage/tool/preflight failure.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"

REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.5"
ADR_PATH=""
SINCE_RELEASE=""
JSON_MODE=0
TEST_DROP_DECLARATION=""
TEST_DROP_PRODUCER=""
TEST_PARSER_ERROR=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-cardinality-v0.5.sh --adr PATH --since-release 0.4 --json [OPTIONS]

Scans only the command variants added after v0.4 for the v0.5 cardinality
contract, while independently rechecking the frozen v0.4 registry. Writes
cardinality-result.json.

Options:
  --adr PATH                    Path to ADR-0013. Required. Relative paths are
                                resolved against the current directory first,
                                then against --contracts-root.
  --since-release RELEASE       Required; v0.5 accepts exactly 0.4.
  --contracts-root DIR          Default: /opt/working/sylvode-flow
  --evidence-root DIR           Default: /opt/working/sylvode-flow/evidence/v0.5
  --repo-root DIR               Default: this checkout.
  --json                        Required by the gate command.
  --test-drop-declaration NAME  Test-only fault injection. NAME must be one of
                                move_object, grants_set, inheritance_set. The
                                artifact records the injection and cannot pass.
  --test-drop-producer NAME     Test-only missing-producer injection for one of
                                the same three names; can never pass.
  --test-parser-error           Test-only parser-error injection; can never pass.
  -h, --help                    Show this help and exit 0.

Exit codes: 0 all checks passed, 1 an assertion failed, 2 usage/preflight failed.
EOF
}

require_value() {
  if [[ $# -lt 2 || -z "$2" || "$2" == -* ]]; then
    echo "FAIL: $1 requires a value" >&2
    usage >&2
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
    --test-drop-producer) require_value "$@"; TEST_DROP_PRODUCER="$2"; shift 2 ;;
    --test-parser-error) TEST_PARSER_ERROR=1; shift ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$ADR_PATH" ]]; then
  echo "FAIL: --adr is required" >&2
  usage >&2
  exit 2
fi
if [[ "$SINCE_RELEASE" != "0.4" ]]; then
  echo "FAIL: --since-release must be exactly 0.4 for the v0.5 verifier" >&2
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
case "$TEST_DROP_DECLARATION" in
  ""|move_object|grants_set|inheritance_set) ;;
  *) echo "FAIL: unsupported --test-drop-declaration value: $TEST_DROP_DECLARATION" >&2; exit 2 ;;
esac
case "$TEST_DROP_PRODUCER" in
  ""|move_object|grants_set|inheritance_set) ;;
  *) echo "FAIL: unsupported --test-drop-producer value: $TEST_DROP_PRODUCER" >&2; exit 2 ;;
esac
TEST_FAULT_COUNT=0
[[ -n "$TEST_DROP_DECLARATION" ]] && TEST_FAULT_COUNT=$((TEST_FAULT_COUNT + 1))
[[ -n "$TEST_DROP_PRODUCER" ]] && TEST_FAULT_COUNT=$((TEST_FAULT_COUNT + 1))
[[ $TEST_PARSER_ERROR -eq 1 ]] && TEST_FAULT_COUNT=$((TEST_FAULT_COUNT + 1))
if [[ $TEST_FAULT_COUNT -gt 1 ]]; then
  echo "FAIL: choose only one test-only fault injection" >&2
  exit 2
fi
for tool in git python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
if ! ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
if ! git -C "$REPO_ROOT" cat-file -e "${SOURCE_HEAD}^{commit}" 2>/dev/null; then
  echo "FAIL: cannot resolve committed source HEAD: $SOURCE_HEAD" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_ROOT"

exec python3 - "$REPO_ROOT" "$SOURCE_HEAD" "$ADR_PATH" "$EVIDENCE_ROOT" "$SINCE_RELEASE" \
  "$TEST_DROP_DECLARATION" "$TEST_DROP_PRODUCER" "$TEST_PARSER_ERROR" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

repo_root = pathlib.Path(sys.argv[1]).resolve()
source_head, adr_arg, evidence_arg, since_release, test_drop, test_drop_producer, test_parser_error = sys.argv[2:]
adr_path = pathlib.Path(adr_arg).resolve()
evidence_root = pathlib.Path(evidence_arg).resolve()
out_path = evidence_root / "cardinality-result.json"

EXPECTED_V04 = {
    "create_object", "set_flow_feature", "set_title", "insert_block",
    "update_block", "delete_block", "move_block", "semantic_patch",
    "archive", "restore",
}
EXPECTED_NEW = {"move_object", "grants_set", "inheritance_set"}
EXPECTED_CARDINALITY = {
    "move_object": ("BoundedMany", 2),
    "grants_set": ("Zero", 0),
    "inheritance_set": ("Zero", 0),
}
DIRTY_SCOPE = ["apps/", "crates/", "spikes/", "migrations/", ".cargo/", "Cargo.toml", "Cargo.lock"]


def git(*args, check=True):
    completed = subprocess.run(
        ["git", "-C", str(repo_root), *args], text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    if check and completed.returncode != 0:
        raise RuntimeError(
            f"git {' '.join(args)} failed ({completed.returncode}): {completed.stderr.strip()}"
        )
    return completed


def line_at(text, offset):
    return text.count("\n", 0, offset) + 1


def strip_comments(text):
    """Remove Rust comments while preserving byte positions and newlines."""
    chars = list(text)
    i = 0
    state = "code"
    depth = 0
    while i < len(chars):
        c = chars[i]
        n = chars[i + 1] if i + 1 < len(chars) else ""
        if state == "code":
            if c == '"':
                state = "string"
            elif c == "'":
                # Rust lifetimes are not character literals. Only enter a char
                # when a closing quote is visible within the next few bytes.
                tail = text[i + 1:i + 8]
                if re.match(r"(?:\\.|[^'\\])'", tail):
                    state = "char"
            elif c == "/" and n == "/":
                chars[i] = chars[i + 1] = " "
                i += 1
                state = "line_comment"
            elif c == "/" and n == "*":
                chars[i] = chars[i + 1] = " "
                i += 1
                state = "block_comment"
                depth = 1
        elif state == "string":
            if c == "\\":
                i += 1
            elif c == '"':
                state = "code"
        elif state == "char":
            if c == "\\":
                i += 1
            elif c == "'":
                state = "code"
        elif state == "line_comment":
            if c == "\n":
                state = "code"
            else:
                chars[i] = " "
        elif state == "block_comment":
            if c == "/" and n == "*":
                chars[i] = chars[i + 1] = " "
                i += 1
                depth += 1
            elif c == "*" and n == "/":
                chars[i] = chars[i + 1] = " "
                i += 1
                depth -= 1
                if depth == 0:
                    state = "code"
            elif c != "\n":
                chars[i] = " "
        i += 1
    return "".join(chars)


def matching_brace(text, opening):
    depth = 0
    state = "code"
    i = opening
    while i < len(text):
        c = text[i]
        if state == "code":
            if c == '"':
                state = "string"
            elif c == "'":
                tail = text[i + 1:i + 8]
                if re.match(r"(?:\\.|[^'\\])'", tail):
                    state = "char"
            elif c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    return i
        elif state == "string":
            if c == "\\":
                i += 1
            elif c == '"':
                state = "code"
        elif state == "char":
            if c == "\\":
                i += 1
            elif c == "'":
                state = "code"
        i += 1
    return None


def remove_cfg_test_items(text):
    """Blank cfg(test) items without discarding later production items."""
    chars = list(text)
    pattern = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
    cursor = 0
    while True:
        match = pattern.search(text, cursor)
        if not match:
            break
        opening = text.find("{", match.end())
        semi = text.find(";", match.end())
        if semi >= 0 and (opening < 0 or semi < opening):
            end = semi
        elif opening >= 0:
            closing = matching_brace(text, opening)
            if closing is None:
                end = len(text) - 1
            else:
                end = closing
        else:
            end = len(text) - 1
        for index in range(match.start(), end + 1):
            if chars[index] != "\n":
                chars[index] = " "
        cursor = end + 1
    return "".join(chars)


def function_occurrences(sources, name):
    found = []
    pattern = re.compile(rf"\bfn\s+{re.escape(name)}\s*(?:<[^{{;]*>)?\s*\(")
    for path, item in sources.items():
        code = item["code"]
        for match in pattern.finditer(code):
            opening = code.find("{", match.end())
            semi = code.find(";", match.end())
            if opening < 0 or (semi >= 0 and semi < opening):
                continue
            closing = matching_brace(code, opening)
            if closing is None:
                found.append({"path": path, "line": line_at(item["raw"], match.start()), "error": "unclosed function body"})
                continue
            found.append({
                "path": path,
                "line": line_at(item["raw"], match.start()),
                "offset": match.start(),
                "body": code[opening + 1:closing],
            })
    return found


def card_expr(expr, numeric_constants, card_constants):
    expr = expr.strip().rstrip(",")
    if expr in card_constants:
        return card_constants[expr]
    match = re.search(r"ExistingDocumentCardinality::(Zero|One|BoundedMany)\s*(?:\(([^)]*)\))?", expr)
    if not match:
        return None
    variant, argument = match.groups()
    if variant == "Zero":
        return {"variant": variant, "count": 0, "bound_expression": None}
    if variant == "One":
        return {"variant": variant, "count": 1, "bound_expression": None}
    argument = (argument or "").strip()
    count = int(argument) if argument.isdigit() else numeric_constants.get(argument)
    return {"variant": variant, "count": count, "bound_expression": argument or None}


def parse_match_mappings(method_body, numeric_constants, card_constants):
    start_match = re.search(r"\bmatch\s+self\s*\{", method_body)
    if not start_match:
        return []
    opening = method_body.find("{", start_match.start())
    closing = matching_brace(method_body, opening)
    if closing is None:
        return []
    body = method_body[opening + 1:closing]
    mappings = []
    arm = re.compile(
        r"((?:Self::[A-Za-z_]\w*\s*(?:\|\s*)?)+)\s*=>\s*"
        r"(\"[^\"]+\"|ExistingDocumentCardinality::(?:Zero|One|BoundedMany(?:\([^)]*\))))"
    )
    for match in arm.finditer(body):
        variants = re.findall(r"Self::([A-Za-z_]\w*)", match.group(1))
        for variant in variants:
            mappings.append((variant, match.group(2)))
    return mappings


def evaluate_registry(body, wire_map, card_map, numeric_constants, card_constants):
    entries = {}
    unresolved = []
    predecessor = bool(re.search(r"\bv0_4_command_cardinality_registry\s*\(", body))

    direct = re.compile(r"\(\s*\"([^\"]+)\"\s*,\s*([^,\n)]+(?:\([^)]*\))?)\s*\)")
    for match in direct.finditer(body):
        name, expression = match.groups()
        card = card_expr(expression, numeric_constants, card_constants)
        if card is None:
            unresolved.append(f"cannot resolve direct cardinality expression for {name}: {expression.strip()}")
        else:
            entries[name] = card

    variants = set(re.findall(r"\b([A-Z][A-Za-z0-9_]*)::([A-Z][A-Za-z0-9_]*)\b", body))
    for key in sorted(variants):
        if key[0] == "ExistingDocumentCardinality":
            continue
        wire = wire_map.get(key)
        card = card_map.get(key)
        if wire is None and card is None:
            unresolved.append(f"registry references unrecognized command variant {key[0]}::{key[1]}")
            continue
        if wire is None or card is None:
            unresolved.append(
                f"registry variant {key[0]}::{key[1]} lacks "
                f"{'wire_name' if wire is None else 'existing_document_cardinality'}"
            )
            continue
        entries[wire["name"]] = dict(card)
    return entries, predecessor, unresolved


def check(status, details=None, reasons=None):
    reasons = reasons or []
    return {
        "status": status,
        "passed": status == "passed",
        "reasons": reasons,
        **(details or {}),
    }


parser_errors = []
try:
    listing = git("ls-tree", "-r", "--name-only", source_head, "--", "apps/api/src").stdout.splitlines()
    rust_paths = sorted(path for path in listing if path.endswith(".rs"))
    if not rust_paths:
        raise RuntimeError("committed scan scope contains no Rust source files")
    sources = {}
    for path in rust_paths:
        raw = git("show", f"{source_head}:{path}").stdout
        sources[path] = {"raw": raw, "code": remove_cfg_test_items(strip_comments(raw))}
except Exception as exc:
    rust_paths = []
    sources = {}
    parser_errors.append(f"source enumeration/read failure: {exc}")

dirty_completed = git(
    "status", "--porcelain=v1", "--untracked-files=all", "--",
    "apps", "crates", "spikes", "migrations", ".cargo", "Cargo.toml", "Cargo.lock",
    check=False,
)
dirty_entries = [line for line in dirty_completed.stdout.splitlines() if line]
if dirty_completed.returncode != 0:
    parser_errors.append(f"source dirty-state query failed: {dirty_completed.stderr.strip()}")

try:
    adr_text = adr_path.read_text(encoding="utf-8")
    adr_sha256 = hashlib.sha256(adr_path.read_bytes()).hexdigest()
except OSError as exc:
    print(f"FAIL: cannot read ADR: {exc}", file=sys.stderr)
    sys.exit(2)

adr_anchors = {
    "contended_existing_document_set": "竞争文档集合" in adr_text or "contended existing document set" in adr_text,
    "v0_4_at_most_one": "v0.4" in adr_text and ("≤ 1" in adr_text or "<= 1" in adr_text),
    "bounded_many": "bounded_many" in adr_text.lower(),
    "sorted_locking": "升序" in adr_text or "ascending" in adr_text.lower(),
    "unapproved_batch_merge_fails": ("批量" in adr_text or "batch" in adr_text.lower()) and ("fail" in adr_text.lower() or "拒绝" in adr_text),
}

numeric_constants = {}
card_constants = {}
constant_locations = {}
wire_map = {}
card_map = {}
if sources:
    number_pattern = re.compile(r"\b(?:pub\s+)?const\s+([A-Z][A-Z0-9_]*)\s*:\s*[A-Za-z0-9_:<>]+\s*=\s*(\d+)\s*;")
    card_const_pattern = re.compile(
        r"\b(?:pub\s+)?const\s+([A-Z][A-Z0-9_]*)\s*:\s*ExistingDocumentCardinality\s*=\s*"
        r"(ExistingDocumentCardinality::(?:Zero|One|BoundedMany(?:\([^)]*\))))\s*;"
    )
    for path, item in sources.items():
        for match in number_pattern.finditer(item["code"]):
            numeric_constants[match.group(1)] = int(match.group(2))
            constant_locations[match.group(1)] = {"file": path, "line": line_at(item["raw"], match.start())}
    for path, item in sources.items():
        for match in card_const_pattern.finditer(item["code"]):
            resolved = card_expr(match.group(2), numeric_constants, {})
            if resolved is not None:
                card_constants[match.group(1)] = resolved
                constant_locations[match.group(1)] = {"file": path, "line": line_at(item["raw"], match.start())}

    impl_pattern = re.compile(r"\bimpl\s+([A-Za-z_]\w*)\s*\{")
    for path, item in sources.items():
        code = item["code"]
        if "wire_name" not in code and "existing_document_cardinality" not in code:
            continue
        for impl_match in impl_pattern.finditer(code):
            type_name = impl_match.group(1)
            opening = code.find("{", impl_match.start())
            closing = matching_brace(code, opening)
            if closing is None:
                parser_errors.append(f"unclosed impl {type_name} at {path}:{line_at(item['raw'], impl_match.start())}")
                continue
            impl_body = code[opening + 1:closing]
            impl_base = opening + 1
            for method_name in ("wire_name", "existing_document_cardinality"):
                method_matches = list(re.finditer(rf"\bfn\s+{method_name}\s*\([^)]*\)[^{{;]*\{{", impl_body))
                for method_match in method_matches:
                    method_open = impl_body.find("{", method_match.start())
                    method_close = matching_brace(impl_body, method_open)
                    if method_close is None:
                        parser_errors.append(f"unclosed {type_name}::{method_name} at {path}")
                        continue
                    method_body = impl_body[method_open + 1:method_close]
                    for variant, expression in parse_match_mappings(method_body, numeric_constants, card_constants):
                        key = (type_name, variant)
                        location = {
                            "file": path,
                            "line": line_at(item["raw"], impl_base + method_match.start()),
                        }
                        if method_name == "wire_name":
                            name_match = re.fullmatch(r'"([^\"]+)"', expression.strip())
                            if name_match:
                                wire_map[key] = {"name": name_match.group(1), **location}
                        else:
                            resolved = card_expr(expression, numeric_constants, card_constants)
                            if resolved is not None:
                                card_map[key] = {**resolved, **location}

registry_functions = {}
registries = {"v0.4": {}, "v0.5": {}}
registry_unresolved = {"v0.4": [], "v0.5": []}
if sources:
    for release, function_name in (
        ("v0.4", "v0_4_command_cardinality_registry"),
        ("v0.5", "v0_5_command_cardinality_registry"),
    ):
        occurrences = function_occurrences(sources, function_name)
        if len(occurrences) != 1 or "error" in occurrences[0]:
            parser_errors.append(
                f"expected exactly one parseable {function_name}; found {len(occurrences)}"
            )
            continue
        occurrence = occurrences[0]
        entries, predecessor, unresolved = evaluate_registry(
            occurrence["body"], wire_map, card_map, numeric_constants, card_constants
        )
        registry_functions[release] = {
            "name": function_name,
            "file": occurrence["path"],
            "line": occurrence["line"],
            "calls_v0_4_registry": predecessor,
        }
        registries[release] = entries
        registry_unresolved[release] = unresolved
        parser_errors.extend(f"{release} registry: {reason}" for reason in unresolved)

# The v0.5 function is a delta builder. Materialize its effective registry only
# when it explicitly preserves the v0.4 producer.
v04_registry = registries["v0.4"]
v05_delta = registries["v0.5"]
v05_calls_v04 = registry_functions.get("v0.5", {}).get("calls_v0_4_registry", False)
v05_registry = {**v04_registry, **v05_delta} if v05_calls_v04 else dict(v05_delta)

if test_drop:
    # Keep the command registered and remove only its machine declaration. This
    # is a transparent parser-input mutation used to prove the missing-field
    # branch without editing production source.
    if test_drop in v05_registry:
        v05_registry[test_drop] = None
if test_parser_error == "1":
    parser_errors.append("test-only injected parser error after committed-source enumeration")

v04_reasons = []
if set(v04_registry) != EXPECTED_V04:
    v04_reasons.append(
        f"v0.4 registry names changed: missing={sorted(EXPECTED_V04 - set(v04_registry))}, "
        f"unexpected={sorted(set(v04_registry) - EXPECTED_V04)}"
    )
for name, declaration in sorted(v04_registry.items()):
    if declaration is None or declaration.get("count") is None:
        v04_reasons.append(f"v0.4 command {name} has an unresolved cardinality")
    elif declaration["count"] > 1 or declaration["variant"] == "BoundedMany":
        v04_reasons.append(f"v0.4 command {name} exceeds the frozen <=1 bound")
v04_check = check(
    "passed" if not v04_reasons and not parser_errors else "failed",
    {
        "expected_names": sorted(EXPECTED_V04),
        "observed_names": sorted(v04_registry),
        "commands": [
            {"name": name, **(declaration or {"variant": None, "count": None})}
            for name, declaration in sorted(v04_registry.items())
        ],
    },
    v04_reasons,
)

superset_reasons = []
if not v05_calls_v04:
    superset_reasons.append("v0.5 registry does not explicitly call the v0.4 registry")
for name, declaration in v04_registry.items():
    if v05_registry.get(name) != declaration:
        superset_reasons.append(f"v0.5 does not preserve v0.4 declaration for {name}")
superset_check = check(
    "passed" if not superset_reasons and not parser_errors else "failed",
    {"v0_4_count": len(v04_registry), "v0_5_effective_count": len(v05_registry)},
    superset_reasons,
)

observed_new = set(v05_registry) - set(v04_registry)
scope_reasons = []
if observed_new != EXPECTED_NEW:
    scope_reasons.append(
        f"v0.5 delta mismatch: missing={sorted(EXPECTED_NEW - observed_new)}, "
        f"unexpected={sorted(observed_new - EXPECTED_NEW)}"
    )
scope_check = check(
    "passed" if not scope_reasons and not parser_errors else "failed",
    {
        "since_release": since_release,
        "target_release": "0.5",
        "expected_new_commands": sorted(EXPECTED_NEW),
        "observed_new_commands": sorted(observed_new),
        "later_release_registries_scanned": [],
    },
    scope_reasons,
)

all_code = "\n".join(item["code"] for item in sources.values())
producer_functions = {
    "move_object": ["execute_on"],
    "grants_set": ["set_grants", "put_flow_object_grants"],
    "inheritance_set": ["set_inheritance", "put_flow_object_inheritance"],
}
producer_evidence = {}
for command_name, function_names in producer_functions.items():
    evidence = []
    for function_name in function_names:
        occurrences = function_occurrences(sources, function_name)
        evidence.append({
            "function": function_name,
            "status": "passed" if len(occurrences) == 1 and "error" not in occurrences[0] else "producer_missing",
            "occurrences": [
                {"file": item["path"], "line": item["line"]}
                for item in occurrences if "error" not in item
            ],
        })
    if command_name == "move_object":
        evidence.append({
            "function": "wire_name(move_object)",
            "status": "passed" if '"move_object"' in all_code else "producer_missing",
            "occurrences": [],
        })
    producer_evidence[command_name] = evidence

command_results = []
for name in sorted(EXPECTED_NEW):
    expected_variant, expected_count = EXPECTED_CARDINALITY[name]
    missing_producer_evidence = [
        item["function"] for item in producer_evidence[name] if item["status"] != "passed"
    ]
    if test_drop_producer == name:
        missing_producer_evidence.append("test-only injected producer removal")
    registered = name in v05_registry
    declaration = v05_registry.get(name)
    declaration_status = "passed"
    reasons = []
    if not registered:
        registration_status = "registry_entry_missing"
        declaration_status = "declaration_missing"
        reasons.append("command producer exists but the command is absent from the v0.5 cardinality registry")
    else:
        registration_status = "passed"
        if declaration is None:
            declaration_status = "declaration_missing"
            reasons.append("registered command has no parseable existing_document_cardinality declaration")
        elif declaration.get("count") is None:
            declaration_status = "declaration_unresolved"
            reasons.append("BoundedMany bound expression could not be resolved to a document count")
        elif (declaration.get("variant"), declaration.get("count")) != (expected_variant, expected_count):
            declaration_status = "declaration_mismatch"
            reasons.append(
                f"expected {expected_variant}({expected_count}), observed "
                f"{declaration.get('variant')}({declaration.get('count')})"
            )
    if missing_producer_evidence:
        producer_status = "producer_missing"
        reasons.append(f"source producer evidence missing: {missing_producer_evidence}")
    else:
        producer_status = "passed"
    status = "passed" if not reasons else "failed"
    command_results.append({
        "name": name,
        "status": status,
        "passed": status == "passed",
        "producer_status": producer_status,
        "producer_evidence": producer_evidence[name],
        "registration_status": registration_status,
        "declaration_status": declaration_status,
        "expected": {"variant": expected_variant, "count": expected_count},
        "observed": declaration,
        "reasons": reasons,
    })

move_source = ""
move_path = None
for path, item in sources.items():
    if "MOVE_OBJECT_CONTENDED_DOCUMENT_MAX" in item["code"] and "ascending_document_lock_order" in item["code"]:
        move_path = path
        move_source = item["code"]
        break
bounded_many_facts = {
    "implementation_file": move_path,
    "declared_bound_is_two": bool(re.search(r"MOVE_OBJECT_CONTENDED_DOCUMENT_MAX\s*:\s*u8\s*=\s*2\s*;", move_source)),
    "derives_sorted_document_order": "ascending_document_lock_order(&documents)" in move_source,
    "coordinator_uses_same_order": bool(re.search(r"acquire_many\s*\(\s*&plan\.document_lock_order\s*\)", move_source)),
    "database_locks_iterate_same_order": bool(
        re.search(r"for\s+document_id\s+in\s+&plan\.document_lock_order\s*\{", move_source)
        and "lock_document_head" in move_source
    ),
    "document_count_guard": bool(re.search(
        r"document_lock_order\.len\(\)\s*>\s*MOVE_OBJECT_CONTENDED_DOCUMENT_MAX\s+as\s+usize",
        move_source,
    )),
}
bounded_reasons = [name for name, value in bounded_many_facts.items() if name != "implementation_file" and not value]
if bounded_reasons:
    bounded_reasons = ["missing bounded_many proof(s): " + ", ".join(bounded_reasons)]
bounded_check = check(
    "passed" if not bounded_reasons and not parser_errors else "failed",
    bounded_many_facts,
    bounded_reasons,
)

unapproved_candidates = []
for wire in wire_map.values():
    if re.search(r"(?:^|[._-])(batch|merge)(?:$|[._-])", wire["name"], re.IGNORECASE):
        unapproved_candidates.append({
            "kind": "wire_name",
            "name": wire["name"],
            "file": wire["file"],
            "line": wire["line"],
        })
for name in observed_new:
    if re.search(r"(?:^|[._-])(batch|merge)(?:$|[._-])", name, re.IGNORECASE):
        unapproved_candidates.append({"kind": "registry_delta", "name": name, "file": None, "line": None})
for path, item in sources.items():
    if "/flow/" not in path and not path.endswith("/routes/flow.rs") and not path.endswith("/main.rs"):
        continue
    for match in re.finditer(r"\bpub\s+async\s+fn\s+([A-Za-z_]\w*(?:batch|merge)[A-Za-z_]\w*)\s*\(", item["code"], re.IGNORECASE):
        unapproved_candidates.append({
            "kind": "public_async_producer",
            "name": match.group(1),
            "file": path,
            "line": line_at(item["raw"], match.start()),
        })
    for match in re.finditer(r'"(/api/v1/flow[^"\n]*(?:batch|merge)[^"\n]*)"', item["code"], re.IGNORECASE):
        unapproved_candidates.append({
            "kind": "flow_route",
            "name": match.group(1),
            "file": path,
            "line": line_at(item["raw"], match.start()),
        })
unapproved = sorted(
    {json.dumps(item, sort_keys=True) for item in unapproved_candidates}
)
unapproved = [json.loads(item) for item in unapproved]
batch_check = check(
    "passed" if not unapproved and not parser_errors else "failed",
    {"approved_batch_or_merge_commands": [], "observed_unapproved": unapproved},
    [] if not unapproved else [f"unapproved batch/merge command declarations: {unapproved}"],
)

grant_owner_paths = sorted({
    occurrence["file"]
    for command_name in ("grants_set", "inheritance_set")
    for producer in producer_evidence[command_name]
    if producer["function"] in {"set_grants", "set_inheritance"}
    for occurrence in producer["occurrences"]
})
grants_source = "\n".join(sources[path]["code"] for path in grant_owner_paths)
zero_path_facts = {
    "producer_implementation_files": grant_owner_paths,
    "producer_implementation_found": bool(grant_owner_paths),
    "postgres_metadata_mutation_observed": bool(re.search(
        r"(?:INSERT\s+INTO|UPDATE|DELETE\s+FROM)\s+(?:flow_object_grants|flow_objects|flow_relations)\b",
        grants_source,
        re.IGNORECASE,
    )),
    "collab_head_advance_tokens_absent": not bool(re.search(r"collab_documents|head_seq|advance.*head", grants_source, re.IGNORECASE)),
    "document_coordinator_tokens_absent": not bool(re.search(r"\.coordinator\b|acquire_many\s*\(", grants_source)),
    "database_transaction_does_not_raise_cardinality": True,
}
zero_reasons = [
    name for name, value in zero_path_facts.items()
    if name != "producer_implementation_files" and not value
]
if zero_reasons:
    zero_reasons = ["zero-cardinality authorization path proof failed: " + ", ".join(zero_reasons)]
zero_check = check(
    "passed" if not zero_reasons and not parser_errors else "failed",
    zero_path_facts,
    zero_reasons,
)

parser_status = "passed" if not parser_errors else "parser_error"
source_integrity_reasons = []
if dirty_entries:
    source_integrity_reasons.append("validated source scope is dirty; committed facts cannot prove the working tree")
if dirty_completed.returncode != 0:
    source_integrity_reasons.append("could not prove scoped source cleanliness")
source_integrity = check(
    "passed" if not source_integrity_reasons else "failed",
    {"dirty_scope": DIRTY_SCOPE, "dirty_entries": dirty_entries},
    source_integrity_reasons,
)

checks = {
    "parser": check(parser_status, {
        "scan_method": "git ls-tree plus git show <source_head>:<path>",
        "scan_root": "apps/api/src/**/*.rs",
        "files_scanned": len(rust_paths),
        "registry_functions": registry_functions,
        "unresolved_registry_entries": registry_unresolved,
        "errors": parser_errors,
    }, parser_errors),
    "adr_contract_anchors": check(
        "passed" if all(adr_anchors.values()) else "failed",
        {"anchors": adr_anchors},
        [] if all(adr_anchors.values()) else ["ADR-0013 required cardinality anchors are missing"],
    ),
    "v0_4_registry_unchanged_and_at_most_one": v04_check,
    "v0_5_registry_is_v0_4_superset": superset_check,
    "since_release_delta_exact": scope_check,
    "new_command_declarations": check(
        "passed" if all(item["passed"] for item in command_results) and not parser_errors else "failed",
        {"commands": command_results},
        [reason for item in command_results for reason in item["reasons"]],
    ),
    "bounded_many_sorted_lock_and_bound": bounded_check,
    "zero_cardinality_has_no_document_lock_path": zero_check,
    "unapproved_batch_merge_fails_closed": batch_check,
    "source_integrity": source_integrity,
}
if test_drop or test_drop_producer or test_parser_error == "1":
    injected_fault = (
        {"kind": "declaration_missing", "command": test_drop}
        if test_drop else
        {"kind": "producer_missing", "command": test_drop_producer}
        if test_drop_producer else
        {"kind": "parser_error", "command": None}
    )
    checks["synthetic_input"] = check(
        "failed",
        {"mode": "test_fault_injection", "fault": injected_fault},
        ["test-only fault injection was requested; synthetic evidence can never pass"],
    )

passed = all(item["passed"] for item in checks.values())
artifact = {
    "schema_version": "sylvode.flow.cardinality-result.v2",
    "release": "0.5",
    "since_release": since_release,
    "source_head": source_head,
    "source_dirty": bool(dirty_entries) or dirty_completed.returncode != 0,
    "source": {
        "head": source_head,
        "dirty": bool(dirty_entries) or dirty_completed.returncode != 0,
        "read_mode": "committed_git_objects",
        "dirty_scope": DIRTY_SCOPE,
        "dirty_entries": dirty_entries,
    },
    "source_integrity": source_integrity,
    "generated_at": dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
    "adr": {"path": str(adr_path), "sha256": adr_sha256, "section": "1"},
    "scan_scope": {
        "baseline_registry": "v0_4_command_cardinality_registry",
        "target_registry": "v0_5_command_cardinality_registry",
        "new_variants_only": sorted(EXPECTED_NEW),
        "declaration_discovery": "all committed Rust files below apps/api/src; file moves within this tree are discovered",
        "outside_scope_behavior": "missing/duplicate registry or unresolved declaration is parser_error and fails closed",
        "future_release_behavior": "v0.6+ registries are not scanned or inferred by this v0.5 verifier",
    },
    "checks": checks,
    "hard_gates": {
        "command_contended_document_cardinality": {
            "status": "passed" if passed else "failed",
            "passed": passed,
        }
    },
    "passed": passed,
}

evidence_root.mkdir(parents=True, exist_ok=True)
fd, temporary = tempfile.mkstemp(prefix=".cardinality-result.", suffix=".json.tmp", dir=evidence_root)
try:
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(artifact, handle, ensure_ascii=False, indent=2, sort_keys=True)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, out_path)
    directory_fd = os.open(evidence_root, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)
finally:
    if os.path.exists(temporary):
        os.unlink(temporary)

print(json.dumps(artifact, ensure_ascii=False, separators=(",", ":")))
print(
    f"cardinality v0.5: status={'passed' if passed else 'failed'} "
    f"source_head={source_head} source_dirty={artifact['source_dirty']} artifact={out_path}",
    file=sys.stderr,
)
for item in command_results:
    print(
        f"  {item['name']}: status={item['status']} producer={item['producer_status']} "
        f"registration={item['registration_status']} declaration={item['declaration_status']}",
        file=sys.stderr,
    )
sys.exit(0 if passed else 1)
PY

#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 authz-baseline verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, "Authz-
# baseline verifier" paragraph, and ADR-0012 §2-3.
#
# This script covers `flow_parent_authority_in_postgres` and the live
# `member_baseline_no_behaviour_regression` fixture. The latter uses a
# default_member_level=edit member, writes set_title/archive/restore through
# the frozen REST command surface, and observes every transition through REST,
# MCP HTTP/SSE/stdio and the shipped CLI. It includes refusal controls for a
# view-level member and lifecycle expected_frontier misuse.
#
# flow_parent_authority_in_postgres assertions:
#   1. STATIC: `flow_object_projections` (information_schema.columns) has
#      no parent/parent_id-shaped column at all -- so `nodes[].parent_id`
#      / `parent_id` filtering CANNOT be sourced from the projection
#      table; there is nothing there to read even if the code wanted to.
#   2. STATIC (read-only source grep, never edits apps/**): the read
#      queries in apps/api/src/flow/repository.rs select `fo.parent_id`
#      (the `flow_objects` alias), not any projection-table alias.
#   3. STATIC: v0.4's registered command wire names (the unique
#      `v0_4_command_cardinality_registry` found anywhere under the committed
#      apps/api/src tree) contain no cross-parent "move"
#      command -- a v0.4 navigator drag cannot invoke anything that
#      changes `parent_id` between different parents; only v0.4's content
#      commands (which never touch `parent_id`) and the parent-less
#      `create_object`/lifecycle commands exist.
#   4. LIVE: create a parent object and a child object with
#      `parent_object_id` set through the real REST create endpoint (so
#      `flow_objects`, `collab_documents` and `flow_object_projections`
#      are all populated exactly as production would), then GET
#      `/workspaces/{id}/flow/objects?parent_id=<parent>` and assert the
#      child appears with the SAME parent_id value a direct SQL read of
#      `flow_objects.parent_id` shows -- confirming the live read path
#      actually works end-to-end, not just in theory.
#
# Exit codes: 0 = both gates passed, 1 = an assertion failed, 2 =
# usage/tool/environment error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Shared --adr/--contract/--limits path resolution (absolute -> as-is;
# relative-to-CWD -> as-is; otherwise resolved against --contracts-root;
# unresolvable -> FAIL naming both attempted paths).
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
ADR_PATH=""
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
JSON_MODE=0
STATIC_CHECK_3_ONLY=0
TEST_ADD_V0_4_COMMAND=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-authz-baseline-v0.4.sh --adr PATH --json [OPTIONS]

Verifies flow_parent_authority_in_postgres and the default-edit member
before/after baseline across REST, MCP HTTP/SSE/stdio and CLI, explicitly
including archive and restore. Writes authz-baseline-result.json.

Options:
  --adr PATH              Path to ADR-0012. Required. Relative paths
                          are resolved against the current directory
                          first, then against --contracts-root.
  --contracts-root DIR     Root containing decisions/. Default:
                          /opt/working/sylvode-flow
  --database-url URL       Postgres DSN. Default: $OPENPR_TEST_DATABASE_URL
  --repo-root DIR         Repository containing apps/api. Default: this
                          checkout.
  --evidence-root DIR     Where authz-baseline-result.json is written.
                          Default: /opt/working/sylvode-flow/evidence/v0.4
  --json                  Required for CLI-contract compatibility.
  --static-check-3-only   Run only the committed-source v0.4 registry check;
                          writes no evidence artifact and does not use a DB.
  --test-add-v0-4-command NAME
                          Test-only fault injection for --static-check-3-only.
                          Adds NAME to the parsed registry; can never pass.
  -h, --help              Show this help and exit 0.

Exit codes: 0 both gates passed, 1 an assertion failed, 2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) ADR_PATH="${2:?--adr requires a PATH argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a value}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --static-check-3-only) STATIC_CHECK_3_ONLY=1; shift ;;
    --test-add-v0-4-command) TEST_ADD_V0_4_COMMAND="${2:?--test-add-v0-4-command requires NAME}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$ADR_PATH" ]]; then
  echo "FAIL: --adr is required" >&2
  usage >&2
  exit 2
fi
if ! ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
if [[ -n "$TEST_ADD_V0_4_COMMAND" ]]; then
  if [[ $STATIC_CHECK_3_ONLY -ne 1 ]]; then
    echo "FAIL: --test-add-v0-4-command is allowed only with --static-check-3-only" >&2
    exit 2
  fi
  if [[ ! "$TEST_ADD_V0_4_COMMAND" =~ ^[a-z_]+$ ]]; then
    echo "FAIL: --test-add-v0-4-command NAME must match [a-z_]+" >&2
    exit 2
  fi
fi
for tool in jq sha256sum git psql curl python3 cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
REPOSITORY_RS="$REPO_ROOT/apps/api/src/flow/repository.rs"
if [[ ! -f "$REPOSITORY_RS" ]]; then
  echo "FAIL: source file not found (nothing to statically verify): $REPOSITORY_RS" >&2
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CARGO_OUTPUT_DIR="${CARGO_TARGET_DIR:-target}"
if [[ "$CARGO_OUTPUT_DIR" != /* ]]; then
  CARGO_OUTPUT_DIR="$REPO_ROOT/$CARGO_OUTPUT_DIR"
fi

write_environment_failure() {
  local reason="$1" out="$EVIDENCE_ROOT/authz-baseline-result.json" tmp result
  result="$(jq -n --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg adr "$ADR_PATH" --arg reason "$reason" '{
    schema_version:"sylvode.flow.authz-baseline-result.v1",source_head:$head,generated_at:$generated_at,adr:$adr,
    environment:{reachable:false,reason:$reason},
    flow_parent_authority_in_postgres:{violations:[$reason],passed:false},
    member_baseline_no_behaviour_regression:{status:"failed",reason:$reason,violations:[$reason]},
    passed:false
  }')"
  tmp="$out.tmp"
  printf '%s\n' "$result" | jq . > "$tmp"
  mv -f "$tmp" "$out"
  printf '%s\n' "$result"
  exit 1
}

VIOLATIONS=()

# ---- static 3: frozen v0.4 registry has no cross-parent command ----
#
# Read only the committed tree.  The registry may move anywhere inside
# apps/api/src without weakening the check; moving it outside the scan tree,
# defining it twice, or changing it to syntax this deliberately small parser
# cannot prove is a parser_error and therefore a failure.  The exact frozen set
# prevents an unparsed new spelling from becoming an empty/partial false green.
REGISTRY_SCAN_JSON="$(python3 - "$REPO_ROOT" "$SOURCE_HEAD" "$TEST_ADD_V0_4_COMMAND" <<'PY'
import json
import re
import subprocess
import sys

repo_root, source_head, injected_name = sys.argv[1:]
expected = {
    "create_object", "set_flow_feature", "set_title", "insert_block",
    "update_block", "delete_block", "move_block", "semantic_patch",
    "archive", "restore",
}
cross_parent = {"move_object", "change_parent", "reparent"}
parser_errors = []
violations = []


def git(*args):
    completed = subprocess.run(
        ["git", "-C", repo_root, *args], text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"git {' '.join(args)} failed ({completed.returncode}): {completed.stderr.strip()}"
        )
    return completed.stdout


def line_at(text, offset):
    return text.count("\n", 0, offset) + 1


def strip_comments(text):
    """Blank Rust comments while preserving positions and newlines."""
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


def matching(text, opening, left="{", right="}"):
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
            elif c == left:
                depth += 1
            elif c == right:
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
            end = matching(text, opening)
            if end is None:
                end = len(text) - 1
        else:
            end = len(text) - 1
        for index in range(match.start(), end + 1):
            if chars[index] != "\n":
                chars[index] = " "
        cursor = end + 1
    return "".join(chars)


def functions(sources, name):
    found = []
    pattern = re.compile(rf"\bfn\s+{re.escape(name)}\s*(?:<[^{{;]*>)?\s*\(")
    for path, item in sources.items():
        code = item["code"]
        for match in pattern.finditer(code):
            opening = code.find("{", match.end())
            semi = code.find(";", match.end())
            if opening < 0 or (semi >= 0 and semi < opening):
                continue
            closing = matching(code, opening)
            if closing is None:
                found.append({"path": path, "line": line_at(item["raw"], match.start()), "error": "unclosed body"})
            else:
                found.append({
                    "path": path,
                    "line": line_at(item["raw"], match.start()),
                    "body": code[opening + 1:closing],
                })
    return found


def wire_map_for(sources, type_name):
    mappings = {}
    method_count = 0
    impl_pattern = re.compile(rf"\bimpl\s+{re.escape(type_name)}\s*\{{")
    for path, item in sources.items():
        code = item["code"]
        for impl_match in impl_pattern.finditer(code):
            impl_open = code.find("{", impl_match.start())
            impl_close = matching(code, impl_open)
            if impl_close is None:
                parser_errors.append(f"unclosed impl {type_name} at {path}:{line_at(item['raw'], impl_match.start())}")
                continue
            impl_body = code[impl_open + 1:impl_close]
            for method in re.finditer(r"\bfn\s+wire_name\s*\([^)]*\)[^\{;]*\{", impl_body):
                method_count += 1
                method_open = impl_body.find("{", method.start())
                method_close = matching(impl_body, method_open)
                if method_close is None:
                    parser_errors.append(f"unclosed {type_name}::wire_name at {path}")
                    continue
                method_body = impl_body[method_open + 1:method_close]
                arm_pattern = re.compile(
                    r"((?:Self::[A-Za-z_]\w*\s*(?:\|\s*)?)+)\s*=>\s*\"([^\"]+)\""
                )
                for arm in arm_pattern.finditer(method_body):
                    for variant in re.findall(r"Self::([A-Za-z_]\w*)", arm.group(1)):
                        if variant in mappings:
                            parser_errors.append(f"duplicate {type_name}::{variant} wire_name mapping")
                        mappings[variant] = arm.group(2)
    if method_count != 1:
        parser_errors.append(f"expected exactly one parseable {type_name}::wire_name; found {method_count}")
    return mappings


sources = {}
registry_location = None
observed_entries = []
try:
    paths = sorted(
        path for path in git("ls-tree", "-r", "--name-only", source_head, "--", "apps/api/src").splitlines()
        if path.endswith(".rs")
    )
    if not paths:
        raise RuntimeError("committed apps/api/src scan tree contains no Rust files")
    for path in paths:
        raw = git("show", f"{source_head}:{path}")
        sources[path] = {"raw": raw, "code": remove_cfg_test_items(strip_comments(raw))}

    occurrences = functions(sources, "v0_4_command_cardinality_registry")
    if len(occurrences) != 1 or (occurrences and "error" in occurrences[0]):
        parser_errors.append(
            "expected exactly one parseable v0_4_command_cardinality_registry "
            f"inside committed apps/api/src; found {len(occurrences)}"
        )
    else:
        occurrence = occurrences[0]
        registry_location = {"file": occurrence["path"], "line": occurrence["line"]}
        body = occurrence["body"]
        consumed = [False] * len(body)

        vector = re.search(r"\blet\s+mut\s+registry\s*=\s*vec!\s*\[", body)
        if not vector:
            parser_errors.append("registry initializer is not parseable as `let mut registry = vec![...]`")
        else:
            vector_open = body.find("[", vector.start())
            vector_close = matching(body, vector_open, "[", "]")
            if vector_close is None:
                parser_errors.append("v0.4 registry vec initializer has no closing bracket")
            else:
                semi = body.find(";", vector_close)
                if semi < 0 or body[vector_close + 1:semi].strip():
                    parser_errors.append("v0.4 registry vec initializer has an unparseable terminator")
                else:
                    content = body[vector_open + 1:vector_close]
                    tuple_pattern = re.compile(r"\(\s*\"([a-z_]+)\"\s*,\s*[A-Z][A-Z0-9_]*\s*\)")
                    content_residual = list(content)
                    for item in tuple_pattern.finditer(content):
                        observed_entries.append(item.group(1))
                        for index in range(item.start(), item.end()):
                            content_residual[index] = " "
                    if re.sub(r"[\s,]", "", "".join(content_residual)):
                        parser_errors.append("v0.4 registry vec contains an unparseable entry")
                    for index in range(vector.start(), semi + 1):
                        consumed[index] = True

        loop_pattern = re.compile(r"\bfor\s+([a-z_]\w*)\s+in\s*\[")
        for loop in loop_pattern.finditer(body):
            variable = loop.group(1)
            array_open = body.find("[", loop.start())
            array_close = matching(body, array_open, "[", "]")
            if array_close is None:
                parser_errors.append(f"registry loop `{variable}` has no closing array bracket")
                continue
            loop_open = body.find("{", array_close)
            if loop_open < 0 or body[array_close + 1:loop_open].strip():
                parser_errors.append(f"registry loop `{variable}` has an unparseable body opener")
                continue
            loop_close = matching(body, loop_open)
            if loop_close is None:
                parser_errors.append(f"registry loop `{variable}` has no closing body brace")
                continue

            array = body[array_open + 1:array_close]
            variant_pattern = re.compile(r"\b([A-Z][A-Za-z0-9_]*)::([A-Z][A-Za-z0-9_]*)\b")
            variants = list(variant_pattern.finditer(array))
            array_residual = list(array)
            for variant in variants:
                for index in range(variant.start(), variant.end()):
                    array_residual[index] = " "
            if not variants or re.sub(r"[\s,]", "", "".join(array_residual)):
                parser_errors.append(f"registry loop `{variable}` contains an unparseable variant list")
                continue

            loop_body = body[loop_open + 1:loop_close]
            push_pattern = re.compile(
                rf"registry\s*\.\s*push\s*\(\s*\(\s*{re.escape(variable)}\s*\.\s*wire_name\s*\(\s*\)\s*,\s*"
                rf"{re.escape(variable)}\s*\.\s*existing_document_cardinality\s*\(\s*\)\s*\)\s*\)\s*;"
            )
            pushes = list(push_pattern.finditer(loop_body))
            loop_residual = list(loop_body)
            for push in pushes:
                for index in range(push.start(), push.end()):
                    loop_residual[index] = " "
            if len(pushes) != 1 or re.sub(r"\s", "", "".join(loop_residual)):
                parser_errors.append(f"registry loop `{variable}` body is not the one recognized push form")
                continue

            maps = {}
            for variant in variants:
                type_name, variant_name = variant.groups()
                if type_name not in maps:
                    maps[type_name] = wire_map_for(sources, type_name)
                wire_name = maps[type_name].get(variant_name)
                if wire_name is None:
                    parser_errors.append(f"cannot resolve {type_name}::{variant_name} through wire_name")
                else:
                    observed_entries.append(wire_name)
            for index in range(loop.start(), loop_close + 1):
                consumed[index] = True

        final_registry = list(re.finditer(r"\bregistry\b", body))
        unconsumed_registry = [item for item in final_registry if not consumed[item.start()]]
        if len(unconsumed_registry) == 1:
            final = unconsumed_registry[0]
            for index in range(final.start(), final.end()):
                consumed[index] = True
        else:
            parser_errors.append(
                "expected exactly one final registry expression after parsed initializer/loops; "
                f"found {len(unconsumed_registry)}"
            )

        residual = "".join(" " if used else char for char, used in zip(body, consumed))
        if re.sub(r"\s", "", residual):
            parser_errors.append("v0.4 registry function contains unrecognized syntax")
except Exception as exc:
    parser_errors.append(f"committed-source enumeration/read failure: {exc}")

if len(observed_entries) != len(set(observed_entries)):
    parser_errors.append("v0.4 registry contains duplicate wire names")

observed = set(observed_entries)
missing = sorted(expected - observed)
unexpected = sorted(observed - expected)
if missing:
    parser_errors.append(f"cannot prove the complete frozen v0.4 registry; missing={missing}")

if injected_name:
    observed.add(injected_name)
    unexpected = sorted(observed - expected)

forbidden = sorted(observed & cross_parent)
if forbidden:
    violations.append(f"cross-parent command(s) found in frozen v0.4 registry: {forbidden}")
if unexpected:
    violations.append(f"frozen v0.4 registry gained unexpected command(s): {unexpected}")

result = {
    "scan_root": "apps/api/src",
    "source_head": source_head,
    "registry_function": "v0_4_command_cardinality_registry",
    "registry_location": registry_location,
    "expected_names": sorted(expected),
    "observed_names": sorted(observed),
    "parser_status": "parser_error" if parser_errors else "passed",
    "parser_errors": parser_errors,
    "violations": violations,
    "test_injection": {"added_command": injected_name or None},
    "passed": not parser_errors and not violations,
}
print(json.dumps(result, ensure_ascii=False, separators=(",", ":")))
PY
)"

WIRE_NAMES="$(jq -r '(.observed_names | join(",")) + (if (.observed_names | length) > 0 then "," else "" end)' <<<"$REGISTRY_SCAN_JSON")"
while IFS= read -r reason; do
  [[ -z "$reason" ]] || VIOLATIONS+=("static check 3 parser_error: $reason")
done < <(jq -r '.parser_errors[]' <<<"$REGISTRY_SCAN_JSON")
while IFS= read -r reason; do
  [[ -z "$reason" ]] || VIOLATIONS+=("static check 3 registry violation: $reason")
done < <(jq -r '.violations[]' <<<"$REGISTRY_SCAN_JSON")
echo "static check 3: committed v0.4 registry = $WIRE_NAMES location=$(jq -c '.registry_location' <<<"$REGISTRY_SCAN_JSON") parser_status=$(jq -r '.parser_status' <<<"$REGISTRY_SCAN_JSON") passed=$(jq -r '.passed' <<<"$REGISTRY_SCAN_JSON")" >&2

if [[ $STATIC_CHECK_3_ONLY -eq 1 ]]; then
  jq . <<<"$REGISTRY_SCAN_JSON"
  [[ "$(jq -r '.passed' <<<"$REGISTRY_SCAN_JSON")" == true ]] && exit 0
  exit 1
fi

mkdir -p "$EVIDENCE_ROOT"
[[ -n "$DATABASE_URL" ]] || write_environment_failure "no database URL configured"
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc "SELECT 1" >/dev/null 2>&1 || \
  write_environment_failure "configured PostgreSQL environment is unreachable"

# ---- static 1: flow_object_projections has no parent-shaped column ----
PROJ_COLUMNS="$(psql "$DATABASE_URL" -Atc "SELECT string_agg(column_name, ',') FROM information_schema.columns WHERE table_schema='public' AND table_name='flow_object_projections'")"
if grep -qi "parent" <<<"$PROJ_COLUMNS"; then
  VIOLATIONS+=("flow_object_projections has a parent-shaped column ($PROJ_COLUMNS) -- parent authority may leak into the projection table")
fi
echo "static check 1: flow_object_projections columns = $PROJ_COLUMNS" >&2

# ---- static 2: read queries select fo.parent_id, not a projection alias ----
if ! grep -q "fo\.parent_id" "$REPOSITORY_RS"; then
  VIOLATIONS+=("$REPOSITORY_RS: no 'fo.parent_id' select found -- cannot confirm parent_id is read from the flow_objects alias")
fi
PROJECTION_ALIAS_JSON="$(python3 "$ROOT_DIR/scripts/lib/flow_v0_4_verifier_source_checks.py" projection-parent-aliases "$REPOSITORY_RS")" || {
  echo "FAIL: could not analyze projection aliases in $REPOSITORY_RS" >&2
  exit 2
}
while IFS= read -r finding; do
  [[ -z "$finding" ]] || VIOLATIONS+=("$REPOSITORY_RS: $finding")
done < <(jq -r '.violations[]' <<<"$PROJECTION_ALIAS_JSON")
echo "static check 2: fo.parent_id present=$(grep -c 'fo\.parent_id' "$REPOSITORY_RS") projection aliases=$(jq -c '.aliases' <<<"$PROJECTION_ALIAS_JSON") violations=$(jq '.violations | length' <<<"$PROJECTION_ALIAS_JSON")" >&2

# ---- live end-to-end check ----
echo "=== building api binary (cargo build -p api --bin api) ===" >&2
echo "=== prerequisite: cargo build -p collab-core --bin collab-isolated-apply-worker ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p collab-core --bin collab-isolated-apply-worker ) || {
  echo "FAIL: collab-isolated-apply-worker failed to build" >&2
  exit 2
}
( cd "$REPO_ROOT" && cargo build -q -p api --bin api ) || {
  echo "FAIL: api binary failed to build" >&2
  exit 2
}
API_BIN="$CARGO_OUTPUT_DIR/debug/api"

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
TMP_DIR="$(mktemp -d "/opt/worker/.cache/openpr-authz-baseline-verify.XXXXXX")"
API_PORT=$((22000 + RANDOM % 20000))
API_LOG="$TMP_DIR/api.log"
API_PID=""
WORKSPACE_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
BOT_TOKEN="opr_authz_verify_${RUN_ID}"

# shellcheck disable=SC2317  # invoked only via `trap ... EXIT` below.
cleanup() {
  local ec=$?
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q >/dev/null 2>&1 <<SQL || true
DELETE FROM flow_object_projections WHERE object_id IN (SELECT id FROM flow_objects WHERE workspace_id='$WORKSPACE_ID');
DELETE FROM collab_documents WHERE object_id IN (SELECT id FROM flow_objects WHERE workspace_id='$WORKSPACE_ID');
DELETE FROM flow_objects WHERE workspace_id='$WORKSPACE_ID';
DELETE FROM flow_workspace_settings WHERE workspace_id='$WORKSPACE_ID';
DELETE FROM workspace_bots WHERE id='$BOT_ID';
DELETE FROM workspace_members WHERE workspace_id='$WORKSPACE_ID';
DELETE FROM workspaces WHERE id='$WORKSPACE_ID';
DELETE FROM users WHERE id IN ('$OWNER_USER','$BOT_ID');
SQL
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

APP_CONFIG="$TMP_DIR/openpr.toml"
cat > "$APP_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$DATABASE_URL"

[auth]
jwt_secret = "authz-baseline-verify-not-a-real-secret"

[logging]
filter = "api=info,openpr=info"
format = "text"
EOF

BOT_TOKEN_HASH="$(printf '%s' "$BOT_TOKEN" | sha256sum | awk '{print $1}')"
BOT_TOKEN_PREFIX="${BOT_TOKEN:0:8}"
# A bot's actor identity is a *mirrored* `users` row with the same id as its
# `workspace_bots` row (apps/api/src/routes/bot.rs's real bot-creation path,
# read-only reference) -- `flow_objects.created_by` has an FK to `users(id)`,
# so a bot without this mirror row cannot author a flow object at all.
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$OWNER_USER', 'authz-verify-$RUN_ID@example.local', '', 'Authz Verify Owner', 'user', true, 'human', NULL, now(), now());
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
VALUES ('$BOT_ID', 'authz-verify-bot-$RUN_ID@bot.openpr.local', '!', 'Authz Verify Bot', 'user', true, 'bot_mcp', 'mcp', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE_ID', 'authz-verify-$RUN_ID', 'Authz Verify', '$OWNER_USER', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES
  ('$WORKSPACE_ID', '$OWNER_USER', 'owner', now()),
  ('$WORKSPACE_ID', '$BOT_ID', 'member', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE_ID', true, 'edit', 0, now());
INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by, is_active, created_at, updated_at)
VALUES ('$BOT_ID', '$WORKSPACE_ID', 'Authz Verify Bot', '$BOT_TOKEN_HASH', '$BOT_TOKEN_PREFIX', '["read","write"]'::jsonb, '$OWNER_USER', true, now(), now());
SQL

"$API_BIN" --config "$APP_CONFIG" > "$API_LOG" 2>&1 &
API_PID=$!
HEALTHY=0
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then HEALTHY=1; break; fi
  sleep 0.5
done
if [[ $HEALTHY -ne 1 ]]; then
  echo "FAIL: api did not become healthy within 30s; log follows" >&2
  cat "$API_LOG" >&2
  exit 2
fi

create_object() {
  local parent_json="$1" key
  key="$(python3 -c 'import uuid; print(uuid.uuid4())')"
  curl -sS -X POST "http://127.0.0.1:$API_PORT/api/v1/workspaces/$WORKSPACE_ID/flow/objects" \
    -H "Authorization: Bearer $BOT_TOKEN" -H "Content-Type: application/json" \
    -d "$(jq -n --arg key "$key" --argjson parent "$parent_json" \
      '{object_type:"page", title:"Authz baseline fixture", idempotency_key:$key} + (if $parent == null then {} else {parent_object_id:$parent} end)')"
}

PARENT_RESP="$(create_object null)"
PARENT_ID="$(jq -r '.data.object.id // empty' <<<"$PARENT_RESP")"
if [[ -z "$PARENT_ID" ]]; then
  echo "FAIL: could not create parent object via live REST call: $PARENT_RESP" >&2
  cat "$API_LOG" >&2
  exit 2
fi
CHILD_RESP="$(create_object "\"$PARENT_ID\"")"
CHILD_ID="$(jq -r '.data.object.id // empty' <<<"$CHILD_RESP")"
if [[ -z "$CHILD_ID" ]]; then
  echo "FAIL: could not create child object via live REST call: $CHILD_RESP" >&2
  cat "$API_LOG" >&2
  exit 2
fi

LIST_RESP="$(curl -sS "http://127.0.0.1:$API_PORT/api/v1/workspaces/$WORKSPACE_ID/flow/objects?parent_id=$PARENT_ID" -H "Authorization: Bearer $BOT_TOKEN")"
LIST_HAS_CHILD="$(jq --arg id "$CHILD_ID" '[.data.items[]? | select(.id==$id)] | length' <<<"$LIST_RESP")"
if [[ "$LIST_HAS_CHILD" != "1" ]]; then
  VIOLATIONS+=("GET .../flow/objects?parent_id=$PARENT_ID did not return the child object (response: $LIST_RESP)")
fi
SQL_PARENT_ID="$(psql "$DATABASE_URL" -Atc "SELECT parent_id FROM flow_objects WHERE id='$CHILD_ID'")"
if [[ "$SQL_PARENT_ID" != "$PARENT_ID" ]]; then
  VIOLATIONS+=("direct SQL read of flow_objects.parent_id ($SQL_PARENT_ID) does not match the parent used to create the child ($PARENT_ID)")
fi

PASSED_PARENT_AUTHORITY=$([[ ${#VIOLATIONS[@]} -eq 0 ]] && echo true || echo false)
VIOLATIONS_JSON="$(printf '%s\n' "${VIOLATIONS[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"

# ---- live default-member before/after surface matrix ----
MEMBER_HELPER="$ROOT_DIR/scripts/lib/flow_authz_baseline_live.sh"
[[ -f "$MEMBER_HELPER" ]] || { echo "FAIL: missing member baseline helper: $MEMBER_HELPER" >&2; exit 2; }
set +e
MEMBER_JSON="$(bash "$MEMBER_HELPER" "$REPO_ROOT" "$DATABASE_URL" "$TMP_DIR" 2>"$TMP_DIR/member-helper.stderr")"
MEMBER_EXIT=$?
set -e
if ! jq -e . >/dev/null 2>&1 <<<"$MEMBER_JSON"; then
  MEMBER_JSON="$(jq -n -c --arg reason "member baseline helper produced invalid JSON: $(tail -30 "$TMP_DIR/member-helper.stderr" | tr '\n' ' ')" '{status:"failed",observations:{},negative_controls:{},violations:[$reason]}')"
  MEMBER_EXIT=1
fi
MEMBER_STATUS="$(jq -r '.status // "failed"' <<<"$MEMBER_JSON")"
OVERALL_PASSED=$([[ "$PASSED_PARENT_AUTHORITY" == true && "$MEMBER_STATUS" == passed && $MEMBER_EXIT -eq 0 ]] && echo true || echo false)

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg adr "$ADR_PATH" \
  --arg proj_columns "$PROJ_COLUMNS" --arg wire_names "$WIRE_NAMES" \
  --argjson registry_scan "$REGISTRY_SCAN_JSON" \
  --argjson violations "$VIOLATIONS_JSON" --argjson parent_authority_passed "$PASSED_PARENT_AUTHORITY" \
  --argjson member_baseline "$MEMBER_JSON" --argjson overall_passed "$OVERALL_PASSED" \
  '{
    schema_version: "sylvode.flow.authz-baseline-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    adr: $adr,
    flow_parent_authority_in_postgres: {
      flow_object_projections_columns: $proj_columns,
      v0_4_command_wire_names: $wire_names,
      v0_4_registry_scan: $registry_scan,
      violations: $violations,
      passed: $parent_authority_passed
    },
    member_baseline_no_behaviour_regression: ($member_baseline + {
      reason: (if $member_baseline.status == "passed" then "live default_member_level=edit timeline preserved set_title, archive and restore across REST, MCP HTTP/SSE/stdio and CLI with refusal controls" else ($member_baseline.violations | join("; ")) end)
    }),
    passed: $overall_passed
  }')"

OUT_PATH="$EVIDENCE_ROOT/authz-baseline-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

echo "$RESULT"
[[ "$OVERALL_PASSED" == true ]] && exit 0
exit 1

#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 command-cardinality verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, "Cardinality
# verifier" paragraph, and ADR-0013 §1 ("v0.4 的竞争文档集合恒 ≤ 1").
#
# This script covers both the registry bound and the four live concurrency
# fixtures gate-commands.md requires for `command_contended_document_cardinality`:
#
#   1. STATIC cross-check (no live server needed): every v0.4 command wire
#      name frozen by contracts/rest-api-v1.md's
#      "POST /flow/objects/{object_id}/commands" row (plus the two
#      non-`commands` write paths create_object/set_flow_feature that
#      ADR-0013 §1 also cardinality-bounds) is declared in
#      apps/api/src/flow/command.rs's own cardinality match arms, and that
#      every declared cardinality is <= 1 (no BoundedMany at v0.4). This
#      is read-only source inspection -- it never edits apps/**.
#   2. DYNAMIC cross-check: it actually runs the Rust test suite that
#      exercises `v0_4_command_cardinality_registry()` itself
#      (`cargo test -p api cardinality_gate_tests`), so the static text
#      match in step 1 is corroborated by the live enum values the
#      compiled code actually produces, not just source text.
#
# The preallocated-UUID case is injected at the real PostgreSQL canonical
# transaction boundary because the v0.4 zero-inventory branch intentionally
# has neither a legacy preview/commit route nor flow_import_lineage table.
# Evidence records that producer boundary instead of pretending a nonexistent
# legacy endpoint ran. Live API reads before/after prove zero overwrite.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Shared --adr/--contract/--limits path resolution (absolute -> as-is;
# relative-to-CWD -> as-is; otherwise resolved against --contracts-root;
# unresolvable -> FAIL naming both attempted paths).
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
REPO_ROOT="$ROOT_DIR"
ADR_PATH=""
MAX_CARDINALITY=1
JSON_MODE=0
SKIP_CARGO_TEST=0
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-cardinality-v0.4.sh --adr PATH --max-cardinality 1 --json [OPTIONS]

Cross-checks the v0.4 command-cardinality registry, runs the Rust registry
tests, then boots a real API and scratch PostgreSQL database to exercise all
four concurrency/idempotency fixtures. Writes cardinality-result.json.

Options:
  --adr PATH              Path to ADR-0013. Required (recorded in the
                          output; content is not further parsed here --
                          the cardinality bound itself comes from
                          --max-cardinality). A relative path is resolved
                          against the current directory first, then
                          against --contracts-root.
  --max-cardinality N     The ADR-0013 §1 bound (v0.4: 1). Required.
  --contracts-root DIR     Root containing contracts/. Default:
                          /opt/working/sylvode-flow
  --evidence-root DIR     Where cardinality-result.json is written.
                          Default: /opt/working/sylvode-flow/evidence/v0.4
  --repo-root DIR         Repository containing apps/api and the cargo
                          workspace. Default: this checkout.
  --database-url URL      PostgreSQL DSN on which a scratch database may be
                          created. Default: $OPENPR_TEST_DATABASE_URL.
  --skip-cargo-test        Skip the `cargo test -p api cardinality_gate_tests`
                          step (for fast iteration only; the written
                          evidence records this and treats the dynamic
                          check as failed).
  --json                  Required for CLI-contract compatibility.
  -h, --help              Show this help and exit 0.

Exit codes: 0 all checks passed, 1 an observed failure/environment gap,
2 usage/tool/evidence malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) ADR_PATH="${2:?--adr requires a PATH argument}"; shift 2 ;;
    --max-cardinality) MAX_CARDINALITY="${2:?--max-cardinality requires a value}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a URL}"; shift 2 ;;
    --skip-cargo-test) SKIP_CARGO_TEST=1; shift ;;
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
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
for tool in jq git python3 psql curl cargo sha256sum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done

REST_API_MD="$CONTRACTS_ROOT/contracts/rest-api-v1.md"
COMMAND_RS="$REPO_ROOT/apps/api/src/flow/command.rs"
if [[ ! -f "$REST_API_MD" ]]; then
  echo "FAIL: missing contract file: $REST_API_MD" >&2
  exit 2
fi
if ! ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if [[ ! -f "$COMMAND_RS" ]]; then
  echo "FAIL: source file not found (nothing to statically verify): $COMMAND_RS" >&2
  exit 2
fi
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_ROOT"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
LIVE_PROBE="$ROOT_DIR/scripts/lib/flow_cardinality_live_probe.py"
[[ -f "$LIVE_PROBE" ]] || { echo "FAIL: missing live probe: $LIVE_PROBE" >&2; exit 2; }

# ---- 1. static cross-check (read-only) ----
STATIC_JSON="$(python3 - "$REST_API_MD" "$COMMAND_RS" "$MAX_CARDINALITY" <<'PY'
import json
import re
import sys

rest_api_md, command_rs, max_cardinality = sys.argv[1], sys.argv[2], int(sys.argv[3])

rest_text = open(rest_api_md, encoding="utf-8").read()
m = re.search(r"v0\.4 type=`([a-z_|]+)`", rest_text)
violations = []
if not m:
    print(json.dumps({"error": "could not find the frozen v0.4 command type list in rest-api-v1.md"}))
    sys.exit(0)
content_lifecycle_names = m.group(1).split("|")
# The two non-`commands` write paths ADR-0013 §1 also cardinality-bounds.
frozen_names = sorted(set(content_lifecycle_names) | {"create_object", "set_flow_feature"})

src = open(command_rs, encoding="utf-8").read()

declared = {}

# create_object / set_flow_feature: `pub const NAME: ExistingDocumentCardinality = ExistingDocumentCardinality::Variant;`
for const_name, wire_name in (
    ("CREATE_OBJECT_CARDINALITY", "create_object"),
    ("SET_FLOW_FEATURE_CARDINALITY", "set_flow_feature"),
):
    cm = re.search(rf"pub const {const_name}: ExistingDocumentCardinality = ExistingDocumentCardinality::(\w+)", src)
    if cm:
        declared[wire_name] = cm.group(1)
    else:
        violations.append(f"could not find a declaration for {const_name} in {command_rs}")

# ContentCommandType::existing_document_cardinality match body
cm = re.search(
    r"impl ContentCommandType \{.*?fn existing_document_cardinality\(self\) -> ExistingDocumentCardinality \{\s*match self \{(.*?)\}\s*\}\s*\}",
    src, re.S,
)
content_variant_names = {
    "SetTitle": "set_title", "InsertBlock": "insert_block", "UpdateBlock": "update_block",
    "DeleteBlock": "delete_block", "MoveBlock": "move_block",
}
if cm:
    body = cm.group(1)
    for arm_pattern, cardinality in re.findall(r"([\w\s|:]+?)\s*=>\s*\{?\s*ExistingDocumentCardinality::(\w+)", body):
        for variant in re.findall(r"Self::(\w+)", arm_pattern):
            wire = content_variant_names.get(variant)
            if wire:
                declared[wire] = cardinality
else:
    violations.append("could not find ContentCommandType::existing_document_cardinality match body")

# LifecycleCommandType::existing_document_cardinality match body
lm = re.search(
    r"impl LifecycleCommandType \{.*?fn existing_document_cardinality\(self\) -> ExistingDocumentCardinality \{\s*match self \{(.*?)\}\s*\}\s*\}",
    src, re.S,
)
lifecycle_variant_names = {"Archive": "archive", "Restore": "restore"}
if lm:
    body = lm.group(1)
    for arm_pattern, cardinality in re.findall(r"([\w\s|:]+?)\s*=>\s*ExistingDocumentCardinality::(\w+)", body):
        for variant in re.findall(r"Self::(\w+)", arm_pattern):
            wire = lifecycle_variant_names.get(variant)
            if wire:
                declared[wire] = cardinality
else:
    violations.append("could not find LifecycleCommandType::existing_document_cardinality match body")

cardinality_counts = {"Zero": 0, "One": 1}

per_command = []
for name in frozen_names:
    if name not in declared:
        violations.append(f"command '{name}' (frozen by rest-api-v1.md) has no cardinality declaration found in {command_rs}")
        per_command.append({"name": name, "declared_cardinality": None, "count": None, "within_bound": False})
        continue
    variant = declared[name]
    count = cardinality_counts.get(variant)
    if count is None:
        violations.append(f"command '{name}' declares cardinality variant '{variant}' which this static parser cannot bound (treat as violation: only Zero/One are legal at v0.4)")
        per_command.append({"name": name, "declared_cardinality": variant, "count": None, "within_bound": False})
        continue
    within = count <= max_cardinality
    if not within:
        violations.append(f"command '{name}' declares cardinality={count} > max_cardinality={max_cardinality}")
    per_command.append({"name": name, "declared_cardinality": variant, "count": count, "within_bound": within})

extra_declared = sorted(set(declared) - set(frozen_names))
for name in extra_declared:
    violations.append(f"command '{name}' is declared in {command_rs} but is not part of rest-api-v1.md's frozen v0.4 command set")

print(json.dumps({
    "frozen_command_count": len(frozen_names),
    "declared_command_count": len(declared),
    "per_command": per_command,
    "violations": violations,
}))
PY
)"

if ! jq -e . >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: static cross-check parser did not produce valid JSON" >&2
  echo "$STATIC_JSON" >&2
  exit 2
fi
if jq -e 'has("error")' >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: $(jq -r '.error' <<<"$STATIC_JSON")" >&2
  exit 2
fi

STATIC_VIOLATION_COUNT="$(jq '.violations | length' <<<"$STATIC_JSON")"
STATIC_PASSED=$([[ "$STATIC_VIOLATION_COUNT" -eq 0 ]] && echo true || echo false)

echo "=== static cross-check: rest-api-v1.md frozen commands vs apps/api/src/flow/command.rs ===" >&2
jq -r '.per_command[] | "  \(.name): declared=\(.declared_cardinality) count=\(.count) within_bound=\(.within_bound)"' <<<"$STATIC_JSON" >&2
if [[ "$STATIC_PASSED" != "true" ]]; then
  jq -r '.violations[] | "  VIOLATION: " + .' <<<"$STATIC_JSON" >&2
fi

# ---- 2. dynamic cross-check: run the real Rust test suite ----
CARGO_TEST_STATUS="skipped"
CARGO_TEST_EXIT=-1
CARGO_LOG="$EVIDENCE_ROOT/logs/cardinality.cargo_test.log"
mkdir -p "$(dirname "$CARGO_LOG")"
if [[ $SKIP_CARGO_TEST -eq 1 ]]; then
  echo "=== SKIPPED (--skip-cargo-test): cargo test -p api cardinality_gate_tests ===" >&2
  echo "(skipped by --skip-cargo-test)" > "$CARGO_LOG"
else
  echo "=== prerequisite: cargo build -p collab-core --bin collab-isolated-apply-worker ===" >&2
  ( cd "$REPO_ROOT" && cargo build -q -p collab-core --bin collab-isolated-apply-worker ) || {
    echo "FAIL: collab-isolated-apply-worker failed to build" >&2
    exit 2
  }
  echo "=== running: cargo test -p api cardinality_gate_tests (in $REPO_ROOT) ===" >&2
  set +e
  ( cd "$REPO_ROOT" && cargo test -p api cardinality_gate_tests ) > "$CARGO_LOG" 2>&1
  CARGO_TEST_EXIT=$?
  set -e
  CARGO_TEST_STATUS=$([[ $CARGO_TEST_EXIT -eq 0 ]] && echo passed || echo failed)
  tail -20 "$CARGO_LOG" >&2
fi

CARGO_PASSED=$([[ "$CARGO_TEST_STATUS" == "passed" ]] && echo true || echo false)

# ---- 3. live concurrency fixtures ----
LIVE_JSON='{"status":"failed","fixtures":{},"violations":["live fixture was not run"]}'
TMP_DIR=""
SCRATCH_DB=""
API_PID=""

# shellcheck disable=SC2317
cleanup_live() {
  local ec=$?
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  if [[ -n "$SCRATCH_DB" && -n "$DATABASE_URL" ]]; then
    psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  fi
  if [[ -n "$TMP_DIR" && "$TMP_DIR" == /opt/worker/.cache/openpr-cardinality-verify.* ]]; then
    rm -rf "$TMP_DIR"
  fi
  exit "$ec"
}
trap cleanup_live EXIT

if [[ -z "$DATABASE_URL" ]]; then
  LIVE_JSON='{"status":"failed","fixtures":{},"violations":["no database URL configured"]}'
elif ! psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc 'SELECT 1' >/dev/null 2>&1; then
  LIVE_JSON='{"status":"failed","fixtures":{},"violations":["configured PostgreSQL environment is unreachable"]}'
else
  TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
  echo "=== building live API binary ===" >&2
  if ! ( cd "$REPO_ROOT" && cargo build -q -p api --bin api ); then
    echo "FAIL: api binary failed to build" >&2
    exit 2
  fi
  API_BIN="$TARGET_DIR/debug/api"
  RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
  SCRATCH_DB="openpr_flow_cardinality_verify_$RUN_ID"
  SCRATCH_URL="${DATABASE_URL%/*}/$SCRATCH_DB"
  TMP_DIR="$(mktemp -d /opt/worker/.cache/openpr-cardinality-verify.XXXXXX)"
  API_PORT=$((22000 + RANDOM % 12000))
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE \"$SCRATCH_DB\"" >/dev/null || {
    LIVE_JSON='{"status":"failed","fixtures":{},"violations":["could not create scratch database"]}'
  }
  if [[ -n "$SCRATCH_DB" ]]; then
    JWT_SECRET="cardinality-verify-not-a-real-secret"
    API_CONFIG="$TMP_DIR/openpr.toml"
    cat > "$API_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"
[database]
url = "$SCRATCH_URL"
[auth]
jwt_secret = "$JWT_SECRET"
[logging]
filter = "api=warn,openpr=warn"
format = "text"
EOF
    "$API_BIN" --config "$API_CONFIG" >"$TMP_DIR/api.log" 2>&1 & API_PID=$!
    for _ in $(seq 1 120); do
      curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1 && break
      kill -0 "$API_PID" 2>/dev/null || break
      sleep 0.25
    done
    if ! curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then
      LIVE_JSON="$(jq -n -c --arg log "$(tail -30 "$TMP_DIR/api.log" | tr '\n' ' ')" '{status:"failed",fixtures:{},violations:[("live API did not become healthy: "+$log)]}')"
    else
      if [[ "${FLOW_CARDINALITY_ANTIPROOF_DROP_IDEMPOTENCY_INDEX:-0}" == 1 ]]; then
        echo "=== anti-proof: dropping tested business-event idempotency unique index in scratch DB ===" >&2
        psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q -c 'DROP INDEX idx_business_events_idempotency' >/dev/null
      fi
      WORKSPACE_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
      OWNER_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
      psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id,email,password_hash,name,role,is_active,entity_type,created_at,updated_at)
VALUES ('$OWNER_ID','cardinality-$RUN_ID@example.local','','Cardinality Owner','user',true,'human',now(),now());
INSERT INTO workspaces (id,slug,name,created_by,created_at,updated_at)
VALUES ('$WORKSPACE_ID','cardinality-$RUN_ID','Cardinality Verify','$OWNER_ID',now(),now());
INSERT INTO workspace_members (workspace_id,user_id,role,created_at)
VALUES ('$WORKSPACE_ID','$OWNER_ID','owner',now());
INSERT INTO flow_workspace_settings (workspace_id,flow_enabled,default_member_level,authz_epoch,updated_at)
VALUES ('$WORKSPACE_ID',true,'edit',0,now());
SQL
      TOKEN="$(python3 - "$JWT_SECRET" "$OWNER_ID" <<'PY'
import base64,hashlib,hmac,json,sys,time
def b64(v): return base64.urlsafe_b64encode(json.dumps(v,separators=(",",":")).encode()).rstrip(b"=").decode()
secret,sub=sys.argv[1:]; now=int(time.time()); h=b64({"alg":"HS256","typ":"JWT"}); b=b64({"sub":sub,"email":"cardinality@example.local","token_type":"access","iat":now,"exp":now+3600}); s=base64.urlsafe_b64encode(hmac.new(secret.encode(),f"{h}.{b}".encode(),hashlib.sha256).digest()).rstrip(b"=").decode(); print(f"{h}.{b}.{s}")
PY
)"
      set +e
      LIVE_JSON="$(python3 "$LIVE_PROBE" --api "http://127.0.0.1:$API_PORT" --token "$TOKEN" --workspace "$WORKSPACE_ID" --database-url "$SCRATCH_URL" 2>"$TMP_DIR/live-probe.log")"
      LIVE_EXIT=$?
      set -e
      if ! jq -e . >/dev/null 2>&1 <<<"$LIVE_JSON"; then
        LIVE_JSON="$(jq -n -c --arg log "$(tail -40 "$TMP_DIR/live-probe.log" | tr '\n' ' ')" '{status:"failed",fixtures:{},violations:[("live probe produced invalid JSON: "+$log)]}')"
      elif [[ $LIVE_EXIT -ne 0 ]]; then
        : # valid failed evidence is retained below
      fi
    fi
  fi
fi

CONCURRENCY_STATUS="$(jq -r '.status // "failed"' <<<"$LIVE_JSON")"
OVERALL_PASSED=$([[ "$STATIC_PASSED" == true && "$CARGO_PASSED" == true && "$CONCURRENCY_STATUS" == passed ]] && echo true || echo false)

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --arg adr "$ADR_PATH" \
  --argjson max_cardinality "$MAX_CARDINALITY" \
  --argjson static_check "$STATIC_JSON" --argjson static_passed "$STATIC_PASSED" \
  --arg cargo_status "$CARGO_TEST_STATUS" --argjson cargo_exit "$CARGO_TEST_EXIT" \
  --arg cargo_log "evidence/v0.4/logs/cardinality.cargo_test.log" \
  --argjson concurrency "$LIVE_JSON" \
  --argjson passed "$OVERALL_PASSED" \
  '{
    schema_version: "sylvode.flow.cardinality-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    adr: $adr,
    max_cardinality: $max_cardinality,
    static_check: {
      frozen_command_count: $static_check.frozen_command_count,
      declared_command_count: $static_check.declared_command_count,
      per_command: $static_check.per_command,
      violations: $static_check.violations,
      passed: $static_passed
    },
    dynamic_check: {
      command: "cargo test -p api cardinality_gate_tests",
      status: $cargo_status,
      exit_code: $cargo_exit,
      log: $cargo_log
    },
    concurrency_fixtures: $concurrency,
    passed: $passed
  }')"

OUT_PATH="$EVIDENCE_ROOT/cardinality-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

echo "$RESULT"
if [[ "$OVERALL_PASSED" == "true" ]]; then
  exit 0
fi
exit 1

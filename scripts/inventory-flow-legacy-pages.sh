#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 legacy `pages` table inventory collector.
#
# Contract: /opt/working/sylvode-flow/contracts/legacy-pages-import-v1.md
# "无条件 inventory evidence" + ADR-0003 "进入 v0.4 前的门禁" item 1.
#
# Collects row_count / workspace_distribution / max_body_md_bytes for the
# read-only legacy `pages` table in exactly three environments
# (development, test, target_deployment) using a read-only transaction per
# environment, and atomically writes
# evidence/v0.4/legacy-pages-inventory.json.
#
# This script NEVER treats a missing/unreachable environment as zero rows:
# per the contract ("缺环境、采集失败或 schema drift 均使 gate 失败，不能按
# 零行处理"), any environment whose database URL is not configured or not
# reachable is a hard failure (exit 1), not a silently-zeroed row. Operational
# failures still atomically write a `collection_status=failed` artifact so the
# report can preserve the failed check instead of degrading into structural
# exit 2. Failed environments never carry row_count or contribute to total_rows.
#
# Environment DSNs (never written to the output; only sha256 identity
# fingerprints are recorded):
#   OPENPR_FLOW_INVENTORY_DEVELOPMENT_DATABASE_URL
#   OPENPR_FLOW_INVENTORY_TEST_DATABASE_URL       (falls back to
#                                                   OPENPR_TEST_DATABASE_URL)
#   OPENPR_FLOW_INVENTORY_TARGET_DEPLOYMENT_DATABASE_URL
#
# Exit codes: 0 = all three environments collected and evidence written,
# 1 = one or more environments missing/unreachable/schema-drifted (failed
# evidence written), 2 = usage/tool error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
REPO_ROOT="$ROOT_DIR"
ENVIRONMENTS_ARG="development,test,target_deployment"
JSON_MODE=0
EXECUTOR_NAME="${OPENPR_FLOW_INVENTORY_EXECUTOR_NAME:-}"
EXECUTOR_ROLE="${OPENPR_FLOW_INVENTORY_EXECUTOR_ROLE:-automation}"

usage() {
  cat <<'EOF'
Usage: scripts/inventory-flow-legacy-pages.sh --environments development,test,target_deployment --json [OPTIONS]

Collects a read-only inventory of the legacy `pages` table
(id,workspace_id,title,body_md,created_by,created_at,updated_at) across
exactly the three required environments and writes
evidence/v0.4/legacy-pages-inventory.json (schema:
docs/schemas/sylvode-flow-legacy-pages-inventory-v1.schema.json).

The set of environments must be exactly {development, test,
target_deployment}; anything else is a usage error (exit 2). Each
environment's Postgres DSN is read from an environment variable (never
written to the output, only its sha256 identity fingerprint is):

  OPENPR_FLOW_INVENTORY_DEVELOPMENT_DATABASE_URL
  OPENPR_FLOW_INVENTORY_TEST_DATABASE_URL        (falls back to
                                                    OPENPR_TEST_DATABASE_URL)
  OPENPR_FLOW_INVENTORY_TARGET_DEPLOYMENT_DATABASE_URL

Options:
  --environments LIST     Comma-separated kinds, must be exactly
                          development,test,target_deployment (order
                          does not matter). Default: all three.
  --evidence-root DIR     Where legacy-pages-inventory.json is written.
                          Default: /opt/working/sylvode-flow/evidence/v0.4
  --repo-root DIR         Repository whose HEAD becomes source_head.
                          Default: this checkout.
  --executor-name NAME    Recorded as executor.name. Default:
                          $OPENPR_FLOW_INVENTORY_EXECUTOR_NAME, or the
                          output of `whoami` if unset.
  --executor-role ROLE    Recorded as executor.role. Default: "automation".
  --json                  No-op flag kept for CLI-contract compatibility
                          with the frozen required_commands entry (output
                          is always JSON on stdout for the written file's
                          path plus a human summary on stderr).
  -h, --help              Show this help and exit 0.

Exit codes: 0 all three environments collected and complete evidence written,
1 an environment is missing/unreachable/schema-drifted and failed evidence was
written, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --environments) ENVIRONMENTS_ARG="${2:?--environments requires a value}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --executor-name) EXECUTOR_NAME="${2:?--executor-name requires a value}"; shift 2 ;;
    --executor-role) EXECUTOR_ROLE="${2:?--executor-role requires a value}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
: "$JSON_MODE"

for tool in jq sha256sum git psql; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    echo "Fix: sudo apt-get install -y $tool (or postgresql-client for psql)" >&2
    exit 2
  fi
done

if [[ -z "$EXECUTOR_NAME" ]]; then
  EXECUTOR_NAME="$(whoami 2>/dev/null || echo unknown)"
fi

IFS=',' read -r -a ENV_LIST <<<"$ENVIRONMENTS_ARG"
declare -A SEEN_KIND=()
for k in "${ENV_LIST[@]}"; do
  case "$k" in
    development|test|target_deployment) SEEN_KIND["$k"]=1 ;;
    *) echo "FAIL: unknown environment kind: $k (valid: development, test, target_deployment)" >&2; exit 2 ;;
  esac
done
if [[ ${#SEEN_KIND[@]} -ne 3 ]]; then
  echo "FAIL: --environments must name exactly development,test,target_deployment (got: $ENVIRONMENTS_ARG)" >&2
  echo "Contract: legacy-pages-import-v1.md requires all three kinds in every inventory run." >&2
  exit 2
fi

if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

mkdir -p "$EVIDENCE_ROOT"

sha256_str() { printf '%s' "$1" | sha256sum | awk '{print $1}'; }

# The exact, frozen read-only query used for every environment (its hash is
# recorded so drift in the query itself is visible in the evidence).
READ_QUERY='SELECT id, workspace_id, octet_length(body_md) AS body_bytes FROM pages'
QUERY_SHA256="$(sha256_str "$READ_QUERY")"

dsn_for_kind() {
  case "$1" in
    development) printf '%s' "${OPENPR_FLOW_INVENTORY_DEVELOPMENT_DATABASE_URL:-}" ;;
    test) printf '%s' "${OPENPR_FLOW_INVENTORY_TEST_DATABASE_URL:-${OPENPR_TEST_DATABASE_URL:-}}" ;;
    target_deployment) printf '%s' "${OPENPR_FLOW_INVENTORY_TARGET_DEPLOYMENT_DATABASE_URL:-}" ;;
  esac
}

env_var_name_for_kind() {
  case "$1" in
    development) printf 'OPENPR_FLOW_INVENTORY_DEVELOPMENT_DATABASE_URL' ;;
    test) printf 'OPENPR_FLOW_INVENTORY_TEST_DATABASE_URL (or OPENPR_TEST_DATABASE_URL)' ;;
    target_deployment) printf 'OPENPR_FLOW_INVENTORY_TARGET_DEPLOYMENT_DATABASE_URL' ;;
  esac
}

# identity fingerprint: sha256 of "kind|user@host:port/dbname" -- never the
# password, never the full DSN.
identity_sha256_for_dsn() {
  local dsn="$1" kind="$2" scrubbed
  scrubbed="$(python3 - "$dsn" <<'PY'
import sys
from urllib.parse import urlsplit
u = urlsplit(sys.argv[1])
netloc = u.hostname or ""
if u.port:
    netloc += f":{u.port}"
user = u.username or ""
print(f"{user}@{netloc}{u.path}")
PY
)"
  sha256_str "${kind}|${scrubbed}"
}

FAILED=0
ENVIRONMENTS_JSON="[]"
TOTAL_ROWS=0

# Canonical schema fingerprint for `pages`: information_schema column
# name/type/nullable tuples in ordinal order. Any drift from the frozen
# ADR-0003 shape ("id,workspace_id,title,body_md,created_by,created_at,
# updated_at") changes this hash, which the verifier compares across
# environments.
SCHEMA_QUERY="SELECT string_agg(column_name || ':' || data_type || ':' || is_nullable, ',' ORDER BY ordinal_position) FROM information_schema.columns WHERE table_schema='public' AND table_name='pages'"

collect_one() {
  local kind="$1" dsn="$2"
  COLLECT_FAILURE_REASON_CODE=""
  COLLECT_FAILURE_MESSAGE=""
  echo "=== collecting legacy pages inventory: $kind ===" >&2

  local errfile
  errfile="$(mktemp)"
  if ! psql "$dsn" -v ON_ERROR_STOP=1 -Atc "SELECT 1" >/dev/null 2>"$errfile"; then
    echo "FAIL: environment '$kind' is not reachable" >&2
    sed 's/^/  | /' "$errfile" >&2
    COLLECT_FAILURE_REASON_CODE="unreachable"
    COLLECT_FAILURE_MESSAGE="database connection failed"
    rm -f "$errfile"
    return 1
  fi
  rm -f "$errfile"

  local schema_fp
  schema_fp="$(psql "$dsn" -v ON_ERROR_STOP=1 -Atc "$SCHEMA_QUERY" 2>/dev/null || true)"
  if [[ -z "$schema_fp" ]]; then
    echo "FAIL: environment '$kind' has no readable public.pages table (schema drift or missing table)" >&2
    COLLECT_FAILURE_REASON_CODE="pages_schema_missing"
    COLLECT_FAILURE_MESSAGE="public.pages is absent or unreadable"
    return 1
  fi
  local expected_schema_fp="id:uuid:NO,workspace_id:uuid:NO,title:text:NO,body_md:text:NO,created_by:uuid:YES,created_at:timestamp with time zone:NO,updated_at:timestamp with time zone:NO"
  if [[ "$schema_fp" != "$expected_schema_fp" ]]; then
    echo "FAIL: environment '$kind' public.pages schema drifted from ADR-0003's frozen shape" >&2
    echo "  expected: $expected_schema_fp" >&2
    echo "  actual:   $schema_fp" >&2
    COLLECT_FAILURE_REASON_CODE="pages_schema_drift"
    COLLECT_FAILURE_MESSAGE="public.pages differs from the frozen ADR-0003 shape"
    return 1
  fi
  local source_schema_sha256
  source_schema_sha256="$(sha256_str "$schema_fp")"

  # Single read-only transaction: --single-transaction wraps the whole
  # invocation in BEGIN/COMMIT at the psql level (never printed), and
  # `-q` additionally suppresses the "SET"/"BEGIN"/"COMMIT" command-tag
  # lines psql would otherwise interleave with -Atc output -- without
  # both of these, those tag lines land in raw_csv as phantom rows with
  # an empty workspace_id field, silently inflating row_count.
  local raw_csv
  raw_csv="$(psql "$dsn" -q --single-transaction -v ON_ERROR_STOP=1 -Atc "
SET TRANSACTION READ ONLY;
$READ_QUERY;
" -F$'\t' 2>"$errfile")" || {
    echo "FAIL: environment '$kind' read-only collection query failed" >&2
    sed 's/^/  | /' "$errfile" >&2
    COLLECT_FAILURE_REASON_CODE="read_query_failed"
    COLLECT_FAILURE_MESSAGE="frozen read-only inventory query failed"
    rm -f "$errfile"
    return 1
  }
  rm -f "$errfile"

  local row_count max_body_bytes dist_json
  # raw_csv rows: id \t workspace_id \t body_bytes ; empty output = 0 rows
  if [[ -z "$raw_csv" ]]; then
    row_count=0
    max_body_bytes=0
    dist_json="[]"
  else
    row_count="$(printf '%s\n' "$raw_csv" | grep -c . || true)"
    max_body_bytes="$(printf '%s\n' "$raw_csv" | awk -F'\t' '{print $3}' | sort -rn | head -1)"
    dist_json="$(printf '%s\n' "$raw_csv" | awk -F'\t' '{print $2}' | sort | uniq -c | awk '{print $2","$1}' | \
      while IFS=',' read -r wsid cnt; do
        wsid_hash="$(sha256_str "$wsid")"
        jq -n --arg h "$wsid_hash" --argjson c "$cnt" '{workspace_id_sha256:$h, row_count:$c}'
      done | jq -s '.')"
  fi

  local identity_hash collected_at
  identity_hash="$(identity_sha256_for_dsn "$dsn" "$kind")"
  collected_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

  ENV_ENTRY="$(jq -n \
    --arg kind "$kind" --arg identity "$identity_hash" --arg collected_at "$collected_at" \
    --argjson row_count "$row_count" --argjson dist "$dist_json" \
    --argjson max_bytes "${max_body_bytes:-0}" --arg query_sha "$QUERY_SHA256" \
    --arg schema_sha "$source_schema_sha256" \
    '{kind:$kind, status:"collected", identity_sha256:$identity, collected_at:$collected_at, row_count:$row_count, workspace_distribution:$dist, max_body_md_bytes:$max_bytes, query_sha256:$query_sha, source_schema_sha256:$schema_sha}')"

  ENVIRONMENTS_JSON="$(jq -c --argjson e "$ENV_ENTRY" '. + [$e]' <<<"$ENVIRONMENTS_JSON")"
  TOTAL_ROWS=$((TOTAL_ROWS + row_count))
  echo "  [ok] $kind: row_count=$row_count max_body_md_bytes=${max_body_bytes:-0}" >&2
  return 0
}

MISSING_OR_FAILED=()
for kind in development test target_deployment; do
  dsn="$(dsn_for_kind "$kind")"
  if [[ -z "$dsn" ]]; then
    echo "FAIL: no DSN configured for environment '$kind' (set $(env_var_name_for_kind "$kind"))" >&2
    ENV_ENTRY="$(jq -n --arg kind "$kind" --arg collected_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      '{kind:$kind,status:"failed",collected_at:$collected_at,reason_code:"missing_dsn",message:"required environment DSN is not configured"}')"
    ENVIRONMENTS_JSON="$(jq -c --argjson e "$ENV_ENTRY" '. + [$e]' <<<"$ENVIRONMENTS_JSON")"
    MISSING_OR_FAILED+=("$kind")
    FAILED=1
    continue
  fi
  if ! collect_one "$kind" "$dsn"; then
    ENV_ENTRY="$(jq -n --arg kind "$kind" --arg collected_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      --arg reason_code "${COLLECT_FAILURE_REASON_CODE:-collection_failed}" \
      --arg message "${COLLECT_FAILURE_MESSAGE:-environment collection failed}" \
      '{kind:$kind,status:"failed",collected_at:$collected_at,reason_code:$reason_code,message:$message}')"
    ENVIRONMENTS_JSON="$(jq -c --argjson e "$ENV_ENTRY" '. + [$e]' <<<"$ENVIRONMENTS_JSON")"
    MISSING_OR_FAILED+=("$kind")
    FAILED=1
  fi
done

jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" \
  --arg name "$EXECUTOR_NAME" --arg role "$EXECUTOR_ROLE" \
  --argjson environments "$ENVIRONMENTS_JSON" --argjson total_rows "$TOTAL_ROWS" --argjson failed "$FAILED" \
  '{
    schema_version: "sylvode.flow.legacy-pages-inventory.v1",
    source_head: $head,
    generated_at: $generated_at,
    executor: {name:$name, role:$role},
    collection_status: (if $failed == 0 then "complete" else "failed" end),
    environments: $environments,
    total_rows: (if $failed == 0 then $total_rows else null end)
  }' > "$EVIDENCE_ROOT/legacy-pages-inventory.json.tmp"
sync "$EVIDENCE_ROOT/legacy-pages-inventory.json.tmp" 2>/dev/null || true
mv -f "$EVIDENCE_ROOT/legacy-pages-inventory.json.tmp" "$EVIDENCE_ROOT/legacy-pages-inventory.json"

if [[ $FAILED -ne 0 ]]; then
  echo "" >&2
  echo "INVENTORY: FAIL -- environment(s) not collected: ${MISSING_OR_FAILED[*]}" >&2
  echo "Per legacy-pages-import-v1.md, failures were not treated as zero rows." >&2
  echo "INVENTORY: wrote failed evidence $EVIDENCE_ROOT/legacy-pages-inventory.json (total_rows=null)" >&2
  cat "$EVIDENCE_ROOT/legacy-pages-inventory.json"
  exit 1
fi

echo "INVENTORY: wrote $EVIDENCE_ROOT/legacy-pages-inventory.json (collection_status=complete total_rows=$TOTAL_ROWS)" >&2
cat "$EVIDENCE_ROOT/legacy-pages-inventory.json"
exit 0

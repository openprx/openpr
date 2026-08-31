#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 legacy-pages inventory verifier.
#
# Contract: /opt/working/sylvode-flow/contracts/legacy-pages-import-v1.md
# ("无条件 inventory evidence", "零行与非零分支") and gate-commands.md's
# v0.4 "legacy_pages_entry_verify" required_commands entry.
#
# This does NOT re-collect data from any database. It only recomputes
# invariants from evidence/v0.4/legacy-pages-inventory.json that JSON
# Schema alone cannot express:
#   - exactly one row per {development,test,target_deployment}
#   - total_rows == sum(environment.row_count)
#   - per-environment workspace_distribution sums to that environment's
#     row_count, and no distribution entry has row_count <= 0
#   - zero-row environments have an empty distribution and
#     max_body_md_bytes == 0
#   - the recorded query_sha256/source_schema_sha256 are identical across
#     all three environments (the same frozen query/schema shape was used
#     everywhere -- environments computing different hashes for what
#     should be the same frozen query is itself a drift signal)
#
# It also determines and records the branch (zero vs nonzero) that
# legacy-pages-import-v1.md's "零行与非零分支" require downstream conditional
# checks to honor: zero total_rows -> conditional importer checks are
# permitted to be recorded as passed/not_required_zero_inventory; nonzero
# total_rows -> the importer artifact (evidence/v0.4/legacy-pages-import-result.json)
# becomes mandatory and this script will fail if it is absent.
#
# Exit codes: 0 = structurally valid (branch recorded in the printed
# summary), 1 = an invariant is violated / (nonzero branch without the
# importer artifact), 2 = usage/tool/evidence malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Shared contract/evidence path resolution (absolute -> as-is;
# relative-to-CWD -> as-is; otherwise resolved against --contracts-root;
# unresolvable -> FAIL naming both attempted paths).
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
JSON_MODE=0
INVENTORY_PATH=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-legacy-pages-v0.4.sh [INVENTORY_JSON] --json [OPTIONS]

Recomputes cross-field invariants of a legacy-pages-inventory.json evidence
file that JSON Schema alone cannot express (see script header), and
determines whether the zero-row or nonzero-row branch of
legacy-pages-import-v1.md applies -- failing if the nonzero branch applies
but evidence/v0.4/legacy-pages-import-result.json is missing.

Arguments:
  INVENTORY_JSON     Path to legacy-pages-inventory.json. Default:
                      <evidence-root>/legacy-pages-inventory.json
                      A relative path is resolved against the current
                      directory first, then against --contracts-root (so
                      v0.4-gate.yaml's literal
                      "evidence/v0.4/legacy-pages-inventory.json" works
                      when run from the source repository).

Options:
  --contracts-root DIR  Root the relative INVENTORY_JSON falls back to.
                        Default: /opt/working/sylvode-flow
  --evidence-root DIR   Used for the default INVENTORY_JSON path and to
                        look for legacy-pages-import-result.json in the
                        nonzero branch. Default:
                        /opt/working/sylvode-flow/evidence/v0.4
  --json                Print the verification summary as JSON on stdout.
                        Required for CLI-contract compatibility.
  -h, --help            Show this help and exit 0.

Exit codes: 0 valid, 1 invariant violated / required artifact missing,
2 usage/tool/evidence malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [[ -n "$INVENTORY_PATH" ]]; then
        echo "Unexpected argument: $1" >&2; usage >&2; exit 2
      fi
      INVENTORY_PATH="$1"; shift ;;
  esac
done

if [[ -z "$INVENTORY_PATH" ]]; then
  INVENTORY_PATH="$EVIDENCE_ROOT/legacy-pages-inventory.json"
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "FAIL: missing required command: jq" >&2
  echo "Fix: sudo apt-get install -y jq" >&2
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
if ! INVENTORY_PATH="$(flow_resolve_contract_path INVENTORY_JSON "$INVENTORY_PATH" "$CONTRACTS_ROOT")"; then
  exit 2
fi
if ! jq empty "$INVENTORY_PATH" >/dev/null 2>&1; then
  echo "FAIL: not valid JSON: $INVENTORY_PATH" >&2
  exit 2
fi
if [[ "$(jq -r '.schema_version // empty' "$INVENTORY_PATH")" != "sylvode.flow.legacy-pages-inventory.v1" ]]; then
  echo "FAIL: $INVENTORY_PATH is not a sylvode.flow.legacy-pages-inventory.v1 document" >&2
  exit 2
fi

# ---- structural pre-flight: verify the document's SHAPE before any jq call that
# assumes it (`.environments[]` iteration, `.environments | length`, `add` over a
# projected field) would otherwise crash on a missing/wrong-typed `environments`
# or `total_rows` field and leak a raw jq error under `set -e`. Any problem here
# is "evidence malformed" (exit 2), never folded into VIOLATIONS (exit 1).
STRUCT_ERRORS=()
COLLECTION_STATUS="$(jq -r 'if has("collection_status") then .collection_status else "missing" end' "$INVENTORY_PATH")"
if [[ "$COLLECTION_STATUS" != "complete" && "$COLLECTION_STATUS" != "failed" ]]; then
  STRUCT_ERRORS+=("top-level key 'collection_status' must be complete|failed, found $COLLECTION_STATUS")
fi
ENV_TYPE="$(jq -r 'if has("environments") then (.environments | type) else "missing" end' "$INVENTORY_PATH")"
if [[ "$ENV_TYPE" == "missing" ]]; then
  STRUCT_ERRORS+=("missing required top-level key: environments")
elif [[ "$ENV_TYPE" != "array" ]]; then
  STRUCT_ERRORS+=("top-level key 'environments' must be a JSON array, found $ENV_TYPE")
fi
TOTAL_ROWS_TYPE="$(jq -r 'if has("total_rows") then (.total_rows | type) else "missing" end' "$INVENTORY_PATH")"
if [[ "$TOTAL_ROWS_TYPE" == "missing" ]]; then
  STRUCT_ERRORS+=("missing required top-level key: total_rows")
elif [[ "$COLLECTION_STATUS" == "complete" && "$TOTAL_ROWS_TYPE" != "number" ]]; then
  STRUCT_ERRORS+=("complete inventory total_rows must be a JSON number, found $TOTAL_ROWS_TYPE")
elif [[ "$COLLECTION_STATUS" == "failed" && "$TOTAL_ROWS_TYPE" != "null" ]]; then
  STRUCT_ERRORS+=("failed inventory total_rows must be null, found $TOTAL_ROWS_TYPE")
fi
if [[ ${#STRUCT_ERRORS[@]} -eq 0 && "$ENV_TYPE" == "array" ]]; then
  NON_OBJECT_ENTRIES="$(jq '[.environments[] | select(type != "object")] | length' "$INVENTORY_PATH")"
  if [[ "$NON_OBJECT_ENTRIES" -gt 0 ]]; then
    STRUCT_ERRORS+=("environments[] has $NON_OBJECT_ENTRIES entries that are not JSON objects")
  fi
fi
if [[ ${#STRUCT_ERRORS[@]} -gt 0 ]]; then
  echo "FAIL: $INVENTORY_PATH is structurally malformed -- cannot check its invariants:" >&2
  for e in "${STRUCT_ERRORS[@]}"; do
    echo "  - $e" >&2
  done
  exit 2
fi

VIOLATIONS=()

# --- exactly one row per required kind ---
for kind in development test target_deployment; do
  count="$(jq --arg k "$kind" '[.environments[] | select(.kind==$k)] | length' "$INVENTORY_PATH")"
  if [[ "$count" -ne 1 ]]; then
    VIOLATIONS+=("environment kind '$kind' appears $count times (must be exactly 1)")
  fi
done
env_count="$(jq '.environments | length' "$INVENTORY_PATH")"
if [[ "$env_count" -ne 3 ]]; then
  VIOLATIONS+=("environments array has $env_count entries (must be exactly 3)")
fi

# A collector operational failure is valid failure evidence, not malformed
# evidence and never the zero-row branch. Keep the required command at exit 1
# while giving report/verify/gate a durable artifact to checksum and diagnose.
if [[ "$COLLECTION_STATUS" == "failed" ]]; then
  FAILED_ENV_COUNT="$(jq '[.environments[] | select(.status=="failed")] | length' "$INVENTORY_PATH")"
  if [[ "$FAILED_ENV_COUNT" -eq 0 ]]; then
    echo "FAIL: failed inventory contains no environment with status=failed" >&2
    exit 2
  fi
  MALFORMED_FAILED_COUNT="$(jq '[.environments[] | select(.status=="failed") | select((.reason_code|type)!="string" or (.reason_code|length)==0 or (.message|type)!="string" or (.message|length)==0 or has("row_count") or has("workspace_distribution") or has("max_body_md_bytes"))] | length' "$INVENTORY_PATH")"
  if [[ "$MALFORMED_FAILED_COUNT" -ne 0 ]]; then
    echo "FAIL: failed inventory has $MALFORMED_FAILED_COUNT malformed failed environment entries" >&2
    exit 2
  fi
  while IFS=$'\t' read -r kind reason_code message; do
    VIOLATIONS+=("environment '$kind' collection failed ($reason_code): $message")
  done < <(jq -r '.environments[] | select(.status=="failed") | [.kind,.reason_code,.message] | @tsv' "$INVENTORY_PATH")
  VIOLATIONS_JSON="$(printf '%s\n' "${VIOLATIONS[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"
  jq -n \
    --arg inventory_path "$INVENTORY_PATH" \
    --arg branch "collection_failed" \
    --argjson violations "$VIOLATIONS_JSON" \
    '{inventory_path:$inventory_path,total_rows:null,branch:$branch,violations:$violations,passed:false}' | jq .
  exit 1
fi

NON_COLLECTED_COUNT="$(jq '[.environments[] | select(.status!="collected")] | length' "$INVENTORY_PATH")"
if [[ "$NON_COLLECTED_COUNT" -ne 0 ]]; then
  VIOLATIONS+=("complete inventory has $NON_COLLECTED_COUNT environment entries not marked collected")
fi

# --- total_rows == sum(environment.row_count) ---
declared_total="$(jq '.total_rows' "$INVENTORY_PATH")"
computed_total="$(jq '[.environments[].row_count? // 0] | add // 0' "$INVENTORY_PATH")"
if [[ "$declared_total" != "$computed_total" ]]; then
  VIOLATIONS+=("total_rows=$declared_total does not equal sum(environments[].row_count)=$computed_total")
fi

# --- per-environment: distribution sums to row_count; entries are positive;
#     zero-row environments have empty distribution and max_body_md_bytes=0 ---
# `.workspace_distribution` is read defensively (`// [] | ... ?`) -- a per-entry
# field that is absent or the wrong type becomes an explicit VIOLATION below
# (dist_sum/dist_len won't match expectations), never a crash.
while IFS=$'\t' read -r kind row_count dist_sum dist_len max_bytes; do
  if [[ "$dist_sum" != "$row_count" ]]; then
    VIOLATIONS+=("environment '$kind': workspace_distribution sums to $dist_sum but row_count=$row_count")
  fi
  if [[ "$row_count" -eq 0 ]]; then
    if [[ "$dist_len" -ne 0 ]]; then
      VIOLATIONS+=("environment '$kind': row_count=0 but workspace_distribution has $dist_len entries")
    fi
    if [[ "$max_bytes" != "0" ]]; then
      VIOLATIONS+=("environment '$kind': row_count=0 but max_body_md_bytes=$max_bytes (expected 0)")
    fi
  fi
done < <(jq -r '.environments[] | [(.kind? // "MISSING_KIND"), (.row_count? // 0), (([(.workspace_distribution // [])[]?.row_count]) | add // 0), ((.workspace_distribution // []) | if (type=="array") then length else -1 end), (.max_body_md_bytes? // -1)] | @tsv' "$INVENTORY_PATH")

# any single distribution entry with row_count <= 0
non_positive_dist="$(jq '[.environments[].workspace_distribution[] | select(.row_count <= 0)] | length' "$INVENTORY_PATH")"
if [[ "$non_positive_dist" -ne 0 ]]; then
  VIOLATIONS+=("$non_positive_dist workspace_distribution entries have row_count <= 0")
fi

# --- query_sha256 / source_schema_sha256 identical across all three
#     environments (same frozen query/schema everywhere) ---
distinct_query_hashes="$(jq '[.environments[].query_sha256] | unique | length' "$INVENTORY_PATH")"
if [[ "$distinct_query_hashes" -gt 1 ]]; then
  VIOLATIONS+=("environments recorded $distinct_query_hashes distinct query_sha256 values (must be 1: same frozen query everywhere)")
fi
distinct_schema_hashes="$(jq '[.environments[].source_schema_sha256] | unique | length' "$INVENTORY_PATH")"
if [[ "$distinct_schema_hashes" -gt 1 ]]; then
  VIOLATIONS+=("environments recorded $distinct_schema_hashes distinct source_schema_sha256 values (public.pages schema drifted between environments)")
fi

# --- determine branch ---
if [[ "$declared_total" -eq 0 ]]; then
  BRANCH="zero_inventory"
else
  BRANCH="nonzero_importer_required"
fi

IMPORT_RESULT_PATH="$EVIDENCE_ROOT/legacy-pages-import-result.json"
if [[ "$BRANCH" == "nonzero_importer_required" ]]; then
  if [[ ! -f "$IMPORT_RESULT_PATH" ]]; then
    VIOLATIONS+=("total_rows=$declared_total > 0 (nonzero branch) but required artifact is missing: $IMPORT_RESULT_PATH")
  elif ! jq empty "$IMPORT_RESULT_PATH" >/dev/null 2>&1; then
    VIOLATIONS+=("$IMPORT_RESULT_PATH exists but is not valid JSON")
  elif [[ "$(jq -r '.schema_version // empty' "$IMPORT_RESULT_PATH")" != "sylvode.flow.legacy-pages-import-result.v1" ]]; then
    VIOLATIONS+=("$IMPORT_RESULT_PATH is not a sylvode.flow.legacy-pages-import-result.v1 document")
  elif [[ "$(jq -r '.branch // empty' "$IMPORT_RESULT_PATH")" != "nonzero_importer_required" ]]; then
    VIOLATIONS+=("$IMPORT_RESULT_PATH .branch is not 'nonzero_importer_required'")
  else
    inv_sha_recorded="$(jq -r '.inventory_sha256 // empty' "$IMPORT_RESULT_PATH")"
    inv_sha_actual="$(sha256sum "$INVENTORY_PATH" | awk '{print $1}')"
    if [[ "$inv_sha_recorded" != "$inv_sha_actual" ]]; then
      VIOLATIONS+=("$IMPORT_RESULT_PATH .inventory_sha256=$inv_sha_recorded does not match actual sha256 of $INVENTORY_PATH=$inv_sha_actual")
    fi
  fi
fi

PASSED=true
if [[ ${#VIOLATIONS[@]} -gt 0 ]]; then
  PASSED=false
fi

VIOLATIONS_JSON="$(printf '%s\n' "${VIOLATIONS[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"

jq -n \
  --arg inventory_path "$INVENTORY_PATH" \
  --argjson total_rows "$declared_total" \
  --arg branch "$BRANCH" \
  --argjson violations "$VIOLATIONS_JSON" \
  --argjson passed "$PASSED" \
  '{inventory_path:$inventory_path, total_rows:$total_rows, branch:$branch, violations:$violations, passed:$passed}' | jq .

if [[ "$PASSED" == "true" ]]; then
  exit 0
fi
exit 1

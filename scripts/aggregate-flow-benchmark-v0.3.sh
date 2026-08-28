#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 benchmark aggregator.
#
# Contract: gate-commands.md v0.3 section, same "每候选各写各的文件 + 聚合器
# 原子生成合并结果" rule as convergence and editor-binding. Reads
# evidence/v0.3/benchmark-loro.json and evidence/v0.3/benchmark-yrs-yjs.json
# (single-candidate envelopes scripts/benchmark-flow-v0.3.sh writes) and
# atomically writes the merged evidence/v0.3/benchmark-result.json matching
# sylvode-flow-benchmark-result-v1.schema.json.
#
# Exit codes: 0 = merged file written, 1 = inputs missing/mismatched
# (nothing written), 2 = usage/tool error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCHEMA_DIR="$ROOT_DIR/docs/schemas"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.3"
GATE_YAML="/opt/working/sylvode-flow/gates/v0.3-gate.yaml"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/aggregate-flow-benchmark-v0.3.sh --json [OPTIONS]

Merges evidence/v0.3/benchmark-loro.json and
evidence/v0.3/benchmark-yrs-yjs.json into evidence/v0.3/benchmark-result.json.
Produces a result ONLY if both candidate files exist, share the same
budgets: block (recomputed from --gate-yaml, not trusted from the runner)
and environment.source_commit, and each candidate carries the full
measurements/supplemental_metrics/hostile_input_safety/budget_checks shape
-- never a partial merge. run_sequence records the recorded order the two
candidates were actually benchmarked in (must be a run on the same idle
machine per benchmark-spec.md), read from each candidate file's own
"generated_at" ordering (each candidate file's own timestamp,
recorded when scripts/benchmark-flow-v0.3.sh finished that candidate's run).

Options:
  --json                  Required for contract compatibility.
  --evidence-root DIR    Directory holding benchmark-loro.json /
                          benchmark-yrs-yjs.json; merged output is written
                          here too. Default: /opt/working/sylvode-flow/evidence/v0.3
  --gate-yaml PATH        Path to v0.3-gate.yaml; its budgets: block is
                          recomputed (same extraction as
                          verify-flow-v0.3-json.sh) and used as the merged
                          file's authoritative "budgets" block -- a
                          per-candidate file that disagrees with it fails
                          the merge rather than being silently overwritten.
                          Default: /opt/working/sylvode-flow/gates/v0.3-gate.yaml
  --schema-dir DIR        Directory holding sylvode-flow-*.schema.json.
                          Default: <repo>/docs/schemas
  -h, --help              Show this help and exit 0.

Exit codes: 0 written, 1 inputs missing/mismatched, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --json) JSON_MODE=1; shift ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --gate-yaml) GATE_YAML="${2:?--gate-yaml requires a PATH argument}"; shift 2 ;;
    --schema-dir) SCHEMA_DIR="${2:?--schema-dir requires a DIR argument}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if ! command -v jq >/dev/null 2>&1; then
  echo "FAIL: missing required command: jq" >&2
  echo "Fix: sudo apt-get install -y jq" >&2
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required (frozen required_commands entry uses --json)" >&2
  usage >&2
  exit 2
fi
if [[ ! -f "$GATE_YAML" ]]; then
  echo "FAIL: --gate-yaml file missing: $GATE_YAML" >&2
  exit 2
fi

EXPECTED_BUDGET_KEYS="browser_engine_bundle_gzip_bytes_max cold_start_ms_p95_max apply_update_ms_p95_max bootstrap_10k_ops_ms_p95_max bootstrap_100k_ops_ms_p95_max peak_memory_100k_ops_bytes_max"
BUDGETS_JSON="$(awk '
  /^budgets:/ { in_block=1; next }
  in_block && /^[a-zA-Z_]/ { in_block=0 }
  in_block && /^[[:space:]]+[a-zA-Z0-9_]+:[[:space:]]*[0-9]+[[:space:]]*$/ {
    line=$0
    sub(/^[[:space:]]+/,"",line)
    split(line, kv, ":")
    key=kv[1]
    val=kv[2]
    gsub(/[[:space:]]/,"",val)
    printf "%s %s\n", key, val
  }
' "$GATE_YAML" | jq -R -n '[inputs | split(" ") | {(.[0]): (.[1] | tonumber)}] | add // {}')"
BUDGET_KEY_COUNT="$(jq 'keys | length' <<<"$BUDGETS_JSON")"
if [[ "$BUDGET_KEY_COUNT" -ne 6 ]]; then
  echo "FAIL: extracted $BUDGET_KEY_COUNT budgets keys from $GATE_YAML, expected exactly 6 ($EXPECTED_BUDGET_KEYS)" >&2
  exit 2
fi

LORO_FILE="$EVIDENCE_ROOT/benchmark-loro.json"
YRS_FILE="$EVIDENCE_ROOT/benchmark-yrs-yjs.json"

MISSING=0
for f in "$LORO_FILE" "$YRS_FILE"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: required candidate file missing: $f" >&2
    MISSING=1
  elif ! jq empty "$f" >/dev/null 2>&1; then
    echo "FAIL: not valid JSON: $f" >&2
    MISSING=1
  fi
done
if [[ $MISSING -eq 1 ]]; then
  echo "Refusing to write a partial evidence/v0.3/benchmark-result.json." >&2
  exit 1
fi

REQUIRED_CR_FIELDS="candidate environment measurements supplemental_metrics hostile_input_safety budget_checks benchmark_budgets_met"
FAILED=0
for pair in "loro:$LORO_FILE" "yrs-yjs:$YRS_FILE"; do
  name="${pair%%:*}"
  file="${pair#*:}"
  actual_name="$(jq -r '.candidate_result.candidate // empty' "$file")"
  if [[ "$actual_name" != "$name" ]]; then
    echo "FAIL: $file candidate_result.candidate=$actual_name, expected $name" >&2
    FAILED=1
  fi
  for field in $REQUIRED_CR_FIELDS; do
    if [[ "$(jq --arg f "$field" 'has($f) | not' <<<"$(jq '.candidate_result' "$file")" 2>/dev/null)" != "false" ]]; then
      echo "FAIL: $file candidate_result missing required field: $field" >&2
      FAILED=1
    fi
  done
  file_budgets="$(jq '.candidate_result_budgets // .budgets // empty' "$file")"
  if [[ -n "$file_budgets" && "$file_budgets" != "null" ]]; then
    if [[ "$(jq --argjson a "$BUDGETS_JSON" --argjson b "$file_budgets" -n '$a == $b')" != "true" ]]; then
      echo "FAIL: $file's own budgets block does not match gates/v0.3-gate.yaml's budgets: block" >&2
      FAILED=1
    fi
  fi
done
if [[ $FAILED -eq 1 ]]; then
  echo "Refusing to write a partial evidence/v0.3/benchmark-result.json." >&2
  exit 1
fi

LORO_HEAD="$(jq -r '.source_head // empty' "$LORO_FILE")"
YRS_HEAD="$(jq -r '.source_head // empty' "$YRS_FILE")"
if [[ -z "$LORO_HEAD" || "$LORO_HEAD" != "$YRS_HEAD" ]]; then
  echo "FAIL: source_head differs across candidate files: loro=$LORO_HEAD yrs-yjs=$YRS_HEAD" >&2
  exit 1
fi

LORO_COMMIT="$(jq -r '.candidate_result.environment.source_commit' "$LORO_FILE")"
YRS_COMMIT="$(jq -r '.candidate_result.environment.source_commit' "$YRS_FILE")"
if [[ "$LORO_COMMIT" != "$YRS_COMMIT" ]]; then
  echo "FAIL: environment.source_commit differs across candidates: loro=$LORO_COMMIT yrs-yjs=$YRS_COMMIT" >&2
  exit 1
fi

GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
LORO_AT="$(jq -r '.generated_at // empty' "$LORO_FILE")"
YRS_AT="$(jq -r '.generated_at // empty' "$YRS_FILE")"
if [[ "$LORO_AT" < "$YRS_AT" ]]; then
  RUN_SEQUENCE='["loro","yrs-yjs"]'
else
  RUN_SEQUENCE='["yrs-yjs","loro"]'
fi

MERGED="$(jq -n \
  --arg head "$LORO_HEAD" --arg generated_at "$GENERATED_AT" \
  --argjson budgets "$BUDGETS_JSON" \
  --argjson run_sequence "$RUN_SEQUENCE" \
  --argjson loro "$(jq '.candidate_result' "$LORO_FILE")" \
  --argjson yrs "$(jq '.candidate_result' "$YRS_FILE")" \
  '{
    schema_version: "sylvode.flow.benchmark-result.v1",
    schema_path: "docs/schemas/sylvode-flow-benchmark-result-v1.schema.json",
    release: "0.3.0",
    source_head: $head,
    generated_at: $generated_at,
    budgets: $budgets,
    run_sequence: $run_sequence,
    candidates: {"loro": $loro, "yrs-yjs": $yrs}
  }')"

OUT_PATH="$EVIDENCE_ROOT/benchmark-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$MERGED" | jq . > "$OUT_TMP"

SCHEMA_FILE="$SCHEMA_DIR/sylvode-flow-benchmark-result-v1.schema.json"
if [[ -f "$SCHEMA_FILE" ]]; then
  if command -v check-jsonschema >/dev/null 2>&1; then
    if ! check-jsonschema --schemafile "$SCHEMA_FILE" "$OUT_TMP" >/tmp/aggregate-benchmark-schema.$$ 2>&1; then
      echo "FAIL: merged result does not match $SCHEMA_FILE:" >&2
      sed 's/^/  /' /tmp/aggregate-benchmark-schema.$$ >&2
      rm -f /tmp/aggregate-benchmark-schema.$$ "$OUT_TMP"
      exit 1
    fi
    rm -f /tmp/aggregate-benchmark-schema.$$
  elif python3 -c "import jsonschema" >/dev/null 2>&1; then
    if ! python3 - "$SCHEMA_FILE" "$OUT_TMP" <<'PYEOF'
import json, sys, jsonschema
schema = json.load(open(sys.argv[1], encoding="utf-8"))
data = json.load(open(sys.argv[2], encoding="utf-8"))
validator_cls = jsonschema.validators.validator_for(schema)
validator_cls.check_schema(schema)
errors = list(validator_cls(schema).iter_errors(data))
for e in errors:
    print("/".join(str(p) for p in e.path) or "<root>", ":", e.message)
sys.exit(1 if errors else 0)
PYEOF
    then
      echo "FAIL: merged result does not match $SCHEMA_FILE (see above)" >&2
      rm -f "$OUT_TMP"
      exit 1
    fi
  else
    echo "FAIL: no JSON Schema validator available (need check-jsonschema or python3+jsonschema)" >&2
    echo "Fix: pip install --user check-jsonschema" >&2
    rm -f "$OUT_TMP"
    exit 2
  fi
fi

mv -f "$OUT_TMP" "$OUT_PATH"
echo "Wrote $OUT_PATH"
exit 0

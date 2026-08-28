#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 convergence aggregator.
#
# Contract: gate-commands.md v0.3 section -- "benchmark、convergence、editor
# binding 三者一律「每候选各写各的文件 + 聚合器原子生成合并结果」...两个独立
# --candidate 命令写同一个文件是未定义行为...每个聚合器都只在两候选文件都在、
# 参数相同、必需字段齐全时才产出合并结果，否则 exit 1."
#
# Reads evidence/v0.3/convergence-loro.json and
# evidence/v0.3/convergence-yrs-yjs.json (each the single-candidate envelope
# scripts/verify-flow-convergence-v0.3.sh writes: {schema_version,release,
# source_head,generated_at,candidate_result}, reusing the convergence-result
# schema's candidate_result $def per gate-commands.md -- no separate
# single-candidate schema exists), and atomically writes the merged
# evidence/v0.3/convergence-result.json matching
# sylvode-flow-convergence-result-v1.schema.json.
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
Usage: scripts/aggregate-flow-convergence-v0.3.sh --json [OPTIONS]

Merges evidence/v0.3/convergence-loro.json and
evidence/v0.3/convergence-yrs-yjs.json into evidence/v0.3/convergence-result.json.
Produces a result ONLY if both candidate files exist, share the same
source_head, and each candidate_result carries all fields the schema
requires (corpus_seed, case_count, budget_hash, isolation, hard_gates,
fixture_coverage, boundary_results, cases) -- never a partial merge.

Options:
  --json                  Required for contract compatibility
                          (aggregate-flow-benchmark/convergence/editor-binding
                          -v0.3.sh --json are the frozen required_commands
                          entries); output is always machine-readable
                          regardless of this flag.
  --evidence-root DIR    Directory holding convergence-loro.json /
                          convergence-yrs-yjs.json; merged output is written
                          here too. Default: /opt/working/sylvode-flow/evidence/v0.3
  --gate-yaml PATH        Path to v0.3-gate.yaml; only checked for existence
                          here as a sanity precondition -- this aggregator
                          only compares the two candidates against each
                          other (equality, completeness, atomic merge), it
                          does not recompute budget_hash from the yaml
                          budgets: block. That deep recompute is
                          scripts/verify-flow-v0.3-json.sh's job, which runs
                          on the merged file this script produces.
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

LORO_FILE="$EVIDENCE_ROOT/convergence-loro.json"
YRS_FILE="$EVIDENCE_ROOT/convergence-yrs-yjs.json"

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
  echo "Refusing to write a partial evidence/v0.3/convergence-result.json." >&2
  exit 1
fi

REQUIRED_CR_FIELDS="candidate corpus_seed case_count budget_hash isolation hard_gates fixture_coverage boundary_results cases"
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
  case_count="$(jq -r '.candidate_result.case_count // -1' "$file")"
  cases_len="$(jq -r '.candidate_result.cases | length' "$file" 2>/dev/null || echo -1)"
  if [[ "$case_count" != "$cases_len" ]]; then
    echo "FAIL: $file case_count=$case_count but cases.length=$cases_len" >&2
    FAILED=1
  fi
done
if [[ $FAILED -eq 1 ]]; then
  echo "Refusing to write a partial evidence/v0.3/convergence-result.json." >&2
  exit 1
fi

LORO_HEAD="$(jq -r '.source_head // empty' "$LORO_FILE")"
YRS_HEAD="$(jq -r '.source_head // empty' "$YRS_FILE")"
if [[ -z "$LORO_HEAD" || "$LORO_HEAD" != "$YRS_HEAD" ]]; then
  echo "FAIL: source_head differs across candidate files: loro=$LORO_HEAD yrs-yjs=$YRS_HEAD (must run the same source revision)" >&2
  exit 1
fi

LORO_SEED="$(jq -r '.candidate_result.corpus_seed' "$LORO_FILE")"
YRS_SEED="$(jq -r '.candidate_result.corpus_seed' "$YRS_FILE")"
if [[ "$LORO_SEED" != "$YRS_SEED" ]]; then
  echo "FAIL: corpus_seed differs across candidates: loro=$LORO_SEED yrs-yjs=$YRS_SEED" >&2
  exit 1
fi
LORO_COUNT="$(jq -r '.candidate_result.case_count' "$LORO_FILE")"
YRS_COUNT="$(jq -r '.candidate_result.case_count' "$YRS_FILE")"
if [[ "$LORO_COUNT" != "$YRS_COUNT" ]]; then
  echo "FAIL: case_count differs across candidates: loro=$LORO_COUNT yrs-yjs=$YRS_COUNT" >&2
  exit 1
fi
LORO_BHASH="$(jq -r '.candidate_result.budget_hash' "$LORO_FILE")"
YRS_BHASH="$(jq -r '.candidate_result.budget_hash' "$YRS_FILE")"
if [[ "$LORO_BHASH" != "$YRS_BHASH" ]]; then
  echo "FAIL: budget_hash differs across candidates: loro=$LORO_BHASH yrs-yjs=$YRS_BHASH" >&2
  exit 1
fi

GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
MERGED="$(jq -n \
  --arg head "$LORO_HEAD" --arg generated_at "$GENERATED_AT" \
  --argjson loro "$(jq '.candidate_result' "$LORO_FILE")" \
  --argjson yrs "$(jq '.candidate_result' "$YRS_FILE")" \
  '{
    schema_version: "sylvode.flow.convergence-result.v1",
    schema_path: "docs/schemas/sylvode-flow-convergence-result-v1.schema.json",
    release: "0.3.0",
    source_head: $head,
    generated_at: $generated_at,
    candidates: {"loro": $loro, "yrs-yjs": $yrs}
  }')"

OUT_PATH="$EVIDENCE_ROOT/convergence-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$MERGED" | jq . > "$OUT_TMP"

SCHEMA_FILE="$SCHEMA_DIR/sylvode-flow-convergence-result-v1.schema.json"
if [[ -f "$SCHEMA_FILE" ]]; then
  if command -v check-jsonschema >/dev/null 2>&1; then
    if ! check-jsonschema --schemafile "$SCHEMA_FILE" "$OUT_TMP" >/tmp/aggregate-convergence-schema.$$ 2>&1; then
      echo "FAIL: merged result does not match $SCHEMA_FILE:" >&2
      sed 's/^/  /' /tmp/aggregate-convergence-schema.$$ >&2
      rm -f /tmp/aggregate-convergence-schema.$$ "$OUT_TMP"
      exit 1
    fi
    rm -f /tmp/aggregate-convergence-schema.$$
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

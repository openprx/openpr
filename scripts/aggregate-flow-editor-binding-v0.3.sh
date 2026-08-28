#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 editor-binding aggregator.
#
# Contract: gate-commands.md v0.3 section, same "每候选各写各的文件 + 聚合器
# 原子生成合并结果" rule as convergence and benchmark; the editor-binding
# result additionally records candidate_inputs (path+sha256 of the two
# source files) and an "aggregation" block per
# sylvode-flow-editor-binding-result-v1.schema.json.
#
# Reads evidence/v0.3/editor-binding-loro.json and
# evidence/v0.3/editor-binding-yrs-yjs.json (single-candidate envelopes
# scripts/verify-flow-editor-binding-v0.3.sh writes) and atomically writes
# the merged evidence/v0.3/editor-binding-result.json.
#
# Exit codes: 0 = merged file written, 1 = inputs missing/mismatched
# (nothing written), 2 = usage/tool error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCHEMA_DIR="$ROOT_DIR/docs/schemas"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.3"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/aggregate-flow-editor-binding-v0.3.sh --json [OPTIONS]

Merges evidence/v0.3/editor-binding-loro.json and
evidence/v0.3/editor-binding-yrs-yjs.json into
evidence/v0.3/editor-binding-result.json. Produces a result ONLY if both
candidate files exist, share the same source_head and
runner_parameters (soak_hours/transactions_per_minute/
destroy_remount_interval_minutes must match -- ADR-0006's six criteria must
be judged under identical run parameters), and each candidate carries all
six judged blocks (editor_corpus, soak, binding_patch, dependency_evidence,
defects, manual_acceptance) -- never a partial merge.

Options:
  --json                  Required for contract compatibility.
  --evidence-root DIR    Directory holding editor-binding-loro.json /
                          editor-binding-yrs-yjs.json; merged output is
                          written here too.
                          Default: /opt/working/sylvode-flow/evidence/v0.3
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
if ! command -v sha256sum >/dev/null 2>&1; then
  echo "FAIL: missing required command: sha256sum" >&2
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required (frozen required_commands entry uses --json)" >&2
  usage >&2
  exit 2
fi

sha256_of() { sha256sum "$1" | awk '{print $1}'; }

LORO_FILE="$EVIDENCE_ROOT/editor-binding-loro.json"
YRS_FILE="$EVIDENCE_ROOT/editor-binding-yrs-yjs.json"

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
  echo "Refusing to write a partial evidence/v0.3/editor-binding-result.json." >&2
  exit 1
fi

REQUIRED_CR_FIELDS="candidate source_head runner_parameters editor_corpus soak binding_patch dependency_evidence defects manual_acceptance editor_binding_maturity"
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
done
if [[ $FAILED -eq 1 ]]; then
  echo "Refusing to write a partial evidence/v0.3/editor-binding-result.json." >&2
  exit 1
fi

LORO_HEAD="$(jq -r '.source_head // empty' "$LORO_FILE")"
YRS_HEAD="$(jq -r '.source_head // empty' "$YRS_FILE")"
if [[ -z "$LORO_HEAD" || "$LORO_HEAD" != "$YRS_HEAD" ]]; then
  echo "FAIL: source_head differs across candidate files: loro=$LORO_HEAD yrs-yjs=$YRS_HEAD" >&2
  exit 1
fi

for param in soak_hours transactions_per_minute destroy_remount_interval_minutes; do
  loro_val="$(jq -r ".candidate_result.runner_parameters.$param" "$LORO_FILE")"
  yrs_val="$(jq -r ".candidate_result.runner_parameters.$param" "$YRS_FILE")"
  if [[ "$loro_val" != "$yrs_val" ]]; then
    echo "FAIL: runner_parameters.$param differs across candidates: loro=$loro_val yrs-yjs=$yrs_val (ADR-0006's six criteria must be judged under identical run parameters)" >&2
    exit 1
  fi
done

GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
LORO_SHA="$(sha256_of "$LORO_FILE")"
YRS_SHA="$(sha256_of "$YRS_FILE")"

MERGED="$(jq -n \
  --arg head "$LORO_HEAD" --arg generated_at "$GENERATED_AT" \
  --arg loro_sha "$LORO_SHA" --arg yrs_sha "$YRS_SHA" \
  --argjson loro "$(jq '.candidate_result' "$LORO_FILE")" \
  --argjson yrs "$(jq '.candidate_result' "$YRS_FILE")" \
  '{
    schema_version: "sylvode.flow.editor-binding-result.v1",
    schema_path: "docs/schemas/sylvode-flow-editor-binding-result-v1.schema.json",
    release: "0.3.0",
    source_head: $head,
    generated_at: $generated_at,
    candidate_inputs: {
      "loro": {path: "evidence/v0.3/editor-binding-loro.json", sha256: $loro_sha},
      "yrs-yjs": {path: "evidence/v0.3/editor-binding-yrs-yjs.json", sha256: $yrs_sha}
    },
    aggregation: {
      both_candidate_files_present: true,
      parameters_match: true,
      six_criteria_complete: true,
      atomic_write: true
    },
    candidates: {"loro": $loro, "yrs-yjs": $yrs}
  }')"

OUT_PATH="$EVIDENCE_ROOT/editor-binding-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$MERGED" | jq . > "$OUT_TMP"

SCHEMA_FILE="$SCHEMA_DIR/sylvode-flow-editor-binding-result-v1.schema.json"
if [[ -f "$SCHEMA_FILE" ]]; then
  if command -v check-jsonschema >/dev/null 2>&1; then
    if ! check-jsonschema --schemafile "$SCHEMA_FILE" "$OUT_TMP" >/tmp/aggregate-editor-binding-schema.$$ 2>&1; then
      echo "FAIL: merged result does not match $SCHEMA_FILE:" >&2
      sed 's/^/  /' /tmp/aggregate-editor-binding-schema.$$ >&2
      rm -f /tmp/aggregate-editor-binding-schema.$$ "$OUT_TMP"
      exit 1
    fi
    rm -f /tmp/aggregate-editor-binding-schema.$$
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

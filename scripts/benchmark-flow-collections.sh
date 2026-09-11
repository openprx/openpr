#!/usr/bin/env bash
set -euo pipefail

# Runs the real 10,000-row typed-projection query gate. The Rust test deliberately
# stores invalid document bytes, requires an EXPLAIN plan containing a typed index,
# and succeeds only without decoding those documents.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.6"
RECORDS=""
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/benchmark-flow-collections.sh --records 10000 --json [OPTIONS]

Options:
  --records 10000       Required exact contractual scale
  --evidence-root DIR   Default: /opt/working/sylvode-flow/evidence/v0.6
  --repo-root DIR       Default: this checkout
  --json                Required; emit JSON and write collection-10k-result.json
  -h, --help            Show help

OPENPR_TEST_DATABASE_URL must identify the isolated PostgreSQL test server.
Exit: 0 benchmark passed, 1 gate failed, 2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --records) RECORDS="${2:?--records requires a value}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a directory}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a directory}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ "$RECORDS" == "10000" ]] || { echo "FAIL: --records must be exactly 10000" >&2; exit 2; }
[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
[[ -n "${OPENPR_TEST_DATABASE_URL:-}" ]] || { echo "FAIL: OPENPR_TEST_DATABASE_URL is required" >&2; exit 2; }
for tool in cargo git jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
[[ -d "$REPO_ROOT/.git" ]] || { echo "FAIL: --repo-root is not a git checkout: $REPO_ROOT" >&2; exit 2; }
mkdir -p "$EVIDENCE_ROOT"
LOG_FILE="$(mktemp)"
trap 'rm -f "$LOG_FILE"' EXIT
START_NS="$(date +%s%N)"
set +e
(cd "$REPO_ROOT" && cargo test -p api ten_thousand_record_query_uses_index_without_decoding_documents -- --nocapture) \
  >"$LOG_FILE" 2>&1
TEST_EXIT=$?
set -e
END_NS="$(date +%s%N)"
DURATION_MS="$(((END_NS - START_NS) / 1000000))"
METRICS_LINE="$(sed -n 's/^FLOW_COLLECTION_10K_METRICS //p' "$LOG_FILE" | tail -1)"
if [[ $TEST_EXIT -ne 0 ]]; then
  cat "$LOG_FILE" >&2
fi
if [[ -z "$METRICS_LINE" ]] || ! jq -e 'type=="object" and .records==10000' >/dev/null 2>&1 <<<"$METRICS_LINE"; then
  STATUS=false
  REASON="test did not emit valid 10k metrics (an environment skip is not a pass)"
else
  STATUS=true
  REASON="typed index was required and invalid document bytes were never decoded"
fi
[[ $TEST_EXIT -eq 0 ]] || STATUS=false
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
RESULT="$(jq -cn \
  --arg schema_version sylvode.flow.collection-capacity-result.v1 \
  --arg release 0.6.0 --arg source_head "$SOURCE_HEAD" \
  --arg command 'cargo test -p api ten_thousand_record_query_uses_index_without_decoding_documents -- --nocapture' \
  --argjson records 10000 --argjson test_exit "$TEST_EXIT" --argjson duration_ms "$DURATION_MS" \
  --argjson metrics "${METRICS_LINE:-null}" --argjson passed "$STATUS" --arg reason "$REASON" \
  '{schema_version:$schema_version,release:$release,source_head:$source_head,command:$command,records:$records,test_exit_code:$test_exit,duration_ms:$duration_ms,metrics:$metrics,passed:$passed,reason:$reason}')"
TMP_RESULT="$(mktemp "$EVIDENCE_ROOT/.collection-10k-result.XXXXXX")"
printf '%s\n' "$RESULT" >"$TMP_RESULT"
mv -f "$TMP_RESULT" "$EVIDENCE_ROOT/collection-10k-result.json"
printf '%s\n' "$RESULT"
[[ "$STATUS" == true ]]

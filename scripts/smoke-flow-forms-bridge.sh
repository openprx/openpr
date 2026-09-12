#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="$ROOT_DIR/evidence/v0.7"
JSON=false
while (($#)); do
  case "$1" in
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a value}"; shift 2 ;;
    --json) JSON=true; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
mkdir -p "$EVIDENCE_ROOT/logs"
export CARGO_BUILD_JOBS=4
BUN_BIN="$(command -v bun || true)"
if [[ -z "$BUN_BIN" && -x /home/ck/.bun/bin/bun ]]; then BUN_BIN=/home/ck/.bun/bin/bun; fi

run() {
  local id="$1"; shift
  local log="$EVIDENCE_ROOT/logs/smoke-$id.log"
  local start end exit_code
  start="$(date +%s%3N)"
  set +e
  (cd "$ROOT_DIR" && "$@") >"$log" 2>&1
  exit_code=$?
  set -e
  end="$(date +%s%3N)"
  jq -n --arg id "$id" --arg command "$*" --argjson exit_code "$exit_code" \
    --argjson duration_ms "$((end-start))" --arg log "logs/$(basename "$log")" \
    '{id:$id,command:$command,exit_code:$exit_code,duration_ms:$duration_ms,log:$log,
      status:(if $exit_code==0 then "passed" else "failed" end),executed_count:1}'
  return "$exit_code"
}

RESULTS="[]"
for spec in api mcp frontend; do
  case "$spec" in
    api) result="$(run api cargo test -p api flow_bridge_ -- --nocapture)" || true ;;
    mcp) result="$(run mcp cargo test -p mcp-server --test flow_bridge_e2e -- --nocapture)" || true ;;
    frontend)
      if [[ -z "$BUN_BIN" ]]; then
        result='{"id":"frontend","command":"bun run --cwd frontend test:flow-bridge","exit_code":127,"duration_ms":0,"log":"logs/smoke-frontend.log","status":"failed","executed_count":0}'
      else
        result="$(run frontend "$BUN_BIN" frontend/tests/flow-bridge.test.ts)" || true
      fi
      ;;
  esac
  RESULTS="$(jq --argjson item "$result" '. + [$item]' <<<"$RESULTS")"
done

STATIC_LOG="$EVIDENCE_ROOT/logs/smoke-no-double-write.log"
static_exit=0
{
  if rg -n 'CREATE[[:space:]]+(OR[[:space:]]+REPLACE[[:space:]]+)?TRIGGER.*(flow_bridge|flow_conversion|flow_object_lineage)' migrations; then
    echo 'FAIL: a bridge synchronization trigger exists'
    static_exit=1
  fi
  if rg -n 'flow_bridge_references|flow_conversion_previews|flow_conversion_jobs|flow_object_lineage' apps/worker; then
    echo 'FAIL: background worker references bridge persistence'
    static_exit=1
  fi
  if [[ "$static_exit" == 0 ]]; then
    echo 'PASS: no bridge synchronization trigger or worker persistence path'
  fi
} >"$STATIC_LOG" 2>&1
static_result="$(jq -n --argjson exit_code "$static_exit" \
  '{id:"no_double_write",command:"production trigger and worker source scan",exit_code:$exit_code,
    duration_ms:0,log:"logs/smoke-no-double-write.log",status:(if $exit_code==0 then "passed" else "failed" end),executed_count:1}')"
RESULTS="$(jq --argjson item "$static_result" '. + [$item]' <<<"$RESULTS")"
PASSED="$(jq 'all(.[]; .status=="passed") and length==4' <<<"$RESULTS")"
OUT="$(jq -n --arg schema_version 'sylvode.flow.bridge-smoke-result.v1' \
  --arg source_head "$(git -C "$ROOT_DIR" rev-parse HEAD)" --argjson checks "$RESULTS" --argjson passed "$PASSED" \
  '{schema_version:$schema_version,source_head:$source_head,executed_count:($checks|map(.executed_count)|add),checks:$checks,passed:$passed}')"
printf '%s\n' "$OUT" | jq . >"$EVIDENCE_ROOT/bridge-smoke-result.json"
if [[ "$JSON" == true ]]; then printf '%s\n' "$OUT" | jq .; else jq -r '"Flow bridge smoke passed="+(.passed|tostring)' <<<"$OUT"; fi
[[ "$PASSED" == true ]]

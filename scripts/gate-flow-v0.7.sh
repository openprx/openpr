#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"; EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.7"; CONTRACTS_ROOT="/opt/working/sylvode-flow"; REPO_ROOT="$ROOT_DIR"; RESULT=""; JSON=0
while [[ $# -gt 0 ]]; do case "$1" in --evidence-root)EVIDENCE_ROOT="$2";shift 2;;--contracts-root)CONTRACTS_ROOT="$2";shift 2;;--repo-root)REPO_ROOT="$2";shift 2;;--gate-result)RESULT="$2";shift 2;;--json)JSON=1;shift;;*)echo "FAIL: unsupported argument $1" >&2;exit 2;;esac;done
[[ $JSON -eq 1 ]] || { echo "FAIL: --json required" >&2;exit 2; };[[ -n "$RESULT" ]]||RESULT="$EVIDENCE_ROOT/gate-result.json"
set +e; V="$("$ROOT_DIR/scripts/verify-flow-v0.7-json.sh" "$RESULT" --evidence-root "$EVIDENCE_ROOT" --contracts-root "$CONTRACTS_ROOT" --repo-root "$REPO_ROOT" --json 2>&1)"; code=$?;set -e
jq -cn --argjson verification "$V" --argjson verifier_exit "$code" --argjson gate_passed "$(jq -r '.accepted' "$RESULT")" '{schema_version:"sylvode.flow.gate.v1",verifier_exit_code:$verifier_exit,verification:$verification,gate_passed:$gate_passed,reason:(if $gate_passed then "accepted" else "candidate or manual requirements blocked" end)}'
[[ $code -eq 0 && "$(jq -r '.accepted' "$RESULT")" == true ]]

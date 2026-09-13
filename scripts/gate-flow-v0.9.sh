#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE="$ROOT/.flow-gate/evidence/v0.9"
CONTRACTS=/opt/working/sylvode-flow
REPO=$ROOT
RESULT=
JSON_MODE=0
while (($#)); do
  case "$1" in
    --evidence-root) EVIDENCE=${2:?}; shift 2 ;;
    --contracts-root) CONTRACTS=${2:?}; shift 2 ;;
    --repo-root) REPO=${2:?}; shift 2 ;;
    --gate-result) RESULT=${2:?}; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo 'FAIL: --json is required' >&2; exit 2; }
[[ -n $RESULT ]] || RESULT="$EVIDENCE/gate-result.json"
set +e
verification=$("$ROOT/scripts/verify-flow-v0.9-json.sh" "$RESULT" --evidence-root "$EVIDENCE" --contracts-root "$CONTRACTS" --repo-root "$REPO" --json 2>&1)
code=$?
set -e
jq -cn --argjson verification "$verification" --argjson verifier_exit "$code" --argjson accepted "$(jq '.accepted' "$RESULT")" \
  '{schema_version:"sylvode.flow.gate.v1",verifier_exit_code:$verifier_exit,verification:$verification,gate_passed:$accepted,reason:(if $accepted then "accepted" else "contract baseline or manual acceptance requirements blocked" end)}'
[[ $code -eq 0 && $(jq -r '.accepted' "$RESULT") == true ]]

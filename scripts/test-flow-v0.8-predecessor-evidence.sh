#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
WORKSPACE_ROOT=$(cd "$REPO_ROOT/../.." && pwd)
EVIDENCE_ROOT=${1:-"$REPO_ROOT/.flow-gate/evidence/v0.8"}
REAL_PREDECESSOR=${2:-"$WORKSPACE_ROOT/evidence/v0.7/gate-result.json"}
RESULT="$EVIDENCE_ROOT/gate-result.json"

[[ -f $REAL_PREDECESSOR ]] || { echo "FAIL: missing accepted predecessor: $REAL_PREDECESSOR" >&2; exit 2; }
[[ $(jq -r '.accepted' "$REAL_PREDECESSOR") == true ]] || {
  echo "FAIL: predecessor fixture is not accepted: $REAL_PREDECESSOR" >&2
  exit 2
}

MUTATION_DIR=$(mktemp -d "$EVIDENCE_ROOT/.predecessor-mutation.XXXXXX")
trap 'rm -rf -- "$MUTATION_DIR"' EXIT
BASELINE_RECEIPT="$MUTATION_DIR/gate-result.json"
PREDECESSOR_RELEASE=$(jq -r '.release' "$REAL_PREDECESSOR")
jq --arg path "$(realpath "$REAL_PREDECESSOR")" --arg release "$PREDECESSOR_RELEASE" \
  '.predecessor={path:$path,accepted:true,release:$release}' "$RESULT" >"$BASELINE_RECEIPT"

run_verifier() {
  local predecessor=$1
  local output=$2
  set +e
  "$REPO_ROOT/scripts/verify-flow-v0.8-json.sh" "$BASELINE_RECEIPT" \
    --repo-root "$REPO_ROOT" --evidence-root "$EVIDENCE_ROOT" \
    --predecessor-evidence "$predecessor" --json >"$output"
  local status=$?
  set -e
  [[ $status -ne 0 ]] || { echo "FAIL: predecessor mutation unexpectedly passed" >&2; exit 1; }
  jq -e '.drift | any(.field=="predecessor")' "$output" >/dev/null || {
    echo "FAIL: verifier went red without attributing the predecessor drift" >&2
    exit 1
  }
}

REJECTED="$MUTATION_DIR/rejected.json"
jq '.accepted=false | .candidate_ready=false' "$REAL_PREDECESSOR" >"$REJECTED"
run_verifier "$REJECTED" "$MUTATION_DIR/rejected-verification.json"
run_verifier "$MUTATION_DIR/does-not-exist.json" "$MUTATION_DIR/missing-verification.json"

printf '%s\n' '{"passed":true,"executed_count":2,"mutations":{"accepted_false":"red","missing":"red"}}'

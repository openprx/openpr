#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE="$ROOT/.flow-gate/evidence/v1.0"
JSON=0
while (($#)); do
  case "$1" in
    --repo-root) ROOT=${2:?}; shift 2 ;;
    --contracts-root) shift 2 ;;
    --evidence-root) EVIDENCE=${2:?}; shift 2 ;;
    --json) JSON=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done
[[ $JSON -eq 1 ]] || { echo 'FAIL: --json required' >&2; exit 2; }
mkdir -p "$EVIDENCE/logs"

set +e
env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 "$ROOT/scripts/verify-flow-forms-regression-v0.4.sh" \
  --evidence-root "$EVIDENCE" --repo-root "$ROOT" --json >"$EVIDENCE/logs/forms-v10.log" 2>&1
BASE_EXIT=$?
set -e

RUN=$(mktemp -d /opt/worker/.cache/v10-signoff-domains.XXXXXX)
trap 'rm -rf -- "$RUN"' EXIT
FLOW_RECORDER="$ROOT/scripts/record-flow-v1.0-manual-signoff.sh"
FORMS_RECORDER="$ROOT/scripts/record-universal-forms-manual-signoff.sh"
DOMAIN_CHECKER="$ROOT/scripts/lib/verify_flow_signoff_domains.py"

set +e
python3 "$DOMAIN_CHECKER" --flow-recorder "$FLOW_RECORDER" --forms-recorder "$FORMS_RECORDER" >"$RUN/domains-positive.json" 2>"$RUN/domains-positive.log"
DOMAIN_EXIT=$?
set -e

ln -s "$FLOW_RECORDER" "$RUN/forms-recorder-alias"
set +e
python3 "$DOMAIN_CHECKER" --flow-recorder "$FLOW_RECORDER" --forms-recorder "$RUN/forms-recorder-alias" >"$RUN/domains-alias.json" 2>"$RUN/domains-alias.log"
ALIAS_EXIT=$?
set -e

cp /opt/worker/report/openpr/docs/openpr-universal-form-user-acceptance-runbook-2026-05-31.md "$RUN/forms-runbook.md"
cp /opt/worker/report/openpr/docs/openpr-universal-form-acceptance-evidence-2026-05-31.md "$RUN/forms-evidence.md"
python3 - "$RUN/flow-gate-result.json" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
manual = {
    key: {"status": "pending", "signed_by": None, "signed_at": None, "note": None}
    for key in ("release_owner", "rollback_owner", "on_call_runbook", "stable_contract_approval")
}
path.write_text(json.dumps({
    "candidate_ready": False,
    "accepted": False,
    "hard_gates": {"frontend_track_accepted": "pending"},
    "manual_signoffs": manual,
    "blockers": [f"manual_signoff_pending:{key}" for key in manual],
}, sort_keys=True, indent=2) + "\n")
PY

FLOW_BEFORE_FORMS=$(sha256sum "$RUN/flow-gate-result.json" | awk '{print $1}')
set +e
"$FORMS_RECORDER" --item restaurant_template --status accepted --reviewer mutation-control \
  --evidence "isolated mutation control only" --runbook "$RUN/forms-runbook.md" \
  --report "$RUN/forms-evidence.md" >"$RUN/forms-only-sign.log" 2>&1
FORMS_SIGN_EXIT=$?
set -e
FLOW_AFTER_FORMS=$(sha256sum "$RUN/flow-gate-result.json" | awk '{print $1}')
FORMS_ONLY_ISOLATED=false
if [[ $FORMS_SIGN_EXIT -eq 0 && $FLOW_BEFORE_FORMS == "$FLOW_AFTER_FORMS" ]] &&
   grep -Fq '| Restaurant template can create a project directly | Accepted | mutation-control | isolated mutation control only |' "$RUN/forms-evidence.md"; then
  FORMS_ONLY_ISOLATED=true
fi

FORMS_RUNBOOK_BEFORE_FLOW=$(sha256sum "$RUN/forms-runbook.md" | awk '{print $1}')
FORMS_EVIDENCE_BEFORE_FLOW=$(sha256sum "$RUN/forms-evidence.md" | awk '{print $1}')
set +e
"$FLOW_RECORDER" "$RUN/flow-gate-result.json" release_owner passed mutation-control \
  "isolated mutation control only" >"$RUN/flow-only-sign.log" 2>&1
FLOW_SIGN_EXIT=$?
set -e
FORMS_RUNBOOK_AFTER_FLOW=$(sha256sum "$RUN/forms-runbook.md" | awk '{print $1}')
FORMS_EVIDENCE_AFTER_FLOW=$(sha256sum "$RUN/forms-evidence.md" | awk '{print $1}')
FLOW_ONLY_ISOLATED=false
if [[ $FLOW_SIGN_EXIT -eq 0 && $FORMS_RUNBOOK_BEFORE_FLOW == "$FORMS_RUNBOOK_AFTER_FLOW" &&
      $FORMS_EVIDENCE_BEFORE_FLOW == "$FORMS_EVIDENCE_AFTER_FLOW" ]] &&
   [[ $(jq -r '.manual_signoffs.release_owner.status' "$RUN/flow-gate-result.json") == passed ]]; then
  FLOW_ONLY_ISOLATED=true
fi

cp "$RUN/domains-positive.json" "$EVIDENCE/logs/forms-signoff-domains-positive.json"
cp "$RUN/domains-positive.log" "$EVIDENCE/logs/forms-signoff-domains-positive.log"
cp "$RUN/domains-alias.log" "$EVIDENCE/logs/forms-signoff-domains-alias.log"
cp "$RUN/forms-only-sign.log" "$EVIDENCE/logs/forms-only-sign.log"
cp "$RUN/flow-only-sign.log" "$EVIDENCE/logs/flow-only-sign.log"

python3 - "$ROOT" "$EVIDENCE" "$BASE_EXIT" "$DOMAIN_EXIT" "$ALIAS_EXIT" "$FORMS_SIGN_EXIT" "$FLOW_SIGN_EXIT" "$FORMS_ONLY_ISOLATED" "$FLOW_ONLY_ISOLATED" <<'PY'
import datetime as dt
import json
import os
import pathlib
import subprocess
import sys
import tempfile

repo, evidence = map(pathlib.Path, sys.argv[1:3])
base_exit, domain_exit, alias_exit, forms_sign_exit, flow_sign_exit = map(int, sys.argv[3:8])
forms_only_isolated = sys.argv[8] == "true"
flow_only_isolated = sys.argv[9] == "true"
try:
    base = json.loads((evidence / "forms-regression-result.json").read_text())
except Exception as error:
    base = {"passed": False, "executed_count": 0, "error": str(error)}
try:
    domain_result = json.loads((evidence / "logs/forms-signoff-domains-positive.json").read_text())
except Exception as error:
    domain_result = {"passed": False, "domains": [], "checks": {}, "error": str(error)}
assertions = base.get("forms_gate_run", {}).get("assertions", {})
base_executed = int(base.get("executed_count", assertions.get("passed", 0) + assertions.get("failed", 0)))
checks = {
    "base_forms_regression": base_exit == 0 and base.get("passed") is True and base_executed > 0,
    "domains_derived_from_recorders": domain_exit == 0 and domain_result.get("passed") is True,
    "forms_only_does_not_sign_flow": forms_only_isolated,
    "flow_only_does_not_sign_forms": flow_only_isolated,
}
mutation = {
    "name": "aliased_signoff_recorders",
    "command": "verify_flow_signoff_domains.py with forms recorder symlinked to the Flow recorder",
    "exit_code": alias_exit,
    "red": alias_exit != 0,
}
result = {
    "schema_version": "sylvode.flow.forms-signoffs-result.v2",
    "release": "1.0.0",
    "source_head": subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip(),
    "domains": domain_result.get("domains", []),
    "domain_checks": domain_result.get("checks", {}),
    "checks": checks,
    "base": base,
    "isolation_runs": {
        "forms_only": {"exit_code": forms_sign_exit, "flow_source_unchanged": forms_only_isolated},
        "flow_only": {"exit_code": flow_sign_exit, "forms_sources_unchanged": flow_only_isolated},
    },
    "mutation": mutation,
    "execution_counts": {
        "forms_static_audit_assertions": base_executed,
        "signoff_independence_controls": len(checks) + 1,
    },
    "executed_count": len(checks) + 1,
    "executed_kind": "signoff_independence_controls",
    "passed": all(checks.values()) and mutation["red"],
    "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, temporary = tempfile.mkstemp(prefix=".flow-forms-signoffs-result.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "flow-forms-signoffs-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if result["passed"] else 1)
PY

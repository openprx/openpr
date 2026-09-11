#!/usr/bin/env bash
set -euo pipefail

# Sole supported writer for the v0.6 field_secrecy_denial manual row.

EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.6"
GATE_RESULT=""
KEY=""
STATUS_VALUE=""
REVIEWER=""
EVIDENCE_NOTE=""
FORCE=0
DRY_RUN=0

usage() {
  cat <<'EOF'
Usage: scripts/record-flow-v0.6-manual-signoff.sh --key field_secrecy_denial --status STATUS --reviewer NAME --evidence NOTE [OPTIONS]

Statuses: pending, passed, failed, needs_rework
Options:
  --gate-result PATH   Default: <evidence-root>/gate-result.json
  --evidence-root DIR  Default: /opt/working/sylvode-flow/evidence/v0.6
  --force              Permit replacing a passed/failed signed row
  --dry-run            Validate and print without writing
  --list-keys          Print field_secrecy_denial
  -h, --help           Show help

Exit: 0 recorded/valid dry run, 1 rejected, 2 usage/tool/malformed receipt.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --key) KEY="${2:?--key requires a value}"; shift 2 ;;
    --status) STATUS_VALUE="${2:?--status requires a value}"; shift 2 ;;
    --reviewer) REVIEWER="${2:?--reviewer requires a value}"; shift 2 ;;
    --evidence) EVIDENCE_NOTE="${2:?--evidence requires a value}"; shift 2 ;;
    --gate-result) GATE_RESULT="${2:?--gate-result requires a path}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a directory}"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --list-keys) echo field_secrecy_denial; exit 0 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$GATE_RESULT" ]] || GATE_RESULT="$EVIDENCE_ROOT/gate-result.json"
[[ "$KEY" == "field_secrecy_denial" ]] || { echo "FAIL: only field_secrecy_denial is valid" >&2; exit 1; }
case "$STATUS_VALUE" in pending|passed|failed|needs_rework) ;; *) echo "FAIL: invalid --status" >&2; exit 1 ;; esac
[[ -n "$REVIEWER" && -n "$EVIDENCE_NOTE" ]] || { echo "FAIL: --reviewer and --evidence must be non-empty" >&2; exit 1; }
[[ -f "$GATE_RESULT" ]] || { echo "FAIL: gate result missing: $GATE_RESULT" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "FAIL: python3 is required" >&2; exit 2; }

python3 - "$GATE_RESULT" "$STATUS_VALUE" "$REVIEWER" "$EVIDENCE_NOTE" "$FORCE" "$DRY_RUN" <<'PY'
import datetime as dt
import json
import os
import pathlib
import sys
import tempfile

path = pathlib.Path(sys.argv[1]).resolve()
status, reviewer, evidence, force, dry_run = sys.argv[2:]
try:
    doc = json.loads(path.read_text(encoding="utf-8"))
except (OSError, json.JSONDecodeError) as exc:
    print(f"FAIL: malformed gate result: {exc}", file=sys.stderr)
    raise SystemExit(2)
if doc.get("release") != "0.6.0" or set(doc.get("manual_signoffs", {})) != {"field_secrecy_denial"}:
    print("FAIL: gate result is not the v0.6 receipt", file=sys.stderr)
    raise SystemExit(2)
previous = doc["manual_signoffs"]["field_secrecy_denial"].get("status")
if previous in {"passed", "failed"} and force != "1":
    print(f"FAIL: refusing to overwrite signed {previous} row without --force", file=sys.stderr)
    raise SystemExit(1)
row = {
    "status": status,
    "reviewer": reviewer,
    "evidence": evidence,
    "signed_at": dt.datetime.now(dt.timezone.utc).isoformat(),
}
if dry_run == "1":
    print(json.dumps({"dry_run": True, "key": "field_secrecy_denial", "previous": previous, "new": row}, sort_keys=True))
    raise SystemExit(0)
doc["manual_signoffs"]["field_secrecy_denial"] = row
doc["accepted"] = bool(doc.get("candidate_ready")) and status == "passed"
doc["gate_passed"] = doc["accepted"]
doc["mode"] = "accepted" if doc["accepted"] else "blocked"
blockers = [item for item in doc.get("blockers", []) if not item.startswith("manual_field_secrecy_denial_")]
if status != "passed":
    blockers.append(f"manual_field_secrecy_denial_{status}")
doc["blockers"] = blockers
doc["counts"]["manual_pending"] = 1 if status == "pending" else 0
doc["counts"]["unresolved"] = len(blockers)
fd, temporary = tempfile.mkstemp(prefix=".gate-result.", suffix=".json", dir=path.parent)
with os.fdopen(fd, "w", encoding="utf-8") as handle:
    json.dump(doc, handle, ensure_ascii=False, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, path)
print(json.dumps({"recorded": True, "key": "field_secrecy_denial", "previous": previous,
                  "current": row, "candidate_ready": doc.get("candidate_ready"),
                  "gate_passed": doc["gate_passed"], "blockers": blockers}, ensure_ascii=False, sort_keys=True))
PY

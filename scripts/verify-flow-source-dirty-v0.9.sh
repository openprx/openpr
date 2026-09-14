#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
EVIDENCE_ROOT="$REPO_ROOT/.flow-gate/evidence/v0.9"
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT=${2:?}; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT=${2:?}; shift 2 ;;
    --json) shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done

mkdir -p /opt/worker/.cache "$EVIDENCE_ROOT/logs"
WORKTREE=$(mktemp -d /opt/worker/.cache/v09-source-dirty.XXXXXX)
LOG="$EVIDENCE_ROOT/logs/source-dirty-scope.log"
cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
  rm -rf -- "$WORKTREE"
}
trap cleanup EXIT
rm -rf -- "$WORKTREE"
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

status_lines() {
  git -C "$WORKTREE" status --porcelain=v1
}

: >"$LOG"
[[ -z $(status_lines) ]] || { echo 'baseline worktree is dirty' >>"$LOG"; exit 1; }
printf 'baseline_clean=green\n' >>"$LOG"

check_untracked() {
  local relative=$1 id=$2
  : >"$WORKTREE/$relative"
  if status_lines | grep -Fq "?? $relative"; then
    printf '%s=red\n' "$id" >>"$LOG"
  else
    printf '%s=missed\n' "$id" >>"$LOG"
    exit 1
  fi
  rm -f -- "$WORKTREE/$relative"
}

check_modified() {
  local relative=$1 id=$2
  printf '\n# v0.9 dirty-scope mutation\n' >>"$WORKTREE/$relative"
  if status_lines | grep -Fq " M $relative"; then
    printf '%s=red\n' "$id" >>"$LOG"
  else
    printf '%s=missed\n' "$id" >>"$LOG"
    exit 1
  fi
  git -C "$WORKTREE" checkout -- "$relative"
}

check_untracked docs/.v09-dirty-scope-mutation docs
check_untracked .github/.v09-dirty-scope-mutation github
check_modified README.md readme
check_untracked config/.v09-dirty-scope-mutation config
check_modified .gitignore gitignore

python3 - "$REPO_ROOT" "$EVIDENCE_ROOT" "$LOG" <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile

repo, evidence, log = map(pathlib.Path, sys.argv[1:])
rows = dict(line.split("=", 1) for line in log.read_text().splitlines())
mutations = {key: {"red": rows.get(key) == "red"} for key in ("docs", "github", "readme", "config", "gitignore")}
passed = rows.get("baseline_clean") == "green" and all(value["red"] for value in mutations.values())
result = {
    "schema_version":"sylvode.flow.source-dirty-scope-result.v1",
    "source_head":subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip(),
    "baseline_clean":{"green":rows.get("baseline_clean") == "green"},
    "mutations":mutations,
    "executed_count":6,
    "log":str(log),
    "log_sha256":hashlib.sha256(log.read_bytes()).hexdigest(),
    "passed":passed,
    "generated_at":dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, temporary = tempfile.mkstemp(prefix=".source-dirty-scope-result.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(temporary, evidence / "source-dirty-scope-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY

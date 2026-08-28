#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 editor-binding maturity runner -- SKELETON.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md v0.3 section
# ("scripts/verify-flow-editor-binding-v0.3.sh --candidate X --soak-hours 8
# --out evidence/v0.3/editor-binding-X.json"),
# sylvode-flow-editor-binding-result-v1.schema.json, and ADR-0006's six
# judging criteria (editor_corpus, soak, binding_patch, dependency_evidence,
# defects, manual_acceptance).
#
# This round only builds the skeleton: correct argument parsing, and a
# clear, non-zero-exit refusal when the underlying capability the real
# runner needs is not yet available. It must NEVER write a
# placeholder/fabricated evidence JSON.
#
# Exit codes: 0 = editor-binding-<candidate>.json written, 1 = required
# capability missing, 2 = usage/tool error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.3"
CANDIDATE=""
OUT_PATH=""
SOAK_HOURS=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-editor-binding-v0.3.sh --candidate {loro|yrs-yjs} --soak-hours N --out PATH [OPTIONS]

Runs the v0.3 editor-binding maturity suite for one candidate (IME/emoji/
combining-mark/selection-fallback/local-undo corpus, an N-hour dual-tab soak
with periodic destroy/remount, a binding-patch size diff, dependency
provenance evidence, and a defect/manual-acceptance summary) and writes a
schema-valid evidence/v0.3/editor-binding-<candidate>.json (single-candidate
shape consumed by aggregate-flow-editor-binding-v0.3.sh).

SKELETON STATUS: this round only implements argument parsing and the
capability check below; it does not run a real suite and never writes a
fabricated evidence file.

Required options:
  --candidate NAME    "loro" or "yrs-yjs".
  --soak-hours N        Soak duration; the frozen v0.3 value is 8
                          (sylvode-flow-editor-binding-result-v1.schema.json
                          pins runner_parameters.soak_hours to the const 8).
  --out PATH            Output path for the single-candidate JSON.

Options:
  --evidence-root DIR   Reserved for future use by the real runner.
                          Default: /opt/working/sylvode-flow/evidence/v0.3
  -h, --help             Show this help and exit 0.

Exit codes: 0 written, 1 capability missing, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --candidate) CANDIDATE="${2:?--candidate requires loro or yrs-yjs}"; shift 2 ;;
    --soak-hours) SOAK_HOURS="${2:?--soak-hours requires a number}"; shift 2 ;;
    --out) OUT_PATH="${2:?--out requires a PATH argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$CANDIDATE" ]]; then
  echo "FAIL: --candidate is required (loro or yrs-yjs)" >&2
  usage >&2
  exit 2
fi
if [[ "$CANDIDATE" != "loro" && "$CANDIDATE" != "yrs-yjs" ]]; then
  echo "FAIL: --candidate must be 'loro' or 'yrs-yjs', got: $CANDIDATE" >&2
  exit 2
fi
if [[ -z "$SOAK_HOURS" ]]; then
  echo "FAIL: --soak-hours is required" >&2
  usage >&2
  exit 2
fi
if ! [[ "$SOAK_HOURS" =~ ^[0-9]+$ ]]; then
  echo "FAIL: --soak-hours must be a non-negative integer, got: $SOAK_HOURS" >&2
  exit 2
fi
if [[ "$SOAK_HOURS" != "8" ]]; then
  echo "FAIL: --soak-hours must be 8 for v0.3 (sylvode-flow-editor-binding-result-v1.schema.json pins runner_parameters.soak_hours to the const 8), got: $SOAK_HOURS" >&2
  exit 2
fi
if [[ -z "$OUT_PATH" ]]; then
  echo "FAIL: --out is required" >&2
  usage >&2
  exit 2
fi

SPIKE_DIR="$ROOT_DIR/spikes/collab-$CANDIDATE"
echo "Evidence root (reserved for the real runner): $EVIDENCE_ROOT" >&2
if [[ ! -d "$SPIKE_DIR" ]]; then
  echo "FAIL: candidate spike directory not found: $SPIKE_DIR" >&2
  exit 2
fi

MISSING=()

# The soak harness (long-running dual-tab destroy/remount driver with leak
# accounting) is genuinely absent from this repo today -- unlike the
# isolation host, this is not blocked by an ADR, it simply has not been
# built yet. Probe for it explicitly rather than guessing.
SOAK_RUNNER_CANDIDATES=(
  "$ROOT_DIR/spikes/collab-shared/src/soak-runner.ts"
  "$ROOT_DIR/spikes/collab-shared/src/editor-soak.ts"
  "$ROOT_DIR/scripts/soak-flow-editor-binding-v0.3.sh"
)
SOAK_RUNNER_FOUND=""
for candidate_path in "${SOAK_RUNNER_CANDIDATES[@]}"; do
  if [[ -f "$candidate_path" ]]; then
    SOAK_RUNNER_FOUND="$candidate_path"
    break
  fi
done
if [[ -z "$SOAK_RUNNER_FOUND" ]]; then
  MISSING+=("8-hour dual-tab soak harness: none of ${SOAK_RUNNER_CANDIDATES[*]} exist yet (needs: periodic destroy/remount every <=15 min, >=30 transactions/min, subscription_leak_delta/timer_leak_delta accounting -- ADR-0006's soak criterion)")
fi

# manual_acceptance (reviewer, browser, OS, input_method, per-item
# pass/fail, video_index/issue_link) is inherently produced by a human
# following a review script, not by automation -- there is no "capability"
# to build that removes this precondition; verify-flow-v0.3-json.sh does
# not (and should not) attempt to synthesize it either.
MISSING+=("manual_acceptance: requires a human reviewer to actually run the IME/selection acceptance script and record browser/OS/input_method/per-item results -- this can never be produced by an automated runner and must come from scripts/record-flow-v0.3-manual-signoff.sh's editor_ime/selection_cursor rows instead")

if [[ ${#MISSING[@]} -gt 0 ]]; then
  echo "FAIL: verify-flow-editor-binding-v0.3.sh cannot produce a schema-valid evidence/v0.3/editor-binding-$CANDIDATE.json yet." >&2
  echo "Missing capability/precondition:" >&2
  for m in "${MISSING[@]}"; do
    echo "  - $m" >&2
  done
  echo "Refusing to write a placeholder/fabricated $OUT_PATH." >&2
  exit 1
fi

echo "FAIL: all listed preconditions satisfied but verify-flow-editor-binding-v0.3.sh still has no real editor_corpus/binding_patch/dependency_evidence/defects implementation (skeleton only)." >&2
exit 1

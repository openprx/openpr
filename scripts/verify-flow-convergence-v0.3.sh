#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 P2a/P2b/P3 convergence corpus runner -- SKELETON.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md v0.3 section
# ("scripts/verify-flow-convergence-v0.3.sh --candidate X --out
# evidence/v0.3/convergence-X.json"), /opt/working/sylvode-flow/testing/
# convergence-corpus.md, sylvode-flow-convergence-result-v1.schema.json, and
# ADR-0014-isolated-apply-host.md section 9 (the isolation.{rust,web}.cases[]
# five-path wire this file's "isolation" object must satisfy).
#
# This round only builds the skeleton: correct argument parsing, and a
# clear, non-zero-exit refusal when the underlying capability the real
# runner needs is not yet available. It must NEVER write a
# placeholder/fabricated evidence JSON.
#
# Exit codes: 0 = convergence-<candidate>.json written, 1 = required
# capability missing, 2 = usage/tool error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.3"
CANDIDATE=""
OUT_PATH=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-convergence-v0.3.sh --candidate {loro|yrs-yjs} --out PATH [OPTIONS]

Runs the v0.3 convergence corpus (P3 shared correctness fixtures) plus the
P2a native / P2b web isolated-apply safety cases for one candidate and
writes a schema-valid evidence/v0.3/convergence-<candidate>.json
(single-candidate shape consumed by aggregate-flow-convergence-v0.3.sh).

SKELETON STATUS: this round only implements argument parsing and the
capability check below; it does not run a real corpus and never writes a
fabricated evidence file.

Required options:
  --candidate NAME   "loro" or "yrs-yjs".
  --out PATH          Output path for the single-candidate convergence JSON.

Options:
  --evidence-root DIR   Reserved for future use by the real runner.
                          Default: /opt/working/sylvode-flow/evidence/v0.3
  --contracts-root DIR  Root for decisions/*.md lookups.
                          Default: /opt/working/sylvode-flow
  -h, --help             Show this help and exit 0.

Exit codes: 0 written, 1 capability missing, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --candidate) CANDIDATE="${2:?--candidate requires loro or yrs-yjs}"; shift 2 ;;
    --out) OUT_PATH="${2:?--out requires a PATH argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
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
if [[ -z "$OUT_PATH" ]]; then
  echo "FAIL: --out is required" >&2
  usage >&2
  exit 2
fi

SPIKE_DIR="$ROOT_DIR/spikes/collab-$CANDIDATE"
if [[ ! -f "$SPIKE_DIR/Cargo.toml" ]]; then
  echo "FAIL: candidate spike crate not found: $SPIKE_DIR/Cargo.toml" >&2
  exit 2
fi

ADR_0014="$CONTRACTS_ROOT/decisions/ADR-0014-isolated-apply-host.md"
if [[ ! -f "$ADR_0014" ]]; then
  echo "FAIL: cannot read $ADR_0014 to check isolation host readiness" >&2
  exit 2
fi
ADR_0014_STATUS="$(grep -m1 '^- 状态：' "$ADR_0014" | sed 's/^- 状态：//')"

echo "Candidate spike crate found: $SPIKE_DIR" >&2
echo "Evidence root (reserved for the real runner): $EVIDENCE_ROOT" >&2
echo "ADR-0014 (isolated apply host) status: $ADR_0014_STATUS" >&2

if [[ "$ADR_0014_STATUS" != "Accepted" ]]; then
  cat >&2 <<EOF
FAIL: verify-flow-convergence-v0.3.sh cannot produce a schema-valid
evidence/v0.3/convergence-$CANDIDATE.json yet.

Missing capability: sylvode-flow-convergence-result-v1.schema.json requires
per-candidate isolation.rust and isolation.web blocks with cases[] covering
all five paths (completed / cpu_ceiling / wall_ceiling / memory_ceiling /
meter_unavailable), each backed by a real fork-no-exec zygote host
(ADR-0014 section 0), a SIGPROF-armed CPU ceiling (section 1.1), a shared
atomic high-water memory page (section 2), and a wall watchdog. Fabricating
those numbers instead of measuring them is exactly the fake-green pattern
verify-flow-v0.3-json.sh's numeric checks exist to catch -- so this runner
refuses to write anything rather than invent isolation cases.

ADR-0014-isolated-apply-host.md is currently "$ADR_0014_STATUS", not
"Accepted". Its own section 10 (R20, GO-CALIBRATION) explicitly forbids this:
the calibration work under spikes/collab-shared/src/isolation/ (zygote.rs,
shared_page.rs, child.rs, termination.rs, calibrate.rs, alloc.rs, frame.rs)
and spikes/collab-shared/src/bin/isolation-calibrate.rs may build and run,
but is scoped to answering three empirical questions only -- it "不产生
evidence 通过判定" and "isolated_apply_not_async_timeout 继续保持 pending"
until the ADR itself transitions to Accepted with reviewer sign-off.

Separately, and independently of ADR-0014: this skeleton also does not yet
implement the P3 shared-correctness corpus runner (the 14 fixture_coverage
categories and 8 boundary_results pairs from testing/convergence-corpus.md),
which does not depend on the isolation host and could be built sooner --
but a partial convergence-<candidate>.json (correctness cases without
isolation cases) would still fail sylvode-flow-convergence-result-v1.schema.json's
"required": ["isolation", ...] on the candidate_result object, so there is
no partial-but-valid file this runner could legitimately emit today either.

Refusing to write a placeholder/fabricated $OUT_PATH.
EOF
  exit 1
fi

# Unreachable in this round: once ADR-0014 is Accepted, this is where the
# real corpus + isolation runner would execute and atomically write
# $OUT_PATH. Until implemented, reaching here is a bug, not a silent success.
echo "FAIL: ADR-0014 is Accepted but verify-flow-convergence-v0.3.sh still has no real corpus/isolation implementation (skeleton only)." >&2
exit 1

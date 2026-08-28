#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 P1 native benchmark runner -- SKELETON.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md v0.3 section
# ("scripts/benchmark-flow-v0.3.sh --candidate X --out evidence/v0.3/benchmark-X.json"),
# /opt/working/sylvode-flow/testing/benchmark-spec.md, and the
# sylvode-flow-benchmark-result-v1.schema.json "hostile_input_safety" block
# (decode_apply_cpu_ms / decode_apply_wall_ms / isolated_apply_memory_bytes),
# which per ADR-0014-isolated-apply-host.md section 4 must be measured by the
# SAME native isolated-apply host P1 benchmark and P2a safety share.
#
# This round only builds the skeleton: correct argument parsing, and a clear,
# non-zero-exit refusal when the underlying capability the real runner needs
# is not yet available. It must NEVER write a placeholder/fabricated
# evidence JSON -- that is exactly the fake-green pattern this v0.3 work
# exists to close.
#
# Exit codes: 0 = benchmark-<candidate>.json written, 1 = required
# capability missing (documented below), 2 = usage/tool error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.3"
CANDIDATE=""
OUT_PATH=""

usage() {
  cat <<'EOF'
Usage: scripts/benchmark-flow-v0.3.sh --candidate {loro|yrs-yjs} --out PATH [OPTIONS]

Runs the v0.3 P1 native in-process benchmark for one candidate and writes a
schema-valid evidence/v0.3/benchmark-<candidate>.json (single-candidate
shape reused for aggregation by aggregate-flow-benchmark-v0.3.sh, per
gate-commands.md "聚合前的逐候选文件目前复用聚合 schema 的 candidate_result 定义").

SKELETON STATUS: this round only implements argument parsing and the
capability check below; it does not run a real benchmark and never writes a
fabricated evidence file.

Required options:
  --candidate NAME   "loro" or "yrs-yjs".
  --out PATH          Output path for the single-candidate benchmark JSON.

Options:
  --evidence-root DIR   Root used to resolve contract/status file lookups
                          (does not need to match verify's evidence root for
                          this skeleton). Default: /opt/working/sylvode-flow/evidence/v0.3
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
FAIL: benchmark-flow-v0.3.sh cannot produce a schema-valid
evidence/v0.3/benchmark-$CANDIDATE.json yet.

Missing capability: sylvode-flow-benchmark-result-v1.schema.json requires a
"hostile_input_safety" block per candidate (decode_apply_cpu_ms,
decode_apply_wall_ms, isolated_apply_memory_bytes). Per ADR-0014 section 4,
these three numbers must be produced by the SAME native isolated-apply host
that the P2a safety runner uses (not measured a second, independent way) --
so this benchmark runner cannot legitimately fill that block until the
isolation host itself is ready.

ADR-0014-isolated-apply-host.md is currently "$ADR_0014_STATUS", not
"Accepted". Its own section 10 (R20, GO-CALIBRATION) is explicit that this is
intentional: the calibration run in progress under
spikes/collab-shared/src/isolation/ "不产生 evidence 通过判定" and
"isolated_apply_not_async_timeout 继续保持 pending" until three empirical
questions are answered, limits-v1.md's platform-scope revision has landed
(it has), and the ADR itself transitions to Accepted with reviewer sign-off.

This is not this script's decision to override. Re-run once ADR-0014 is
Accepted; at that point wire this runner to the real host binary (see
spikes/collab-shared/src/bin/isolation-calibrate.rs and
spikes/collab-shared/src/isolation/ for the mechanism that will back it) and
to the candidate's own cold-start/apply/bootstrap measurement harness for the
other five budget metrics (browser_engine_bundle, cold_start_ms,
apply_update_ms, bootstrap_10k_ops_ms, bootstrap_100k_ops_ms,
peak_memory_100k_ops_bytes), none of which this skeleton implements either.

Refusing to write a placeholder/fabricated $OUT_PATH.
EOF
  exit 1
fi

# Unreachable in this round: once ADR-0014 is Accepted, this is where the
# real P1 in-process benchmark (bundle build + gzip, cold start, apply,
# 10k/100k bootstrap, peak memory, plus the shared hostile_input_safety
# measurement) would run and atomically write $OUT_PATH. Until that is
# implemented, treat reaching here as a bug, not a silent success.
echo "FAIL: ADR-0014 is Accepted but benchmark-flow-v0.3.sh still has no real measurement implementation (skeleton only)." >&2
exit 1

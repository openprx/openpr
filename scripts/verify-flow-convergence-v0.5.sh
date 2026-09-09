#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v0.5"
CLIENTS=""
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-convergence-v0.5.sh --clients 10 --json [OPTIONS]

Runs a locked ten-client Loro corpus through the production Rust adapter and
the browser WASM binding in independently permuted orders. Both runtimes must
converge to the same semantic hash, import each other's snapshots, and detect
the built-in dropped-client negative control.

Options:
  --clients N          Required; the v0.5 hard gate passes only for N=10.
  --repo-root DIR      Repository root. Default: this checkout.
  --contracts-root DIR Read-only contract root. Default: /opt/working/sylvode-flow.
  --evidence-root DIR  Output directory. Default: contract evidence/v0.5.
  --json               Required.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --clients) CLIENTS="${2:?--clients requires N}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires DIR}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires DIR}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires DIR}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
[[ "$CLIENTS" =~ ^[1-9][0-9]*$ ]] || { echo "FAIL: --clients must be a positive integer" >&2; exit 2; }
for tool in cargo git jq sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
if command -v bun >/dev/null 2>&1; then
  BUN_BIN="$(command -v bun)"
elif [[ -x /home/ck/.bun/bin/bun ]]; then
  BUN_BIN="/home/ck/.bun/bin/bun"
else
  echo "FAIL: missing required command: bun" >&2
  exit 2
fi
[[ -f "$CONTRACTS_ROOT/versions/v0.5-collaboration.md" ]] || {
  echo "FAIL: v0.5 collaboration contract not found under $CONTRACTS_ROOT" >&2; exit 2;
}

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
SOURCE_DIRTY=false
[[ -z "$(git -C "$REPO_ROOT" status --porcelain)" ]] || SOURCE_DIRTY=true
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
WORK_DIR="$(mktemp -d /opt/worker/.cache/flow-v05-convergence.XXXXXX)"
cleanup() { rm -rf "$WORK_DIR"; }
trap cleanup EXIT

started_ms="$(date +%s%3N)"
set +e
(cd "$REPO_ROOT" && cargo run --release -p collab-core --example flow_v05_convergence_rust -- \
  generate "$CLIENTS" "$WORK_DIR") >"$WORK_DIR/rust-generate.log" 2>&1
RUST_GENERATE_EXIT=$?
set -e

WASM_EXIT=125
RUST_VERIFY_EXIT=125
if [[ $RUST_GENERATE_EXIT -eq 0 ]]; then
  set +e
  (cd "$REPO_ROOT" && "$BUN_BIN" spikes/collab-roundtrip/src/v05-convergence-wasm.ts "$WORK_DIR" "$CLIENTS") \
    >"$WORK_DIR/wasm.log" 2>&1
  WASM_EXIT=$?
  set -e
fi
if [[ $WASM_EXIT -eq 0 ]]; then
  EXPECTED_HASH="$(jq -r '.semantic_hash' "$WORK_DIR/wasm-result.json")"
  set +e
  (cd "$REPO_ROOT" && cargo run --release -p collab-core --example flow_v05_convergence_rust -- \
    verify "$WORK_DIR/wasm-merged.bin" "$EXPECTED_HASH" "$WORK_DIR/rust-verify-wasm.json") \
    >"$WORK_DIR/rust-verify.log" 2>&1
  RUST_VERIFY_EXIT=$?
  set -e
fi
duration_ms="$(( $(date +%s%3N) - started_ms ))"

rust='{}'; wasm='{}'; verify='{}'
[[ -f "$WORK_DIR/rust-result.json" ]] && rust="$(jq -c . "$WORK_DIR/rust-result.json")"
[[ -f "$WORK_DIR/wasm-result.json" ]] && wasm="$(jq -c . "$WORK_DIR/wasm-result.json")"
[[ -f "$WORK_DIR/rust-verify-wasm.json" ]] && verify="$(jq -c . "$WORK_DIR/rust-verify-wasm.json")"

VIOLATIONS='[]'
add_violation() { VIOLATIONS="$(jq -c --arg value "$1" '. + [$value]' <<<"$VIOLATIONS")"; }
[[ "$CLIENTS" -eq 10 ]] || add_violation "ten_client_convergence requires exactly 10 clients, got $CLIENTS"
[[ "$SOURCE_DIRTY" == false ]] || add_violation "source worktree is dirty; official convergence evidence requires a clean source"
[[ $RUST_GENERATE_EXIT -eq 0 ]] || add_violation "Rust corpus generation/replay failed (exit=$RUST_GENERATE_EXIT)"
[[ $WASM_EXIT -eq 0 ]] || add_violation "WASM corpus replay failed or was not run (exit=$WASM_EXIT)"
[[ $RUST_VERIFY_EXIT -eq 0 ]] || add_violation "Rust re-import of the WASM snapshot failed or was not run (exit=$RUST_VERIFY_EXIT)"
if [[ $RUST_GENERATE_EXIT -eq 0 && $WASM_EXIT -eq 0 && $RUST_VERIFY_EXIT -eq 0 ]]; then
  [[ "$(jq -r '.semantic_hash' <<<"$rust")" == "$(jq -r '.semantic_hash' <<<"$wasm")" ]] || add_violation "Rust and WASM semantic hashes differ"
  [[ "$(jq -r '.semantic_node_count' <<<"$rust")" -eq "$CLIENTS" ]] || add_violation "Rust replay did not retain one semantic node per client"
  [[ "$(jq -r '.semantic_node_count' <<<"$wasm")" -eq "$CLIENTS" ]] || add_violation "WASM replay did not retain one semantic node per client"
  [[ "$(jq -r '.reverse_replay_equal and .mutation.detected' <<<"$rust")" == true ]] || add_violation "Rust reorder/negative controls did not both pass"
  [[ "$(jq -r '.reverse_replay_equal and .rust_wasm_hash_equal and .rust_snapshot_import_equal and .mutation.detected' <<<"$wasm")" == true ]] || add_violation "WASM reorder/cross-runtime/negative controls did not all pass"
  [[ "$(jq -r '.matched' <<<"$verify")" == true ]] || add_violation "Rust could not re-import the WASM snapshot at the same hash"
fi

PASSED=false
[[ "$(jq 'length' <<<"$VIOLATIONS")" -eq 0 ]] && PASSED=true
mkdir -p "$EVIDENCE_ROOT/logs"
cp "$WORK_DIR/rust-generate.log" "$EVIDENCE_ROOT/logs/convergence-v0.5-rust-generate.log"
cp "$WORK_DIR/wasm.log" "$EVIDENCE_ROOT/logs/convergence-v0.5-wasm.log" 2>/dev/null || true
cp "$WORK_DIR/rust-verify.log" "$EVIDENCE_ROOT/logs/convergence-v0.5-rust-verify.log" 2>/dev/null || true

OUT="$EVIDENCE_ROOT/convergence-result.json"
jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" --argjson dirty "$SOURCE_DIRTY" \
  --argjson clients "$CLIENTS" --argjson duration_ms "$duration_ms" \
  --argjson rust "$rust" --argjson wasm "$wasm" --argjson verify "$verify" \
  --argjson rust_exit "$RUST_GENERATE_EXIT" --argjson wasm_exit "$WASM_EXIT" \
  --argjson verify_exit "$RUST_VERIFY_EXIT" --argjson violations "$VIOLATIONS" --argjson passed "$PASSED" \
  '{
    schema_version:"sylvode.flow.convergence-v0.5-result.v1",
    release:"0.5", source_head:$head, source_dirty:$dirty, generated_at:$generated_at,
    fixture:{locked:true,clients:$clients,distinct_client_updates:$clients,operations_per_client:3,
      unicode:true,replay_injection:["duplicate","reorder"],corpus_seed:424242},
    execution:{duration_ms:$duration_ms,rust_generate_exit:$rust_exit,wasm_replay_exit:$wasm_exit,
      rust_verify_wasm_exit:$verify_exit,build_profile:"release"},
    rust:$rust, wasm:$wasm, rust_verify_wasm:$verify,
    falsification:{drop_one_client_update_detected:(($rust.mutation.detected // false) and ($wasm.mutation.detected // false))},
    hard_gates:{ten_client_convergence:{status:(if $passed then "passed" else "failed" end),passed:$passed}},
    violations:$violations, passed:$passed
  }' >"$OUT.tmp"
mv -f "$OUT.tmp" "$OUT"
jq . "$OUT"
[[ "$PASSED" == true ]]

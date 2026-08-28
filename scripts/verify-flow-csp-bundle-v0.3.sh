#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 CSP bundle verifier -- SKELETON.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md v0.3 section
# ("scripts/verify-flow-csp-bundle-v0.3.sh --policy deploy/caddy/Caddyfile
# --json"). Runs a production build of BOTH candidates, serves each behind
# the real Caddy CSP header, loads a cold deep route in a real browser,
# exercises editor/CRDT/worker/sync, scans the final HTML/JS/worker/glue for
# executable inline script / eval / indirect eval / new Function / 3rd-party
# connect, and checks the policy's escape tokens (unsafe-inline, unsafe-eval,
# wasm-unsafe-eval) are justified per-candidate-artifact, not blanket.
#
# gate-commands.md also flags that this artifact currently has NO frozen
# schema (docs/schemas/sylvode-flow-*-v1.schema.json has no
# csp-bundle-result schema yet) -- so even once the browser-driving part of
# this runner exists, the gate can only check its path/checksum, not its
# candidate-scoped CSP-escape content, until that schema is written.
#
# This round only builds the skeleton: correct argument parsing, and a
# clear, non-zero-exit refusal when the underlying capability the real
# runner needs is not yet available. It must NEVER write a
# placeholder/fabricated evidence JSON.
#
# Exit codes: 0 = csp-bundle-result.json written, 1 = required capability
# missing, 2 = usage/tool error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.3"
SCHEMA_DIR="$ROOT_DIR/docs/schemas"
POLICY_PATH=""
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-csp-bundle-v0.3.sh --policy PATH [--json] [OPTIONS]

Runs the v0.3 CSP bundle verifier against both candidates and writes
evidence/v0.3/csp-bundle-result.json.

SKELETON STATUS: this round only implements argument parsing and the
capability check below; it does not drive a real browser and never writes a
fabricated evidence file.

Required options:
  --policy PATH   Path to the production Caddyfile whose CSP header is
                    under test (contract default: deploy/caddy/Caddyfile).

Options:
  --json                 Accepted for contract compatibility
                          (scripts/verify-flow-csp-bundle-v0.3.sh --policy
                          deploy/caddy/Caddyfile --json is the frozen
                          required_commands entry); output is always
                          machine-readable regardless of this flag.
  --evidence-root DIR    Reserved for future use by the real runner.
                          Default: /opt/working/sylvode-flow/evidence/v0.3
  --schema-dir DIR        Reserved for future use once a
                          csp-bundle-result schema is frozen.
                          Default: <repo>/docs/schemas
  -h, --help              Show this help and exit 0.

Exit codes: 0 written, 1 capability missing, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --policy) POLICY_PATH="${2:?--policy requires a PATH argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --schema-dir) SCHEMA_DIR="${2:?--schema-dir requires a DIR argument}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$POLICY_PATH" ]]; then
  echo "FAIL: --policy is required" >&2
  usage >&2
  exit 2
fi
if [[ ! -f "$POLICY_PATH" ]]; then
  echo "FAIL: --policy file does not exist: $POLICY_PATH" >&2
  exit 2
fi

echo "Policy file found: $POLICY_PATH" >&2
echo "Evidence root (reserved for the real runner): $EVIDENCE_ROOT" >&2
echo "Schema dir (reserved until a csp-bundle-result schema is frozen): $SCHEMA_DIR" >&2
if [[ $JSON_MODE -eq 1 ]]; then
  echo "(--json acknowledged; this skeleton has no machine output to emit yet)" >&2
fi

MISSING=()

SCHEMA_FILE="$SCHEMA_DIR/sylvode-flow-csp-bundle-result-v1.schema.json"
if [[ ! -f "$SCHEMA_FILE" ]]; then
  MISSING+=("frozen schema: $SCHEMA_FILE does not exist yet (gate-commands.md: 'csp-bundle-result.json 是 v0.3 必需 artifact 却还没有 schema, gate schema 目前只能校验它的路径与 checksum, 验不了 candidate-scoped 的 CSP escape、浏览器 violation 与 policy 正当化字段')")
fi

BUILD_TARGET_CANDIDATES=(
  "$ROOT_DIR/spikes/collab-loro/vite.config.ts"
  "$ROOT_DIR/spikes/collab-yrs-yjs/vite.config.ts"
)
for build_target in "${BUILD_TARGET_CANDIDATES[@]}"; do
  if [[ ! -f "$build_target" ]]; then
    MISSING+=("candidate production bundle build target not found: $build_target (each candidate needs its own minimal Page/Block entry build per benchmark-spec.md's bundle measurement, which this CSP verifier also depends on to serve a cold deep route)")
  fi
done

if ! command -v npx >/dev/null 2>&1 && [[ ! -x "$ROOT_DIR/node_modules/.bin/playwright" ]]; then
  MISSING+=("headless browser driver: neither 'npx' nor $ROOT_DIR/node_modules/.bin/playwright is available to load a cold deep route, exercise editor/CRDT/worker, and capture securitypolicyviolation events")
fi

if [[ ${#MISSING[@]} -gt 0 ]]; then
  echo "FAIL: verify-flow-csp-bundle-v0.3.sh cannot produce a real evidence/v0.3/csp-bundle-result.json yet." >&2
  echo "Missing capability/precondition:" >&2
  for m in "${MISSING[@]}"; do
    echo "  - $m" >&2
  done
  echo "Refusing to write a placeholder/fabricated csp-bundle-result.json (a static grep or dev-server-only check would not satisfy gate-commands.md: '只跑 dev server、只静态 grep、未实际触发 editor/CRDT 路径或遗漏 CSP violation 均失败')." >&2
  exit 1
fi

echo "FAIL: all listed preconditions satisfied but verify-flow-csp-bundle-v0.3.sh still has no real per-candidate build/serve/browser-drive/scan implementation (skeleton only)." >&2
exit 1

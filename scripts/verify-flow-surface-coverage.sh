#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow REST/MCP/CLI/UI surface-coverage verifier (v0.4-v1.0 shared).
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md, "Surface
# coverage verifier (v0.4-v1.0 共用)" section.
#
# Parses the five frozen contract files and recomputes every cross-reference
# they require, then checks every declared promise against the
# shipped implementation: MCP names come from executing list-tools, CLI
# commands from executing sylvode's command tree, and REST identities from
# the Axum route registrations assembled by apps/api/src/main.rs.
# the full 49-endpoint REST<->matrix 1:1 correspondence, matrix<->live
# MCP tool/resource/CLI-command bidirectional coverage (no orphans, no
# unknown refs), the three-item MCP not_exposed allowlist with
# endpoint/reason matching, CLI not_exposed reason legality, UI adapter
# membership, reason-column/token consistency, and version inversions.
# It never substitutes a hand-written expected count for a real set
# comparison (parsing logic: scripts/lib/flow_surface_coverage.py).
#
# Exit codes: 0 = zero contract or implementation-parity violations,
# 1 = one or more violations found, 2 = usage/tool/evidence malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
REPO_ROOT="$ROOT_DIR"
RELEASE=""
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-surface-coverage.sh --release X.Y --json [OPTIONS]

Parses contracts/{rest-api-v1,mcp-surface-v1,cli-surface-v1,ui-surface-v1,
surface-coverage-v1}.md under --contracts-root, recomputes the full
REST<->MCP<->CLI<->UI cross-reference, probes the shipped MCP/CLI binaries
and API route registrations, and writes
evidence/vX.Y/surface-coverage-result.json (schema:
docs/schemas/sylvode-flow-surface-coverage-result-v1.schema.json).

Options:
  --release X.Y           Release view, e.g. "0.4". Required. Determines
                          counts.rest_in_release (REST rows whose own
                          version is <= X.Y); the full 49-row contract and
                          all violation checks are always evaluated in
                          full regardless of release.
  --contracts-root DIR     Root containing contracts/. Default:
                          /opt/working/sylvode-flow
  --evidence-root DIR     Where surface-coverage-result.json is written.
                          Default: <contracts-root>/evidence/vX.Y
  --repo-root DIR         Repository whose HEAD becomes source_head.
                          Default: this checkout.
  --json                  Required for CLI-contract compatibility (the
                          frozen required_commands entry uses --json).
  -h, --help              Show this help and exit 0.

Exit codes: 0 zero violations, 1 one or more violations, 2 usage/tool error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) RELEASE="${2:?--release requires a value, e.g. 0.4}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$RELEASE" ]]; then
  echo "FAIL: --release is required (e.g. --release 0.4)" >&2
  usage >&2
  exit 2
fi
if ! [[ "$RELEASE" =~ ^[0-9]+\.[0-9]+$ ]]; then
  echo "FAIL: --release must look like X.Y (got: $RELEASE)" >&2
  exit 2
fi
if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$EVIDENCE_ROOT" ]]; then
  EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v$RELEASE"
fi

for tool in jq sha256sum git python3 cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
for f in rest-api-v1.md mcp-surface-v1.md cli-surface-v1.md ui-surface-v1.md surface-coverage-v1.md; do
  if [[ ! -f "$CONTRACTS_ROOT/contracts/$f" ]]; then
    echo "FAIL: missing contract file: $CONTRACTS_ROOT/contracts/$f" >&2
    exit 2
  fi
done
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
mkdir -p "$EVIDENCE_ROOT"

# These binaries are the shipped registries. Building and executing them is
# intentional: source grep cannot prove that a declaration reached the binary.
(cd "$REPO_ROOT" && cargo build -p mcp-server --bin list-tools --bin sylvode) >&2
MCP_OUTPUT="$(mktemp)"
"$REPO_ROOT/target/debug/list-tools" > "$MCP_OUTPUT"
IMPL_OUT="$(mktemp)"
set +e
python3 "$ROOT_DIR/scripts/lib/flow_surface_implementation.py" \
  --contracts-root "$CONTRACTS_ROOT" --repo-root "$REPO_ROOT" --release "$RELEASE" \
  --mcp-output "$MCP_OUTPUT" --cli-binary "$REPO_ROOT/target/debug/sylvode" > "$IMPL_OUT"
IMPL_EXIT=$?
set -e
rm -f "$MCP_OUTPUT"
if ! jq empty "$IMPL_OUT" >/dev/null 2>&1; then
  echo "FAIL: implementation surface probe did not produce valid JSON (exit=$IMPL_EXIT)" >&2
  cat "$IMPL_OUT" >&2
  rm -f "$IMPL_OUT"
  exit 2
fi

PARSE_OUT="$(mktemp)"
set +e
python3 "$ROOT_DIR/scripts/lib/flow_surface_coverage.py" --contracts-root "$CONTRACTS_ROOT" --release "$RELEASE" > "$PARSE_OUT"
PARSE_EXIT=$?
set -e

if ! jq empty "$PARSE_OUT" >/dev/null 2>&1; then
  echo "FAIL: parser did not produce valid JSON (exit=$PARSE_EXIT)" >&2
  cat "$PARSE_OUT" >&2
  rm -f "$PARSE_OUT"
  exit 2
fi

RESULT="$(jq --slurpfile implementation "$IMPL_OUT" \
  --arg release "$RELEASE" --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" \
  '{
    schema_version: "sylvode.flow.surface-coverage-result.v1",
    release: $release,
    source_head: $head,
    generated_at: $generated_at,
    contracts: .contracts,
    counts: .counts,
    implementation_parity: $implementation[0],
    violations: (.violations + {
      contract_mcp_missing_live: $implementation[0].mcp.contract_missing_in_implementation,
      contract_rest_missing_implementation: $implementation[0].rest.contract_missing_in_implementation,
      contract_cli_missing_implementation: $implementation[0].cli.contract_missing_in_implementation
    }),
    passed: (.passed and $implementation[0].passed)
  }' "$PARSE_OUT")"
rm -f "$PARSE_OUT" "$IMPL_OUT"

# Sanity floor: an empty/gutted/unparseable contract file produces zero rows on
# every side of the comparison, which is trivially self-consistent (0 == 0 == 0
# == 0) and would otherwise fall out of the violation logic as a vacuous
# `passed: true` -- a false green on genuinely malformed input, not a real
# empty-but-valid state (surface-coverage-v1.md freezes rest_total etc. at a
# fixed nonzero count; zero is never legitimate). Caught here, before writing
# any artifact, as "evidence malformed" (exit 2), not folded into `passed`.
for count_key in rest_total matrix_rows mcp_tools_total cli_commands_total; do
  count_val="$(jq -r --arg k "$count_key" '.counts[$k] // 0' <<<"$RESULT")"
  if [[ "$count_val" -eq 0 ]]; then
    echo "FAIL: counts.$count_key=0 -- one or more contract files under $CONTRACTS_ROOT/contracts parsed to zero rows (empty file, wrong format, or path mix-up); refusing to write a vacuously-consistent surface-coverage-result.json" >&2
    exit 2
  fi
done

OUT_PATH="$EVIDENCE_ROOT/surface-coverage-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"

PASSED="$(jq -r '.passed' <<<"$RESULT")"
TOTAL_VIOLATIONS="$(jq '[.violations[] | length] | add' <<<"$RESULT")"
echo "SURFACE COVERAGE (release=$RELEASE): passed=$PASSED total_violations=$TOTAL_VIOLATIONS" >&2
echo "wrote $OUT_PATH" >&2
if [[ "$PASSED" != "true" ]]; then
  jq -r '.violations | to_entries[] | select(.value | length > 0) | "  [\(.key)] " + (.value | join("; "))' <<<"$RESULT" >&2
fi

echo "$RESULT"

if [[ "$PASSED" == "true" ]]; then
  exit 0
fi
exit 1

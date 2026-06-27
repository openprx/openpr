#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAP_PATH="${1:-$ROOT_DIR/docs/universal-forms-implementation-map.md}"
VERIFY="$ROOT_DIR/scripts/verify-universal-forms-implementation-map.sh"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-implementation-map-contract.sh [MAP_PATH]

Runs negative contract checks for the universal forms implementation map
verifier. The canonical map must pass; malformed temporary copies must fail.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ ! -f "$MAP_PATH" ]]; then
  echo "Implementation map not found: $MAP_PATH" >&2
  exit 2
fi
if [[ ! -x "$VERIFY" ]]; then
  echo "Implementation map verifier is not executable: $VERIFY" >&2
  exit 2
fi

failures=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
}

printf 'Universal forms implementation map contract smoke\n'
printf '  map: %s\n' "$MAP_PATH"
printf '\n'

if "$VERIFY" "$MAP_PATH" >/dev/null; then
  pass "canonical implementation map passes verifier"
else
  fail "canonical implementation map passes verifier"
fi

tmp_dir="$(mktemp -d /tmp/openpr-uf-implementation-map-contract.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT

mutate_candidate() {
  local mutation="$1"
  local candidate="$2"

  case "$mutation" in
    missing_delivery_row)
      sed -i '/| Project types and scenario templates |/d' "$candidate"
      ;;
    tested_marker_drift)
      perl -0pi -e 's/(\| Project types and scenario templates \|[^\n]+\| )`已测试`/$1`已完成`/' "$candidate"
      ;;
    manual_marker_drift)
      perl -0pi -e 's/(\| User-side manual acceptance \|[^\n]+\| )`待处理`/$1`已测试`/' "$candidate"
      ;;
    missing_implementation_path)
      sed -i 's#apps/api/src/routes/project.rs#apps/api/src/routes/project_missing.rs#' "$candidate"
      ;;
    missing_verification_command)
      sed -i 's#scripts/smoke-scenario-template-forms.sh#scripts/smoke-scenario-template-forms-missing.sh#' "$candidate"
      ;;
    missing_status_marker)
      sed -i 's/`开发中`/`开发中_missing`/g' "$candidate"
      ;;
    *)
      return 1
      ;;
  esac
}

expect_reject() {
  local label="$1"
  local mutation="$2"
  local candidate="$tmp_dir/${label//[^A-Za-z0-9_]/_}.md"

  cp "$MAP_PATH" "$candidate"
  if ! mutate_candidate "$mutation" "$candidate"; then
    fail "$label mutation can be generated"
    return
  fi

  if "$VERIFY" "$candidate" >/dev/null 2>&1; then
    fail "$label is rejected by verifier"
  else
    pass "$label is rejected by verifier"
  fi
}

expect_reject "missing delivery row" "missing_delivery_row"
expect_reject "tested marker drift" "tested_marker_drift"
expect_reject "manual marker drift" "manual_marker_drift"
expect_reject "missing implementation path" "missing_implementation_path"
expect_reject "missing verification command" "missing_verification_command"
expect_reject "missing status marker" "missing_status_marker"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms implementation map contract smoke failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms implementation map contract smoke passed.\n'

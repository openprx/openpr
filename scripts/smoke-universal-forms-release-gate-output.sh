#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-release-gate-output.sh

Verifies that the human-readable release gate output mirrors the release gate
JSON decision for both the pre-signoff handoff mode and strict release mode.
This is read-only and never changes signoff or tracker state.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ $# -gt 0 ]]; then
  echo "Unknown argument: $1" >&2
  usage >&2
  exit 2
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for release gate output smoke" >&2
  exit 2
fi

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -Fq -- "$needle" "$path"; then
    pass "$description"
  else
    printf 'Expected to find in %s:\n%s\n' "$path" "$needle" >&2
    fail "$description"
  fi
}

not_contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -Fq -- "$needle" "$path"; then
    printf 'Unexpected text found in %s:\n%s\n' "$path" "$needle" >&2
    fail "$description"
  else
    pass "$description"
  fi
}

json_value() {
  local json_path="$1"
  local filter="$2"
  jq -r "$filter" "$json_path"
}

verify_text_matches_json() {
  local label="$1"
  local text_path="$2"
  local json_path="$3"

  contains "$label starts with release gate heading" "$text_path" "Universal forms release gate"
  contains "$label mirrors mode" "$text_path" "  mode: $(json_value "$json_path" '.mode')"
  contains "$label mirrors release flag" "$text_path" "  release_allowed: $(json_value "$json_path" '.release_allowed')"
  contains "$label mirrors reason" "$text_path" "  reason: $(json_value "$json_path" '.reason')"
  contains "$label mirrors stage" "$text_path" "  stage: $(json_value "$json_path" '.stage')"
  contains "$label mirrors automated counts" "$text_path" "  automated checks: $(json_value "$json_path" '.automated_checks') total, $(json_value "$json_path" '.failed_automated_checks') failed"
  contains "$label mirrors non-manual unresolved count" "$text_path" "  non-manual unresolved items: $(json_value "$json_path" '.non_manual_unresolved_items')"
  contains "$label mirrors manual pending rows" "$text_path" "  manual signoff pending rows: $(json_value "$json_path" '.manual_signoff_pending_rows')"
  contains "$label mirrors tracker end-to-end status" "$text_path" "  tracker end-to-end acceptance: $(json_value "$json_path" '.tracker_status.end_to_end_acceptance')"
  contains "$label mirrors tracker manual status" "$text_path" "  tracker user-side manual acceptance: $(json_value "$json_path" '.tracker_status.user_side_manual_acceptance')"

  local next_key
  next_key="$(json_value "$json_path" '.next_manual_signoff_key')"
  if [[ -n "$next_key" && "$next_key" != "null" ]]; then
    contains "$label mirrors next manual signoff key" "$text_path" "  next manual signoff key: $next_key"
  else
    not_contains "$label omits next key after final signoff" "$text_path" "  next manual signoff key:"
  fi

  not_contains "$label does not leak JSON nulls" "$text_path" "null"
}

tmp_dir="$(mktemp -d /tmp/openpr-uf-release-gate-output.XXXXXX)"
trap 'rm -rf "$tmp_dir"' EXIT

allow_text="$tmp_dir/allow-pending.txt"
allow_json="$tmp_dir/allow-pending.json"
strict_text="$tmp_dir/strict.txt"
strict_json="$tmp_dir/strict.json"

printf 'Universal forms release gate output smoke\n'
printf '  repo: %s\n' "$ROOT_DIR"
printf '\n'

"$ROOT_DIR/scripts/gate-universal-forms-release.sh" --allow-pending >"$allow_text"
"$ROOT_DIR/scripts/gate-universal-forms-release.sh" --allow-pending --json >"$allow_json"
jq empty "$allow_json" >/dev/null
pass "allow-pending release gate JSON is valid JSON"
"$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh" "$allow_json" >/dev/null
pass "allow-pending release gate JSON passes verifier"
verify_text_matches_json "allow-pending release gate output" "$allow_text" "$allow_json"

strict_status=0
"$ROOT_DIR/scripts/gate-universal-forms-release.sh" >"$strict_text" 2>&1 || strict_status=$?
strict_json_status=0
"$ROOT_DIR/scripts/gate-universal-forms-release.sh" --json >"$strict_json" 2>&1 || strict_json_status=$?
jq empty "$strict_json" >/dev/null
pass "strict release gate JSON is valid JSON"
verify_text_matches_json "strict release gate output" "$strict_text" "$strict_json"

strict_release_allowed="$(json_value "$strict_json" '.release_allowed')"
strict_pending_rows="$(json_value "$strict_json" '.manual_signoff_pending_rows')"
if [[ "$strict_release_allowed" == "true" && "$strict_pending_rows" == "0" ]]; then
  if [[ "$strict_status" -eq 0 && "$strict_json_status" -eq 0 ]]; then
    pass "strict release gate exits zero after final signoff"
  else
    fail "strict release gate exits zero after final signoff"
  fi
else
  if [[ "$strict_status" -ne 0 && "$strict_json_status" -ne 0 ]]; then
    pass "strict release gate exits nonzero before final signoff"
  else
    fail "strict release gate exits nonzero before final signoff"
  fi
fi

printf '\nUniversal forms release gate output smoke passed.\n'

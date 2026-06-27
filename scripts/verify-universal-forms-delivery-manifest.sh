#!/usr/bin/env bash
set -euo pipefail

MANIFEST_PATH="${1:-/opt/worker/report/openpr/docs/openpr-universal-form-delivery-manifest-2026-05-31.md}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-delivery-manifest.sh [MANIFEST_PATH]

Verifies every file row in the universal forms delivery manifest:
  - path exists
  - byte size matches
  - SHA256 matches

This script is intentionally independent from the larger delivery-bundle audit
so reviewers and CI can validate the checksum manifest directly.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ ! -f "$MANIFEST_PATH" ]]; then
  echo "Delivery manifest not found: $MANIFEST_PATH" >&2
  exit 2
fi

failures=0
checked=0

trim() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "$value"
}

strip_backticks() {
  local value="$1"
  value="${value#\`}"
  value="${value%\`}"
  printf '%s' "$value"
}

printf 'Universal forms delivery manifest verification\n'
printf '  manifest: %s\n' "$MANIFEST_PATH"
printf '\n'

while IFS='|' read -r _ label path expected_size expected_sha _rest; do
  label="$(trim "$label")"
  path="$(strip_backticks "$(trim "$path")")"
  expected_size="$(trim "$expected_size")"
  expected_sha="$(trim "$expected_sha")"

  if [[ -z "$label" || "$label" == "Label" || "$label" == "---" ]]; then
    continue
  fi
  if [[ -z "$path" || "$path" != /* || ! "$expected_size" =~ ^[0-9]+$ || ! "$expected_sha" =~ ^[0-9a-f]{64}$ ]]; then
    continue
  fi

  checked=$((checked + 1))
  if [[ ! -f "$path" ]]; then
    printf 'FAIL: %s missing: %s\n' "$label" "$path" >&2
    failures=$((failures + 1))
    continue
  fi

  actual_size="$(stat -c '%s' "$path")"
  actual_sha="$(sha256sum "$path" | awk '{print $1}')"

  if [[ "$actual_size" != "$expected_size" ]]; then
    printf 'FAIL: %s size mismatch: expected %s, actual %s\n' "$label" "$expected_size" "$actual_size" >&2
    failures=$((failures + 1))
  elif [[ "$actual_sha" != "$expected_sha" ]]; then
    printf 'FAIL: %s sha256 mismatch: expected %s, actual %s\n' "$label" "$expected_sha" "$actual_sha" >&2
    failures=$((failures + 1))
  else
    printf 'PASS: %s\n' "$label"
  fi
done <"$MANIFEST_PATH"

if [[ "$checked" -eq 0 ]]; then
  echo "No file rows found in delivery manifest" >&2
  exit 1
fi

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms delivery manifest verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms delivery manifest verification passed: %s file row(s).\n' "$checked"

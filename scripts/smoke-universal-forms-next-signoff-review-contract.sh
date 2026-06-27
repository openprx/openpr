#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
REVIEW_PATH="${1:-$REPORT_DIR/openpr-universal-form-next-signoff-review-2026-05-31.md}"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
VERIFY="$ROOT_DIR/scripts/verify-universal-forms-next-signoff-review.sh"

usage() {
  cat <<'EOF'
Usage: scripts/smoke-universal-forms-next-signoff-review-contract.sh [REVIEW_PATH]

Runs negative contract checks for the next manual signoff review verifier. The
canonical review must pass; drifted temporary copies must fail.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for next signoff review contract smoke" >&2
  exit 2
fi

if [[ ! -f "$REVIEW_PATH" ]]; then
  echo "Next signoff review not found: $REVIEW_PATH" >&2
  exit 2
fi

if [[ ! -f "$SIGNOFF_STATUS_JSON_PATH" ]]; then
  echo "Signoff status JSON not found: $SIGNOFF_STATUS_JSON_PATH" >&2
  exit 2
fi

pass() {
  printf 'PASS: %s\n' "$1"
}

replace_or_fail() {
  local path="$1"
  local needle="$2"
  local replacement="$3"
  NEEDLE="$needle" REPLACEMENT="$replacement" perl -0pi -e '
    BEGIN {
      $needle = $ENV{NEEDLE};
      $replacement = $ENV{REPLACEMENT};
    }
    $changed = s/\Q$needle\E/$replacement/;
    END {
      die "needle not found\n" unless $changed;
    }
  ' "$path"
}

expect_reject() {
  local description="$1"
  local needle="$2"
  local replacement="$3"
  local tmp
  tmp="$(mktemp /tmp/openpr-uf-next-signoff-review.XXXXXX.md)"
  cp "$REVIEW_PATH" "$tmp"
  if ! replace_or_fail "$tmp" "$needle" "$replacement"; then
    rm -f "$tmp"
    echo "FAIL: $description mutation could not be applied" >&2
    exit 1
  fi
  if "$VERIFY" "$tmp" >/dev/null 2>&1; then
    rm -f "$tmp"
    echo "FAIL: $description was accepted" >&2
    exit 1
  fi
  rm -f "$tmp"
  pass "$description is rejected by verifier"
}

printf 'Universal forms next signoff review contract smoke\n'
printf '  review: %s\n' "$REVIEW_PATH"
printf '\n'

"$VERIFY" "$REVIEW_PATH" >/dev/null
pass "canonical next signoff review passes verifier"

automated_checks="$(jq -r '.gate_summary.automated_checks' "$SIGNOFF_STATUS_JSON_PATH")"
pending_rows="$(jq -r '.manual_signoff.pending_rows' "$SIGNOFF_STATUS_JSON_PATH")"
next_key="$(jq -r '.manual_signoff.next_row.key // ""' "$SIGNOFF_STATUS_JSON_PATH")"
recorder_command="$(jq -r '.manual_signoff.next_row.recorder_command // ""' "$SIGNOFF_STATUS_JSON_PATH")"

if [[ "$pending_rows" == "0" ]]; then
  drifted_pending_rows="1"
else
  drifted_pending_rows="$((pending_rows - 1))"
fi

expect_reject \
  "heading drift" \
  "# OpenPR Universal Forms Next Signoff Review" \
  "# Broken Review"

expect_reject \
  "automated check count drift" \
  "| Automated checks | $automated_checks | all PASS |" \
  "| Automated checks | $((automated_checks + 1)) | all PASS |"

expect_reject \
  "pending row count drift" \
  "| Pending manual rows | $pending_rows | 0 |" \
  "| Pending manual rows | $drifted_pending_rows | 0 |"

if [[ "$pending_rows" == "0" ]]; then
  expect_reject \
    "finalizer command drift" \
    "scripts/finalize-universal-forms-acceptance.sh" \
    "scripts/missing-finalizer.sh"
else
  if [[ -z "$next_key" || -z "$recorder_command" ]]; then
    echo "Pending signoff status must expose next key and recorder command" >&2
    exit 2
  fi

  wrong_key="frontend_usability"
  if [[ "$next_key" == "$wrong_key" ]]; then
    wrong_key="restaurant_template"
  fi
  wrong_recorder_command="${recorder_command/--item $next_key/--item $wrong_key}"

  expect_reject \
    "next key drift" \
    "| Key | \`$next_key\` |" \
    "| Key | \`$wrong_key\` |"

  expect_reject \
    "recorder command drift" \
    "$recorder_command" \
    "$wrong_recorder_command"

  expect_reject \
    "screenshot path drift" \
    "project-template-wizard-desktop.png" \
    "missing-template-desktop.png"

  expect_reject \
    "missing verification command" \
    "scripts/verify-universal-forms-ui-review-gallery.sh" \
    "scripts/missing-ui-review-gallery.sh"
fi

printf '\nUniversal forms next signoff review contract smoke passed.\n'

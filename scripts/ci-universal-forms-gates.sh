#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
export CARGO_BUILD_JOBS=4

usage() {
  cat <<'EOF'
Usage: scripts/ci-universal-forms-gates.sh

Runs the repository-local Universal Forms CI gate bundle. This is the same
entrypoint used by GitHub Actions so contributors can reproduce the CI-only
universal business-platform checks locally.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

printf 'Universal Forms CI Gates\n'
printf '  repo: %s\n\n' "$ROOT_DIR"

scripts/audit-universal-forms-security-scope.sh
printf '\n'
scripts/audit-universal-forms-source-coverage.sh
printf '\n'
scripts/audit-universal-forms-production-readiness.sh

printf '\nUniversal Forms static gates passed.\n'

[[ -n "${OPENPR_TEST_DATABASE_URL:-}" ]] || {
  echo 'FAIL: OPENPR_TEST_DATABASE_URL is required for the Forms regression tests' >&2
  exit 2
}

cargo test -p api 'forms::' -- --nocapture
cargo test -p api 'routes::form::' -- --nocapture

printf '\nUniversal Forms static and Rust regression gates passed.\n'

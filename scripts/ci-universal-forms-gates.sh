#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

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

printf '\nUniversal Forms CI Gates passed.\n'

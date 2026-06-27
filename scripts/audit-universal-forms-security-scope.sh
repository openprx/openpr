#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AUDIT_JSON="${OPENPR_SECURITY_SCOPE_AUDIT_JSON:-}"

usage() {
  cat <<'EOF'
Usage: scripts/audit-universal-forms-security-scope.sh

Verifies the universal forms delivery security-audit scope:
  - cargo audit succeeds with the repository audit policy
  - the only ignored advisory is the documented SQLx inactive MySQL backend case
  - workspace dependency resolution has no active rsa or sqlx-mysql tree
  - the workspace enables sqlx-postgres through SeaORM

This script is intentionally explicit because OpenPR's production delivery path
is PostgreSQL-only. Lockfile-only or transitive metadata for other SQL backends
must not be treated as an active runtime database backend.

Environment:
  OPENPR_SECURITY_SCOPE_AUDIT_JSON  Optional path for the cargo-audit JSON file.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

failures=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
}

require_command() {
  local command="$1"
  if command -v "$command" >/dev/null 2>&1; then
    pass "$command is available"
  else
    fail "$command is available"
  fi
}

contains() {
  local description="$1"
  local path="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" "$path"; then
    pass "$description"
  else
    fail "$description"
    printf '  missing in %s: %s\n' "$path" "$needle" >&2
  fi
}

not_contains_output() {
  local description="$1"
  local output="$2"
  local needle="$3"
  if rg -q --fixed-strings -- "$needle" <<<"$output"; then
    fail "$description"
    printf '  unexpected output: %s\n' "$needle" >&2
  else
    pass "$description"
  fi
}

equals() {
  local description="$1"
  local actual="$2"
  local expected="$3"
  if [[ "$actual" == "$expected" ]]; then
    pass "$description"
  else
    fail "$description"
    printf '  expected: %s\n  actual: %s\n' "$expected" "${actual:-<empty>}" >&2
  fi
}

printf 'Universal forms security scope audit\n'
printf '  repo: %s\n' "$ROOT_DIR"
printf '\n'

require_command cargo
require_command cargo-audit
require_command jq
require_command rg

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms security scope audit failed before checks: %s issue(s)\n' "$failures" >&2
  exit 1
fi

if [[ -z "$AUDIT_JSON" ]]; then
  AUDIT_JSON="$(mktemp -t openpr-cargo-audit.XXXXXX.json)"
  trap 'rm -f "$AUDIT_JSON"' EXIT
else
  mkdir -p "$(dirname "$AUDIT_JSON")"
fi

contains "workspace enables SeaORM sqlx-postgres" "$ROOT_DIR/Cargo.toml" '"sqlx-postgres"'
contains "cargo audit documents SQLx inactive MySQL advisory" "$ROOT_DIR/.cargo/audit.toml" "SQLx's inactive MySQL backend"
contains "cargo audit ignore is limited to RUSTSEC-2023-0071" "$ROOT_DIR/.cargo/audit.toml" 'ignore = ["RUSTSEC-2023-0071"]'
contains "cargo deny documents SQLx inactive MySQL advisory" "$ROOT_DIR/deny.toml" "SQLx's inactive MySQL backend"
contains "cargo deny ignore includes RUSTSEC-2023-0071" "$ROOT_DIR/deny.toml" '"RUSTSEC-2023-0071"'

if (cd "$ROOT_DIR" && cargo audit --json >"$AUDIT_JSON"); then
  pass "cargo audit succeeds with repository policy"
else
  fail "cargo audit succeeds with repository policy"
fi

if jq empty "$AUDIT_JSON" >/dev/null; then
  pass "cargo audit JSON is valid"
  equals "cargo audit reports zero active vulnerabilities" "$(jq -r '.vulnerabilities.count' "$AUDIT_JSON")" "0"
  equals "cargo audit ignore list has one advisory" "$(jq -r '.settings.ignore | length' "$AUDIT_JSON")" "1"
  equals "cargo audit ignored advisory is SQLx inactive MySQL scope" "$(jq -r '.settings.ignore[0]' "$AUDIT_JSON")" "RUSTSEC-2023-0071"
else
  fail "cargo audit JSON is valid"
fi

rsa_tree="$(cd "$ROOT_DIR" && cargo tree -i rsa --workspace 2>&1 || true)"
sqlx_mysql_tree="$(cd "$ROOT_DIR" && cargo tree -i sqlx-mysql --workspace 2>&1 || true)"
features_tree="$(cd "$ROOT_DIR" && cargo tree -e features --workspace 2>&1)"

not_contains_output "workspace has no active rsa dependency tree" "$rsa_tree" "rsa v"
not_contains_output "workspace has no active sqlx-mysql dependency tree" "$sqlx_mysql_tree" "sqlx-mysql v"
if rg -q --fixed-strings -- 'sea-orm feature "sqlx-postgres"' <<<"$features_tree" \
  && rg -q --fixed-strings -- 'sqlx-postgres v' <<<"$features_tree"; then
  pass "workspace feature tree resolves PostgreSQL SQLx backend"
else
  fail "workspace feature tree resolves PostgreSQL SQLx backend"
fi
not_contains_output "workspace feature tree does not enable SQLx MySQL backend" "$features_tree" 'sqlx feature "mysql"'
not_contains_output "workspace feature tree does not enable sqlx-mysql crate" "$features_tree" 'sqlx-mysql v'

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms security scope audit failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms security scope audit passed.\n'

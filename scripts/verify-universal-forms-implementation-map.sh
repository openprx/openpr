#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAP_PATH="${1:-$ROOT_DIR/docs/universal-forms-implementation-map.md}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-universal-forms-implementation-map.sh [MAP_PATH]

Verifies the universal forms implementation map as a handoff contract:
  - required status markers are documented;
  - required delivery areas are present;
  - implementation-path references exist;
  - script references in primary verification cells exist;
  - current markers match the pre-signoff delivery state.
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

contains() {
  local description="$1"
  local needle="$2"
  if rg -q --fixed-strings -- "$needle" "$MAP_PATH"; then
    pass "$description"
  else
    fail "$description"
    printf '  missing: %s\n' "$needle" >&2
  fi
}

require_path() {
  local description="$1"
  local path="$2"
  if [[ "$path" == /* ]]; then
    resolved="$path"
  else
    resolved="$ROOT_DIR/$path"
  fi

  if [[ "$path" == */ ]]; then
    if [[ -d "$resolved" ]]; then
      pass "$description"
    else
      fail "$description"
      printf '  missing directory: %s\n' "$resolved" >&2
    fi
  else
    if [[ -e "$resolved" ]]; then
      pass "$description"
    else
      fail "$description"
      printf '  missing path: %s\n' "$resolved" >&2
    fi
  fi
}

if [[ -f "$MAP_PATH" ]]; then
  pass "implementation map exists"
else
  fail "implementation map exists"
  printf '\nUniversal forms implementation map verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf 'Universal forms implementation map verification\n'
printf '  map: %s\n' "$MAP_PATH"
printf '\n'

contains "map has title" "# Universal Forms Implementation Map"
contains "map documents status markers" "## Status Markers"
contains "map documents module map" "## Module Map"
contains "map documents delivery rule" "## Delivery Rule"

for marker in "待处理" "开发中" "已完成" "已测试" "已验收"; do
  contains "status marker documented: $marker" "\`$marker\`"
done

required_areas=(
  "Project types and scenario templates"
  "Universal form definitions and records"
  "Decimal-safe amount fields"
  "Subforms and record links"
  "Business events and delivery ledger"
  "Connectors, webhooks, and print"
  "WASM plugin runtime"
  "MCP business surface"
  "Frontend operator workflow"
  "Restaurant reference scenario"
  "Delivery evidence and release gates"
  "User-side manual acceptance"
)

for area in "${required_areas[@]}"; do
  contains "delivery area documented: $area" "| $area |"
done

row_count="$(awk -F'|' '
  /^\|/ && NF >= 6 {
    area = $2
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", area)
    if (area != "" && area != "Delivery area" && area !~ /^-+$/) {
      count += 1
    }
  }
  END { print count + 0 }
' "$MAP_PATH")"
if [[ "$row_count" == "12" ]]; then
  pass "module map has 12 delivery rows"
else
  fail "module map has 12 delivery rows"
  printf '  actual: %s\n' "$row_count" >&2
fi

while IFS=$'\t' read -r area paths verification marker; do
  [[ -z "$area" ]] && continue

  if [[ "$area" == "User-side manual acceptance" ]]; then
    if [[ "$marker" == "\`待处理\`" ]]; then
      pass "manual acceptance marker is pending"
    else
      fail "manual acceptance marker is pending"
      printf '  actual: %s\n' "$marker" >&2
    fi
  else
    if [[ "$marker" == "\`已测试\`" ]]; then
      pass "delivery row is tested: $area"
    else
      fail "delivery row is tested: $area"
      printf '  actual: %s\n' "$marker" >&2
    fi
  fi

  while IFS= read -r ref; do
    [[ -z "$ref" ]] && continue
    if [[ "$ref" == /* || "$ref" == */* ]]; then
      require_path "implementation reference exists: $ref" "$ref"
    fi
  done < <(printf '%s\n' "$paths" | grep -oE '`[^`]+`' | sed 's/^`//; s/`$//' || true)

  while IFS= read -r command_ref; do
    [[ -z "$command_ref" ]] && continue
    command_path="${command_ref%% *}"
    case "$command_path" in
      scripts/*|frontend/scripts/*|skills/*)
        require_path "verification command exists: $command_path" "$command_path"
        ;;
      cargo|bun)
        pass "verification command is tool invocation: $command_ref"
        ;;
      *)
        pass "verification command is descriptive: $command_ref"
        ;;
    esac
  done < <(printf '%s\n' "$verification" | grep -oE '`[^`]+`' | sed 's/^`//; s/`$//' || true)
done < <(
  awk -F'|' '
    /^\|/ && NF >= 6 {
      area = $2
      paths = $3
      verification = $5
      marker = $6
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", area)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", paths)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", verification)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", marker)
      if (area != "" && area != "Delivery area" && area !~ /^-+$/) {
        print area "\t" paths "\t" verification "\t" marker
      }
    }
  ' "$MAP_PATH"
)

contains "compact status command is documented" "scripts/status-universal-forms-delivery.sh --json"
contains "delivery-bundle audit is documented" "scripts/audit-universal-forms-delivery-bundle.sh"
contains "strict release gate is documented" "scripts/gate-universal-forms-release.sh --json"
contains "manual signoff boundary is documented" "Do not mark final"

if [[ "$failures" -ne 0 ]]; then
  printf '\nUniversal forms implementation map verification failed: %s issue(s)\n' "$failures" >&2
  exit 1
fi

printf '\nUniversal forms implementation map verification passed.\n'

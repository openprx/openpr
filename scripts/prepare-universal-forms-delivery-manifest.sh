#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="/opt/worker/report/openpr/docs"
ARTIFACT_DIR="/opt/worker/report/openpr/artifacts/universal-forms-ui-2026-05-31"
OUTPUT_PATH="${OPENPR_DELIVERY_MANIFEST:-$REPORT_DIR/openpr-universal-form-delivery-manifest-2026-05-31.md}"

usage() {
  cat <<'EOF'
Usage: scripts/prepare-universal-forms-delivery-manifest.sh [--output PATH]

Generates a checksum manifest for the universal forms delivery bundle. The
manifest is a reviewer and release-engineering aid: it records the exact
reports, docs, scripts, UI artifacts, and current gate counts that make up the
handoff bundle. It does not mark acceptance as passed.

Environment:
  OPENPR_DELIVERY_MANIFEST  Optional output path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output)
      OUTPUT_PATH="${2:-}"
      if [[ -z "$OUTPUT_PATH" ]]; then
        echo "--output requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

EVIDENCE_PATH="$REPORT_DIR/openpr-universal-form-acceptance-evidence-2026-05-31.md"
COMPLETION_AUDIT_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.md"
COMPLETION_AUDIT_JSON_PATH="$REPORT_DIR/openpr-universal-form-completion-audit-2026-05-31.json"
USER_ACCEPTANCE_PACKET_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-packet-2026-05-31.md"
NEXT_SIGNOFF_REVIEW_PATH="$REPORT_DIR/openpr-universal-form-next-signoff-review-2026-05-31.md"
READINESS_SUMMARY_PATH="$REPORT_DIR/openpr-universal-form-readiness-summary-2026-05-31.md"
READINESS_JSON_PATH="$REPORT_DIR/openpr-universal-form-readiness-2026-05-31.json"
READINESS_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-readiness.schema.json"
SIGNOFF_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.json"
SIGNOFF_STATUS_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-signoff-status.schema.json"
DEVELOPMENT_STATUS_JSON_PATH="$REPORT_DIR/openpr-universal-form-development-status-2026-05-31.json"
COMPLETION_AUDIT_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-completion-audit.schema.json"
DEVELOPMENT_STATUS_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-development-status.schema.json"
SCENARIO_CATALOG_JSON_PATH="$REPORT_DIR/openpr-universal-form-scenario-catalog-2026-05-31.json"
SCENARIO_CATALOG_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-scenario-catalog.schema.json"
IMPLEMENTATION_MAP_JSON_PATH="$REPORT_DIR/openpr-universal-form-implementation-map-2026-05-31.json"
IMPLEMENTATION_MAP_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-implementation-map.schema.json"
DELIVERY_STATUS_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-delivery-status.schema.json"
RELEASE_GATE_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-release-gate.schema.json"
DELIVERY_MANIFEST_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-universal-forms-delivery-manifest.schema.json"
PROJECT_RELEASE_READINESS_JSON_SCHEMA_PATH="$ROOT_DIR/docs/schemas/openpr-project-release-readiness.schema.json"
MANUAL_EVIDENCE_MAP_PATH="$REPORT_DIR/openpr-universal-form-manual-evidence-map-2026-05-31.md"
SIGNOFF_STATUS_PATH="$REPORT_DIR/openpr-universal-form-signoff-status-2026-05-31.md"
RUNBOOK_PATH="$REPORT_DIR/openpr-universal-form-user-acceptance-runbook-2026-05-31.md"
TRACKER_PATH="$REPORT_DIR/openpr-universal-form-development-execution-tracker-2026-05-31.md"
UI_ARTIFACT_MANIFEST_PATH="$REPORT_DIR/openpr-universal-form-ui-artifacts-2026-05-31.md"
UI_REVIEW_GALLERY_PATH="$REPORT_DIR/openpr-universal-form-ui-review-gallery-2026-05-31.html"
SIGNOFF_DASHBOARD_PATH="$REPORT_DIR/openpr-universal-form-signoff-dashboard-2026-05-31.html"
REPORT_INDEX_PATH="$REPORT_DIR/README.md"

require_file() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "Missing required file: $path" >&2
    exit 1
  fi
}

file_size() {
  stat -c '%s' "$1"
}

file_sha256() {
  sha256sum "$1" | awk '{print $1}'
}

manifest_row() {
  local label="$1"
  local path="$2"
  require_file "$path"
  printf '| %s | `%s` | %s | %s |\n' "$label" "$path" "$(file_size "$path")" "$(file_sha256 "$path")"
}

status_for() {
  local label="$1"
  awk -F'|' -v label="$label" '
    NF >= 4 {
      item = $2
      status = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", item)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", status)
      if (item == label) {
        print status
        exit
      }
    }
  ' "$TRACKER_PATH"
}

for path in \
  "$EVIDENCE_PATH" \
  "$COMPLETION_AUDIT_PATH" \
  "$COMPLETION_AUDIT_JSON_PATH" \
  "$USER_ACCEPTANCE_PACKET_PATH" \
  "$NEXT_SIGNOFF_REVIEW_PATH" \
  "$READINESS_SUMMARY_PATH" \
  "$READINESS_JSON_PATH" \
  "$SIGNOFF_STATUS_JSON_PATH" \
  "$DEVELOPMENT_STATUS_JSON_PATH" \
  "$SCENARIO_CATALOG_JSON_PATH" \
  "$IMPLEMENTATION_MAP_JSON_PATH" \
  "$MANUAL_EVIDENCE_MAP_PATH" \
  "$SIGNOFF_STATUS_PATH" \
  "$RUNBOOK_PATH" \
  "$TRACKER_PATH" \
  "$UI_ARTIFACT_MANIFEST_PATH" \
  "$UI_REVIEW_GALLERY_PATH" \
  "$SIGNOFF_DASHBOARD_PATH"; do
  require_file "$path"
done

mkdir -p "$(dirname "$OUTPUT_PATH")"
output_tmp="$(mktemp "$(dirname "$OUTPUT_PATH")/.delivery-manifest.XXXXXX")"
trap 'rm -f "$output_tmp"' EXIT

summary_total="$(sed -n 's/^- Total automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
summary_failed="$(sed -n 's/^- Failed automated checks: //p' "$EVIDENCE_PATH" | tail -n 1)"
pass_count="$(rg -c '^\*\*Status:\*\* PASS$' "$EVIDENCE_PATH" || true)"
manual_pending_count="$(rg -c '^\| .* \| Pending \|' "$EVIDENCE_PATH" || true)"
e2e_status="$(status_for "端到端验收")"
manual_status="$(status_for "用户侧人工验收")"

{
  printf '# OpenPR Universal Forms Delivery Manifest\n\n'
  printf '%s\n' "- Generated at: $(date -Is)"
  printf '%s\n' "- Repository: \`$ROOT_DIR\`"
  printf '%s\n' "- Report directory: \`$REPORT_DIR\`"
  printf '%s\n' "- Artifact directory: \`$ARTIFACT_DIR\`"
  printf '\n'

  printf '## Gate Summary\n\n'
  printf '| Gate | Value |\n'
  printf '| --- | --- |\n'
  printf '| Automated checks | %s |\n' "${summary_total:-missing}"
  printf '| PASS status lines | %s |\n' "$pass_count"
  printf '| Failed automated checks | %s |\n' "${summary_failed:-missing}"
  printf '| Manual signoff rows pending | %s |\n' "$manual_pending_count"
  printf '| End-to-end acceptance | %s |\n' "${e2e_status:-missing}"
  printf '| User-side manual acceptance | %s |\n' "${manual_status:-missing}"
  printf '\n'

  printf '## Delivery Files\n\n'
  printf '| Label | Path | Bytes | SHA256 |\n'
  printf '| --- | --- | --- | --- |\n'
  manifest_row "tracker" "$TRACKER_PATH"
  manifest_row "report docs index" "$REPORT_INDEX_PATH"
  manifest_row "acceptance evidence" "$EVIDENCE_PATH"
  manifest_row "completion audit" "$COMPLETION_AUDIT_PATH"
  manifest_row "completion audit JSON" "$COMPLETION_AUDIT_JSON_PATH"
  manifest_row "user acceptance packet" "$USER_ACCEPTANCE_PACKET_PATH"
  manifest_row "next signoff review" "$NEXT_SIGNOFF_REVIEW_PATH"
  manifest_row "readiness summary" "$READINESS_SUMMARY_PATH"
  manifest_row "readiness JSON" "$READINESS_JSON_PATH"
  manifest_row "manual signoff status JSON" "$SIGNOFF_STATUS_JSON_PATH"
  manifest_row "development status JSON" "$DEVELOPMENT_STATUS_JSON_PATH"
  manifest_row "scenario catalog JSON" "$SCENARIO_CATALOG_JSON_PATH"
  manifest_row "implementation map JSON" "$IMPLEMENTATION_MAP_JSON_PATH"
  manifest_row "manual evidence map" "$MANUAL_EVIDENCE_MAP_PATH"
  manifest_row "manual signoff status report" "$SIGNOFF_STATUS_PATH"
  manifest_row "manual runbook" "$RUNBOOK_PATH"
  manifest_row "UI artifact manifest" "$UI_ARTIFACT_MANIFEST_PATH"
  manifest_row "UI review gallery" "$UI_REVIEW_GALLERY_PATH"
  manifest_row "signoff dashboard" "$SIGNOFF_DASHBOARD_PATH"
  manifest_row "signoff dashboard desktop render" "$ARTIFACT_DIR/signoff-dashboard/signoff-dashboard-desktop.png"
  manifest_row "signoff dashboard mobile render" "$ARTIFACT_DIR/signoff-dashboard/signoff-dashboard-mobile.png"
  manifest_row "UI review gallery desktop render" "$ARTIFACT_DIR/ui-review-gallery/ui-review-gallery-desktop.png"
  manifest_row "UI review gallery mobile render" "$ARTIFACT_DIR/ui-review-gallery/ui-review-gallery-mobile.png"
  manifest_row "project template desktop screenshot" "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-desktop.png"
  manifest_row "project template mobile screenshot" "$ARTIFACT_DIR/project-template-wizard/project-template-wizard-mobile.png"
  manifest_row "template work items desktop screenshot" "$ARTIFACT_DIR/template-work-items/template-work-items-desktop.png"
  manifest_row "template work items mobile screenshot" "$ARTIFACT_DIR/template-work-items/template-work-items-mobile.png"
  manifest_row "Forms UI desktop screenshot" "$ARTIFACT_DIR/forms-ui/forms-ui-desktop.png"
  manifest_row "Forms UI mobile screenshot" "$ARTIFACT_DIR/forms-ui/forms-ui-mobile.png"
  manifest_row "restaurant desktop screenshot" "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-desktop.png"
  manifest_row "restaurant mobile screenshot" "$ARTIFACT_DIR/restaurant-ordering/restaurant-ordering-mobile.png"
  printf '\n'

  printf '## Gate Scripts\n\n'
  printf '| Label | Path | Bytes | SHA256 |\n'
  printf '| --- | --- | --- | --- |\n'
  manifest_row "refresh delivery bundle" "$ROOT_DIR/scripts/refresh-universal-forms-delivery-bundle.sh"
  manifest_row "restaurant demo bootstrap" "$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh"
  manifest_row "restaurant demo MCP HTTP smoke" "$ROOT_DIR/scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh"
  manifest_row "universal forms deployed export smoke" "$ROOT_DIR/scripts/smoke-universal-forms-deployed-export.mjs"
  manifest_row "universal forms deployed import smoke" "$ROOT_DIR/scripts/smoke-universal-forms-deployed-import.mjs"
  manifest_row "universal forms deployed permissions smoke" "$ROOT_DIR/scripts/smoke-universal-forms-deployed-permissions.mjs"
  manifest_row "universal forms deployed duplicate smoke" "$ROOT_DIR/scripts/smoke-universal-forms-deployed-duplicate.mjs"
  manifest_row "acceptance evidence collector" "$ROOT_DIR/scripts/acceptance-universal-forms.sh"
  manifest_row "security scope audit" "$ROOT_DIR/scripts/audit-universal-forms-security-scope.sh"
  manifest_row "completion audit JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-completion-audit-json.sh"
  manifest_row "completion audit JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-completion-audit-json.sh"
  manifest_row "completion audit JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-completion-audit-json-contract.sh"
  manifest_row "CI universal forms gates" "$ROOT_DIR/scripts/ci-universal-forms-gates.sh"
  manifest_row "source coverage audit" "$ROOT_DIR/scripts/audit-universal-forms-source-coverage.sh"
  manifest_row "docs/protocol audit" "$ROOT_DIR/scripts/audit-universal-forms-docs.sh"
  manifest_row "delivery-state audit" "$ROOT_DIR/scripts/audit-universal-forms-delivery-state.sh"
  manifest_row "delivery-bundle audit" "$ROOT_DIR/scripts/audit-universal-forms-delivery-bundle.sh"
  manifest_row "delivery status command" "$ROOT_DIR/scripts/status-universal-forms-delivery.sh"
  manifest_row "delivery status JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-delivery-status-json.sh"
  manifest_row "delivery status JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-json-contract.sh"
  manifest_row "delivery status output smoke" "$ROOT_DIR/scripts/smoke-universal-forms-delivery-status-output.sh"
  manifest_row "release gate" "$ROOT_DIR/scripts/gate-universal-forms-release.sh"
  manifest_row "release gate JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-release-gate-json.sh"
  manifest_row "release gate JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-json-contract.sh"
  manifest_row "release gate smoke" "$ROOT_DIR/scripts/smoke-universal-forms-release-gate.sh"
  manifest_row "release gate output smoke" "$ROOT_DIR/scripts/smoke-universal-forms-release-gate-output.sh"
  manifest_row "delivery manifest verifier" "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest.sh"
  manifest_row "delivery manifest JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-delivery-manifest-json.sh"
  manifest_row "delivery manifest JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-delivery-manifest-json.sh"
  manifest_row "delivery manifest JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-delivery-manifest-json-contract.sh"
  manifest_row "implementation map verifier" "$ROOT_DIR/scripts/verify-universal-forms-implementation-map.sh"
  manifest_row "implementation map contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-contract.sh"
  manifest_row "MCP skill validator" "$ROOT_DIR/skills/openpr-mcp/scripts/validate-mcp.sh"
  manifest_row "MCP skill regression" "$ROOT_DIR/skills/openpr-mcp/scripts/mcp-regression.py"
  manifest_row "UI artifact collector" "$ROOT_DIR/scripts/collect-universal-forms-ui-artifacts.sh"
  manifest_row "UI review gallery generator" "$ROOT_DIR/scripts/prepare-universal-forms-ui-review-gallery.sh"
  manifest_row "UI review gallery verifier" "$ROOT_DIR/scripts/verify-universal-forms-ui-review-gallery.sh"
  manifest_row "UI review gallery browser render smoke" "$ROOT_DIR/scripts/smoke-universal-forms-ui-review-gallery-render.sh"
  manifest_row "signoff dashboard generator" "$ROOT_DIR/scripts/prepare-universal-forms-signoff-dashboard.sh"
  manifest_row "signoff dashboard verifier" "$ROOT_DIR/scripts/verify-universal-forms-signoff-dashboard.sh"
  manifest_row "signoff dashboard browser render smoke" "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-render.sh"
  manifest_row "signoff dashboard progression smoke" "$ROOT_DIR/scripts/smoke-universal-forms-signoff-dashboard-progression.sh"
  manifest_row "report output boundary smoke" "$ROOT_DIR/scripts/smoke-universal-forms-report-output-boundaries.sh"
  manifest_row "manual signoff recorder" "$ROOT_DIR/scripts/record-universal-forms-manual-signoff.sh"
  manifest_row "manual signoff status reporter" "$ROOT_DIR/scripts/report-universal-forms-signoff-status.sh"
  manifest_row "manual signoff status JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-signoff-status-json.sh"
  manifest_row "manual signoff status JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-signoff-status-json.sh"
  manifest_row "manual signoff status JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-json-contract.sh"
  manifest_row "manual signoff status output smoke" "$ROOT_DIR/scripts/smoke-universal-forms-signoff-status-output.sh"
  manifest_row "next signoff review generator" "$ROOT_DIR/scripts/prepare-universal-forms-next-signoff-review.sh"
  manifest_row "next signoff review verifier" "$ROOT_DIR/scripts/verify-universal-forms-next-signoff-review.sh"
  manifest_row "next signoff review contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-review-contract.sh"
  manifest_row "next signoff command smoke" "$ROOT_DIR/scripts/smoke-universal-forms-next-signoff-command.sh"
  manifest_row "manual signoff progression smoke" "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-progression.sh"
  manifest_row "manual signoff commands smoke" "$ROOT_DIR/scripts/smoke-universal-forms-manual-signoff-commands.sh"
  manifest_row "readiness summary reporter" "$ROOT_DIR/scripts/report-universal-forms-readiness-summary.sh"
  manifest_row "readiness JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-readiness-json.sh"
  manifest_row "readiness JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-readiness-json.sh"
  manifest_row "readiness JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-readiness-json-contract.sh"
  manifest_row "development status JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-development-status-json.sh"
  manifest_row "development status JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-development-status-json.sh"
  manifest_row "development status JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-development-status-json-contract.sh"
  manifest_row "scenario catalog JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-scenario-catalog-json.sh"
  manifest_row "scenario catalog JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-scenario-catalog-json.sh"
  manifest_row "scenario catalog JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-scenario-catalog-json-contract.sh"
  manifest_row "implementation map JSON reporter" "$ROOT_DIR/scripts/report-universal-forms-implementation-map-json.sh"
  manifest_row "implementation map JSON verifier" "$ROOT_DIR/scripts/verify-universal-forms-implementation-map-json.sh"
  manifest_row "implementation map JSON contract smoke" "$ROOT_DIR/scripts/smoke-universal-forms-implementation-map-json-contract.sh"
  manifest_row "manual signoff verifier" "$ROOT_DIR/scripts/verify-universal-forms-acceptance-signoff.sh"
  manifest_row "manual consistency verifier" "$ROOT_DIR/scripts/verify-universal-forms-manual-signoff-consistency.sh"
  manifest_row "finalizer" "$ROOT_DIR/scripts/finalize-universal-forms-acceptance.sh"
  printf '\n'

  printf '## Repository Docs\n\n'
  printf '| Label | Path | Bytes | SHA256 |\n'
  printf '| --- | --- | --- | --- |\n'
  manifest_row "README" "$ROOT_DIR/README.md"
  manifest_row "MCP app README" "$ROOT_DIR/apps/mcp-server/README.md"
  manifest_row "MCP app AGENTS guide" "$ROOT_DIR/apps/mcp-server/AGENTS.md"
  manifest_row "MCP skill guide" "$ROOT_DIR/skills/openpr-mcp/SKILL.md"
  manifest_row "docs index" "$ROOT_DIR/docs/README.md"
  manifest_row "scenario template catalog" "$ROOT_DIR/docs/scenario-templates.md"
  manifest_row "implementation map" "$ROOT_DIR/docs/universal-forms-implementation-map.md"
  manifest_row "completion audit JSON schema" "$COMPLETION_AUDIT_JSON_SCHEMA_PATH"
  manifest_row "readiness JSON schema" "$READINESS_JSON_SCHEMA_PATH"
  manifest_row "manual signoff status JSON schema" "$SIGNOFF_STATUS_JSON_SCHEMA_PATH"
  manifest_row "development status JSON schema" "$DEVELOPMENT_STATUS_JSON_SCHEMA_PATH"
  manifest_row "scenario catalog JSON schema" "$SCENARIO_CATALOG_JSON_SCHEMA_PATH"
  manifest_row "implementation map JSON schema" "$IMPLEMENTATION_MAP_JSON_SCHEMA_PATH"
  manifest_row "delivery status JSON schema" "$DELIVERY_STATUS_JSON_SCHEMA_PATH"
  manifest_row "release gate JSON schema" "$RELEASE_GATE_JSON_SCHEMA_PATH"
  manifest_row "project release readiness JSON schema" "$PROJECT_RELEASE_READINESS_JSON_SCHEMA_PATH"
  manifest_row "delivery manifest JSON schema" "$DELIVERY_MANIFEST_JSON_SCHEMA_PATH"
  manifest_row "universal forms guide" "$ROOT_DIR/docs/universal-forms-and-plugins.md"
  manifest_row "acceptance guide" "$ROOT_DIR/docs/universal-forms-acceptance.md"
  manifest_row "production runbook" "$ROOT_DIR/docs/universal-forms-production.md"
  printf '\n'

  printf '## Contributor Docs\n\n'
  printf '| Label | Path | Bytes | SHA256 |\n'
  printf '| --- | --- | --- | --- |\n'
  manifest_row "contributing guide" "$ROOT_DIR/CONTRIBUTING.md"
  manifest_row "CI workflow" "$ROOT_DIR/.github/workflows/ci.yml"
  manifest_row "frontend README" "$ROOT_DIR/frontend/README.md"
  manifest_row "frontend quickstart" "$ROOT_DIR/frontend/QUICKSTART.md"
  printf '\n'

  printf '## Runtime Packaging\n\n'
  printf '| Label | Path | Bytes | SHA256 |\n'
  printf '| --- | --- | --- | --- |\n'
  manifest_row "compose stack" "$ROOT_DIR/docker-compose.yml"
  manifest_row "source Dockerfile" "$ROOT_DIR/Dockerfile"
  manifest_row "prebuilt runtime Dockerfile" "$ROOT_DIR/Dockerfile.prebuilt"
  manifest_row "frontend Dockerfile" "$ROOT_DIR/frontend/Dockerfile"
  manifest_row "frontend nginx config" "$ROOT_DIR/frontend/nginx.conf"
  manifest_row "env example" "$ROOT_DIR/.env.example"
  manifest_row "webhook example config" "$ROOT_DIR/config/openpr-webhook.example.toml"
  printf '\n'

  printf '## Operational Scripts\n\n'
  printf '| Label | Path | Bytes | SHA256 |\n'
  printf '| --- | --- | --- | --- |\n'
  manifest_row "start script" "$ROOT_DIR/scripts/start.sh"
  manifest_row "verify script" "$ROOT_DIR/scripts/verify.sh"
  manifest_row "e2e test script" "$ROOT_DIR/scripts/e2e-test.sh"
  manifest_row "API test script" "$ROOT_DIR/scripts/test-api.sh"
  manifest_row "MCP test script" "$ROOT_DIR/scripts/test-mcp.sh"
  manifest_row "benchmark script" "$ROOT_DIR/scripts/benchmark.sh"
  manifest_row "dev database script" "$ROOT_DIR/scripts/dev-up.sh"
  manifest_row "dev check script" "$ROOT_DIR/scripts/dev-check.sh"
  manifest_row "database init script" "$ROOT_DIR/scripts/init-db.sh"
  manifest_row "backup script" "$ROOT_DIR/scripts/backup-db.sh"
  manifest_row "restore script" "$ROOT_DIR/scripts/restore-db.sh"
  manifest_row "stop script" "$ROOT_DIR/scripts/stop.sh"
  manifest_row "clean script" "$ROOT_DIR/scripts/clean.sh"
} >"$output_tmp"

if [[ -f "$OUTPUT_PATH" ]]; then
  chmod --reference="$OUTPUT_PATH" "$output_tmp"
fi
mv -f "$output_tmp" "$OUTPUT_PATH"
output_tmp=""

echo "delivery manifest: $OUTPUT_PATH" >&2

if [[ "${summary_failed:-missing}" != "0" || "$pass_count" != "${summary_total:-}" ]]; then
  exit 1
fi

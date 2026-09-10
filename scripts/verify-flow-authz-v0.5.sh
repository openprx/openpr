#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.5 authorization verifier.
#
# This verifier is intentionally conservative. It parses the contract before it
# inspects the implementation, runs the named Rust evidence against PostgreSQL,
# and records every missing criterion as a non-pass. In particular, it does not
# manufacture Collection/retention fixtures that the v0.5 schema cannot express.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/flow_contract_path.sh
source "$ROOT_DIR/scripts/lib/flow_contract_path.sh"

REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
ADR_PATH=""
LIMITS_PATH=""
EVIDENCE_ROOT=""
JSON_MODE=0
DATABASE_URL="postgresql://flowtest:flowtest@127.0.0.1:25433/postgres"

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-authz-v0.5.sh --adr PATH --limits PATH --json [OPTIONS]

Verifies all ten v0.5 authorization hard gates and writes authz-result.json.

Options:
  --adr PATH              ADR-0012 path (required).
  --limits PATH           limits-v1.md path (required).
  --contracts-root DIR    Contract checkout (default: /opt/working/sylvode-flow).
  --evidence-root DIR     Output directory (default: /tmp/openpr-flow-evidence/v0.5).
  --repo-root DIR         Source checkout (default: this checkout).
  --database-url URL      PostgreSQL test authority (default: fixed Flow test DSN).
  --json                  Required; emit the artifact on stdout.
  -h, --help              Show this help.

Exit codes: 0 all ten gates passed; 1 a gate/evidence requirement failed;
2 usage, contract parsing, tool, or artifact integrity failure.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adr) ADR_PATH="${2:?--adr requires a path}"; shift 2 ;;
    --limits) LIMITS_PATH="${2:?--limits requires a path}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a directory}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a directory}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a directory}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a URL}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "FAIL: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "FAIL: unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$ADR_PATH" || -z "$LIMITS_PATH" || $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --adr, --limits, and --json are required" >&2
  usage >&2
  exit 2
fi
for tool in cargo dirname git grep jq mktemp python3 realpath rmdir; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required tool: $tool" >&2; exit 2; }
done
ADR_PATH="$(flow_resolve_contract_path --adr "$ADR_PATH" "$CONTRACTS_ROOT")" || exit 2
LIMITS_PATH="$(flow_resolve_contract_path --limits "$LIMITS_PATH" "$CONTRACTS_ROOT")" || exit 2
GATE_COMMANDS_PATH="$CONTRACTS_ROOT/gates/gate-commands.md"
[[ -f "$GATE_COMMANDS_PATH" ]] || { echo "FAIL: gate commands not found: $GATE_COMMANDS_PATH" >&2; exit 2; }
git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
  echo "FAIL: --repo-root is not a git checkout: $REPO_ROOT" >&2
  exit 2
}

if [[ -z "$EVIDENCE_ROOT" ]]; then
  EVIDENCE_ROOT="${TMPDIR:-/tmp}/openpr-flow-evidence/v0.5"
fi
CONTRACTS_REAL="$(realpath -m "$CONTRACTS_ROOT")"
EVIDENCE_REAL="$(realpath -m "$EVIDENCE_ROOT")"
case "$EVIDENCE_REAL/" in
  "$CONTRACTS_REAL/"*)
    echo "FAIL: refusing to write authorization evidence inside the read-only contract checkout: $EVIDENCE_REAL" >&2
    exit 2
    ;;
esac
mkdir -p "$EVIDENCE_REAL/logs"

SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
SOURCE_DIRTY_STATUS="$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all -- \
  apps crates spikes migrations .cargo Cargo.toml Cargo.lock)"
SOURCE_DIRTY=$([[ -n "$SOURCE_DIRTY_STATUS" ]] && echo true || echo false)
SOURCE_DIRTY_ENTRIES="$(printf '%s\n' "$SOURCE_DIRTY_STATUS" | jq -R 'select(length > 0)' | jq -s '.')"

BUILD_LOG="$EVIDENCE_REAL/logs/authz.worker-build.log"
LIST_LOG="$EVIDENCE_REAL/logs/authz.test-list.log"
TEST_LOG="$EVIDENCE_REAL/logs/authz.cargo-test.log"
REQUIRED_TESTS_FILE="$EVIDENCE_REAL/logs/authz.required-tests.txt"
SURFACE_LOG="$EVIDENCE_REAL/logs/authz.surface-parity.log"
SURFACE_EVIDENCE_ROOT="$EVIDENCE_REAL/dependencies/surface-v0.4"
SURFACE_RESULT="$SURFACE_EVIDENCE_ROOT/surface-coverage-result.json"
BASELINE_LOG="$EVIDENCE_REAL/logs/authz.member-baseline.log"
BASELINE_EVIDENCE_ROOT="$EVIDENCE_REAL/dependencies/authz-baseline-v0.4"
BASELINE_RESULT="$BASELINE_EVIDENCE_ROOT/authz-baseline-result.json"
MUTATION_BOUNDARY_LOG="$EVIDENCE_REAL/logs/authz.mutation-boundary.log"
MUTATION_PRESENCE_LOG="$EVIDENCE_REAL/logs/authz.mutation-presence.log"
MUTATION_FENCE_LOG="$EVIDENCE_REAL/logs/authz.mutation-fence.log"
MUTATION_REAUTHORIZE_LOG="$EVIDENCE_REAL/logs/authz.mutation-reauthorize.log"
MUTATION_MOVE_REAUTH_LOG="$EVIDENCE_REAL/logs/authz.mutation-move-reauthorize.log"
MUTATION_TICKET_EXPIRY_LOG="$EVIDENCE_REAL/logs/authz.mutation-ticket-expiry.log"
MUTATION_CACHE_POISON_LOG="$EVIDENCE_REAL/logs/authz.mutation-cache-poison.log"
MUTATION_DRY_RUN_LOG="$EVIDENCE_REAL/logs/authz.mutation-dry-run.log"
MUTATION_ADMIN_BOT_LOG="$EVIDENCE_REAL/logs/authz.mutation-admin-bot.log"
MUTATION_DEPTH_BUDGET_LOG="$EVIDENCE_REAL/logs/authz.mutation-depth-budget.log"
MUTATION_NUMERIC_BUDGET_LOG="$EVIDENCE_REAL/logs/authz.mutation-numeric-budget.log"

run_timed() {
  local log="$1"
  shift
  local started ended status
  started="$(date +%s%N)"
  set +e
  (cd "$REPO_ROOT" && "$@") >"$log" 2>&1
  status=$?
  set -e
  ended="$(date +%s%N)"
  printf '%s %s\n' "$status" "$(((ended - started) / 1000000))"
}

echo "=== build collab isolated-apply worker ===" >&2
read -r BUILD_EXIT BUILD_MS < <(run_timed "$BUILD_LOG" cargo build -p collab-core --bin collab-isolated-apply-worker)
echo "  exit=$BUILD_EXIT duration_ms=$BUILD_MS log=$BUILD_LOG" >&2

echo "=== enumerate api library tests ===" >&2
read -r LIST_EXIT LIST_MS < <(run_timed "$LIST_LOG" cargo test -p api --lib --no-fail-fast -- --list)
echo "  exit=$LIST_EXIT duration_ms=$LIST_MS log=$LIST_LOG" >&2

echo "=== run Flow authorization evidence against PostgreSQL ===" >&2
readarray -t REQUIRED_TESTS <<'EOF'
flow::collab::authz::database_tests::a_boundary_on_the_deepest_legal_node_is_still_seen
flow::collab::authz::database_tests::a_chain_one_node_past_the_limit_is_rejected
flow::collab::authz::database_tests::a_parent_id_cycle_is_rejected
flow::collab::authz::database_tests::a_parent_in_another_workspace_is_rejected_not_treated_as_a_root
flow::collab::authz::database_tests::boundary_and_baseline_semantics_are_unchanged
flow::collab::authz::database_tests::a_workspace_admin_keeps_the_rescue_path_when_the_chain_is_broken
flow::collab::authz::database_tests::depth_1_inherits_from_the_database_then_breaks_at_the_child_boundary
flow::move_object::database_tests::depth_20_evaluation_crosses_concurrent_grant_and_move
routes::flow::flow_database_tests::depth_32_content_commit_path_stays_inside_the_frozen_authz_budgets
flow::collab::write::database_tests::epoch_fencing_blocks_a_write_that_straddles_a_concurrent_revocation
flow::command::database_tests::authz_epoch_is_read_before_permission_so_a_revocation_between_them_cannot_be_fenced_out
flow::move_object::database_tests::concurrent_source_revocation_and_target_downgrade_are_rechecked_before_move_commit
flow::collab::permission_cache::tests::stale_epoch_poison_is_a_miss_and_is_removed
routes::flow::flow_database_tests::poisoned_high_privilege_cache_cannot_make_read_visible_or_write_persist
routes::collab::collab_database_tests::an_expired_ticket_is_rejected_at_atomic_consumption
routes::collab::collab_database_tests::ticket_issuance_holds_current_epoch_through_insert_and_rechecks_membership
routes::collab::collab_database_tests::a_flow_enabled_workspace_still_issues_a_ticket_and_still_upgrades
flow::grants::database_tests::grant_count_ceilings_are_enforced_and_never_silently_truncate
events::dispatcher::dispatcher_database_tests::coalescing_cap_freezes_the_row_and_opens_a_new_one_with_exact_source_counts
flow::grants::database_tests::initial_grants_replaces_the_roster_and_cuts_the_principals_it_omits
flow::move_object::database_tests::moving_under_a_boundary_denies_a_baseline_member_immediately
routes::flow::flow_database_tests::member_add_role_change_and_remove_advance_epoch_in_their_transactions
routes::flow::flow_database_tests::baseline_change_advances_epoch_emits_event_and_physically_invalidates_cache
flow::grants::database_tests::subtree_revocation_removes_presence_closes_only_insufficient_sessions_and_stops_fanout
routes::flow::flow_database_tests::effective_permission_filters_every_flow_read_path_without_count_leakage
routes::flow::flow_database_tests::flow_search_filters_before_cardinality_cursor_snippet_and_frontier
routes::flow::flow_database_tests::object_reads_reauthorize_after_their_final_epoch_check_changes
flow::relations::database_tests::duplicate_link_is_typed_invalid_update_and_target_permission_is_a_real_second_gate
flow::relations::database_tests::relation_read_paginates_and_unavailable_is_the_exact_one_field_union
flow::grants::database_tests::self_lockout_needs_confirmation_and_leaves_an_admin_rescue_path
flow::grants::database_tests::a_dry_run_returns_the_same_summary_and_writes_nothing
flow::grants::database_tests::an_admin_bot_does_not_bypass_an_object_boundary_but_keeps_workspace_admin
flow::grants::database_tests::archive_and_restore_stay_in_the_edit_tier_but_a_navigator_does_not
flow::command::database_tests::lifecycle_tier_uses_the_real_impact_set_and_preserves_the_edit_baseline
flow::command::database_tests::lifecycle_descendant_growth_does_not_turn_a_plain_archive_into_a_cascade
flow::command::database_tests::lifecycle_holds_the_current_authz_epoch_fence_until_its_commit
EOF
printf '%s\n' "${REQUIRED_TESTS[@]}" >"$REQUIRED_TESTS_FILE"
: >"$TEST_LOG"
TEST_START="$(date +%s%N)"
TEST_EXIT=0
for test_name in "${REQUIRED_TESTS[@]}"; do
  printf '=== %s ===\n' "$test_name" >>"$TEST_LOG"
  set +e
  (cd "$REPO_ROOT" && OPENPR_TEST_DATABASE_URL="$DATABASE_URL" \
    cargo test -p api --lib --no-fail-fast "$test_name" -- --exact --nocapture --test-threads=1) >>"$TEST_LOG" 2>&1
  test_status=$?
  set -e
  printf '=== exit=%s test=%s ===\n' "$test_status" "$test_name" >>"$TEST_LOG"
  if [[ $test_status -ne 0 ]]; then
    TEST_EXIT=$test_status
  fi
done
TEST_END="$(date +%s%N)"
TEST_MS="$(((TEST_END - TEST_START) / 1000000))"
echo "  exit=$TEST_EXIT duration_ms=$TEST_MS log=$TEST_LOG" >&2

mkdir -p "$SURFACE_EVIDENCE_ROOT" "$BASELINE_EVIDENCE_ROOT"
echo "=== run v0.4 surface parity dependency ===" >&2
read -r SURFACE_EXIT SURFACE_MS < <(run_timed "$SURFACE_LOG" \
  "$REPO_ROOT/scripts/verify-flow-surface-coverage.sh" \
  --release 0.4 --contracts-root "$CONTRACTS_ROOT" --evidence-root "$SURFACE_EVIDENCE_ROOT" \
  --repo-root "$REPO_ROOT" --json)
echo "  exit=$SURFACE_EXIT duration_ms=$SURFACE_MS log=$SURFACE_LOG" >&2

echo "=== run v0.4 member-baseline dependency ===" >&2
read -r BASELINE_EXIT BASELINE_MS < <(run_timed "$BASELINE_LOG" \
  "$REPO_ROOT/scripts/verify-flow-authz-baseline-v0.4.sh" \
  --adr "$ADR_PATH" --contracts-root "$CONTRACTS_ROOT" --database-url "$DATABASE_URL" \
  --repo-root "$REPO_ROOT" --evidence-root "$BASELINE_EVIDENCE_ROOT" --json)
echo "  exit=$BASELINE_EXIT duration_ms=$BASELINE_MS log=$BASELINE_LOG" >&2

# Real source mutations run in a detached disposable worktree with its own default target. Sharing
# the primary checkout's target would let the worktree's compile-time manifest path contaminate
# build provenance in later primary-checkout builds. Each mutation is restored before the next one,
# and the worktree is removed by the EXIT trap even when compilation or a criterion fails.
# Keep the independent target on the same filesystem as the checkout: system /tmp is commonly a
# small tmpfs and cannot safely hold a full Rust workspace build.
MUTATION_PARENT="$(mktemp -d "$(dirname "$REPO_ROOT")/openpr-authz-mutations.XXXXXX")"
MUTATION_REPO="$MUTATION_PARENT/source"
cleanup_mutation_worktree() {
  if git -C "$REPO_ROOT" worktree list --porcelain | grep -Fxq "worktree $MUTATION_REPO"; then
    git -C "$REPO_ROOT" worktree remove --force "$MUTATION_REPO" >/dev/null 2>&1 || true
  fi
  rmdir "$MUTATION_PARENT" >/dev/null 2>&1 || true
}
trap cleanup_mutation_worktree EXIT
git -C "$REPO_ROOT" worktree add --detach "$MUTATION_REPO" "$SOURCE_HEAD" >/dev/null

apply_exact_mutation() {
  local relative_path="$1" before="$2" after="$3"
  python3 - "$MUTATION_REPO/$relative_path" "$before" "$after" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
before, after = sys.argv[2:]
source = path.read_text(encoding="utf-8")
count = source.count(before)
if count != 1:
    raise SystemExit(f"mutation target must occur exactly once in {path}: observed {count}")
path.write_text(source.replace(before, after, 1), encoding="utf-8")
PY
}

run_mutation_test() {
  local log="$1" marker="$2" test_name="$3"
  shift 3
  local started ended status
  started="$(date +%s%N)"
  {
    printf '%s\n' "$marker"
    set +e
    (cd "$MUTATION_REPO" && OPENPR_TEST_DATABASE_URL="$DATABASE_URL" env "$@" \
      cargo test --locked -p api --lib --no-fail-fast \
      "$test_name" -- --exact --nocapture --test-threads=1)
    status=$?
    set -e
    printf 'mutation_exit=%s test=%s\n' "$status" "$test_name"
  } >"$log" 2>&1
  ended="$(date +%s%N)"
  printf '%s %s\n' "$status" "$(((ended - started) / 1000000))"
}

BOUNDARY_TEST="flow::move_object::database_tests::depth_20_evaluation_crosses_concurrent_grant_and_move"
apply_exact_mutation "apps/api/src/flow/collab/authz.rs" \
  'Ok(best_grant_in(bounded).unwrap_or(PermissionLevel::Denied))' \
  'Ok(best_grant_in(bounded).unwrap_or(PermissionLevel::View))'
apply_exact_mutation "apps/api/src/flow/collab/authz.rs" \
  'best_grant.unwrap_or(PermissionLevel::Denied)' \
  'best_grant.unwrap_or(PermissionLevel::View)'
read -r MUTATION_BOUNDARY_EXIT MUTATION_BOUNDARY_MS < <(run_mutation_test \
  "$MUTATION_BOUNDARY_LOG" "WP24_MUTATION_DENIED_TO_VIEW_ACTIVE" "$BOUNDARY_TEST")
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/flow/collab/authz.rs

PRESENCE_TEST="flow::grants::database_tests::subtree_revocation_removes_presence_closes_only_insufficient_sessions_and_stops_fanout"
apply_exact_mutation "apps/api/src/flow/collab/revocation.rs" \
  'let presence_removed = registry.remove_presence_for_sessions(revoked);' \
  'let presence_removed = 0;'
read -r MUTATION_PRESENCE_EXIT MUTATION_PRESENCE_MS < <(run_mutation_test \
  "$MUTATION_PRESENCE_LOG" "WP24_MUTATION_PRESENCE_REMOVAL_DISABLED_ACTIVE" "$PRESENCE_TEST")
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/flow/collab/revocation.rs

FENCE_TEST="flow::collab::write::database_tests::epoch_fencing_blocks_a_write_that_straddles_a_concurrent_revocation"
apply_exact_mutation "apps/api/src/flow/collab/write.rs" \
  'match fence_epoch_for_share(tx, request.workspace_id, request.checked_epoch).await {' \
  'match Ok::<(), ApiError>(()) {'
read -r MUTATION_FENCE_EXIT MUTATION_FENCE_MS < <(run_mutation_test \
  "$MUTATION_FENCE_LOG" "WP24_MUTATION_EPOCH_FENCE_DISABLED_ACTIVE" "$FENCE_TEST")
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/flow/collab/write.rs

REAUTHORIZE_TEST="routes::flow::flow_database_tests::object_reads_reauthorize_after_their_final_epoch_check_changes"
apply_exact_mutation "apps/api/src/flow/policy.rs" \
  'Ok(current == context.authz_epoch)' \
  'Ok(true)'
read -r MUTATION_REAUTHORIZE_EXIT MUTATION_REAUTHORIZE_MS < <(run_mutation_test \
  "$MUTATION_REAUTHORIZE_LOG" "WP24_MUTATION_FINAL_EPOCH_CHECK_ALWAYS_TRUE_ACTIVE" "$REAUTHORIZE_TEST")
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/flow/policy.rs

MOVE_REAUTH_TEST="flow::move_object::database_tests::concurrent_source_revocation_and_target_downgrade_are_rechecked_before_move_commit"
read -r MUTATION_MOVE_REAUTH_EXIT MUTATION_MOVE_REAUTH_MS < <(run_mutation_test \
  "$MUTATION_MOVE_REAUTH_LOG" "GATECHAIN_MUTATION_MOVE_LOCKED_REAUTH_DISABLED_ACTIVE" "$MOVE_REAUTH_TEST" \
  OPENPR_FLOW_TEST_MUTATION_SKIP_MOVE_REAUTH=1)

TICKET_EXPIRY_TEST="routes::collab::collab_database_tests::an_expired_ticket_is_rejected_at_atomic_consumption"
apply_exact_mutation "apps/api/src/flow/collab/ticket.rs" \
  '              AND expires_at > now()' \
  '              AND expires_at <= now()'
read -r MUTATION_TICKET_EXPIRY_EXIT MUTATION_TICKET_EXPIRY_MS < <(run_mutation_test \
  "$MUTATION_TICKET_EXPIRY_LOG" "GATECHAIN_MUTATION_EXPIRED_TICKET_ACCEPTED_ACTIVE" "$TICKET_EXPIRY_TEST")
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/flow/collab/ticket.rs

CACHE_POISON_TEST="routes::flow::flow_database_tests::poisoned_high_privilege_cache_cannot_make_read_visible_or_write_persist"
apply_exact_mutation "apps/api/src/flow/collab/permission_cache.rs" \
  'if entry.authz_epoch == current_epoch && now.saturating_duration_since(entry.last_access) <= self.idle_ttl {' \
  'if now.saturating_duration_since(entry.last_access) <= self.idle_ttl {'
read -r MUTATION_CACHE_POISON_EXIT MUTATION_CACHE_POISON_MS < <(run_mutation_test \
  "$MUTATION_CACHE_POISON_LOG" "GATECHAIN_MUTATION_CACHE_EPOCH_IGNORED_ACTIVE" "$CACHE_POISON_TEST")
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/flow/collab/permission_cache.rs

DRY_RUN_TEST="flow::grants::database_tests::a_dry_run_returns_the_same_summary_and_writes_nothing"
apply_exact_mutation "apps/api/src/flow/grants.rs" \
  $'Ok((outcome, Disposition::Rollback)) => {\n            tx.rollback().await?;' \
  $'Ok((outcome, Disposition::Rollback)) => {\n            tx.commit().await?;'
read -r MUTATION_DRY_RUN_EXIT MUTATION_DRY_RUN_MS < <(run_mutation_test \
  "$MUTATION_DRY_RUN_LOG" "GATECHAIN_MUTATION_DRY_RUN_COMMITTED_ACTIVE" "$DRY_RUN_TEST")
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/flow/grants.rs

ADMIN_BOT_TEST="flow::grants::database_tests::an_admin_bot_does_not_bypass_an_object_boundary_but_keeps_workspace_admin"
apply_exact_mutation "apps/api/src/flow/collab/authz.rs" \
  $'if principal_kind == "user" && (role == "owner" || role == "admin") {\n        if object_exists_in_workspace(conn, workspace_id, object_id).await? {' \
  $'if role == "owner" || role == "admin" {\n        if object_exists_in_workspace(conn, workspace_id, object_id).await? {'
read -r MUTATION_ADMIN_BOT_EXIT MUTATION_ADMIN_BOT_MS < <(run_mutation_test \
  "$MUTATION_ADMIN_BOT_LOG" "GATECHAIN_MUTATION_ADMIN_BOT_FALLBACK_ACTIVE" "$ADMIN_BOT_TEST")
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/flow/collab/authz.rs

DEPTH_BUDGET_TEST="routes::flow::flow_database_tests::depth_32_content_commit_path_stays_inside_the_frozen_authz_budgets"
apply_exact_mutation "apps/api/src/flow/collab/authz.rs" \
  'pub(crate) const TREE_DEPTH_MAX: usize = 32;' \
  'pub(crate) const TREE_DEPTH_MAX: usize = 20;'
read -r MUTATION_DEPTH_BUDGET_EXIT MUTATION_DEPTH_BUDGET_MS < <(run_mutation_test \
  "$MUTATION_DEPTH_BUDGET_LOG" "GATECHAIN_MUTATION_AUTHZ_DEPTH_REDUCED_ACTIVE" "$DEPTH_BUDGET_TEST")
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/flow/collab/authz.rs

apply_exact_mutation "apps/api/src/events/dispatcher.rs" \
  'const COALESCED_SOURCE_EVENTS_MAX: i64 = 20;' \
  'const COALESCED_SOURCE_EVENTS_MAX: i64 = 21;'
NUMERIC_BUDGET_STARTED="$(date +%s%N)"
set +e
{
  printf '%s\n' "GATECHAIN_MUTATION_AUTHZ_NUMERIC_BUDGET_DRIFT_ACTIVE"
  python3 - "$MUTATION_REPO/apps/api/src/flow/collab/authz.rs" \
    "$MUTATION_REPO/apps/api/src/events/dispatcher.rs" "$LIMITS_PATH" <<'PY'
import pathlib
import re
import sys

authz = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
dispatcher = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8")
limits = pathlib.Path(sys.argv[3]).read_text(encoding="utf-8")

def source_value(text, name):
    match = re.search(rf"(?:pub\s+)?const\s+{name}\s*:\s*[^=]+\s*=\s*([0-9_]+)\s*;", text)
    if not match:
        raise SystemExit(f"missing source constant {name}")
    return int(match.group(1).replace("_", ""))

def contract_value(name):
    start = re.search(rf"^{name}:\s*$", limits, re.M)
    if not start:
        raise SystemExit(f"missing contract block {name}")
    tail = limits[start.end():]
    end = re.search(r"^[a-z][a-z0-9_]*\s*:\s*$", tail, re.M)
    body = tail[:end.start()] if end else tail
    match = re.search(r"^\s+status:\s*([0-9][0-9,]*)", body, re.M)
    if not match:
        raise SystemExit(f"missing frozen contract value {name}")
    return int(match.group(1).replace(",", ""))

pairs = [
    ("OBJECT_GRANTS_MAX", source_value(authz, "OBJECT_GRANTS_MAX"), contract_value("object_grants_max")),
    ("COALESCED_SOURCE_EVENTS_MAX", source_value(dispatcher, "COALESCED_SOURCE_EVENTS_MAX"), contract_value("coalesced_source_events_max")),
]
for name, actual, expected in pairs:
    if actual != expected:
        raise SystemExit(f"{name} drift: source={actual} contract={expected}")
print("numeric budgets match contract")
PY
  MUTATION_NUMERIC_BUDGET_EXIT=$?
  printf 'mutation_exit=%s check=authorization_numeric_budgets\n' "$MUTATION_NUMERIC_BUDGET_EXIT"
} >"$MUTATION_NUMERIC_BUDGET_LOG" 2>&1
set -e
NUMERIC_BUDGET_ENDED="$(date +%s%N)"
MUTATION_NUMERIC_BUDGET_MS="$(((NUMERIC_BUDGET_ENDED - NUMERIC_BUDGET_STARTED) / 1000000))"
git -C "$MUTATION_REPO" restore --source=HEAD -- apps/api/src/events/dispatcher.rs
cleanup_mutation_worktree
trap - EXIT

echo "=== real mutation results ===" >&2
echo "  boundary exit=$MUTATION_BOUNDARY_EXIT duration_ms=$MUTATION_BOUNDARY_MS log=$MUTATION_BOUNDARY_LOG" >&2
echo "  presence exit=$MUTATION_PRESENCE_EXIT duration_ms=$MUTATION_PRESENCE_MS log=$MUTATION_PRESENCE_LOG" >&2
echo "  fence exit=$MUTATION_FENCE_EXIT duration_ms=$MUTATION_FENCE_MS log=$MUTATION_FENCE_LOG" >&2
echo "  reauthorize exit=$MUTATION_REAUTHORIZE_EXIT duration_ms=$MUTATION_REAUTHORIZE_MS log=$MUTATION_REAUTHORIZE_LOG" >&2
echo "  move reauthorize exit=$MUTATION_MOVE_REAUTH_EXIT duration_ms=$MUTATION_MOVE_REAUTH_MS log=$MUTATION_MOVE_REAUTH_LOG" >&2
echo "  ticket expiry exit=$MUTATION_TICKET_EXPIRY_EXIT duration_ms=$MUTATION_TICKET_EXPIRY_MS log=$MUTATION_TICKET_EXPIRY_LOG" >&2
echo "  cache poison exit=$MUTATION_CACHE_POISON_EXIT duration_ms=$MUTATION_CACHE_POISON_MS log=$MUTATION_CACHE_POISON_LOG" >&2
echo "  dry run exit=$MUTATION_DRY_RUN_EXIT duration_ms=$MUTATION_DRY_RUN_MS log=$MUTATION_DRY_RUN_LOG" >&2
echo "  admin bot exit=$MUTATION_ADMIN_BOT_EXIT duration_ms=$MUTATION_ADMIN_BOT_MS log=$MUTATION_ADMIN_BOT_LOG" >&2
echo "  depth budget exit=$MUTATION_DEPTH_BUDGET_EXIT duration_ms=$MUTATION_DEPTH_BUDGET_MS log=$MUTATION_DEPTH_BUDGET_LOG" >&2
echo "  numeric budget exit=$MUTATION_NUMERIC_BUDGET_EXIT duration_ms=$MUTATION_NUMERIC_BUDGET_MS log=$MUTATION_NUMERIC_BUDGET_LOG" >&2
echo "  depth budget exit=$MUTATION_DEPTH_BUDGET_EXIT duration_ms=$MUTATION_DEPTH_BUDGET_MS log=$MUTATION_DEPTH_BUDGET_LOG" >&2

STATIC_DYNAMIC_JSON="$(python3 - \
  "$REPO_ROOT" "$CONTRACTS_ROOT" "$ADR_PATH" "$LIMITS_PATH" "$GATE_COMMANDS_PATH" \
  "$LIST_LOG" "$LIST_EXIT" "$LIST_MS" "$TEST_LOG" "$TEST_EXIT" "$TEST_MS" "$REQUIRED_TESTS_FILE" \
  "$BUILD_LOG" "$BUILD_EXIT" "$BUILD_MS" "$DATABASE_URL" \
  "$SURFACE_RESULT" "$SURFACE_LOG" "$SURFACE_EXIT" "$SURFACE_MS" \
  "$BASELINE_RESULT" "$BASELINE_LOG" "$BASELINE_EXIT" "$BASELINE_MS" \
  "$MUTATION_BOUNDARY_LOG" "$MUTATION_BOUNDARY_EXIT" "$MUTATION_BOUNDARY_MS" "$BOUNDARY_TEST" \
  "$MUTATION_PRESENCE_LOG" "$MUTATION_PRESENCE_EXIT" "$MUTATION_PRESENCE_MS" "$PRESENCE_TEST" \
  "$MUTATION_FENCE_LOG" "$MUTATION_FENCE_EXIT" "$MUTATION_FENCE_MS" "$FENCE_TEST" \
  "$MUTATION_REAUTHORIZE_LOG" "$MUTATION_REAUTHORIZE_EXIT" "$MUTATION_REAUTHORIZE_MS" "$REAUTHORIZE_TEST" \
  "$MUTATION_MOVE_REAUTH_LOG" "$MUTATION_MOVE_REAUTH_EXIT" "$MUTATION_MOVE_REAUTH_MS" "$MOVE_REAUTH_TEST" \
  "$MUTATION_TICKET_EXPIRY_LOG" "$MUTATION_TICKET_EXPIRY_EXIT" "$MUTATION_TICKET_EXPIRY_MS" "$TICKET_EXPIRY_TEST" \
  "$MUTATION_CACHE_POISON_LOG" "$MUTATION_CACHE_POISON_EXIT" "$MUTATION_CACHE_POISON_MS" "$CACHE_POISON_TEST" \
  "$MUTATION_DRY_RUN_LOG" "$MUTATION_DRY_RUN_EXIT" "$MUTATION_DRY_RUN_MS" "$DRY_RUN_TEST" \
  "$MUTATION_ADMIN_BOT_LOG" "$MUTATION_ADMIN_BOT_EXIT" "$MUTATION_ADMIN_BOT_MS" "$ADMIN_BOT_TEST" \
  "$MUTATION_DEPTH_BUDGET_LOG" "$MUTATION_DEPTH_BUDGET_EXIT" "$MUTATION_DEPTH_BUDGET_MS" "$DEPTH_BUDGET_TEST" \
  "$MUTATION_NUMERIC_BUDGET_LOG" "$MUTATION_NUMERIC_BUDGET_EXIT" "$MUTATION_NUMERIC_BUDGET_MS" <<'PY'
import json
import pathlib
import re
import sys

(
    repo_s, contracts_s, adr_s, limits_s, commands_s,
    list_log_s, list_exit_s, list_ms_s, test_log_s, test_exit_s, test_ms_s, required_tests_s,
    build_log_s, build_exit_s, build_ms_s, database_url_s,
    surface_result_s, surface_log_s, surface_exit_s, surface_ms_s,
    baseline_result_s, baseline_log_s, baseline_exit_s, baseline_ms_s,
    mutation_boundary_log_s, mutation_boundary_exit_s, mutation_boundary_ms_s, boundary_test,
    mutation_presence_log_s, mutation_presence_exit_s, mutation_presence_ms_s, presence_test,
    mutation_fence_log_s, mutation_fence_exit_s, mutation_fence_ms_s, fence_test,
    mutation_reauthorize_log_s, mutation_reauthorize_exit_s, mutation_reauthorize_ms_s, reauthorize_test,
    mutation_move_reauth_log_s, mutation_move_reauth_exit_s, mutation_move_reauth_ms_s, move_reauth_test,
    mutation_ticket_expiry_log_s, mutation_ticket_expiry_exit_s, mutation_ticket_expiry_ms_s, ticket_expiry_test,
    mutation_cache_poison_log_s, mutation_cache_poison_exit_s, mutation_cache_poison_ms_s, cache_poison_test,
    mutation_dry_run_log_s, mutation_dry_run_exit_s, mutation_dry_run_ms_s, dry_run_test,
    mutation_admin_bot_log_s, mutation_admin_bot_exit_s, mutation_admin_bot_ms_s, admin_bot_test,
    mutation_depth_budget_log_s, mutation_depth_budget_exit_s, mutation_depth_budget_ms_s, depth_budget_test,
    mutation_numeric_budget_log_s, mutation_numeric_budget_exit_s, mutation_numeric_budget_ms_s,
) = sys.argv[1:]
repo = pathlib.Path(repo_s)
contracts = pathlib.Path(contracts_s)

def read(path):
    return pathlib.Path(path).read_text(encoding="utf-8", errors="replace")

adr = read(adr_s)
limits = read(limits_s)
commands = read(commands_s)
list_text = read(list_log_s)
test_text = read(test_log_s)
required_tests = [line for line in read(required_tests_s).splitlines() if line]
if not required_tests or len(required_tests) != len(set(required_tests)):
    raise SystemExit("required test set parse failed: it must be non-empty and unique")

def read_json(path):
    candidate = pathlib.Path(path)
    if not candidate.is_file():
        return {}
    try:
        value = json.loads(read(candidate))
    except json.JSONDecodeError:
        return {}
    return value if isinstance(value, dict) else {}

surface_result = read_json(surface_result_s)
baseline_result = read_json(baseline_result_s)
mutation_boundary_log = read(mutation_boundary_log_s)
mutation_presence_log = read(mutation_presence_log_s)
mutation_fence_log = read(mutation_fence_log_s)
mutation_reauthorize_log = read(mutation_reauthorize_log_s)
mutation_move_reauth_log = read(mutation_move_reauth_log_s)
mutation_ticket_expiry_log = read(mutation_ticket_expiry_log_s)
mutation_cache_poison_log = read(mutation_cache_poison_log_s)
mutation_dry_run_log = read(mutation_dry_run_log_s)
mutation_admin_bot_log = read(mutation_admin_bot_log_s)
mutation_depth_budget_log = read(mutation_depth_budget_log_s)
mutation_numeric_budget_log = read(mutation_numeric_budget_log_s)

GATES = [
    "permission_inheritance_and_break",
    "authz_linearization_no_escalation",
    "permission_cache_is_not_authority",
    "permission_revocation_closes_subtree_sessions",
    "search_and_relation_use_effective_permission",
    "authz_boundary_self_lockout_guarded",
    "archive_tier_by_object_scope",
    "token_expiry_and_permission_revocation",
    "authz_numeric_budgets_locked",
    "admin_bot_does_not_bypass_object_boundary",
]

def parse_limit(name):
    match = re.search(rf"^\|\s*`{re.escape(name)}`\s*\|\s*([0-9][0-9,]*)\s*(?:ms)?\s*\|", limits, re.M)
    if not match:
        raise SystemExit(f"contract parse failed: non-empty numeric limit required for {name}")
    return int(match.group(1).replace(",", ""))

tree_depth = parse_limit("tree_depth_max")
hold_p95 = parse_limit("document_lock_hold_ms_p95_max")
hold_max = parse_limit("document_lock_hold_ms_max")
if not all((tree_depth, hold_p95, hold_max)):
    raise SystemExit("contract parse failed: authorization limits must be positive")
object_grants_match = re.search(r"^object_grants_max:\s*\n\s*status:\s*([^\s#]+)", limits, re.M)
if not object_grants_match:
    raise SystemExit("contract parse failed: object_grants_max structured status is empty")
object_grants_status = object_grants_match.group(1)
object_grants_max = int(object_grants_status.replace(",", "")) if object_grants_status.isdigit() else None

def structured_limit(name):
    start = re.search(rf"^{re.escape(name)}:\s*$", limits, re.M)
    if not start:
        raise SystemExit(f"contract parse failed: structured block absent for {name}")
    tail = limits[start.end():]
    end = re.search(r"^(?:[a-z][a-z0-9_]*)\s*:\s*$", tail, re.M)
    body = tail[:end.start()] if end else tail
    def field(key):
        match = re.search(rf"^\s+{key}:\s*(.+?)\s*$", body, re.M)
        if not match or not match.group(1).strip():
            raise SystemExit(f"contract parse failed: {name}.{key} is empty")
        return match.group(1).strip()
    status, set_by, rule = field("status"), field("set_by"), field("rule")
    value_match = re.match(r"([0-9][0-9,]*)", status)
    value = int(value_match.group(1).replace(",", "")) if value_match else None
    return {"status": status, "value": value, "set_by": set_by, "rule": rule,
            "frozen": value is not None and value > 0 and status != "unset"}

object_grants_contract = structured_limit("object_grants_max")
coalesced_sources_contract = structured_limit("coalesced_source_events_max")

authz_start = commands.find("Authz verifier 覆盖 v0.5")
authz_end = commands.find("Multi-document verifier 产出", authz_start)
if authz_start < 0 or authz_end <= authz_start:
    raise SystemExit("contract parse failed: authz_verify paragraph is empty")
authz_clause = commands[authz_start:authz_end]
authz_paragraph_gates = GATES[:7]
parsed_gates = [name for name in authz_paragraph_gates if re.search(rf"`{re.escape(name)}`", authz_clause)]
if not parsed_gates or parsed_gates != authz_paragraph_gates:
    raise SystemExit(f"contract parse failed: seven authz gates not found in order: {parsed_gates!r}")

for marker in ["(a)", "(b)", "(c)", "checked_epoch", "committed_epoch", "barrier 真实可控"]:
    if marker not in authz_clause:
        raise SystemExit(f"contract parse failed: authz linearization marker absent: {marker}")
if not re.search(r"(?:状态|status)\s*[:：]\s*(?:Accepted|已接受)", adr, re.I):
    raise SystemExit("contract parse failed: ADR-0012 is not Accepted")

files = {
    "authz": repo / "apps/api/src/flow/collab/authz.rs",
    "permission_cache": repo / "apps/api/src/flow/collab/permission_cache.rs",
    "policy": repo / "apps/api/src/flow/policy.rs",
    "write": repo / "apps/api/src/flow/collab/write.rs",
    "revocation": repo / "apps/api/src/flow/collab/revocation.rs",
    "registry": repo / "apps/api/src/flow/collab/registry.rs",
    "grants": repo / "apps/api/src/flow/grants.rs",
    "command": repo / "apps/api/src/flow/command.rs",
    "move": repo / "apps/api/src/flow/move_object.rs",
    "relations": repo / "apps/api/src/flow/relations.rs",
    "model": repo / "apps/api/src/flow/model.rs",
    "search": repo / "apps/api/src/flow/search.rs",
    "routes_flow": repo / "apps/api/src/routes/flow.rs",
    "routes_collab": repo / "apps/api/src/routes/collab.rs",
    "ticket": repo / "apps/api/src/flow/collab/ticket.rs",
    "dispatcher": repo / "apps/api/src/events/dispatcher.rs",
    "migration": repo / "migrations/0054_flow_data_layer.sql",
    "core_limits": repo / "crates/collab-core/src/limits.rs",
}
source = {}
for key, path in files.items():
    if not path.is_file():
        raise SystemExit(f"source cross-check failed: required file absent: {path}")
    source[key] = read(path)

def const_int(text, name):
    match = re.search(rf"(?:pub(?:\([^)]*\))?\s+)?const\s+{re.escape(name)}\s*:\s*[^=]+\s*=\s*([0-9][0-9_]*)\s*;", text)
    return int(match.group(1).replace("_", "")) if match else None

def fn_body(text, name):
    match = re.search(rf"(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+{re.escape(name)}\s*\([^{{]*\)\s*(?:->[^{{]+)?\{{", text)
    if not match:
        return ""
    start = text.find("{", match.start())
    depth = 0
    for index in range(start, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return text[start:index + 1]
    return ""

listed = set()
for line in list_text.splitlines():
    match = re.match(r"^(.+): test$", line)
    if match:
        listed.add(match.group(1))
if not listed:
    raise SystemExit("test inventory parse failed: zero tests discovered")

statuses = {}
for line in test_text.splitlines():
    match = re.match(r"^test (.+) \.\.\. (ok|FAILED|ignored)$", line)
    if match:
        statuses[match.group(1)] = match.group(2)
# Evidence markers emitted during a test appear between libtest's `...` prefix and its final
# `ok`/`FAILED` line. Parse those segments too instead of silently losing every instrumented test.
for name in required_tests:
    if name in statuses:
        continue
    match = re.search(
        r"^test " + re.escape(name) + r" \.\.\.(.*?)(?=^test result:)",
        test_text,
        re.M | re.S,
    )
    if not match:
        continue
    segment = match.group(1)
    if re.search(r"(?:^|\n)ok\b|\sok$", segment, re.M):
        statuses[name] = "ok"
    elif re.search(r"(?:^|\n)FAILED\b|\sFAILED$", segment, re.M):
        statuses[name] = "FAILED"
summary_matches = re.findall(
    r"^test result: (ok|FAILED)\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored; ([0-9]+) measured; ([0-9]+) filtered out; finished in ([0-9.]+)s$",
    test_text,
    re.M,
)
if len(summary_matches) != len(required_tests):
    raise SystemExit(
        f"strict cargo summary parse failed: expected {len(required_tests)} summaries, observed {len(summary_matches)}"
    )
summary_status = "ok" if all(item[0] == "ok" for item in summary_matches) else "FAILED"
summary_passed = sum(int(item[1]) for item in summary_matches)
summary_failed = sum(int(item[2]) for item in summary_matches)
summary_ignored = sum(int(item[3]) for item in summary_matches)
summary_measured = sum(int(item[4]) for item in summary_matches)
summary_filtered = sum(int(item[5]) for item in summary_matches)
summary_seconds = sum(float(item[6]) for item in summary_matches)
early_return = "skipped: OPENPR_TEST_DATABASE_URL is not set" in test_text

def test_ok(name):
    return name in listed and statuses.get(name) == "ok" and int(test_exit_s) == 0 and not early_return

observed = {gate: [] for gate in GATES}
reason_codes = {gate: [] for gate in GATES}
not_covered_reason_codes = {gate: [] for gate in GATES}

def add(gate, criterion, passed, actual, evidence, reason=None):
    item = {
        "criterion": criterion,
        "status": "passed" if passed else "failed",
        "passed": bool(passed),
        "actual": actual,
        "evidence": evidence if isinstance(evidence, list) else [evidence],
    }
    if reason and not passed:
        item["reason_code"] = reason
        if reason not in reason_codes[gate]:
            reason_codes[gate].append(reason)
    observed[gate].append(item)

def add_not_covered(gate, criterion, actual, evidence, reason):
    item = {
        "criterion": criterion,
        "status": "not_covered",
        "passed": None,
        "actual": actual,
        "evidence": evidence if isinstance(evidence, list) else [evidence],
        "reason_code": reason,
    }
    observed[gate].append(item)
    if reason not in not_covered_reason_codes[gate]:
        not_covered_reason_codes[gate].append(reason)

def add_test(gate, criterion, name, reason="required_test_missing_or_failed"):
    ok = test_ok(name)
    add(gate, criterion, ok, statuses.get(name, "not_observed"), [f"cargo_test:{name}"], None if ok else reason)

# permission_inheritance_and_break
gate = GATES[0]
api_depth = const_int(source["authz"], "TREE_DEPTH_MAX")
core_depth = const_int(source["core_limits"], "TREE_DEPTH_MAX")
add(gate, "tree_depth_constant_matches_contract", api_depth == tree_depth and core_depth == tree_depth,
    {"contract": tree_depth, "api": api_depth, "collab_core": core_depth},
    [str(files["authz"]), str(files["core_limits"])], "tree_depth_constant_drift")
source_object_grants_max = const_int(source["authz"], "OBJECT_GRANTS_MAX")
object_grants_ok = object_grants_max is None or source_object_grants_max == object_grants_max
add(gate, "object_grants_limit_is_parsed_not_invented", object_grants_ok,
    {"contract_status": object_grants_status, "contract_value": object_grants_max, "source_value": source_object_grants_max},
    [limits_s, str(files["authz"])], None if object_grants_ok else "object_grants_limit_drift")
for criterion, name in [
    ("explicit_depth_1_database_fixture", "flow::collab::authz::database_tests::depth_1_inherits_from_the_database_then_breaks_at_the_child_boundary"),
    ("depth_32_and_boundary_at_depth_32", "flow::collab::authz::database_tests::a_boundary_on_the_deepest_legal_node_is_still_seen"),
    ("depth_33_fail_closed", "flow::collab::authz::database_tests::a_chain_one_node_past_the_limit_is_rejected"),
    ("cycle_fail_closed", "flow::collab::authz::database_tests::a_parent_id_cycle_is_rejected"),
    ("incomplete_chain_fail_closed", "flow::collab::authz::database_tests::a_parent_in_another_workspace_is_rejected_not_treated_as_a_root"),
    ("boundary_no_grant_is_denied_not_view", "flow::collab::authz::database_tests::boundary_and_baseline_semantics_are_unchanged"),
    ("workspace_admin_rescue", "flow::collab::authz::database_tests::a_workspace_admin_keeps_the_rescue_path_when_the_chain_is_broken"),
]: add_test(gate, criterion, name)
add_test(gate, "depth_evaluation_crosses_concurrent_grant_and_move",
    "flow::move_object::database_tests::depth_20_evaluation_crosses_concurrent_grant_and_move",
    "concurrent_depth_grant_move_fixture_missing_or_failed")
poison_test = "routes::flow::flow_database_tests::poisoned_high_privilege_cache_cannot_make_read_visible_or_write_persist"
add_test(gate, "permission_cache_miss_forces_authoritative_source", poison_test,
    "cache_miss_authority_fixture_missing_or_failed")
depth_budget_test = "routes::flow::flow_database_tests::depth_32_content_commit_path_stays_inside_the_frozen_authz_budgets"
add_test(gate, "depth_32_commit_path_executes", depth_budget_test)
budget_match = re.search(r"AUTHZ_DEPTH32_COMMIT_BUDGET_EVIDENCE\s+(\{[^\n]+\})", test_text)
budget_evidence = json.loads(budget_match.group(1)) if budget_match else {}
budget_values_ok = all((
    budget_evidence.get("depth") == tree_depth,
    isinstance(budget_evidence.get("samples"), int) and budget_evidence.get("samples", 0) > 0,
    isinstance(budget_evidence.get("p95_ms"), (int, float)),
    isinstance(budget_evidence.get("max_ms"), (int, float)),
    budget_evidence.get("p95_ms", float("inf")) <= hold_p95,
    budget_evidence.get("max_ms", float("inf")) <= hold_max,
))
add(gate, "depth_32_commit_path_lock_hold_budget", budget_values_ok,
    {"required_p95_ms": hold_p95, "required_max_ms": hold_max, "measurement": budget_evidence},
    [test_log_s, f"cargo_test:{depth_budget_test}"], None if budget_values_ok else "commit_path_depth_budget_not_measured")

# authz_linearization_no_escalation
gate = GATES[1]
write_prod = source["write"].split("#[cfg(test)]", 1)[0]
fence_lock = "fence_epoch_for_share(tx" in write_prod and "SELECT authz_epoch" in source["authz"] and "FOR SHARE" in source["authz"]
add(gate, "commit_time_epoch_fence_held_in_write_transaction", fence_lock, "present" if fence_lock else "absent",
    [str(files["write"]), str(files["authz"])], None if fence_lock else "commit_time_fence_missing")
race_c_test = "flow::collab::write::database_tests::epoch_fencing_blocks_a_write_that_straddles_a_concurrent_revocation"
add_test(gate, "race_c_inflight_content_after_revocation", race_c_test)
add_test(gate, "epoch_read_precedes_permission", "flow::command::database_tests::authz_epoch_is_read_before_permission_so_a_revocation_between_them_cannot_be_fenced_out")
move_race_test = "flow::move_object::database_tests::concurrent_source_revocation_and_target_downgrade_are_rechecked_before_move_commit"
add_test(gate, "race_a_move_after_source_grant_revocation", move_race_test, "linearization_race_a_missing_or_failed")
add_test(gate, "race_b_move_after_target_downgrade", move_race_test, "linearization_race_b_missing_or_failed")
epoch_match = re.search(
    r"AUTHZ_CONTENT_EPOCH_EVIDENCE checked_epoch=(\d+) committed_epoch=(\d+) outcome=policy_rejected persisted_updates=0",
    test_text,
)
content_epoch_artifact = bool(epoch_match and int(epoch_match.group(2)) > int(epoch_match.group(1)))
add(gate, "every_content_write_artifact_records_checked_and_committed_epoch", content_epoch_artifact,
    {"checked_epoch": int(epoch_match.group(1)), "committed_epoch": int(epoch_match.group(2)), "persisted_updates": 0} if epoch_match else {},
    [test_log_s], None if content_epoch_artifact else "content_epoch_artifact_fields_missing")
move_mutation_red = int(mutation_move_reauth_exit_s) != 0 and "GATECHAIN_MUTATION_MOVE_LOCKED_REAUTH_DISABLED_ACTIVE" in mutation_move_reauth_log and "test result: FAILED. 0 passed; 1 failed;" in mutation_move_reauth_log
controllable = move_mutation_red and int(mutation_fence_exit_s) != 0
add(gate, "barrier_control_replays_races_a_and_c_without_fencing", controllable,
    {"race_a_b_mutation_exit": int(mutation_move_reauth_exit_s), "race_a_b_duration_ms": int(mutation_move_reauth_ms_s),
     "race_c_mutation_exit": int(mutation_fence_exit_s), "race_c_duration_ms": int(mutation_fence_ms_s)},
    [mutation_move_reauth_log_s, mutation_fence_log_s],
    None if controllable else "barrier_control_mutation_not_implemented")

# permission_cache_is_not_authority
gate = GATES[2]
add_test(gate, "stale_epoch_entry_is_a_miss", "flow::collab::permission_cache::tests::stale_epoch_poison_is_a_miss_and_is_removed")
authorize_body = fn_body(source["policy"], "authorize_flow_objects")
first_epoch_check = authorize_body.find("ensure_epoch_current(")
cache_read = authorize_body.find("cache.get(")
database_read = authorize_body.find("authz::effective_permissions(")
second_epoch_check = authorize_body.find("ensure_epoch_current(", first_epoch_check + 1)
authority_order = min(first_epoch_check, cache_read, database_read, second_epoch_check) >= 0 and first_epoch_check < cache_read < database_read < second_epoch_check
add(gate, "cache_miss_and_epoch_mismatch_return_to_database", authority_order,
    {"first_epoch_check": first_epoch_check, "cache_read": cache_read, "database_read": database_read,
     "second_epoch_check": second_epoch_check},
    [str(files["policy"])], None if authority_order else "database_authority_path_missing")
add_test(gate, "poisoned_high_privilege_cache_cannot_read_or_write", poison_test,
    "poisoned_cache_e2e_fixture_missing_or_failed")
cache_poison_mutation_red = int(mutation_cache_poison_exit_s) != 0 and "GATECHAIN_MUTATION_CACHE_EPOCH_IGNORED_ACTIVE" in mutation_cache_poison_log and "test result: FAILED. 0 passed; 1 failed;" in mutation_cache_poison_log
add(gate, "poisoned_cache_detector_mutation_red", cache_poison_mutation_red,
    {"exit": int(mutation_cache_poison_exit_s), "duration_ms": int(mutation_cache_poison_ms_s)},
    [mutation_cache_poison_log_s], None if cache_poison_mutation_red else "poisoned_cache_mutation_did_not_produce_red")
for criterion, name in [
    ("grant_change_invalidation", "flow::grants::database_tests::initial_grants_replaces_the_roster_and_cuts_the_principals_it_omits"),
    ("parent_id_subtree_invalidation", "flow::move_object::database_tests::moving_under_a_boundary_denies_a_baseline_member_immediately"),
    ("workspace_member_subtree_invalidation", "routes::flow::flow_database_tests::member_add_role_change_and_remove_advance_epoch_in_their_transactions"),
    ("default_member_level_subtree_invalidation", "routes::flow::flow_database_tests::baseline_change_advances_epoch_emits_event_and_physically_invalidates_cache"),
]: add_test(gate, criterion, name)
inherit_invalidation = "invalidate_workspace" in source["grants"] and "set_inheritance" in source["grants"]
add(gate, "inherit_from_parent_subtree_invalidation", inherit_invalidation,
    "workspace-wide invalidation (stricter physical scope)" if inherit_invalidation else "not found",
    [str(files["grants"])], None if inherit_invalidation else "inheritance_invalidation_missing")

# permission_revocation_closes_subtree_sessions
gate = GATES[3]
revocation_test = "flow::grants::database_tests::subtree_revocation_removes_presence_closes_only_insufficient_sessions_and_stops_fanout"
add_test(gate, "single_instance_subtree_revocation", revocation_test)
rev_body = fn_body(source["grants"], "subtree_revocation_removes_presence_closes_only_insufficient_sessions_and_stops_fanout")
for criterion, terms, reason in [
    ("presence_removed_immediately", ("presence_removed", "presence_count"), "presence_removal_assertion_missing"),
    ("revoked_session_excluded_from_presence_fanout", ("broadcast", "must not receive later presence"), "presence_fanout_assertion_missing"),
    ("close_reason_omits_identifiers", ("!reason.contains", "object_id", "member_id"), "close_reason_redaction_assertion_missing"),
]:
    ok = all(term in rev_body for term in terms)
    add(gate, criterion, ok, "asserted in named fixture" if ok else "assertion absent", [str(files["grants"])], None if ok else reason)
production_remove = "remove_presence_for_sessions(revoked)" in source["revocation"]
add(gate, "production_revocation_calls_presence_removal", production_remove, "present" if production_remove else "absent",
    [str(files["revocation"])], None if production_remove else "production_presence_removal_missing")
presence_mutation_red = (
    int(mutation_presence_exit_s) != 0
    and "WP24_MUTATION_PRESENCE_REMOVAL_DISABLED_ACTIVE" in mutation_presence_log
    and f"test {presence_test} ..." in mutation_presence_log
    and "presence" in mutation_presence_log.lower()
    and bool(re.search(r"^test result: FAILED\. 0 passed; 1 failed;", mutation_presence_log, re.M))
)
add(gate, "presence_removal_detector_mutation_red", presence_mutation_red,
    {"exit": int(mutation_presence_exit_s), "duration_ms": int(mutation_presence_ms_s),
     "failure_reason_matched": presence_mutation_red},
    [mutation_presence_log_s, f"cargo_test:{presence_test}"],
    None if presence_mutation_red else "presence_mutation_did_not_produce_expected_red")

# search_and_relation_use_effective_permission
gate = GATES[4]
for criterion, name in [
    ("all_read_paths_zero_unauthorized_hits_and_no_count_leak", "routes::flow::flow_database_tests::effective_permission_filters_every_flow_read_path_without_count_leakage"),
    ("search_filters_before_count_cursor_snippet_and_frontier", "routes::flow::flow_database_tests::flow_search_filters_before_cardinality_cursor_snippet_and_frontier"),
    ("read_reauthorizes_after_final_epoch_change", "routes::flow::flow_database_tests::object_reads_reauthorize_after_their_final_epoch_check_changes"),
    ("relation_target_permission_is_second_gate", "flow::relations::database_tests::duplicate_link_is_typed_invalid_update_and_target_permission_is_a_real_second_gate"),
    ("unavailable_is_exact_fieldless_union", "flow::relations::database_tests::relation_read_paginates_and_unavailable_is_the_exact_one_field_union"),
]: add_test(gate, criterion, name)
reauthorize_mutation_red = (
    int(mutation_reauthorize_exit_s) != 0
    and "WP24_MUTATION_FINAL_EPOCH_CHECK_ALWAYS_TRUE_ACTIVE" in mutation_reauthorize_log
    and f"test {reauthorize_test} ..." in mutation_reauthorize_log
    and "a read prepared before revocation escaped" in mutation_reauthorize_log
    and bool(re.search(r"^test result: FAILED\. 0 passed; 1 failed;", mutation_reauthorize_log, re.M))
)
add(gate, "read_reauthorization_mutation_red", reauthorize_mutation_red,
    {"exit": int(mutation_reauthorize_exit_s), "duration_ms": int(mutation_reauthorize_ms_s),
     "failure_reason_matched": reauthorize_mutation_red},
    [mutation_reauthorize_log_s, f"cargo_test:{reauthorize_test}"],
    None if reauthorize_mutation_red else "read_reauthorization_mutation_did_not_produce_expected_red")
fieldless = bool(re.search(r"pub\s+enum\s+RelationView\s*\{[\s\S]*?\bUnavailable\s*,\s*\}", source["model"]))
add(gate, "unavailable_omits_identifier_and_reference_material", fieldless, "fieldless variant" if fieldless else "not proven fieldless",
    [str(files["model"]), str(files["relations"])], None if fieldless else "unavailable_union_leaks_fields_or_is_unproven")

# authz_boundary_self_lockout_guarded
gate = GATES[5]
for criterion, name in [
    ("unconfirmed_self_lockout_policy_rejected_zero_writes", "flow::grants::database_tests::self_lockout_needs_confirmation_and_leaves_an_admin_rescue_path"),
    ("dry_run_summary_parity_and_authorization", "flow::grants::database_tests::a_dry_run_returns_the_same_summary_and_writes_nothing"),
]: add_test(gate, criterion, name)
dry = fn_body(source["grants"], "a_dry_run_returns_the_same_summary_and_writes_nothing")
dry_checks = {
    "dry_run_applied_false": "assert!(!preview.applied)" in dry,
    "dry_run_event_id_absent": "preview.event_id.is_none()" in dry,
    "dry_run_grants_unchanged": "grant_count" in dry,
    "dry_run_inheritance_unchanged": "inherit_flag" in dry,
    "dry_run_business_events_zero": "event_count" in dry,
    "dry_run_event_dispatch_zero": "event_dispatch" in dry,
    "dry_run_epoch_unchanged": "epoch_before" in dry,
    "dry_run_idempotency_unconsumed": "same key still works" in dry,
    "dry_run_unauthorized_no_summary": "without `full_access`" in dry and "Err(ApiError::Forbidden" in dry,
}
for criterion, ok in dry_checks.items():
    add(gate, criterion, ok, "asserted" if ok else "assertion absent", [str(files["grants"])],
        None if ok else f"{criterion}_not_implemented")
rescue = fn_body(source["grants"], "self_lockout_needs_confirmation_and_leaves_an_admin_rescue_path")
rescue_audited = "admin" in rescue and "event_count" in rescue and "rescue" in rescue
add(gate, "admin_rescue_remains_available_and_audited", rescue_audited,
    "asserted" if rescue_audited else "audit assertion absent", [str(files["grants"])],
    None if rescue_audited else "admin_rescue_audit_not_proven")
surface_count_literals = re.findall(r"surface verifier\s*的\s*(\d+)/(\d+)/(\d+)", authz_clause)
surface_counts = surface_result.get("counts", {})
expected_surface_counts = [int(value) for value in surface_count_literals[0]] if len(surface_count_literals) == 1 else []
observed_surface_counts = [
    surface_counts.get("rest_total"), surface_counts.get("mcp_tools_total"), surface_counts.get("cli_commands_total")
]
surface_parity = (
    int(surface_exit_s) == 0 and surface_result.get("passed") is True
    and expected_surface_counts == observed_surface_counts
)
add(gate, "dry_run_adds_no_surface", surface_parity,
    {"contract_counts": expected_surface_counts, "observed_counts": observed_surface_counts,
     "surface_verifier_exit": int(surface_exit_s), "duration_ms": int(surface_ms_s)},
    [surface_result_s, surface_log_s, commands_s], None if surface_parity else "surface_parity_failed_or_count_drift")
dry_run_mutation_red = int(mutation_dry_run_exit_s) != 0 and "GATECHAIN_MUTATION_DRY_RUN_COMMITTED_ACTIVE" in mutation_dry_run_log and "test result: FAILED. 0 passed; 1 failed;" in mutation_dry_run_log
add(gate, "dry_run_zero_change_detector_mutation_red", dry_run_mutation_red,
    {"exit": int(mutation_dry_run_exit_s), "duration_ms": int(mutation_dry_run_ms_s)},
    [mutation_dry_run_log_s], None if dry_run_mutation_red else "dry_run_mutation_did_not_produce_red")

# archive_tier_by_object_scope
gate = GATES[6]
for criterion, name in [
    ("ordinary_non_root_page_edit_tier", "flow::grants::database_tests::archive_and_restore_stay_in_the_edit_tier_but_a_navigator_does_not"),
    ("lifecycle_scope_and_impact_classification", "flow::command::database_tests::lifecycle_tier_uses_the_real_impact_set_and_preserves_the_edit_baseline"),
    ("archive_revalidates_impact_and_rolls_back_drift", "flow::command::database_tests::lifecycle_descendant_growth_does_not_turn_a_plain_archive_into_a_cascade"),
    ("archive_holds_current_authz_epoch_to_commit", "flow::command::database_tests::lifecycle_holds_the_current_authz_epoch_fence_until_its_commit"),
]: add_test(gate, criterion, name)
lifecycle_body = fn_body(source["command"], "execute_lifecycle_command")
affected = "affected_object_ids" in lifecycle_body and "fence_epoch_for_share" in lifecycle_body
add(gate, "event_records_affected_object_ids", affected, "present" if affected else "absent",
    [str(files["command"])], None if affected else "affected_object_ids_or_epoch_recheck_missing")
restore_idempotent = "lifecycle_replay_affected_ids" in source["command"] and "idempotency" in source["command"]
add(gate, "restore_is_idempotent", restore_idempotent, "replay path present" if restore_idempotent else "not found",
    [str(files["command"])], None if restore_idempotent else "restore_idempotency_not_proven")
schema_types_match = re.search(r"object_type IN \(([^)]+)\)", source["migration"])
if not schema_types_match:
    raise SystemExit("source parse failed: flow_objects object_type constraint is empty")
schema_types = re.findall(r"'([^']+)'", schema_types_match.group(1))
if not schema_types:
    raise SystemExit("source parse failed: flow_objects object_type set is empty")
has_collection = "collection" in schema_types
if has_collection:
    collection_fixture = bool(re.search(r"fn\s+[a-z0-9_]*collection[a-z0-9_]*(?:archive|tier)", source["command"], re.I))
    add(gate, "collection_container_tier", collection_fixture,
        {"schema_object_types": schema_types, "constructive_fixture_found": collection_fixture},
        [str(files["migration"]), str(files["command"])],
        None if collection_fixture else "collection_container_tier_fixture_not_implemented")
else:
    add_not_covered(gate, "collection_container_tier",
        {"schema_object_types": schema_types, "schema_supports_collection": False, "version_boundary": "v0.5"},
        [str(files["migration"])], "collection_container_not_implemented_v0_5")
retention_action = bool(re.search(r"permanent[_ -]?(?:delete|purge)|retention[_ -]?(?:delete|purge)", source["command"], re.I))
if retention_action:
    retention_fixture = bool(re.search(r"fn\s+[a-z0-9_]*(?:retention|permanent)[a-z0-9_]*(?:archive|tier)", source["command"], re.I))
    add(gate, "retention_or_permanent_cleanup_tier", retention_fixture,
        {"production_action_found": True, "constructive_fixture_found": retention_fixture}, [str(files["command"])],
        None if retention_fixture else "retention_cleanup_tier_fixture_not_implemented")
else:
    add_not_covered(gate, "retention_or_permanent_cleanup_tier",
        {"production_action_found": False, "version_boundary": "v0.5"}, [str(files["command"])],
        "retention_permanent_cleanup_not_implemented_v0_5")
member_baseline = baseline_result.get("member_baseline_no_behaviour_regression", {})
baseline_passed = int(baseline_exit_s) == 0 and member_baseline.get("status") == "passed"
add(gate, "v0_4_member_baseline_fixture_reused", baseline_passed,
    {"verifier_exit": int(baseline_exit_s), "duration_ms": int(baseline_ms_s),
     "status": member_baseline.get("status", "not_observed")},
    [baseline_result_s, baseline_log_s, commands_s],
    None if baseline_passed else "v0_4_member_baseline_fixture_failed_or_not_covered")
gap_path = pathlib.Path("/opt/worker/task/openpr/contract-gaps-v05-2026-09-09.md")
gap_text = read(gap_path) if gap_path.is_file() else ""
g5 = "G5" in gap_text and "root Page" in gap_text and "v0.4" in gap_text and "v0.5" in gap_text
if g5:
    add_not_covered(gate, "root_page_policy_is_contract_consistent",
        {"g5_record_observed": True, "resolution": "contract decision required", "version_boundary": "v0.5"},
        [str(gap_path)], "contract_conflict_g5_root_page_archive_tier")
else:
    add(gate, "root_page_policy_is_contract_consistent", True,
        {"g5_record_observed": False, "resolution": "no registered v0.4/v0.5 conflict"}, [commands_s])

# token_expiry_and_permission_revocation
gate = GATES[7]
for criterion, name in [
    ("valid_ticket_positive_control", "routes::collab::collab_database_tests::a_flow_enabled_workspace_still_issues_a_ticket_and_still_upgrades"),
    ("expired_ticket_rejected_at_atomic_consume", "routes::collab::collab_database_tests::an_expired_ticket_is_rejected_at_atomic_consumption"),
    ("permission_revocation_wins_before_ticket_insert", "routes::collab::collab_database_tests::ticket_issuance_holds_current_epoch_through_insert_and_rechecks_membership"),
]: add_test(gate, criterion, name)
ticket_mutation_red = int(mutation_ticket_expiry_exit_s) != 0 and "GATECHAIN_MUTATION_EXPIRED_TICKET_ACCEPTED_ACTIVE" in mutation_ticket_expiry_log and "test result: FAILED. 0 passed; 1 failed;" in mutation_ticket_expiry_log
add(gate, "ticket_expiry_detector_mutation_red", ticket_mutation_red,
    {"exit": int(mutation_ticket_expiry_exit_s), "duration_ms": int(mutation_ticket_expiry_ms_s)},
    [mutation_ticket_expiry_log_s], None if ticket_mutation_red else "ticket_expiry_mutation_did_not_produce_red")

# authz_numeric_budgets_locked
gate = GATES[8]
source_object_grants = const_int(source["authz"], "OBJECT_GRANTS_MAX")
source_coalesced = const_int(source["dispatcher"], "COALESCED_SOURCE_EVENTS_MAX")
numeric_contract_ok = object_grants_contract["frozen"] and coalesced_sources_contract["frozen"]
add(gate, "authorization_structured_budgets_have_value_set_by_and_rule", numeric_contract_ok,
    {"object_grants_max": object_grants_contract, "coalesced_source_events_max": coalesced_sources_contract},
    [limits_s], None if numeric_contract_ok else "authorization_budget_structured_fields_missing")
numeric_constants_ok = source_object_grants == object_grants_contract["value"] and source_coalesced == coalesced_sources_contract["value"]
add(gate, "contract_values_match_executed_source_constants", numeric_constants_ok,
    {"object_grants_max": {"contract": object_grants_contract["value"], "source": source_object_grants},
     "coalesced_source_events_max": {"contract": coalesced_sources_contract["value"], "source": source_coalesced}},
    [str(files["authz"]), str(files["dispatcher"])], None if numeric_constants_ok else "authorization_budget_constant_drift")
add_test(gate, "whole_table_grant_replacement_boundary_and_plus_one", "flow::grants::database_tests::grant_count_ceilings_are_enforced_and_never_silently_truncate")
add_test(gate, "coalesced_source_rows_exact_and_plus_one", "events::dispatcher::dispatcher_database_tests::coalescing_cap_freezes_the_row_and_opens_a_new_one_with_exact_source_counts")
incremental_surface_absent = "Change::AddGrant" not in source["grants"] and "append_grant" not in source["grants"]
add(gate, "no_incremental_grant_surface_requires_object_grants_boundary", incremental_surface_absent,
    {"incremental_surface_found": not incremental_surface_absent, "boundary_exemption": "whole_table_replacement"},
    [str(files["grants"])], None if incremental_surface_absent else "incremental_grant_path_requires_boundary_fixture")
numeric_mutation_red = int(mutation_numeric_budget_exit_s) != 0 and "GATECHAIN_MUTATION_AUTHZ_NUMERIC_BUDGET_DRIFT_ACTIVE" in mutation_numeric_budget_log and "COALESCED_SOURCE_EVENTS_MAX drift" in mutation_numeric_budget_log
add(gate, "numeric_budget_drift_detector_mutation_red", numeric_mutation_red,
    {"exit": int(mutation_numeric_budget_exit_s), "duration_ms": int(mutation_numeric_budget_ms_s)},
    [mutation_numeric_budget_log_s], None if numeric_mutation_red else "authorization_numeric_budget_mutation_did_not_produce_red")

# admin_bot_does_not_bypass_object_boundary
gate = GATES[9]
admin_test = "flow::grants::database_tests::an_admin_bot_does_not_bypass_an_object_boundary_but_keeps_workspace_admin"
add_test(gate, "admin_bot_denied_at_object_boundary_and_workspace_admin_preserved", admin_test)
admin_mutation_red = int(mutation_admin_bot_exit_s) != 0 and "GATECHAIN_MUTATION_ADMIN_BOT_FALLBACK_ACTIVE" in mutation_admin_bot_log and "test result: FAILED. 0 passed; 1 failed;" in mutation_admin_bot_log
add(gate, "admin_bot_boundary_detector_mutation_red", admin_mutation_red,
    {"exit": int(mutation_admin_bot_exit_s), "duration_ms": int(mutation_admin_bot_ms_s)},
    [mutation_admin_bot_log_s], None if admin_mutation_red else "admin_bot_mutation_did_not_produce_red")

depth_mutation_red = int(mutation_depth_budget_exit_s) != 0 and "GATECHAIN_MUTATION_AUTHZ_DEPTH_REDUCED_ACTIVE" in mutation_depth_budget_log and "test result: FAILED. 0 passed; 1 failed;" in mutation_depth_budget_log
add(GATES[0], "depth_32_detector_mutation_red", depth_mutation_red,
    {"exit": int(mutation_depth_budget_exit_s), "duration_ms": int(mutation_depth_budget_ms_s)},
    [mutation_depth_budget_log_s], None if depth_mutation_red else "depth_budget_mutation_did_not_produce_red")

boundary_mutation_red = (
    int(mutation_boundary_exit_s) != 0
    and "WP24_MUTATION_DENIED_TO_VIEW_ACTIVE" in mutation_boundary_log
    and f"test {boundary_test} ..." in mutation_boundary_log
    and "the target boundary starts with no applicable grant" in mutation_boundary_log
    and bool(re.search(r"^test result: FAILED\. 0 passed; 1 failed;", mutation_boundary_log, re.M))
)
add(GATES[0], "boundary_denied_detector_mutation_red", boundary_mutation_red,
    {"exit": int(mutation_boundary_exit_s), "duration_ms": int(mutation_boundary_ms_s),
     "failure_reason_matched": boundary_mutation_red},
    [mutation_boundary_log_s, f"cargo_test:{boundary_test}"],
    None if boundary_mutation_red else "boundary_mutation_did_not_produce_expected_red")
fence_mutation_red = (
    int(mutation_fence_exit_s) != 0
    and "WP24_MUTATION_EPOCH_FENCE_DISABLED_ACTIVE" in mutation_fence_log
    and f"test {fence_test} ..." in mutation_fence_log
    and (
        "A must still be blocked on B's row lock" in mutation_fence_log
        or "Accepted" in mutation_fence_log
        or "accepted" in mutation_fence_log
    )
    and bool(re.search(r"^test result: FAILED\. 0 passed; 1 failed;", mutation_fence_log, re.M))
)
add(GATES[1], "fence_detector_mutation_red", fence_mutation_red,
    {"exit": int(mutation_fence_exit_s), "duration_ms": int(mutation_fence_ms_s),
     "failure_reason_matched": fence_mutation_red},
    [mutation_fence_log_s, f"cargo_test:{fence_test}"],
    None if fence_mutation_red else "fence_mutation_did_not_produce_expected_red")

details = {}
hard_gates = {}
for gate in GATES:
    items = observed[gate]
    if not items:
        raise SystemExit(f"internal verifier error: zero observations for {gate}")
    covered = [item for item in items if item["status"] != "not_covered"]
    if not covered:
        raise SystemExit(f"internal verifier error: zero covered observations for {gate}")
    passed = all(item["passed"] for item in covered)
    details[gate] = {
        "status": "passed" if passed else "failed",
        "passed": passed,
        "reason_codes": reason_codes[gate],
        "not_covered_reason_codes": not_covered_reason_codes[gate],
        "observed": items,
    }
    hard_gates[gate] = details[gate]["status"]

build = {
    "command": "cargo build -p collab-core --bin collab-isolated-apply-worker",
    "exit": int(build_exit_s), "duration_ms": int(build_ms_s), "log": build_log_s,
    "passed": int(build_exit_s) == 0,
}
inventory = {
    "command": "cargo test -p api --lib --no-fail-fast -- --list",
    "exit": int(list_exit_s), "duration_ms": int(list_ms_s), "log": list_log_s,
    "count": len(listed), "passed": int(list_exit_s) == 0 and bool(listed),
}
tests = {
    "command": "cargo test -p api --lib --no-fail-fast <required-test> -- --exact --nocapture --test-threads=1",
    "required_tests": required_tests,
    "database_url": database_url_s,
    "exit": int(test_exit_s), "duration_ms": int(test_ms_s), "log": test_log_s,
    "summary": {
        "status": summary_status, "passed": summary_passed, "failed": summary_failed,
        "ignored": summary_ignored, "measured": summary_measured,
        "filtered_out": summary_filtered, "libtest_seconds": summary_seconds,
        "blocks": len(summary_matches),
    },
    "early_return_detected": early_return,
    "observed_test_status_count": len(statuses),
    "passed": (
        int(test_exit_s) == 0 and summary_status == "ok" and summary_failed == 0
        and summary_passed == len(required_tests) and not early_return
    ),
}
dependencies = {
    "surface_parity": {"command": "verify-flow-surface-coverage.sh --release 0.4 --json",
        "exit": int(surface_exit_s), "duration_ms": int(surface_ms_s), "log": surface_log_s,
        "artifact": surface_result_s},
    "member_baseline": {"command": "verify-flow-authz-baseline-v0.4.sh --json",
        "exit": int(baseline_exit_s), "duration_ms": int(baseline_ms_s), "log": baseline_log_s,
        "artifact": baseline_result_s},
}
mutations = {
    "denied_to_view": {"test": boundary_test, "exit": int(mutation_boundary_exit_s),
        "duration_ms": int(mutation_boundary_ms_s), "log": mutation_boundary_log_s},
    "presence_removal_disabled": {"test": presence_test, "exit": int(mutation_presence_exit_s),
        "duration_ms": int(mutation_presence_ms_s), "log": mutation_presence_log_s},
    "epoch_fence_disabled": {"test": fence_test, "exit": int(mutation_fence_exit_s),
        "duration_ms": int(mutation_fence_ms_s), "log": mutation_fence_log_s},
    "final_epoch_check_always_true": {"test": reauthorize_test, "exit": int(mutation_reauthorize_exit_s),
        "duration_ms": int(mutation_reauthorize_ms_s), "log": mutation_reauthorize_log_s},
    "move_locked_reauthorization_disabled": {"test": move_reauth_test, "exit": int(mutation_move_reauth_exit_s),
        "duration_ms": int(mutation_move_reauth_ms_s), "log": mutation_move_reauth_log_s},
    "expired_ticket_accepted": {"test": ticket_expiry_test, "exit": int(mutation_ticket_expiry_exit_s),
        "duration_ms": int(mutation_ticket_expiry_ms_s), "log": mutation_ticket_expiry_log_s},
    "cache_epoch_ignored": {"test": cache_poison_test, "exit": int(mutation_cache_poison_exit_s),
        "duration_ms": int(mutation_cache_poison_ms_s), "log": mutation_cache_poison_log_s},
    "dry_run_committed": {"test": dry_run_test, "exit": int(mutation_dry_run_exit_s),
        "duration_ms": int(mutation_dry_run_ms_s), "log": mutation_dry_run_log_s},
    "admin_bot_fallback_enabled": {"test": admin_bot_test, "exit": int(mutation_admin_bot_exit_s),
        "duration_ms": int(mutation_admin_bot_ms_s), "log": mutation_admin_bot_log_s},
    "authz_depth_reduced": {"test": depth_budget_test, "exit": int(mutation_depth_budget_exit_s),
        "duration_ms": int(mutation_depth_budget_ms_s), "log": mutation_depth_budget_log_s},
    "authorization_numeric_budget_drift": {"check": "source constants equal frozen contract values",
        "exit": int(mutation_numeric_budget_exit_s), "duration_ms": int(mutation_numeric_budget_ms_s),
        "log": mutation_numeric_budget_log_s},
}

print(json.dumps({
    "contract": {
        "adr": adr_s, "limits": limits_s, "gate_commands": commands_s,
        "parsed_gate_names": parsed_gates,
        "parsed_limits": {
            "tree_depth_max": tree_depth,
            "document_lock_hold_ms_p95_max": hold_p95,
            "document_lock_hold_ms_max": hold_max,
            "object_grants_max_status": object_grants_status,
            "object_grants_max": object_grants_max,
            "object_grants_max_structured": object_grants_contract,
            "coalesced_source_events_max_structured": coalesced_sources_contract,
        },
    },
    "commands": {"worker_build": build, "test_inventory": inventory, "cargo_test": tests,
                 "dependencies": dependencies, "mutations": mutations},
    "hard_gates": hard_gates,
    "gate_details": details,
    "dynamic_passed": build["passed"] and inventory["passed"] and tests["passed"],
}, separators=(",", ":")))
PY
)" || {
  echo "FAIL: authorization evidence parser failed" >&2
  exit 2
}

if ! jq -e . >/dev/null 2>&1 <<<"$STATIC_DYNAMIC_JSON"; then
  echo "FAIL: authorization evidence parser emitted malformed JSON" >&2
  exit 2
fi

SOURCE_INTEGRITY_PASSED=$([[ "$SOURCE_DIRTY" == false ]] && echo true || echo false)
GATES_PASSED="$(jq -r '[.hard_gates[] == "passed"] | all' <<<"$STATIC_DYNAMIC_JSON")"
DYNAMIC_PASSED="$(jq -r '.dynamic_passed' <<<"$STATIC_DYNAMIC_JSON")"
OVERALL_PASSED=$([[ "$SOURCE_INTEGRITY_PASSED" == true && "$GATES_PASSED" == true && "$DYNAMIC_PASSED" == true ]] && echo true || echo false)

RESULT="$(jq -n \
  --arg source_head "$SOURCE_HEAD" \
  --arg generated_at "$GENERATED_AT" \
  --argjson source_dirty "$SOURCE_DIRTY" \
  --argjson dirty_entries "$SOURCE_DIRTY_ENTRIES" \
  --argjson source_integrity_passed "$SOURCE_INTEGRITY_PASSED" \
  --argjson evidence "$STATIC_DYNAMIC_JSON" \
  --argjson passed "$OVERALL_PASSED" \
  '{
    schema_version: "sylvode.flow.authz-result.v1",
    source_head: $source_head,
    source_dirty: $source_dirty,
    source_integrity: {
      status: (if $source_integrity_passed then "passed" else "failed" end),
      checked_scope: ["apps/","crates/","spikes/","migrations/",".cargo/","Cargo.toml","Cargo.lock"],
      dirty_entries: $dirty_entries,
      passed: $source_integrity_passed
    },
    generated_at: $generated_at,
    contract: $evidence.contract,
    commands: $evidence.commands,
    hard_gates: $evidence.hard_gates,
    gate_details: $evidence.gate_details,
    passed: $passed
  }')"

OUT_PATH="$EVIDENCE_REAL/authz-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . >"$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2
jq -r '.hard_gates | to_entries[] | "  \(.key): \(.value)"' <<<"$RESULT" >&2
printf '%s\n' "$RESULT"

if [[ "$OVERALL_PASSED" == true ]]; then
  exit 0
fi
exit 1

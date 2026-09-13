#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_TEST_DATABASE_URL:?OPENPR_TEST_DATABASE_URL is required}"

CACHE_ROOT=/opt/worker/.cache/openpr-v08-operations-mutations
WORKTREE="$CACHE_ROOT/worktree"
TARGET_DIR=/opt/worker/.cache/openpr-v08-shared-target
LOG_DIR="$CACHE_ROOT/logs"
ROUTE_TEST=routes::flow::flow_database_tests::flow_operations_require_exact_confirm_and_keep_dry_runs_canonical_zero_write
TOOL_TEST=routes::flow::v08_admin_tool_tests::dangerous_admin_tools_require_an_exact_registered_name_without_blocking_native_users
MCP_TEST=mcp_admin_operations_fail_closed_and_only_exact_execute_changes_canonical_state

cleanup() {
  git -C "$REPO_ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$CACHE_ROOT" "$TARGET_DIR" "$LOG_DIR"
cleanup
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE" HEAD >/dev/null

run_case() {
  local label=$1
  local expected=$2
  local test_name=$3
  local log="$LOG_DIR/$label.log"
  set +e
  env -u RUST_TEST_THREADS OPENPR_TEST_DATABASE_URL="$OPENPR_TEST_DATABASE_URL" \
    CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
    cargo test --manifest-path "$WORKTREE/Cargo.toml" -p api --lib "$test_name" -- --exact --nocapture \
      >"$log" 2>&1
  local status=$?
  set -e
  if [[ $expected == green && $status -ne 0 ]]; then
    tail -80 "$log" >&2
    echo "FAIL: green control exited $status" >&2
    exit 1
  fi
  if [[ $expected == red && $status -eq 0 ]]; then
    tail -80 "$log" >&2
    echo "FAIL: mutation $label was not detected" >&2
    exit 1
  fi
  printf '%s status=%s expected=%s log=%s\n' "$label" "$status" "$expected" "$log"
}

run_mcp_case() {
  local label=$1
  local expected=$2
  local log="$LOG_DIR/$label.log"
  set +e
  env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
    cargo test --manifest-path "$WORKTREE/Cargo.toml" -p mcp-server \
      --test flow_admin_operations_e2e "$MCP_TEST" -- --exact --nocapture >"$log" 2>&1
  local status=$?
  set -e
  if [[ $expected == green && $status -ne 0 ]]; then
    tail -80 "$log" >&2
    echo "FAIL: MCP green control exited $status" >&2
    exit 1
  fi
  if [[ $expected == red && $status -eq 0 ]]; then
    tail -80 "$log" >&2
    echo "FAIL: MCP mutation $label was not detected" >&2
    exit 1
  fi
  printf '%s status=%s expected=%s log=%s\n' "$label" "$status" "$expected" "$log"
}

run_case operations_green_control green "$ROUTE_TEST"
run_case exact_tool_green_control green "$TOOL_TEST"
run_mcp_case mcp_operations_green_control green

ROUTES="$WORKTREE/apps/api/src/routes/flow.rs"
OPERATIONS="$WORKTREE/apps/api/src/flow/operations.rs"

perl -0pi -e 's/if !req\.dry_run && req\.confirm_document_id != Some\(document_id\) \{/if false {/' "$ROUTES"
grep -Fq 'if false {' "$ROUTES"
run_case compact_execute_ignores_exact_document_confirm red "$ROUTE_TEST"
git -C "$WORKTREE" restore apps/api/src/routes/flow.rs

perl -0pi -e 's/if !req\.dry_run && req\.confirm_object_id != Some\(object_id\) \{/if false {/' "$ROUTES"
grep -Fq 'if false {' "$ROUTES"
run_case projection_execute_ignores_exact_object_confirm red "$ROUTE_TEST"
git -C "$WORKTREE" restore apps/api/src/routes/flow.rs

perl -0pi -e 's/Some\(expected_head_seq\), !dry_run/Some(expected_head_seq), true/' "$OPERATIONS"
grep -Fq 'Some(expected_head_seq), true' "$OPERATIONS"
run_case projection_dry_run_executes_canonical_write_path red "$ROUTE_TEST"
git -C "$WORKTREE" restore apps/api/src/flow/operations.rs

perl -0pi -e 's/"expected_head_seq": expected_head_seq/"expected_head_seq": 0/' "$OPERATIONS"
grep -Fq '"expected_head_seq": 0' "$OPERATIONS"
run_case idempotency_hash_ignores_expected_head red "$ROUTE_TEST"
git -C "$WORKTREE" restore apps/api/src/flow/operations.rs

perl -0pi -e 's/if idempotency_key\.trim\(\)\.is_empty\(\) \{/if false {/' "$OPERATIONS"
grep -Fq 'if false {' "$OPERATIONS"
run_case empty_idempotency_key_is_accepted red "$ROUTE_TEST"
git -C "$WORKTREE" restore apps/api/src/flow/operations.rs

perl -0pi -e 's/(pub async fn post_flow_compact_document.*?let expected = req\s*\.expected_head_seq\s*)\.ok_or_else\(\|\| ApiError::BadRequest\("expected_head_seq is required"\.to_string\(\)\)\)\?;/${1}.unwrap_or(0);/s' "$ROUTES"
sed -n '/pub async fn post_flow_compact_document/,/pub async fn post_flow_rebuild_projection/p' "$ROUTES" | grep -Fq '.unwrap_or(0);'
run_case compact_missing_expected_head_is_accepted red "$ROUTE_TEST"
git -C "$WORKTREE" restore apps/api/src/routes/flow.rs

perl -0pi -e 's/(pub async fn post_flow_compact_document.*?policy::)require_flow_workspace_admin_access/${1}require_flow_workspace_access/s' "$ROUTES"
sed -n '/pub async fn post_flow_compact_document/,/pub async fn post_flow_rebuild_projection/p' "$ROUTES" | grep -Fq 'require_flow_workspace_access'
run_case compact_accepts_non_admin_member red "$ROUTE_TEST"
git -C "$WORKTREE" restore apps/api/src/routes/flow.rs

perl -0pi -e 's/context\.tool_name\.as_deref\(\) != Some\(expected\)/context.tool_name.as_deref() == Some(expected)/' "$ROUTES"
grep -Fq 'context.tool_name.as_deref() == Some(expected)' "$ROUTES"
run_case exact_tool_policy_is_inverted red "$TOOL_TEST"
git -C "$WORKTREE" restore apps/api/src/routes/flow.rs

MCP_OBJECTS="$WORKTREE/apps/mcp-server/src/tools/objects.rs"
perl -0pi -e 's/(pub async fn compact_flow_document.*?pointer\("\/data\/document_id"\)\.and_then\(Value::as_str\) )== Some\(document_id\)/${1}!= Some(document_id)/s' "$MCP_OBJECTS"
sed -n '/pub async fn compact_flow_document/,/pub async fn replay_flow_deliveries/p' "$MCP_OBJECTS" | grep -Fq '!= Some(document_id)'
run_mcp_case mcp_compact_scope_comparison_is_inverted red

printf 'PASS: 3 green controls passed and 9/9 production-source mutations were detected\n'

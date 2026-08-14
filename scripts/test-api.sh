#!/usr/bin/env bash
set -euo pipefail

# API Integration Test Script
# Tests authentication, workspace, project, issue and comment management.
#
# Contract this script is written against (apps/api/src/main.rs, response.rs, error.rs):
#   * Every route lives under /api/v1; there is no unversioned /api prefix.
#   * Every handled response is HTTP 200. Success and failure are told apart by
#     the `code` field of the {code, message, data} envelope: 0 means success,
#     anything else is the error code (400/401/403/404/409/500). Checking the
#     HTTP status code therefore proves nothing.
#   * Auth handlers answer with {user, tokens}; the tokens are at
#     .data.tokens.access_token and .data.tokens.refresh_token.
#
# Registration is open only while the users table is empty (the first account
# becomes admin). Against a database that already has users, registration needs
# an admin access token: export OPENPR_ADMIN_EMAIL and OPENPR_ADMIN_PASSWORD and
# this script will bootstrap its throwaway account through that admin.

API_URL="${API_URL:-http://localhost:8081}"
ADMIN_EMAIL="${OPENPR_ADMIN_EMAIL:-}"
ADMIN_PASSWORD="${OPENPR_ADMIN_PASSWORD:-}"

# Unique per run so the script can be re-run against the same database. A caller that has to name
# the account afterwards pins it instead: on a freshly reset database this is the first account,
# which makes it the admin, and scripts/e2e-test.sh hands it to
# scripts/bootstrap-restaurant-demo.sh, which has to log in as somebody to create the MCP bot.
RUN_ID="$(date +%s)-$$"
TEST_EMAIL="${OPENPR_TEST_EMAIL:-test-${RUN_ID}@example.com}"
TEST_PASSWORD="${OPENPR_TEST_PASSWORD:-TestPassword123!}"

echo "🧪 Starting API Integration Tests"
echo "API URL: $API_URL"
echo ""

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "❌ Missing required command: $1" >&2
    exit 2
  }
}

require_cmd curl
require_cmd jq

# Helper function for API calls. Returns the raw response body; the HTTP status
# is deliberately ignored because the API reports errors inside the body.
api_call() {
  local method=$1
  local endpoint=$2
  local data=$3
  local token=${4:-}

  if [ -n "$token" ]; then
    curl -sS -X "$method" "$API_URL$endpoint" \
      -H "Content-Type: application/json" \
      -H "Authorization: Bearer $token" \
      -d "$data"
  else
    curl -sS -X "$method" "$API_URL$endpoint" \
      -H "Content-Type: application/json" \
      -d "$data"
  fi
}

# Envelope code of a response, or "invalid" when the body is not a
# {code, message, data} object (axum rejects a malformed JSON body before the
# handler runs and answers with plain text).
response_code() {
  printf '%s' "$1" | jq -r 'if type == "object" and has("code") then (.code | tostring) else "invalid" end' 2>/dev/null \
    || printf 'invalid'
}

# Fail unless the envelope reports success.
expect_success() {
  local label=$1
  local body=$2
  local code
  code="$(response_code "$body")"
  if [ "$code" != "0" ]; then
    echo "❌ $label failed (code=$code)"
    echo "Response: $body"
    exit 1
  fi
}

# Read a required field out of a response, failing loudly when it is absent.
require_field() {
  local label=$1
  local body=$2
  local filter=$3
  local value
  if ! value="$(printf '%s' "$body" | jq -er "$filter" 2>/dev/null)"; then
    echo "❌ $label: response is missing $filter" >&2
    echo "Response: $body" >&2
    return 1
  fi
  printf '%s' "$value"
}

# Assert a response field equals an expected value.
expect_field() {
  local label=$1
  local body=$2
  local filter=$3
  local expected=$4
  local actual
  actual="$(require_field "$label" "$body" "$filter")"
  if [ "$actual" != "$expected" ]; then
    echo "❌ $label: expected $filter to be '$expected', got '$actual'"
    echo "Response: $body"
    exit 1
  fi
}

# Test 1: Health Check
# The status word is matched case-insensitively with grep -Eiq; services in this
# stack answer both "ok" and "OK".
echo "📋 Test 1: Health Check"
health_response=$(curl -sS "$API_URL/health")
expect_success "Health check" "$health_response"
health_status="$(require_field "Health check" "$health_response" '.data.status')"
if printf '%s' "$health_status" | grep -Eiq "^(ok|healthy)$"; then
  echo "✅ Health check passed"
else
  echo "❌ Health check failed: unexpected status '$health_status'"
  echo "Response: $health_response"
  exit 1
fi
echo ""

# Test 2: User Registration
# The first account of an empty database is created without a token and becomes
# admin. Later accounts need an admin bearer token, so retry through the
# configured admin when the open path is refused.
echo "📋 Test 2: User Registration"
register_body="{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\",\"name\":\"Test User\"}"
register_response=$(api_call POST "/api/v1/auth/register" "$register_body")
register_code="$(response_code "$register_response")"

if [ "$register_code" = "403" ]; then
  if [ -z "$ADMIN_EMAIL" ] || [ -z "$ADMIN_PASSWORD" ]; then
    echo "❌ Registration failed: this database already has users, so registration requires an admin token."
    echo "   Export OPENPR_ADMIN_EMAIL and OPENPR_ADMIN_PASSWORD, or run against a freshly reset database."
    echo "Response: $register_response"
    exit 1
  fi
  echo "ℹ️  Open registration is closed; bootstrapping through $ADMIN_EMAIL"
  admin_login_response=$(api_call POST "/api/v1/auth/login" \
    "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\"}")
  expect_success "Admin login" "$admin_login_response"
  ADMIN_TOKEN="$(require_field "Admin login" "$admin_login_response" '.data.tokens.access_token')"
  register_response=$(api_call POST "/api/v1/auth/register" "$register_body" "$ADMIN_TOKEN")
fi

expect_success "Registration" "$register_response"
expect_field "Registration" "$register_response" '.data.user.email' "$TEST_EMAIL"
require_field "Registration" "$register_response" '.data.tokens.access_token' >/dev/null
echo "✅ Registration successful"
echo ""

# Test 3: User Login
echo "📋 Test 3: User Login"
login_response=$(api_call POST "/api/v1/auth/login" \
  "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}")
expect_success "Login" "$login_response"
expect_field "Login" "$login_response" '.data.user.email' "$TEST_EMAIL"
ACCESS_TOKEN="$(require_field "Login" "$login_response" '.data.tokens.access_token')"
REFRESH_TOKEN="$(require_field "Login" "$login_response" '.data.tokens.refresh_token')"
echo "✅ Login successful"
echo ""

# Test 4: Create Workspace
# The slug must be lowercase letters, digits and hyphens only.
echo "📋 Test 4: Create Workspace"
workspace_slug="test-workspace-${RUN_ID}"
workspace_response=$(api_call POST "/api/v1/workspaces" \
  "{\"slug\":\"$workspace_slug\",\"name\":\"Test Workspace\"}" \
  "$ACCESS_TOKEN")
expect_success "Workspace creation" "$workspace_response"
expect_field "Workspace creation" "$workspace_response" '.data.slug' "$workspace_slug"
WORKSPACE_ID="$(require_field "Workspace creation" "$workspace_response" '.data.id')"
echo "✅ Workspace created ($WORKSPACE_ID)"
echo ""

# Test 5: Create Project
# The key must be uppercase letters and digits only.
echo "📋 Test 5: Create Project"
project_response=$(api_call POST "/api/v1/workspaces/$WORKSPACE_ID/projects" \
  "{\"key\":\"TEST\",\"name\":\"Test Project\",\"description\":\"Integration test project\"}" \
  "$ACCESS_TOKEN")
expect_success "Project creation" "$project_response"
expect_field "Project creation" "$project_response" '.data.key' "TEST"
PROJECT_ID="$(require_field "Project creation" "$project_response" '.data.id')"
echo "✅ Project created ($PROJECT_ID)"
echo ""

# Test 6: Create Issue
# CreateIssueRequest accepts title/description/state/priority/assignee_id/due_at/sprint_id.
# Omitting `state` lets the project's effective workflow pick the initial state.
echo "📋 Test 6: Create Issue"
issue_response=$(api_call POST "/api/v1/projects/$PROJECT_ID/issues" \
  "{\"title\":\"Test Issue\",\"description\":\"This is a test issue\",\"priority\":\"high\"}" \
  "$ACCESS_TOKEN")
expect_success "Issue creation" "$issue_response"
expect_field "Issue creation" "$issue_response" '.data.title' "Test Issue"
expect_field "Issue creation" "$issue_response" '.data.priority' "high"
ISSUE_ID="$(require_field "Issue creation" "$issue_response" '.data.id')"
echo "✅ Issue created ($ISSUE_ID)"
echo ""

# Test 7: Add Comment
# CreateCommentRequest takes `content`, falling back to `body`.
echo "📋 Test 7: Add Comment to Issue"
comment_response=$(api_call POST "/api/v1/issues/$ISSUE_ID/comments" \
  "{\"body\":\"This is a test comment\"}" \
  "$ACCESS_TOKEN")
expect_success "Comment creation" "$comment_response"
expect_field "Comment creation" "$comment_response" '.data.body' "This is a test comment"
expect_field "Comment creation" "$comment_response" '.data.work_item_id' "$ISSUE_ID"
echo "✅ Comment added"
echo ""

# Test 8: Update Issue State
# The issue update route is PUT, and the workflow field is `state`, not `status`.
# `in_progress` is one of the four default workflow states.
echo "📋 Test 8: Update Issue State"
update_response=$(api_call PUT "/api/v1/issues/$ISSUE_ID" \
  "{\"state\":\"in_progress\"}" \
  "$ACCESS_TOKEN")
expect_success "Issue update" "$update_response"
expect_field "Issue update" "$update_response" '.data.state' "in_progress"
echo "✅ Issue state updated"
echo ""

# Test 9: Token Refresh
echo "📋 Test 9: Token Refresh"
refresh_response=$(api_call POST "/api/v1/auth/refresh" \
  "{\"refresh_token\":\"$REFRESH_TOKEN\"}")
expect_success "Token refresh" "$refresh_response"
expect_field "Token refresh" "$refresh_response" '.data.user.email' "$TEST_EMAIL"
require_field "Token refresh" "$refresh_response" '.data.tokens.access_token' >/dev/null
echo "✅ Token refresh successful"
echo ""

echo "🎉 All integration tests passed!"

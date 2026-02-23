#!/bin/bash
set -e

# API Integration Test Script
# Tests authentication, workspace, project, and issue management

API_URL="${API_URL:-http://localhost:8080}"
TEST_EMAIL="test-$(date +%s)@example.com"
TEST_PASSWORD="TestPassword123!"

echo "🧪 Starting API Integration Tests"
echo "API URL: $API_URL"
echo ""

# Helper function for API calls
api_call() {
  local method=$1
  local endpoint=$2
  local data=$3
  local token=$4
  
  if [ -n "$token" ]; then
    curl -s -X "$method" "$API_URL$endpoint" \
      -H "Content-Type: application/json" \
      -H "Authorization: Bearer $token" \
      -d "$data"
  else
    curl -s -X "$method" "$API_URL$endpoint" \
      -H "Content-Type: application/json" \
      -d "$data"
  fi
}

# Test 1: Health Check
echo "📋 Test 1: Health Check"
response=$(curl -s "$API_URL/health")
if echo "$response" | grep -q "ok\|healthy"; then
  echo "✅ Health check passed"
else
  echo "❌ Health check failed"
  exit 1
fi
echo ""

# Test 2: User Registration
echo "📋 Test 2: User Registration"
register_response=$(api_call POST "/api/auth/register" \
  "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\",\"display_name\":\"Test User\"}")

if echo "$register_response" | grep -q "token\|access_token"; then
  echo "✅ Registration successful"
  ACCESS_TOKEN=$(echo "$register_response" | grep -o '"access_token":"[^"]*' | cut -d'"' -f4)
else
  echo "❌ Registration failed: $register_response"
  exit 1
fi
echo ""

# Test 3: User Login
echo "📋 Test 3: User Login"
login_response=$(api_call POST "/api/auth/login" \
  "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}")

if echo "$login_response" | grep -q "token\|access_token"; then
  echo "✅ Login successful"
  ACCESS_TOKEN=$(echo "$login_response" | grep -o '"access_token":"[^"]*' | cut -d'"' -f4)
  REFRESH_TOKEN=$(echo "$login_response" | grep -o '"refresh_token":"[^"]*' | cut -d'"' -f4)
else
  echo "❌ Login failed: $login_response"
  exit 1
fi
echo ""

# Test 4: Create Workspace
echo "📋 Test 4: Create Workspace"
workspace_slug="test-workspace-$(date +%s)"
workspace_response=$(api_call POST "/api/workspaces" \
  "{\"slug\":\"$workspace_slug\",\"name\":\"Test Workspace\"}" \
  "$ACCESS_TOKEN")

if echo "$workspace_response" | grep -q "id\|workspace"; then
  echo "✅ Workspace created"
  WORKSPACE_ID=$(echo "$workspace_response" | grep -o '"id":"[^"]*' | head -1 | cut -d'"' -f4)
else
  echo "❌ Workspace creation failed: $workspace_response"
  exit 1
fi
echo ""

# Test 5: Create Project
echo "📋 Test 5: Create Project"
project_response=$(api_call POST "/api/workspaces/$WORKSPACE_ID/projects" \
  "{\"name\":\"Test Project\",\"key\":\"TEST\",\"description\":\"Integration test project\"}" \
  "$ACCESS_TOKEN")

if echo "$project_response" | grep -q "id\|project"; then
  echo "✅ Project created"
  PROJECT_ID=$(echo "$project_response" | grep -o '"id":"[^"]*' | head -1 | cut -d'"' -f4)
else
  echo "❌ Project creation failed: $project_response"
  exit 1
fi
echo ""

# Test 6: Create Issue
echo "📋 Test 6: Create Issue"
issue_response=$(api_call POST "/api/projects/$PROJECT_ID/issues" \
  "{\"title\":\"Test Issue\",\"description\":\"This is a test issue\",\"issue_type\":\"task\"}" \
  "$ACCESS_TOKEN")

if echo "$issue_response" | grep -q "id\|issue"; then
  echo "✅ Issue created"
  ISSUE_ID=$(echo "$issue_response" | grep -o '"id":"[^"]*' | head -1 | cut -d'"' -f4)
else
  echo "❌ Issue creation failed: $issue_response"
  exit 1
fi
echo ""

# Test 7: Add Comment
echo "📋 Test 7: Add Comment to Issue"
comment_response=$(api_call POST "/api/issues/$ISSUE_ID/comments" \
  "{\"body\":\"This is a test comment\"}" \
  "$ACCESS_TOKEN")

if echo "$comment_response" | grep -q "id\|comment"; then
  echo "✅ Comment added"
else
  echo "❌ Comment creation failed: $comment_response"
  exit 1
fi
echo ""

# Test 8: Update Issue Status
echo "📋 Test 8: Update Issue Status"
update_response=$(api_call PATCH "/api/issues/$ISSUE_ID" \
  "{\"status\":\"in_progress\"}" \
  "$ACCESS_TOKEN")

if echo "$update_response" | grep -q "in_progress\|success"; then
  echo "✅ Issue status updated"
else
  echo "❌ Issue update failed: $update_response"
  exit 1
fi
echo ""

# Test 9: Token Refresh
echo "📋 Test 9: Token Refresh"
refresh_response=$(api_call POST "/api/auth/refresh" \
  "{\"refresh_token\":\"$REFRESH_TOKEN\"}")

if echo "$refresh_response" | grep -q "access_token"; then
  echo "✅ Token refresh successful"
else
  echo "❌ Token refresh failed: $refresh_response"
  exit 1
fi
echo ""

echo "🎉 All integration tests passed!"

#!/bin/bash
set -e

# API Performance Benchmark Script
# Tests API response times and throughput

API_URL="${API_URL:-http://localhost:8080}"

echo "⚡ OpenPR API Performance Benchmark"
echo "==================================="
echo "API URL: $API_URL"
echo ""

# Check if wrk is available
if ! command -v wrk &> /dev/null; then
  echo "⚠️  wrk not found, trying ab (Apache Bench)..."
  USE_AB=1
else
  USE_AB=0
fi

# Test 1: Health endpoint
echo "📋 Test 1: Health Endpoint"
echo "  - Duration: 10s"
echo "  - Connections: 10"
echo "  - Threads: 2"
echo ""

if [ $USE_AB -eq 1 ]; then
  # Using Apache Bench
  if command -v ab &> /dev/null; then
    ab -t 10 -c 10 -n 10000 "$API_URL/health"
  else
    echo "❌ Neither wrk nor ab is installed"
    echo "Install wrk: https://github.com/wg/wrk"
    echo "Or install ab: sudo apt-get install apache2-utils"
    exit 1
  fi
else
  # Using wrk
  wrk -t2 -c10 -d10s "$API_URL/health"
fi

echo ""
echo "✅ Benchmark complete"
echo ""
echo "📊 Recommendations:"
echo "  - Health endpoint should handle >1000 req/sec"
echo "  - Latency should be <10ms for health checks"
echo "  - For authenticated endpoints, expect 100-500 req/sec"
echo ""
echo "🔍 For detailed profiling:"
echo "  - Database queries: Check PostgreSQL logs"
echo "  - API tracing: Set RUST_LOG=debug"
echo "  - Memory usage: docker stats"

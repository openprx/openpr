#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$ROOT_DIR"

# Covers form.record.* delivery from event_outbox to a plain HTTP connector
# receiver. This verifies webhook/connector delivery without requiring an agent.
./scripts/smoke-form-events-outbox.sh

echo "webhook generic consumer smoke passed"

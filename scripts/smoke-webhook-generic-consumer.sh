#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$ROOT_DIR"

# Covers the form.record.* writes a webhook consumer subscribes to. The outbox and the
# connector delivery pipeline this once described were retired; what remains verifiable
# without an agent is that those writes still happen.
./scripts/smoke-form-events-outbox.sh

echo "webhook generic consumer smoke passed"

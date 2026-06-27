#!/usr/bin/env bash
set -euo pipefail

WEBHOOK_DIR="${1:-/opt/worker/code/openpr-webhook}"
TMP_DIR="$(mktemp -d)"
PORT="$((19000 + $$ % 1000))"
SERVER_PID=""

cleanup() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

cat > "${TMP_DIR}/config.toml" <<EOF
[server]
listen = "127.0.0.1:${PORT}"

[security]
allow_unsigned = true

[[agents]]
id = "contract-review"
name = "Document review connection"
agent_type = "custom"
message_template = "{event}"

[agents.route]
bot_names = ["Document review connection"]
bot_agent_types = ["webhook"]
project_types = ["contract_review"]
trigger_kinds = ["mention"]

[agents.custom]
command = "/bin/echo"
args = ["contract-route"]

[[agents]]
id = "maintenance-dispatch"
name = "Maintenance dispatch"
agent_type = "custom"
message_template = "{event}"

[agents.route]
bot_agent_types = ["webhook"]
project_types = ["equipment_maintenance"]
trigger_kinds = ["mention"]

[agents.custom]
command = "/bin/echo"
args = ["maintenance-route"]
EOF

(cd "${WEBHOOK_DIR}" && cargo build --quiet)
"${WEBHOOK_DIR}/target/debug/openpr-webhook" "${TMP_DIR}/config.toml" > "${TMP_DIR}/webhook.log" 2>&1 &
SERVER_PID="$!"

python3 - "${PORT}" <<'PY'
import json
import sys
import time
import urllib.error
import urllib.request

port = int(sys.argv[1])
base = f"http://127.0.0.1:{port}"


def wait_health():
    deadline = time.time() + 20
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"{base}/health", timeout=1) as response:
                if response.read().decode() == "ok":
                    return
        except (urllib.error.URLError, TimeoutError):
            time.sleep(0.2)
    raise AssertionError("openpr-webhook did not become healthy")


def post(payload):
    body = json.dumps(payload).encode()
    request = urllib.request.Request(
        f"{base}/webhook",
        data=body,
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.loads(response.read().decode())


wait_health()

contract = post({
    "event": "comment.created",
    "bot_context": {
        "is_bot_task": True,
        "bot_name": "Document review connection",
        "bot_agent_type": "webhook",
        "project_type": "contract_review",
        "trigger_kind": "mention",
    },
})
assert contract["status"] == "dispatched", contract
assert contract["agent"] == "contract-review", contract
assert "contract-route" in contract["result"], contract
assert contract["route"]["project_type"] == "contract_review", contract
assert contract["route"]["trigger_kind"] == "mention", contract

mismatch = post({
    "event": "comment.created",
    "bot_context": {
        "is_bot_task": True,
        "bot_name": "Document review connection",
        "bot_agent_type": "webhook",
        "project_type": "customer_delivery",
        "trigger_kind": "mention",
    },
})
assert mismatch["status"] == "no_agent", mismatch

maintenance = post({
    "task_id": "task-1",
    "ai_participant_id": "unbound-bot",
    "ai_participant_agent_type": "webhook",
    "task_type": "mention",
    "payload": {
        "project_type": "equipment_maintenance",
    },
})
assert maintenance["status"] == "dispatched", maintenance
assert maintenance["agent"] == "maintenance-dispatch", maintenance
assert "maintenance-route" in maintenance["result"], maintenance

print("openpr-webhook routing smoke passed")
PY

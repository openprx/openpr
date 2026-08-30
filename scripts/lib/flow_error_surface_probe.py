#!/usr/bin/env python3
"""Exercise one server_draining producer across REST, MCP transports and CLI.

The producer is a single in-process HTTP fixture whose response can be switched
between the two contract variants.  Every downstream process talks to that same
socket; no MCP or CLI result is synthesized by this probe.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from mcp_transport_probe import run_http, run_sse, run_stdio  # noqa: E402


OBJECT_ID = "11111111-1111-4111-8111-111111111111"
CLI_TOKEN = "opr_error_surface_cli_probe"


class Producer:
    reason = "drain"
    retry_after_ms = 15_000
    calls: dict[str, int] = {}
    lock = threading.Lock()


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        token = self.headers.get("Authorization", "").removeprefix("Bearer ")
        with Producer.lock:
            Producer.calls[token] = Producer.calls.get(token, 0) + 1
            call_number = Producer.calls[token]
        if token.startswith("opr_error_surface_mcp_") and call_number % 2 == 1:
            body = json.dumps(
                {"code": 0, "message": "ok", "data": {"id": OBJECT_ID, "project_id": None}}
            ).encode()
        else:
            body = json.dumps(
                {
                    "code": 409,
                    "message": "server_draining",
                    "data": None,
                    "error_code": "server_draining",
                    "details": {
                        "reason": Producer.reason,
                        "retry_after_ms": Producer.retry_after_ms,
                    },
                }
            ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: object) -> None:
        return


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def wait_port(port: int, proc: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"server on port {port} exited {proc.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.1)
    raise RuntimeError(f"server on port {port} did not accept connections")


def write_config(path: Path, api_url: str, transport: str, token: str, bind: str = "") -> None:
    bind_line = f'bind_addr = "{bind}"\n' if bind else ""
    path.write_text(
        "[database]\nurl = \"postgres://unused:unused@127.0.0.1:5432/unused\"\n\n"
        "[auth]\njwt_secret = \"unused-error-probe-secret\"\n\n"
        "[logging]\nfilter = \"error\"\nformat = \"text\"\n\n"
        f'[mcp]\napi_url = "{api_url}"\nbot_token = "{token}"\n'
        f'workspace_id = "22222222-2222-4222-8222-222222222222"\ntransport = "{transport}"\n'
        f"{bind_line}",
        encoding="utf-8",
    )


def response_for(raw: list[dict], request_id: int) -> dict | None:
    return next((item for item in raw if item.get("id") == request_id), None)


def mcp_error(resp: dict | None) -> dict:
    result = (resp or {}).get("result") or {}
    text = "".join(
        item.get("text", "") for item in result.get("content", []) if isinstance(item, dict)
    )
    try:
        parsed = json.loads(text)
        return parsed["error"]
    except (json.JSONDecodeError, KeyError, TypeError):
        return {"probe_parse_error": True, "response": resp, "text": text}


def run_cli(binary: str, config: Path, api_url: str, fmt: str) -> dict:
    proc = subprocess.run(
        [
            binary,
            "--config",
            str(config),
            "--api-url",
            api_url,
            "--bot-token",
            CLI_TOKEN,
            "--format",
            fmt,
            "objects",
            "get",
            OBJECT_ID,
        ],
        text=True,
        capture_output=True,
        timeout=20,
        check=False,
        env={**os.environ, "RUST_LOG": "error"},
    )
    row: dict = {"exit_code": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr}
    if fmt == "json":
        try:
            row["envelope"] = json.loads(proc.stdout)
        except json.JSONDecodeError as exc:
            row["parse_error"] = str(exc)
    return row


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mcp-binary", required=True)
    parser.add_argument("--cli-binary", required=True)
    parser.add_argument("--scratch-root", required=True)
    args = parser.parse_args()

    scratch = Path(args.scratch_root).resolve()
    scratch.mkdir(parents=True, exist_ok=True)
    api_port, http_port, sse_port = free_port(), free_port(), free_port()
    api_url = f"http://127.0.0.1:{api_port}"
    http_config, sse_config, stdio_config = (
        scratch / "mcp-http.toml",
        scratch / "mcp-sse.toml",
        scratch / "mcp-stdio.toml",
    )
    cli_config = scratch / "cli.toml"
    transport_tokens = {
        "http": "opr_error_surface_mcp_http",
        "sse": "opr_error_surface_mcp_sse",
        "stdio": "opr_error_surface_mcp_stdio",
    }
    write_config(http_config, api_url, "http", transport_tokens["http"], f"127.0.0.1:{http_port}")
    write_config(sse_config, api_url, "sse", transport_tokens["sse"], f"127.0.0.1:{sse_port}")
    write_config(stdio_config, api_url, "stdio", transport_tokens["stdio"])
    write_config(cli_config, api_url, "stdio", CLI_TOKEN)

    producer = ThreadingHTTPServer(("127.0.0.1", api_port), Handler)
    producer_thread = threading.Thread(target=producer.serve_forever, daemon=True)
    producer_thread.start()

    logs = [open(scratch / name, "w", encoding="utf-8") for name in ("mcp-http.log", "mcp-sse.log")]
    children = [
        subprocess.Popen(
            [args.mcp_binary, "--config", str(http_config), "serve", "--transport", "http"],
            stdout=logs[0], stderr=subprocess.STDOUT, text=True,
        ),
        subprocess.Popen(
            [args.mcp_binary, "--config", str(sse_config), "serve", "--transport", "sse"],
            stdout=logs[1], stderr=subprocess.STDOUT, text=True,
        ),
    ]
    try:
        wait_port(http_port, children[0])
        wait_port(sse_port, children[1])
        requests = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "error-surface-probe", "version": "1"}}},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "objects.get", "arguments": {"object_id": OBJECT_ID}}},
        ]
        variants = {}
        for reason, retry_after_ms in (("drain", 15_000), ("contention", 250)):
            Producer.reason = reason
            Producer.retry_after_ms = retry_after_ms
            Producer.calls = {}
            req = urllib.request.Request(f"{api_url}/api/v1/flow/objects/{OBJECT_ID}")
            with urllib.request.urlopen(req, timeout=5) as response:
                rest = json.loads(response.read())
            http_raw, http_errors = run_http(
                f"http://127.0.0.1:{http_port}", transport_tokens["http"], requests, 20
            )
            sse_raw, sse_errors = run_sse(
                "127.0.0.1", sse_port, transport_tokens["sse"], requests, 20
            )
            stdio_raw, stdio_errors = run_stdio(args.mcp_binary, str(stdio_config), requests, 20)
            variants[reason] = {
                "producer": {"socket": api_url, "reason": reason, "retry_after_ms": retry_after_ms},
                "rest": rest,
                "mcp": {
                    "http": {"error": mcp_error(response_for(http_raw, 2)), "transport_errors": http_errors},
                    "sse": {"error": mcp_error(response_for(sse_raw, 2)), "transport_errors": sse_errors},
                    "stdio": {"error": mcp_error(response_for(stdio_raw, 2)), "transport_errors": stdio_errors},
                },
                "cli": {
                    "json": run_cli(args.cli_binary, cli_config, api_url, "json"),
                    "table": run_cli(args.cli_binary, cli_config, api_url, "table"),
                },
            }
        print(json.dumps({"schema_version": "sylvode.flow.error-surface-probe.v1", "variants": variants}))
        return 0
    finally:
        for child in children:
            child.terminate()
        for child in children:
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
        producer.shutdown()
        producer.server_close()
        for log in logs:
            log.close()


if __name__ == "__main__":
    sys.exit(main())

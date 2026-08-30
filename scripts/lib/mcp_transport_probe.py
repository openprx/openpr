#!/usr/bin/env python3
"""Drive one MCP JSON-RPC conversation over HTTP, SSE or stdio and print the answers.

Used by scripts/verify-flow-mcp-transports-v0.4.sh. Each transport is spoken the
way a real client speaks it -- there is no shared shortcut that would hide a
transport-specific defect:

* ``http``  -- one ``POST /mcp/rpc`` per request, bearer token on every request.
* ``sse``   -- ``GET /sse`` opens the stream, the stream announces the endpoint
  its messages must be posted to, ``POST <endpoint>`` only *accepts* the call
  (``202``), and the result is read back off the stream. The stream is read over
  a raw socket because the frames are ``event: <name>\\ndata: <payload>\\n\\n``.
* ``stdio`` -- the shipped binary is spawned with ``serve --transport stdio`` and
  spoken to in newline-delimited JSON-RPC on its stdin/stdout.

Output: a single JSON object on stdout::

    {"transport": ..., "responses": [<jsonrpc response>, ...], "errors": [...]}

A transport that cannot be brought up at all reports its reason in ``errors``
and an empty ``responses`` -- it is never silently downgraded to "same as the
others".
"""

from __future__ import annotations

import argparse
import json
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request


def _requests_from(spec: str) -> list[dict]:
    return json.loads(spec)


# --------------------------------------------------------------------------- http
def run_http(base_url: str, token: str, requests: list[dict], timeout: float) -> tuple[list, list]:
    responses, errors = [], []
    for payload in requests:
        body = json.dumps(payload).encode()
        req = urllib.request.Request(
            f"{base_url}/mcp/rpc",
            data=body,
            method="POST",
            headers={"Content-Type": "application/json", "Authorization": f"Bearer {token}"},
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                responses.append(json.loads(resp.read().decode()))
        except urllib.error.HTTPError as e:
            errors.append(f"http {payload.get('method')}: HTTP {e.code}: {e.read().decode()[:400]}")
        except Exception as e:  # noqa: BLE001 -- any transport failure is reportable data
            errors.append(f"http {payload.get('method')}: {type(e).__name__}: {e}")
    return responses, errors


# ---------------------------------------------------------------------------- sse
class SseStream:
    """The ``GET /sse`` half of the SSE transport, read off a raw socket."""

    def __init__(self, host: str, port: int, token: str, timeout: float) -> None:
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.settimeout(timeout)
        request = (
            f"GET /sse HTTP/1.1\r\n"
            f"Host: {host}:{port}\r\n"
            f"Authorization: Bearer {token}\r\n"
            f"Accept: text/event-stream\r\n"
            f"Connection: keep-alive\r\n\r\n"
        )
        self.sock.sendall(request.encode())
        self.buf = b""

    def _read_more(self) -> bool:
        try:
            chunk = self.sock.recv(65536)
        except socket.timeout:
            return False
        if not chunk:
            return False
        self.buf += chunk
        return True

    def next_frame(self, deadline: float) -> tuple[str, str] | None:
        """The next ``event:``/``data:`` pair in the byte stream, or None on timeout.

        Chunked-transfer size lines cannot collide with the ``event:``/``data:``
        shape, so frames are located in the raw bytes directly.
        """
        while True:
            text = self.buf.decode("utf-8", "replace")
            start = text.find("event: ")
            if start != -1:
                end = text.find("\n\n", start)
                if end != -1:
                    frame = text[start:end]
                    self.buf = text[end + 2 :].encode()
                    name, _, rest = frame.partition("\n")
                    name = name[len("event: ") :].strip()
                    data_lines = [ln[len("data: ") :] for ln in rest.split("\n") if ln.startswith("data: ")]
                    return name, "\n".join(data_lines)
            if time.monotonic() > deadline or not self._read_more():
                return None

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


def run_sse(host: str, port: int, token: str, requests: list[dict], timeout: float) -> tuple[list, list]:
    responses, errors = [], []
    try:
        stream = SseStream(host, port, token, timeout)
    except Exception as e:  # noqa: BLE001
        return [], [f"sse: could not open the stream: {type(e).__name__}: {e}"]

    endpoint = None
    frame = stream.next_frame(time.monotonic() + timeout)
    if frame and frame[0] == "endpoint":
        endpoint = frame[1].strip()
    if not endpoint:
        stream.close()
        return [], [f"sse: the stream did not announce an endpoint (first frame: {frame})"]

    base = f"http://{host}:{port}"
    for payload in requests:
        body = json.dumps(payload).encode()
        req = urllib.request.Request(
            f"{base}{endpoint}",
            data=body,
            method="POST",
            headers={"Content-Type": "application/json", "Authorization": f"Bearer {token}"},
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                status = resp.status
                accept_body = resp.read().decode()
        except urllib.error.HTTPError as e:
            errors.append(f"sse {payload.get('method')}: POST endpoint HTTP {e.code}: {e.read().decode()[:400]}")
            continue
        except Exception as e:  # noqa: BLE001
            errors.append(f"sse {payload.get('method')}: POST endpoint {type(e).__name__}: {e}")
            continue
        if status != 202:
            errors.append(f"sse {payload.get('method')}: POST endpoint answered {status}, expected 202 ({accept_body[:200]})")
        deadline = time.monotonic() + timeout
        got = None
        while time.monotonic() <= deadline:
            frame = stream.next_frame(deadline)
            if frame is None:
                break
            if frame[0] == "message":
                try:
                    got = json.loads(frame[1])
                except json.JSONDecodeError as e:
                    errors.append(f"sse {payload.get('method')}: message frame was not JSON: {e}")
                    got = None
                break
        if got is None:
            errors.append(f"sse {payload.get('method')}: no result arrived on the stream within {timeout}s")
        else:
            responses.append(got)
    stream.close()
    return responses, errors


# -------------------------------------------------------------------------- stdio
def run_stdio(binary: str, config: str, requests: list[dict], timeout: float) -> tuple[list, list]:
    responses, errors = [], []
    try:
        proc = subprocess.Popen(  # noqa: S603 -- the binary path comes from the caller, not input
            [binary, "--config", config, "serve", "--transport", "stdio"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
    except Exception as e:  # noqa: BLE001
        return [], [f"stdio: could not spawn the binary: {type(e).__name__}: {e}"]

    try:
        for payload in requests:
            proc.stdin.write(json.dumps(payload) + "\n")
            proc.stdin.flush()
            line = proc.stdout.readline()
            if not line:
                errors.append(f"stdio {payload.get('method')}: the process closed its stdout without answering")
                break
            try:
                responses.append(json.loads(line))
            except json.JSONDecodeError as e:
                errors.append(f"stdio {payload.get('method')}: answer was not JSON ({e}): {line[:300]}")
    finally:
        try:
            proc.stdin.close()
        except (OSError, ValueError):
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
    return responses, errors


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--transport", required=True, choices=["http", "sse", "stdio"])
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int)
    ap.add_argument("--token", default="")
    ap.add_argument("--binary")
    ap.add_argument("--config")
    ap.add_argument("--requests", required=True, help="JSON array of JSON-RPC request objects")
    ap.add_argument("--timeout", type=float, default=20.0)
    args = ap.parse_args()

    requests = _requests_from(args.requests)

    if args.transport == "http":
        if args.port is None:
            print(json.dumps({"transport": "http", "responses": [], "errors": ["--port is required"]}))
            return 0
        responses, errors = run_http(f"http://{args.host}:{args.port}", args.token, requests, args.timeout)
    elif args.transport == "sse":
        if args.port is None:
            print(json.dumps({"transport": "sse", "responses": [], "errors": ["--port is required"]}))
            return 0
        responses, errors = run_sse(args.host, args.port, args.token, requests, args.timeout)
    else:
        if not args.binary or not args.config:
            print(json.dumps({"transport": "stdio", "responses": [], "errors": ["--binary and --config are required"]}))
            return 0
        responses, errors = run_stdio(args.binary, args.config, requests, args.timeout)

    print(json.dumps({"transport": args.transport, "responses": responses, "errors": errors}))
    return 0


if __name__ == "__main__":
    sys.exit(main())

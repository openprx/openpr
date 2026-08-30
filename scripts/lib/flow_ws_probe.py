#!/usr/bin/env python3
"""Minimal RFC 6455 WebSocket probe for the Sylvode Flow v0.4 security verifiers.

No third-party WebSocket dependency is available in this environment
(`websockets` and `websocket-client` are both absent), and the two things
these verifiers must prove -- that an upgrade is *refused*, and that a frame
sent on an already open session is *rejected* -- cannot be shown with `curl`
alone. So the handshake, the client masking and the frame framing are
implemented here directly against `socket`.

Deliberately narrow: text frames only, no continuation frames, no
compression, no TLS (every probe targets a loopback `api` the verifier
started itself). Anything outside that raises rather than guessing, so a
protocol surprise surfaces as a verifier error instead of a silently skipped
assertion.

Input is one JSON object on stdin:

    {"host":"127.0.0.1","port":8080,"path":"/api/v1/collab/ws?ticket=..",
     "origin":"https://example.test","read_timeout":5.0,
     "steps":[{"op":"send","frame":{...}},{"op":"recv","count":1}]}

Output is one JSON object on stdout:

    {"upgrade":{"http_status":101,"accept_ok":true,"headers":{...},
                "body":"","body_json":null},
     "events":[{"op":"recv","kind":"text","frame":{...}} ...],
     "error":null}

A refused upgrade is NOT an error here -- it is exactly the observation the
negative fixtures are about -- so the process still exits 0 and the caller
asserts on the payload.
"""
from __future__ import annotations

import base64
import hashlib
import json
import os
import socket
import struct
import sys

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


class ProbeError(Exception):
    """A protocol-level surprise the caller must see, never swallow."""


def _read_until_headers_end(sock: socket.socket) -> bytes:
    buf = b""
    while b"\r\n\r\n" not in buf:
        chunk = sock.recv(4096)
        if not chunk:
            raise ProbeError("connection closed before the HTTP response headers ended")
        buf += chunk
        if len(buf) > (1 << 20):
            raise ProbeError("HTTP response headers exceeded 1 MiB")
    return buf


def _handshake(sock: socket.socket, host: str, port: int, path: str, origin: str) -> dict:
    key_bytes = os.urandom(16)
    key = base64.b64encode(key_bytes).decode("ascii")
    request = (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n"
    )
    if origin:
        request += f"Origin: {origin}\r\n"
    request += "\r\n"
    sock.sendall(request.encode("ascii"))

    raw = _read_until_headers_end(sock)
    head, _, rest = raw.partition(b"\r\n\r\n")
    lines = head.decode("latin-1").split("\r\n")
    status_line = lines[0] if lines else ""
    parts = status_line.split(" ", 2)
    try:
        status = int(parts[1])
    except (IndexError, ValueError) as exc:
        raise ProbeError(f"unparsable HTTP status line: {status_line!r}") from exc

    headers = {}
    for line in lines[1:]:
        name, sep, value = line.partition(":")
        if sep:
            headers[name.strip().lower()] = value.strip()

    expected_accept = base64.b64encode(
        hashlib.sha1((key + WS_GUID).encode("ascii")).digest()  # noqa: S324 - RFC 6455 mandates SHA-1 here
    ).decode("ascii")
    observed_accept = headers.get("sec-websocket-accept", "")
    accept_ok = status == 101 and observed_accept == expected_accept

    body = rest
    if status != 101:
        # A refused upgrade is an ordinary HTTP response: drain whatever body
        # `Content-Length` promises so the caller can assert on the envelope
        # `code` (this API answers every error with HTTP 200 + a JSON body).
        try:
            want = int(headers.get("content-length", "0"))
        except ValueError:
            want = 0
        while len(body) < want:
            chunk = sock.recv(4096)
            if not chunk:
                break
            body += chunk

    body_text = body.decode("utf-8", errors="replace")
    try:
        body_json = json.loads(body_text) if body_text.strip() else None
    except json.JSONDecodeError:
        body_json = None

    return {
        "http_status": status,
        "accept_ok": accept_ok,
        "expected_accept": expected_accept,
        "observed_accept": observed_accept,
        "headers": headers,
        "body": body_text,
        "body_json": body_json,
    }


def _send_text(sock: socket.socket, payload: str) -> None:
    data = payload.encode("utf-8")
    header = bytearray()
    header.append(0x81)  # FIN + text opcode
    mask_bit = 0x80
    length = len(data)
    if length < 126:
        header.append(mask_bit | length)
    elif length < (1 << 16):
        header.append(mask_bit | 126)
        header.extend(struct.pack("!H", length))
    else:
        header.append(mask_bit | 127)
        header.extend(struct.pack("!Q", length))
    mask = os.urandom(4)
    header.extend(mask)
    masked = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
    sock.sendall(bytes(header) + masked)


def _recv_exact(sock: socket.socket, want: int) -> bytes:
    buf = b""
    while len(buf) < want:
        chunk = sock.recv(want - len(buf))
        if not chunk:
            raise ProbeError("connection closed mid-frame")
        buf += chunk
    return buf


def _recv_frame(sock: socket.socket) -> dict:
    first = _recv_exact(sock, 2)
    fin = bool(first[0] & 0x80)
    opcode = first[0] & 0x0F
    masked = bool(first[1] & 0x80)
    length = first[1] & 0x7F
    if length == 126:
        length = struct.unpack("!H", _recv_exact(sock, 2))[0]
    elif length == 127:
        length = struct.unpack("!Q", _recv_exact(sock, 8))[0]
    if masked:
        raise ProbeError("server sent a masked frame, which RFC 6455 forbids")
    if not fin:
        raise ProbeError("server sent a fragmented frame; this probe does not reassemble")
    payload = _recv_exact(sock, length) if length else b""

    if opcode == 0x8:  # close
        code = struct.unpack("!H", payload[:2])[0] if len(payload) >= 2 else None
        return {"kind": "close", "code": code, "reason": payload[2:].decode("utf-8", errors="replace")}
    if opcode == 0x1:
        text = payload.decode("utf-8", errors="replace")
        try:
            return {"kind": "text", "frame": json.loads(text)}
        except json.JSONDecodeError:
            return {"kind": "text", "frame": None, "raw": text}
    if opcode == 0x2:
        return {"kind": "binary", "len": len(payload)}
    if opcode == 0x9:
        return {"kind": "ping"}
    if opcode == 0xA:
        return {"kind": "pong"}
    raise ProbeError(f"unexpected opcode {opcode}")


def run(spec: dict) -> dict:
    host = spec["host"]
    port = int(spec["port"])
    read_timeout = float(spec.get("read_timeout", 5.0))
    events: list = []

    sock = socket.create_connection((host, port), timeout=read_timeout)
    try:
        sock.settimeout(read_timeout)
        upgrade = _handshake(sock, host, port, spec["path"], spec.get("origin", ""))
        if upgrade["http_status"] != 101:
            return {"upgrade": upgrade, "events": events, "error": None}

        for step in spec.get("steps", []):
            op = step.get("op")
            if op == "send":
                try:
                    _send_text(sock, json.dumps(step["frame"]))
                    events.append({"op": "send", "frame": step["frame"]})
                except OSError as exc:
                    # Same reasoning as the recv branch: a server that has
                    # already closed the session is the observation, not an
                    # error that should erase the frames it sent first.
                    events.append({"op": "send", "failed": True, "detail": str(exc)})
                    break
            elif op == "recv":
                for _ in range(int(step.get("count", 1))):
                    try:
                        events.append({"op": "recv", **_recv_frame(sock)})
                    except socket.timeout:
                        events.append({"op": "recv", "kind": "timeout"})
                        break
                    except (ProbeError, OSError) as exc:
                        # The server hanging up mid-read is an ordinary
                        # observation for the fixtures that assert a session
                        # is torn down, so it is recorded as an event rather
                        # than discarding every frame collected so far --
                        # which would hide the `rejected` frame the caller
                        # needs to assert on.
                        events.append({"op": "recv", "kind": "disconnected", "detail": str(exc)})
                        break
            elif op == "run_sql":
                # Runs one statement on `spec["database_url"]` *while this
                # session stays open*. The revocation fixtures need exactly
                # this: the session must already have captured its
                # `checked_epoch` at `open` time before the grant is pulled,
                # which is impossible to arrange from outside the process.
                _run_sql(spec.get("database_url", ""), step["sql"])
                events.append({"op": "run_sql", "sql": step["sql"]})
            else:
                raise ProbeError(f"unknown step op: {op!r}")
        return {"upgrade": upgrade, "events": events, "error": None}
    finally:
        try:
            sock.close()
        except OSError:
            pass


def _run_sql(database_url: str, sql: str) -> None:
    if not database_url:
        raise ProbeError("a run_sql step needs spec.database_url")
    try:
        import psycopg2  # noqa: PLC0415 - optional, only the run_sql steps need it
    except ImportError as exc:  # pragma: no cover - environment guard
        raise ProbeError("run_sql needs psycopg2") from exc
    conn = psycopg2.connect(database_url)
    try:
        conn.autocommit = True
        with conn.cursor() as cur:
            cur.execute(sql)
    finally:
        conn.close()


def main() -> int:
    try:
        spec = json.load(sys.stdin)
    except json.JSONDecodeError as exc:
        json.dump({"upgrade": None, "events": [], "error": f"bad spec: {exc}"}, sys.stdout)
        print()
        return 2
    try:
        result = run(spec)
    except (ProbeError, OSError, KeyError) as exc:
        json.dump({"upgrade": None, "events": [], "error": f"{type(exc).__name__}: {exc}"}, sys.stdout)
        print()
        return 1
    json.dump(result, sys.stdout)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())

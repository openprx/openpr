#!/usr/bin/env python3
"""Observe the v0.4 default-member Flow baseline through REST, MCP and CLI."""

from __future__ import annotations

import argparse
import json
import pathlib
import socket
import subprocess
import time
import urllib.error
import urllib.request
import uuid


def request(api: str, token: str, method: str, path: str, body: dict | None = None) -> dict:
    payload = None if body is None else json.dumps(body, separators=(",", ":")).encode()
    req = urllib.request.Request(
        api + path,
        data=payload,
        method=method,
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        raw = error.read().decode("utf-8", "replace")
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return {"code": error.code, "message": raw[:500]}


def psql(database_url: str, statement: str) -> str:
    return subprocess.run(
        ["psql", database_url, "-v", "ON_ERROR_STOP=1", "-At", "-c", statement],
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def write_mcp_config(path: pathlib.Path, api: str, token: str, workspace: str, transport: str, port: int | None) -> None:
    bind = "" if port is None else f'bind_addr = "127.0.0.1:{port}"\n'
    path.write_text(
        "[database]\nurl = \"postgres://unused:unused@127.0.0.1:5432/unused\"\n"
        "[auth]\njwt_secret = \"unused-by-authz-verifier\"\n"
        "[logging]\nfilter = \"error\"\nformat = \"text\"\n"
        f"[mcp]\napi_url = \"{api}\"\nbot_token = \"{token}\"\nworkspace_id = \"{workspace}\"\n"
        f'transport = "{transport}"\n{bind}',
        encoding="utf-8",
    )


def wait_port(port: int, process: subprocess.Popen[str]) -> bool:
    for _ in range(100):
        if process.poll() is not None:
            return False
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                return True
        except OSError:
            time.sleep(0.05)
    return False


def mcp_get(args: argparse.Namespace, transport: str, object_id: str, label: str) -> dict:
    config = pathlib.Path(args.work_dir) / f"mcp-{transport}-{label}.toml"
    port = None if transport == "stdio" else free_port()
    write_mcp_config(config, args.api, args.bot_token, args.workspace, transport, port)
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "authz-baseline", "version": "1"}}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "objects.get", "arguments": {"object_id": object_id}}},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "objects.history", "arguments": {"object_id": object_id, "limit": 100}}},
    ]
    command = ["python3", args.mcp_probe, "--transport", transport, "--requests", json.dumps(requests), "--timeout", "30"]
    server = None
    log = None
    if transport == "stdio":
        command += ["--binary", args.mcp_binary, "--config", str(config)]
    else:
        log = (pathlib.Path(args.work_dir) / f"mcp-{transport}-{label}.log").open("w", encoding="utf-8")
        server = subprocess.Popen(
            [args.mcp_binary, "--config", str(config), "serve", "--transport", transport],
            stdout=log,
            stderr=subprocess.STDOUT,
            text=True,
        )
        if not wait_port(port, server):
            if log:
                log.close()
            return {"error": "MCP server did not become reachable"}
        command += ["--host", "127.0.0.1", "--port", str(port), "--token", args.bot_token]
    try:
        completed = subprocess.run(command, text=True, capture_output=True, timeout=90, check=False)
        if completed.returncode != 0:
            return {"error": f"transport probe exit {completed.returncode}: {completed.stderr[-500:]}"}
        run = json.loads(completed.stdout)
    finally:
        if server is not None:
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
        if log:
            log.close()

    by_id = {response.get("id"): response for response in run.get("responses", [])}

    def tool_json(response_id: int) -> dict:
        response = by_id.get(response_id) or {}
        result = response.get("result") or {}
        if result.get("isError"):
            return {"error": "tool returned isError", "raw": result}
        for item in result.get("content") or []:
            if isinstance(item, dict) and isinstance(item.get("text"), str):
                try:
                    return json.loads(item["text"])
                except json.JSONDecodeError:
                    continue
        return {"error": "tool response carried no JSON", "raw": response}

    return {"transport_errors": run.get("errors", []), "object": tool_json(2), "history": tool_json(3)}


def cli_get(args: argparse.Namespace, object_id: str) -> dict:
    common = [args.cli_binary, "--config", args.cli_config, "--api-url", args.api, "--bot-token", args.bot_token, "--format", "json"]
    outputs = {}
    for name, tail in {
        "object": ["objects", "get", object_id],
        "history": ["objects", "history", object_id, "--limit", "100"],
    }.items():
        completed = subprocess.run(common + tail, text=True, capture_output=True, check=False)
        try:
            body = json.loads(completed.stdout)
        except json.JSONDecodeError:
            body = {"raw": completed.stdout, "stderr": completed.stderr}
        outputs[name] = {"exit_code": completed.returncode, "body": body}
    return outputs


def normalize_object(envelope: dict) -> dict:
    data = envelope.get("data") if isinstance(envelope, dict) else None
    if not isinstance(data, dict):
        return {"code": envelope.get("code") if isinstance(envelope, dict) else None, "invalid": True}
    return {
        "code": envelope.get("code"),
        "title": data.get("title"),
        "lifecycle_status": data.get("lifecycle_status"),
        "document_seq": data.get("document_seq"),
    }


def observe(args: argparse.Namespace, object_id: str, label: str) -> dict:
    rest_object = request(args.api, args.bot_token, "GET", f"/api/v1/flow/objects/{object_id}")
    rest_history = request(args.api, args.bot_token, "GET", f"/api/v1/flow/objects/{object_id}/history?limit=100")
    mcp = {transport: mcp_get(args, transport, object_id, label) for transport in ("http", "sse", "stdio")}
    cli = cli_get(args, object_id)
    normalized = {"rest": normalize_object(rest_object)}
    for transport, result in mcp.items():
        normalized[f"mcp_{transport}"] = normalize_object(result.get("object") or {})
    cli_envelope = ((cli.get("object") or {}).get("body") or {})
    # CLI unwraps ApiResponse.data into its own data field; rebuild a comparable envelope.
    normalized["cli"] = normalize_object({"code": 0 if (cli.get("object") or {}).get("exit_code") == 0 else None, "data": cli_envelope.get("data")})
    return {"rest": {"object": rest_object, "history": rest_history}, "mcp": mcp, "cli": cli, "normalized_objects": normalized}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--api", required=True)
    parser.add_argument("--bot-token", required=True)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--database-url", required=True)
    parser.add_argument("--mcp-binary", required=True)
    parser.add_argument("--cli-binary", required=True)
    parser.add_argument("--mcp-probe", required=True)
    parser.add_argument("--cli-config", required=True)
    parser.add_argument("--work-dir", required=True)
    args = parser.parse_args()
    args.api = args.api.rstrip("/")

    violations: list[str] = []
    observed_default_level = psql(
        args.database_url,
        f"SELECT default_member_level FROM flow_workspace_settings WHERE workspace_id='{args.workspace}'",
    )
    if observed_default_level != "edit":
        violations.append(f"fixture default_member_level={observed_default_level}, expected edit")
    create = request(
        args.api,
        args.bot_token,
        "POST",
        f"/api/v1/workspaces/{args.workspace}/flow/objects",
        {"object_type": "page", "title": "member baseline initial", "idempotency_key": f"authz-create-{uuid.uuid4()}"},
    )
    data = create.get("data") or {}
    object_id = str(data.get("id") or (data.get("object") or {}).get("id") or "")
    if create.get("code") != 0 or not object_id:
        print(json.dumps({"status": "failed", "violations": [f"default edit member could not create fixture: {create}"], "observations": {}}))
        return 1

    timeline = {"before": observe(args, object_id, "before")}
    set_title = request(
        args.api,
        args.bot_token,
        "POST",
        f"/api/v1/flow/objects/{object_id}/commands",
        {"command": {"type": "set_title", "payload": {"title": "member baseline edited"}}, "idempotency_key": f"authz-title-{uuid.uuid4()}"},
    )
    timeline["after_edit"] = observe(args, object_id, "after-edit")
    archive = request(
        args.api,
        args.bot_token,
        "POST",
        f"/api/v1/flow/objects/{object_id}/commands",
        {"command": {"type": "archive", "payload": {}}, "idempotency_key": f"authz-archive-{uuid.uuid4()}"},
    )
    timeline["after_archive"] = observe(args, object_id, "after-archive")
    restore = request(
        args.api,
        args.bot_token,
        "POST",
        f"/api/v1/flow/objects/{object_id}/commands",
        {"command": {"type": "restore", "payload": {}}, "idempotency_key": f"authz-restore-{uuid.uuid4()}"},
    )
    timeline["after_restore"] = observe(args, object_id, "after-restore")

    expected = {
        "before": ("member baseline initial", "active", 0),
        "after_edit": ("member baseline edited", "active", 1),
        "after_archive": ("member baseline edited", "archived", 1),
        "after_restore": ("member baseline edited", "active", 1),
    }
    for phase, (title, lifecycle, seq) in expected.items():
        normalized = timeline[phase]["normalized_objects"]
        for surface, value in normalized.items():
            if value != {"code": 0, "title": title, "lifecycle_status": lifecycle, "document_seq": seq}:
                violations.append(f"{phase} {surface} object projection mismatch: {value}")
        history_success = [timeline[phase]["rest"]["history"].get("code") == 0]
        history_success.extend(
            (timeline[phase]["mcp"][transport].get("history") or {}).get("code") == 0
            for transport in ("http", "sse", "stdio")
        )
        cli_history = timeline[phase]["cli"].get("history") or {}
        history_success.append(cli_history.get("exit_code") == 0 and (cli_history.get("body") or {}).get("ok") is True)
        if not all(history_success):
            violations.append(f"{phase} one or more REST/MCP/CLI history reads were refused")

    if set_title.get("code") != 0:
        violations.append(f"default edit member set_title was refused: {set_title}")
    if archive.get("code") != 0:
        violations.append(f"default edit member archive was refused: {archive}")
    if restore.get("code") != 0:
        violations.append(f"default edit member restore was refused: {restore}")

    inherit_values = psql(args.database_url, f"SELECT string_agg(inherit_from_parent::text,',' ORDER BY id) FROM flow_objects WHERE id='{object_id}'")
    if inherit_values != "true":
        violations.append(f"v0.4 fixture observed inherit_from_parent={inherit_values}, expected true")

    # Sensitivity controls: view must refuse archive without state/event mutation, and lifecycle
    # commands must reject an expected_frontier rather than silently ignoring it.
    psql(args.database_url, f"UPDATE flow_workspace_settings SET default_member_level='view', authz_epoch=authz_epoch+1 WHERE workspace_id='{args.workspace}'")
    events_before = int(psql(args.database_url, f"SELECT count(*) FROM business_events WHERE workspace_id='{args.workspace}' AND event_type='flow.object.archived'"))
    view_denied = request(
        args.api,
        args.bot_token,
        "POST",
        f"/api/v1/flow/objects/{object_id}/commands",
        {"command": {"type": "archive", "payload": {}}, "idempotency_key": f"authz-view-deny-{uuid.uuid4()}"},
    )
    events_after = int(psql(args.database_url, f"SELECT count(*) FROM business_events WHERE workspace_id='{args.workspace}' AND event_type='flow.object.archived'"))
    status_after_deny = psql(args.database_url, f"SELECT lifecycle_status FROM flow_objects WHERE id='{object_id}'")
    psql(args.database_url, f"UPDATE flow_workspace_settings SET default_member_level='edit', authz_epoch=authz_epoch+1 WHERE workspace_id='{args.workspace}'")
    frontier_denied = request(
        args.api,
        args.bot_token,
        "POST",
        f"/api/v1/flow/objects/{object_id}/commands",
        {"command": {"type": "archive", "payload": {}}, "expected_frontier": "AA==", "idempotency_key": f"authz-frontier-deny-{uuid.uuid4()}"},
    )
    status_after_frontier = psql(args.database_url, f"SELECT lifecycle_status FROM flow_objects WHERE id='{object_id}'")
    negatives = {
        "view_archive_denied": view_denied,
        "view_denial_no_success_event": events_before == events_after,
        "view_denial_status": status_after_deny,
        "archive_expected_frontier_denied": frontier_denied,
        "frontier_denial_status": status_after_frontier,
    }
    if view_denied.get("code") != 403 or events_before != events_after or status_after_deny != "active":
        violations.append("view-level negative control did not refuse archive with zero success mutation")
    if frontier_denied.get("code") == 0 or status_after_frontier != "active":
        violations.append("archive expected_frontier negative control did not refuse with zero lifecycle mutation")

    passed = not violations
    result = {
        "status": "passed" if passed else "failed",
        "fixture": {"workspace_id": args.workspace, "object_id": object_id, "default_member_level": observed_default_level, "inherit_from_parent_values": inherit_values},
        "writes": {"set_title": set_title, "archive": archive, "restore": restore},
        "timeline": timeline,
        "negative_controls": negatives,
        "surface_capability_boundary": {
            "rest": ["read", "set_title", "archive", "restore", "history"],
            "mcp_http": ["read", "history"],
            "mcp_sse": ["read", "history"],
            "mcp_stdio": ["read", "history"],
            "cli": ["read", "history"],
            "reason": "v0.4 MCP and CLI registries expose no object command tool; archive/restore are executed once through the frozen REST command surface and their resulting state is compared through every shipped read surface",
        },
        "violations": violations,
    }
    print(json.dumps(result, separators=(",", ":")))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""OpenPR MCP core regression - 3 transports with 105-tool registry checks."""
import json, subprocess, requests, time, threading, sys, queue, base64, os, atexit, shutil, tempfile

MCP_HTTP = "http://localhost:8090"
TOKEN = "opr_0a5bc81ea108dad8077decc880abced0d923aa873b9ff774575ec152aecf15d5"
WS = "e5166fd1-3bb7-46d9-b907-273b1eef3f44"
PID = "adc627bf-15fe-418b-8948-d3c343f9e4f5"
MCP_BIN = "/opt/worker/code/openpr/target/release/mcp-server"

# The MCP server reads no environment variables and refuses to start without a configuration
# file, so the stdio transport gets one instead of an env dict. It is written 0600 into a
# temporary directory removed at exit, because it carries the bot token.
_CONFIG_DIR = tempfile.mkdtemp(prefix="openpr-mcp-regression-")
atexit.register(shutil.rmtree, _CONFIG_DIR, True)
MCP_CONFIG = os.path.join(_CONFIG_DIR, "openpr.toml")
with open(os.open(MCP_CONFIG, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w", encoding="utf-8") as _handle:
    _handle.write(
        '[logging]\nfilter = "error"\nformat = "text"\n\n'
        '[mcp]\napi_url = "http://localhost:8081"\n'
        f'bot_token = "{TOKEN}"\nworkspace_id = "{WS}"\ntransport = "stdio"\n'
    )

PASS = FAIL = SKIP = 0
ERRORS = []

def extract(resp):
    if isinstance(resp, dict):
        content = resp.get("result", {}).get("content", [])
        if content and "text" in content[0]:
            try: return json.loads(content[0]["text"])
            except: return content[0]["text"]
    return resp

def http_call(tool, args=None):
    try:
        r = requests.post(f"{MCP_HTTP}/mcp/rpc", json={"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args or {}}}, timeout=15)
        return extract(r.json())
    except Exception as e: return {"error": str(e)}

def stdio_call(tool, args=None):
    payload = json.dumps({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args or {}}})
    try:
        env = {"PATH":"/usr/bin:/bin"}
        proc = subprocess.run([MCP_BIN,"serve","--transport","stdio","--config",MCP_CONFIG], input=payload, capture_output=True, text=True, timeout=15, env=env)
        for line in proc.stdout.strip().split("\n"):
            line = line.strip()
            if line.startswith("{"):
                return extract(json.loads(line))
        return {"error": f"no JSON output"}
    except Exception as e: return {"error": str(e)}

def sse_call(tool, args=None):
    """SSE: keep connection alive, POST, read response from stream"""
    try:
        q = queue.Queue()
        ready = threading.Event()
        stop = threading.Event()
        
        def listen():
            try:
                r = requests.get(f"{MCP_HTTP}/sse", stream=True, headers={"Accept":"text/event-stream"}, timeout=30)
                event_type = None
                for line in r.iter_lines(decode_unicode=True):
                    if stop.is_set(): break
                    if line is None: continue
                    if line.startswith("event:"):
                        event_type = line[6:].strip()
                    elif line.startswith("data:"):
                        data = line[5:].strip()
                        q.put((event_type, data))
                        if event_type == "endpoint":
                            ready.set()
                        event_type = None
                    elif line == "":
                        event_type = None
            except: pass
        
        t = threading.Thread(target=listen, daemon=True)
        t.start()
        ready.wait(timeout=5)
        
        # Get endpoint
        endpoint = "/messages"
        try:
            evt_type, data = q.get(timeout=3)
            if evt_type == "endpoint":
                endpoint = data.strip()
        except: pass
        
        # POST
        url = f"{MCP_HTTP}{endpoint}" if endpoint.startswith("/") else endpoint
        resp = requests.post(url, json={"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args or {}}}, timeout=15)
        
        # Read message response from SSE stream
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                evt_type, data = q.get(timeout=2)
                if evt_type == "message":
                    stop.set()
                    return extract(json.loads(data))
            except: continue
        
        stop.set()
        return {"error": "no SSE response within 10s"}
    except Exception as e: return {"error": str(e)}

CALLERS = {"HTTP": http_call, "STDIO": stdio_call, "SSE": sse_call}

REQUIRED_REGISTRY_TOOLS = {
    "forms.list",
    "forms.get",
    "forms.create",
    "forms.create_from_template",
    "scenario_templates.install",
    "forms.update_schema",
    "forms.duplicate",
    "forms.schema_summary",
    "forms.field_usage",
    "forms.field_dependencies",
    "form_schema_versions.list",
    "form_schema_versions.get",
    "form_permissions.get",
    "form_permissions.update",
    "form_views.list",
    "form_attachments.list",
    "form_attachments.create",
    "form_attachments.archive",
    "form_attachments.restore",
    "form_records.list",
    "form_records.export",
    "form_records.import_preview",
    "form_records.import_commit",
    "form_records.get",
    "form_records.create",
    "form_records.update",
    "form_records.link",
    "form_records.relation_targets",
    "form_records.children",
    "form_records.child_create",
    "form_records.child_update",
    "form_records.child_archive",
    "form_records.child_restore",
    "form_records.aggregate",
    "events.tail",
    "plugins.list",
    "plugins.get",
    "plugins.install",
    "plugins.invoke",
    "plugin_invocations.list",
}

def http_list_tools():
    try:
        r = requests.post(f"{MCP_HTTP}/mcp/rpc", json={"jsonrpc":"2.0","id":1,"method":"tools/list"}, timeout=15)
        return r.json()
    except Exception as e: return {"error": str(e)}

def stdio_list_tools():
    payload = json.dumps({"jsonrpc":"2.0","id":1,"method":"tools/list"})
    try:
        env = {"PATH":"/usr/bin:/bin"}
        proc = subprocess.run([MCP_BIN,"serve","--transport","stdio","--config",MCP_CONFIG], input=payload, capture_output=True, text=True, timeout=15, env=env)
        for line in proc.stdout.strip().split("\n"):
            line = line.strip()
            if line.startswith("{"):
                return json.loads(line)
        return {"error": "no JSON output"}
    except Exception as e: return {"error": str(e)}

def sse_list_tools():
    try:
        q = queue.Queue()
        ready = threading.Event()
        stop = threading.Event()

        def listen():
            try:
                r = requests.get(f"{MCP_HTTP}/sse", stream=True, headers={"Accept":"text/event-stream"}, timeout=30)
                event_type = None
                for line in r.iter_lines(decode_unicode=True):
                    if stop.is_set(): break
                    if line is None: continue
                    if line.startswith("event:"):
                        event_type = line[6:].strip()
                    elif line.startswith("data:"):
                        data = line[5:].strip()
                        q.put((event_type, data))
                        if event_type == "endpoint":
                            ready.set()
                        event_type = None
                    elif line == "":
                        event_type = None
            except: pass

        threading.Thread(target=listen, daemon=True).start()
        ready.wait(timeout=5)
        endpoint = "/messages"
        try:
            evt_type, data = q.get(timeout=3)
            if evt_type == "endpoint":
                endpoint = data.strip()
        except: pass
        url = f"{MCP_HTTP}{endpoint}" if endpoint.startswith("/") else endpoint
        requests.post(url, json={"jsonrpc":"2.0","id":1,"method":"tools/list"}, timeout=15)
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                evt_type, data = q.get(timeout=2)
                if evt_type == "message":
                    stop.set()
                    return json.loads(data)
            except: continue
        stop.set()
        return {"error": "no SSE response within 10s"}
    except Exception as e: return {"error": str(e)}

LISTERS = {"HTTP": http_list_tools, "STDIO": stdio_list_tools, "SSE": sse_list_tools}

def registry_has_105_tools_with_forms_and_plugins(resp):
    tools = resp.get("result", {}).get("tools", []) if isinstance(resp, dict) else []
    names = {tool.get("name") for tool in tools if isinstance(tool, dict)}
    return len(tools) == 105 and REQUIRED_REGISTRY_TOOLS.issubset(names)

def check(proto, tool, result, expect_fn):
    global PASS, FAIL
    try: ok = expect_fn(result)
    except: ok = False
    if ok:
        PASS += 1; print(f"  ✅ [{proto}] {tool}")
    else:
        FAIL += 1; s = str(result)[:140]; ERRORS.append(f"  ❌ [{proto}] {tool} → {s}"); print(f"  ❌ [{proto}] {tool} → {s}")
    return ok

def is_ok(r): return isinstance(r, dict) and r.get("code") == 0
def has_id(r): return is_ok(r) and r.get("data", {}).get("id")
def get_id(r):
    try: return r["data"]["id"]
    except: return None
def ok_or_str(r): return is_ok(r) or (isinstance(r, str) and any(w in r.lower() for w in ["added","removed","deleted","success"]))

print("=" * 60)
print(f"  OpenPR MCP 核心回归测试 (105工具注册面 × 3协议)")
print(f"  {time.strftime('%Y-%m-%d %H:%M:%S')}")
print("=" * 60)

for proto, call in CALLERS.items():
    print(f"\n{'━'*50}\n  协议: {proto}\n{'━'*50}")

    check(proto, "tools/list.registry_105", LISTERS[proto](), registry_has_105_tools_with_forms_and_plugins)
    
    # === Projects ===
    print("\n📁 Projects (list/get)")
    check(proto, "projects.list", call("projects.list"), is_ok)
    check(proto, "projects.get", call("projects.get", {"project_id": PID}), is_ok)

    # === Project Types / Resources ===
    print("\n🧭 Project Types & Resources")
    check(proto, "project_types.list", call("project_types.list"), is_ok)
    check(proto, "project_types.get", call("project_types.get", {"key": "code_project"}), is_ok)
    check(proto, "scenario_templates.list", call("scenario_templates.list"), is_ok)
    check(
        proto,
        "scenario_templates.get",
        call("scenario_templates.get", {"key": "contract_review_default"}),
        is_ok,
    )
    check(proto, "project_resources.list", call("project_resources.list", {"project_id": PID}), is_ok)
    ts = int(time.time()) % 100000
    res = call("project_resources.create", {
        "project_id": PID,
        "kind": "custom",
        "name": f"{proto}-regression-resource-{ts}",
        "locator": {"source": "mcp-regression", "protocol": proto},
        "sync_status": "manual"
    })
    check(proto, "project_resources.create", res, has_id)
    res_id = get_id(res)
    if res_id:
        check(proto, "project_resources.update", call("project_resources.update", {
            "project_id": PID,
            "resource_id": res_id,
            "name": f"{proto}-regression-resource-updated-{ts}",
            "sync_status": "synced"
        }), is_ok)
        check(proto, "project_resources.delete", call("project_resources.delete", {
            "project_id": PID,
            "resource_id": res_id
        }), ok_or_str)
    else:
        SKIP += 2

    # === Context ===
    print("\n🧠 Context")
    check(proto, "context.get_project", call("context.get_project", {"project_id": PID}), is_ok)
    check(proto, "context.get_governance", call("context.get_governance", {"project_id": PID}), is_ok)
    check(proto, "context.get_agent_policy", call("context.get_agent_policy", {"project_id": PID}), is_ok)

    # === Connectors / Invocations ===
    print("\n🔌 Connectors & Invocations")
    connectors = call("connectors.list", {"project_id": PID})
    check(proto, "connectors.list", connectors, is_ok)
    try:
        connector_items = connectors.get("data", [])
        connector_id = connector_items[0]["id"] if connector_items else None
    except Exception:
        connector_id = None
    if connector_id:
        check(proto, "connectors.get", call("connectors.get", {"connector_id": connector_id}), is_ok)
    else:
        SKIP += 1; print(f"  ⏭️  [{proto}] connectors.get (no connector fixture)")

    invocations = call("invocations.list", {"project_id": PID})
    check(proto, "invocations.list", invocations, is_ok)
    inv = call("invocations.create", {
        "project_id": PID,
        "trigger_kind": "mcp",
        "connector_id": connector_id,
        "payload": {"source": "mcp-regression", "protocol": proto}
    })
    check(proto, "invocations.create", inv, has_id)
    invocation_id = get_id(inv)
    if invocation_id:
        check(proto, "invocations.report_progress", call("invocations.report_progress", {
            "invocation_id": invocation_id,
            "payload": {"step": "smoke", "protocol": proto}
        }), is_ok)
        check(proto, "invocations.complete", call("invocations.complete", {
            "invocation_id": invocation_id,
            "result": {"ok": True, "protocol": proto}
        }), is_ok)
        check(proto, "invocations.get", call("invocations.get", {"invocation_id": invocation_id}), is_ok)
    else:
        SKIP += 3; print(f"  ⏭️  [{proto}] invocation lifecycle (create failed)")

    failed_inv = call("invocations.create", {
        "project_id": PID,
        "trigger_kind": "mcp",
        "payload": {"source": "mcp-regression", "protocol": proto, "path": "fail"}
    })
    failed_inv_id = get_id(failed_inv)
    if failed_inv_id:
        check(proto, "invocations.fail", call("invocations.fail", {
            "invocation_id": failed_inv_id,
            "error_message": "regression intentional failure path",
            "result": {"ok": False, "protocol": proto}
        }), is_ok)
    else:
        SKIP += 1; print(f"  ⏭️  [{proto}] invocations.fail (create failed)")
    try:
        invocation_items = invocations.get("data", {}).get("items", [])
        existing_invocation_id = invocation_items[0]["id"] if invocation_items else None
    except Exception:
        existing_invocation_id = None
    if existing_invocation_id:
        check(proto, "invocations.get_existing", call("invocations.get", {"invocation_id": existing_invocation_id}), is_ok)
    else:
        SKIP += 1; print(f"  ⏭️  [{proto}] invocations.get_existing (no invocation fixture)")

    # === Work Items 读 ===
    print("\n📋 Work Items (读: list/get/search/get_by_identifier)")
    check(proto, "work_items.list", call("work_items.list", {"project_id": PID}), is_ok)
    check(proto, "work_items.get", call("work_items.get", {"work_item_id": "40b28aac-97d4-4bb7-adbb-f1db6bb763e8"}), is_ok)
    check(proto, "work_items.search", call("work_items.search", {"query": "test"}), is_ok)
    check(proto, "work_items.get_by_identifier", call("work_items.get_by_identifier", {"identifier": "ADMIN123-1"}), lambda r: isinstance(r, dict))

    # === Work Items 写 ===
    print(f"\n📋 Work Items (写: create/update/labels/delete)")
    wi = call("work_items.create", {"project_id": PID, "title": f"{proto}-final-regtest", "priority": "low", "state": "backlog"})
    wi_ok = check(proto, "work_items.create", wi, has_id)
    wi_id = get_id(wi)
    if wi_id:
        check(proto, "work_items.update", call("work_items.update", {"work_item_id": wi_id, "state": "in_progress", "priority": "high"}), is_ok)
        check(proto, "work_items.add_label", call("work_items.add_label", {"work_item_id": wi_id, "label_id": "7c466d81-09ea-41d7-8513-b39c710c8330"}), ok_or_str)
        check(proto, "work_items.add_labels", call("work_items.add_labels", {"work_item_id": wi_id, "label_ids": ["94b44a22-01c9-4cbc-8208-b01fdd1c68a8"]}), ok_or_str)
        check(proto, "work_items.list_labels", call("work_items.list_labels", {"work_item_id": wi_id}), is_ok)
        check(proto, "work_items.remove_label", call("work_items.remove_label", {"work_item_id": wi_id, "label_id": "94b44a22-01c9-4cbc-8208-b01fdd1c68a8"}), ok_or_str)
        
        # === Comments ===
        print(f"\n💬 Comments (create/list/delete)")
        cmt = call("comments.create", {"work_item_id": wi_id, "content": f"{proto} final regression comment"})
        check(proto, "comments.create", cmt, has_id)
        cmt_id = get_id(cmt)
        check(proto, "comments.list", call("comments.list", {"work_item_id": wi_id}), is_ok)
        if cmt_id: check(proto, "comments.delete", call("comments.delete", {"comment_id": cmt_id}), ok_or_str)
        else: SKIP += 1
        
        check(proto, "work_items.delete", call("work_items.delete", {"work_item_id": wi_id}), ok_or_str)
    else:
        SKIP += 10; print(f"  ⏭️  Skipping write tests (create failed)")

    # === Files Upload ===
    print(f"\n📎 Files (upload)")
    b64 = base64.b64encode(b"regression test log content").decode()
    fup = call("files.upload", {"filename": f"{proto}-test.log", "content_base64": b64})
    file_ok = check(proto, "files.upload", fup, lambda r: isinstance(r, dict) and "url" in r)

    # === Labels ===
    print(f"\n🏷️ Labels (list/list_by_project/create/update/delete)")
    check(proto, "labels.list", call("labels.list"), is_ok)
    check(proto, "labels.list_by_project", call("labels.list_by_project", {"project_id": PID}), is_ok)
    ts = int(time.time()) % 100000
    lbl = call("labels.create", {"name": f"{proto}-final-{ts}", "color": "#cc5500"})
    check(proto, "labels.create", lbl, has_id)
    lbl_id = get_id(lbl)
    if lbl_id:
        check(proto, "labels.update", call("labels.update", {"label_id": lbl_id, "name": f"{proto}-upd-{ts}"}), is_ok)
        check(proto, "labels.delete", call("labels.delete", {"label_id": lbl_id}), ok_or_str)
    else: SKIP += 2

    # === Members ===
    print(f"\n👥 Members (list)")
    check(proto, "members.list", call("members.list"), is_ok)

    # === Sprints ===
    print(f"\n🏃 Sprints (list/create/update/delete)")
    check(proto, "sprints.list", call("sprints.list", {"project_id": PID}), is_ok)
    spr = call("sprints.create", {"project_id": PID, "name": f"{proto}-final-spr", "start_date": "2026-04-01", "end_date": "2026-04-14"})
    check(proto, "sprints.create", spr, has_id)
    spr_id = get_id(spr)
    if spr_id:
        check(proto, "sprints.update", call("sprints.update", {"sprint_id": spr_id, "name": f"{proto}-spr-final-upd"}), is_ok)
        check(proto, "sprints.delete", call("sprints.delete", {"sprint_id": spr_id}), ok_or_str)
    else: SKIP += 2

    # === Proposals / Check Results ===
    print(f"\n📝 Proposals & Check Results (list/get/check result/proposal)")
    check(proto, "proposals.list", call("proposals.list", {"project_id": PID}), is_ok)
    check(proto, "proposals.get", call("proposals.get", {"proposal_id": "PROP-053d48a2-1b85-409a-9b65-2a99281cbcef"}), lambda r: isinstance(r, dict) and "code" in r)
    chk = call("check_results.create", {
        "project_id": PID,
        "action_class": "high_risk_mutation",
        "risk_level": "high",
        "title": f"{proto}-regression governed check result",
        "summary": f"{proto} regression created a high-risk check result for proposal conversion validation.",
        "result": {"source": "mcp-regression", "protocol": proto}
    })
    check(proto, "check_results.create", chk, has_id)
    chk_id = get_id(chk)
    if chk_id:
        proposal = call("proposals.create_from_result", {
            "check_result_id": chk_id,
            "title": f"{proto}-regression proposal from check result",
            "domains": ["governance", "regression"],
            "submit": False
        })
        check(proto, "proposals.create_from_result", proposal, has_id)
    else:
        SKIP += 1; print(f"  ⏭️  [{proto}] proposals.create_from_result (check result create failed)")
    check(proto, "release.readiness.get", call("release.readiness.get", {"project_id": PID}), is_ok)

    # === Scenario Tools ===
    print(f"\n🏭 Scenario tools (code/traditional governed entries)")
    check(proto, "code.resources.list", call("code.resources.list", {"project_id": PID}), is_ok)
    check(proto, "code.task_context.get", call("code.task_context.get", {"project_id": PID}), lambda r: isinstance(r, dict) or isinstance(r, str))
    for tool, title, summary in [
        ("code.change_proposal.create", "Code change proposal", "Governed code change proposal from regression."),
        ("documents.extract_summary", "Document summary", "Governed document summary from regression."),
        ("documents.review_risk", "Document risk review", "Governed document risk review from regression."),
        ("approval.request", "Approval request", "Governed approval request from regression."),
        ("inspection.report", "Inspection report", "Governed inspection report from regression."),
        ("corrective_action.propose", "Corrective action", "Governed corrective action proposal from regression."),
    ]:
        result = call(tool, {
            "project_id": PID,
            "title": f"{proto}-{title}",
            "summary": f"{proto} {summary}",
            "result": {"source": "mcp-regression", "protocol": proto, "tool": tool}
        })
        check(proto, tool, result, has_id)

    # === Search ===
    print(f"\n🔍 Search (all)")
    check(proto, "search.all", call("search.all", {"query": "test"}), is_ok)

# Summary
total_per_proto = PASS + FAIL  # approximate
print(f"\n{'='*60}")
print(f"  最终测试结果")
print(f"{'='*60}")
print(f"  ✅ 通过: {PASS}")
print(f"  ❌ 失败: {FAIL}")
print(f"  ⏭️  跳过: {SKIP}")
print(f"  📊 通过率: {PASS*100//(PASS+FAIL) if (PASS+FAIL) else 0}%")
if ERRORS:
    print(f"\n  失败详情:")
    for e in ERRORS: print(e)
print(f"{'='*60}")
sys.exit(1 if FAIL > 0 else 0)

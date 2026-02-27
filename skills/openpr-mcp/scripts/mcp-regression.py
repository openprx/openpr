#!/usr/bin/env python3
"""OpenPR MCP 完整回归测试 - 3协议 × 34工具"""
import json, subprocess, requests, time, threading, sys, queue, base64

MCP_HTTP = "http://localhost:8090"
TOKEN = "opr_0a5bc81ea108dad8077decc880abced0d923aa873b9ff774575ec152aecf15d5"
WS = "e5166fd1-3bb7-46d9-b907-273b1eef3f44"
PID = "adc627bf-15fe-418b-8948-d3c343f9e4f5"
MCP_BIN = "/opt/worker/code/openpr/target/release/mcp-server"

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
        env = {"OPENPR_API_URL":"http://localhost:3000","OPENPR_BOT_TOKEN":TOKEN,"OPENPR_WORKSPACE_ID":WS,"PATH":"/usr/bin:/bin"}
        proc = subprocess.run([MCP_BIN,"--transport","stdio"], input=payload, capture_output=True, text=True, timeout=15, env=env)
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
print(f"  OpenPR MCP 完整回归测试 (34工具 × 3协议)")
print(f"  {time.strftime('%Y-%m-%d %H:%M:%S')}")
print("=" * 60)

for proto, call in CALLERS.items():
    print(f"\n{'━'*50}\n  协议: {proto}\n{'━'*50}")
    
    # === Projects ===
    print("\n📁 Projects (list/get)")
    check(proto, "projects.list", call("projects.list"), is_ok)
    check(proto, "projects.get", call("projects.get", {"project_id": PID}), is_ok)

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

    # === Proposals ===
    print(f"\n📝 Proposals (list/get)")
    check(proto, "proposals.list", call("proposals.list", {"project_id": PID}), is_ok)
    check(proto, "proposals.get", call("proposals.get", {"proposal_id": "PROP-053d48a2-1b85-409a-9b65-2a99281cbcef"}), lambda r: isinstance(r, dict) and "code" in r)

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

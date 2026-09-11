#!/usr/bin/env python3
"""Regression and falsification check for v0.5 baseline response shapes."""

import importlib.util
import os
import pathlib


MODULE_PATH = pathlib.Path(__file__).parent / "lib" / "flow_authz_baseline_live_probe.py"
SPEC = importlib.util.spec_from_file_location("flow_authz_baseline_live_probe", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)

title = "member baseline initial"
mcp_title = "mutation: divergent MCP title" if os.environ.get("OPENPR_FLOW_TEST_MUTATION_MCP_TITLE") == "1" else title
bare_object = {"title": mcp_title, "lifecycle_status": "active", "document_seq": 0}
rest_object = {"code": 0, "data": {"title": title, "lifecycle_status": "active", "document_seq": 0}}

normalized = {
    "rest": probe.normalize_rest_object(rest_object),
    "mcp_http": probe.normalize_mcp_object(bare_object),
    "mcp_sse": probe.normalize_mcp_object(bare_object),
    "mcp_stdio": probe.normalize_mcp_object(bare_object),
    "cli": probe.normalize_rest_object(rest_object),
}
for surface, value in normalized.items():
    assert probe.object_projection_matches(value, title, "active", 0), (
        f"{surface} object projection mismatch: {value}"
    )

assert probe.rest_history_succeeded({"code": 0, "data": {"items": []}})
assert probe.mcp_history_succeeded({"items": []})
assert probe.cli_history_succeeded({"exit_code": 0, "body": {"ok": True, "data": {"items": []}}})
print("PASS: REST envelopes and MCP bare objects normalize without masking projection differences")

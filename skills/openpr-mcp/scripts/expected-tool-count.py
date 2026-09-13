#!/usr/bin/env python3
"""Print the MCP registry baseline after proving it matches the live binary."""

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess


repo = Path(__file__).resolve().parents[3]
baseline_path = repo / "apps/mcp-server/tool-registry-baseline.json"
baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
expected_count = baseline.get("count")
expected_hash = baseline.get("names_sha256")
v0_4_rebase = baseline.get("v0_4_rebase", {})
rebase_rows = sorted(
    (
        (tuple(map(int, match.groups())), value)
        for key, value in baseline.items()
        if (match := re.fullmatch(r"v(\d+)_(\d+)_rebase", key)) and isinstance(value, dict)
    ),
    key=lambda row: row[0],
)
latest_rebase = rebase_rows[-1][1] if rebase_rows else {}
rebase_chain_valid = bool(rebase_rows)
previous_after = None
for _, row in rebase_rows:
    before = row.get("before_count")
    added = row.get("added")
    removed = row.get("removed")
    after = row.get("after_count")
    if not all(isinstance(value, int) for value in (before, added, removed, after)):
        rebase_chain_valid = False
        break
    if before + added - removed != after or (previous_after is not None and before != previous_after):
        rebase_chain_valid = False
        break
    previous_after = after
if (
    baseline.get("schema_version") != "openpr.mcp-tool-registry-baseline.v1"
    or baseline.get("source") != "mcp_server::get_all_tool_definitions"
    or not isinstance(expected_count, int)
    or expected_count <= 0
    or not isinstance(expected_hash, str)
    or re.fullmatch(r"[0-9a-f]{64}", expected_hash) is None
    or not rebase_chain_valid
    or latest_rebase.get("after_count") != expected_count
):
    raise SystemExit(f"invalid MCP tool registry baseline: {baseline_path}")

environment = os.environ.copy()
environment.setdefault("CARGO_BUILD_JOBS", "4")
build = subprocess.run(
    ["cargo", "build", "-q", "-p", "mcp-server", "--bin", "list-tools"],
    cwd=repo,
    env=environment,
    text=True,
    capture_output=True,
    check=False,
)
if build.returncode != 0:
    raise SystemExit(f"cannot build live MCP registry:\n{build.stdout}{build.stderr}")

target_dir = Path(environment.get("CARGO_TARGET_DIR", repo / "target"))
if not target_dir.is_absolute():
    target_dir = repo / target_dir
binary = target_dir / "debug/list-tools"
live = subprocess.run([binary], cwd=repo, text=True, capture_output=True, check=False)
if live.returncode != 0:
    raise SystemExit(f"cannot execute live MCP registry:\n{live.stdout}{live.stderr}")

names = sorted(set(re.findall(r"(?m)^  ([A-Za-z][A-Za-z0-9_.]*)$", live.stdout)))
declared = re.search(r"Available MCP Tools \((\d+) total\)", live.stdout)
live_hash = hashlib.sha256("\n".join(names).encode()).hexdigest()
if (
    declared is None
    or int(declared.group(1)) != len(names)
    or len(names) != expected_count
    or live_hash != expected_hash
):
    raise SystemExit(
        "MCP tool registry drift: "
        f"baseline count/hash={expected_count}/{expected_hash}, "
        f"live count/hash={len(names)}/{live_hash}"
    )

print(expected_count)

#!/usr/bin/env python3
"""Print the repository-owned MCP registry total from the Flow surface snapshot."""

from pathlib import Path
import re


snapshot = (
    Path(__file__).resolve().parents[3]
    / "apps/mcp-server/src/tools/mcp-surface-v05.snapshot.md"
)
text = snapshot.read_text(encoding="utf-8")
matches = re.findall(r"Expected registry totals[^\n]*v0\.5 `([0-9]+)`", text)
if len(matches) != 1 or int(matches[0]) <= 0:
    raise SystemExit(f"cannot derive one positive v0.5 registry total from {snapshot}")

print(matches[0])

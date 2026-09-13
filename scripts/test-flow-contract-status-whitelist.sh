#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

python3 - "$ROOT" <<'PY'
import importlib.util
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
module_path = root / "scripts/lib/flow_contract_status.py"
spec = importlib.util.spec_from_file_location("flow_contract_status", module_path)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)

cases = {
    "active": True,
    "accepted_with_known_gap": True,
    "planned": False,
    "actvie": False,
    "accepted": False,
    "": False,
}
for status, expected in cases.items():
    actual = module.is_accepted_contract_status(status)
    if actual is not expected:
        raise SystemExit(f"status {status!r}: got {actual}, expected {expected}")

print("PASS: explicit Flow contract status whitelist rejects planned, actvie, and unknown values")
PY

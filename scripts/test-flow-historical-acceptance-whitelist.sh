#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

python3 - "$ROOT" <<'PY'
import importlib.util
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
module_path = root / "scripts/lib/flow_historical_acceptance.py"
spec = importlib.util.spec_from_file_location("flow_historical_acceptance", module_path)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)

cases = {
    "accepted": True,
    "accepted_with_known_gap": True,
    "active": False,
    "planned": False,
    "accepted_typo": False,
    "": False,
    None: False,
}
for status, expected in cases.items():
    actual = module.is_accepted_historical_status(status)
    if actual is not expected:
        raise SystemExit(f"status {status!r}: got {actual}, expected {expected}")

print("PASS: historical acceptance whitelist rejects active, planned, and unknown values")
PY

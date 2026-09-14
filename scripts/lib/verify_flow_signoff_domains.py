#!/usr/bin/env python3
import argparse
import hashlib
import json
import pathlib
import subprocess


def digest(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def keys(command: list[str]) -> list[str]:
    return [line for line in subprocess.check_output(command, text=True).splitlines() if line]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--flow-recorder", required=True, type=pathlib.Path)
    parser.add_argument("--forms-recorder", required=True, type=pathlib.Path)
    args = parser.parse_args()
    flow = args.flow_recorder.resolve()
    forms = args.forms_recorder.resolve()
    flow_keys = keys([str(flow), "--list-keys"])
    forms_keys = keys([str(forms), "--list-items"])
    checks = {
        "recorder_paths_distinct": flow != forms,
        "recorder_inodes_distinct": flow.stat() != forms.stat(),
        "recorder_hashes_distinct": digest(flow) != digest(forms),
        "signoff_keys_nonempty": bool(flow_keys) and bool(forms_keys),
        "signoff_keys_disjoint": not set(flow_keys).intersection(forms_keys),
    }
    result = {
        "domains": [
            {"domain": "flow", "recorder": str(flow), "sha256": digest(flow), "keys": flow_keys},
            {"domain": "forms", "recorder": str(forms), "sha256": digest(forms), "keys": forms_keys},
        ],
        "checks": checks,
        "passed": all(checks.values()),
    }
    print(json.dumps(result, sort_keys=True))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

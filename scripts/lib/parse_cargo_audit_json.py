#!/usr/bin/env python3
import json
import pathlib
import sys


def parse_vulnerability_count(payload: object) -> int:
    if not isinstance(payload, dict):
        raise ValueError("cargo audit JSON root must be an object")
    vulnerabilities = payload.get("vulnerabilities")
    if not isinstance(vulnerabilities, dict):
        raise ValueError("cargo audit JSON vulnerabilities must be an object")
    count = vulnerabilities.get("count")
    found = vulnerabilities.get("found")
    entries = vulnerabilities.get("list")
    if isinstance(count, bool) or not isinstance(count, int) or count < 0:
        raise ValueError("cargo audit JSON vulnerabilities.count must be a non-negative integer")
    if not isinstance(found, bool):
        raise ValueError("cargo audit JSON vulnerabilities.found must be a boolean")
    if not isinstance(entries, list):
        raise ValueError("cargo audit JSON vulnerabilities.list must be an array")
    if found != (count > 0):
        raise ValueError("cargo audit JSON vulnerabilities.found disagrees with count")
    if len(entries) != count:
        raise ValueError("cargo audit JSON vulnerabilities.list length disagrees with count")
    return count


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: parse_cargo_audit_json.py CARGO_AUDIT_JSON", file=sys.stderr)
        return 2
    path = pathlib.Path(sys.argv[1])
    try:
        payload = json.loads(path.read_text())
        count = parse_vulnerability_count(payload)
        result = {"status": "parsed", "vulnerability_count": count, "error": None}
        exit_code = 0
    except (OSError, json.JSONDecodeError, ValueError) as error:
        result = {
            "status": "unparseable",
            "vulnerability_count": None,
            "error": str(error),
        }
        exit_code = 1
    print(json.dumps(result, sort_keys=True))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())

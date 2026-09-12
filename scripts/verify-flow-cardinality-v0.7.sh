#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT=""
ADR_PATH=""
SINCE_RELEASE=""
DROP=""
JSON_MODE=0

while (($#)); do
  case "$1" in
    --adr) ADR_PATH="${2:?}"; shift 2 ;;
    --since-release) SINCE_RELEASE="${2:?}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?}"; shift 2 ;;
    --test-drop-declaration) DROP="${2:?}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    *) echo "FAIL: unsupported argument: $1" >&2; exit 2 ;;
  esac
done

[[ "$SINCE_RELEASE" == "0.6" && "$JSON_MODE" == 1 ]] || {
  echo 'FAIL: require --since-release 0.6 --json' >&2
  exit 2
}
[[ -n "${OPENPR_TEST_DATABASE_URL:-}" ]] || {
  echo 'FAIL: OPENPR_TEST_DATABASE_URL is required' >&2
  exit 2
}
[[ -n "$ADR_PATH" ]] || ADR_PATH="$CONTRACTS_ROOT/decisions/ADR-0013-multi-document-atomicity.md"
[[ -n "$EVIDENCE_ROOT" ]] || EVIDENCE_ROOT="$CONTRACTS_ROOT/evidence/v0.7"
[[ -f "$ADR_PATH" && -d "$REPO_ROOT/.git" ]] || {
  echo 'FAIL: repository or ADR missing' >&2
  exit 2
}
mkdir -p "$EVIDENCE_ROOT"
export CARGO_BUILD_JOBS=4

python3 - "$REPO_ROOT" "$ADR_PATH" "$EVIDENCE_ROOT" "$DROP" <<'PY'
import datetime as dt, hashlib, json, os, pathlib, re, subprocess, sys, tempfile, time
repo, adr, evidence = map(lambda p: pathlib.Path(p).resolve(), sys.argv[1:4])
drop = sys.argv[4]
head = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
main = (repo / "apps/api/src/main.rs").read_text()
expected = {
    "objects.reference": ("/api/v1/flow/objects/{object_id}/references", "post_flow_object_reference"),
    "objects.unreference": ("/api/v1/flow/objects/{object_id}/references/{reference_id}", "delete_flow_object_reference"),
    "objects.convert_preview": ("/api/v1/flow/conversions/preview", "post_flow_conversion_preview"),
    "objects.convert_commit": ("/api/v1/flow/conversions", "post_flow_conversion"),
    "objects.convert_status": ("/api/v1/flow/conversions/{job_id}", "get_flow_conversion"),
    "objects.convert_retry": ("/api/v1/flow/conversions/{job_id}/retry", "post_flow_conversion_retry"),
}
observed = {}
for wire, (path, handler) in expected.items():
    start = main.find(f'"{path}"')
    if start >= 0 and handler in main[start:start + 900]:
        observed[wire] = {"path": path, "handler": handler}
if drop:
    observed.pop(drop, None)

tests = {
    "reference_unreference_zero": "flow_bridge_reference_embed_reauthorizes_forms_policy_and_missing_policy_is_read_only",
    "conversion_commands_zero": "flow_bridge_conversion_commit_rechecks_policy_and_is_idempotent_without_rewriting_source",
    "guest_subset_fail_closed": "flow_bridge_guest_routes_hide_reference_cardinality_and_target_existence",
}
test_results = []
for test_id, test_name in tests.items():
    started = time.monotonic()
    process = subprocess.run(
        ["cargo", "test", "-p", "api", test_name, "--", "--nocapture"],
        cwd=repo, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    )
    log = evidence / f"cardinality-{test_id}.log"
    log.write_text(process.stdout)
    summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;", process.stdout, re.M)
    passed_count = sum(int(item[1]) for item in summaries)
    failed_count = sum(int(item[2]) for item in summaries)
    ok = process.returncode == 0 and passed_count > 0 and failed_count == 0
    test_results.append({
        "id": test_id, "status": "passed" if ok else "failed", "exit_code": process.returncode,
        "executed_count": passed_count + failed_count,
        "duration_ms": round((time.monotonic() - started) * 1000),
        "log": log.name, "sha256": hashlib.sha256(process.stdout.encode()).hexdigest(),
    })

adr_text = adr.read_text()
checks = [
    {"id": "production_routes_complete", "status": "passed" if set(observed) == set(expected) else "failed", "detail": observed},
    {"id": "runtime_existing_heads_unchanged", "status": "passed" if all(x["status"] == "passed" for x in test_results[:2]) else "failed", "detail": test_results[:2]},
    {"id": "guest_command_subset", "status": test_results[2]["status"], "detail": test_results[2]},
    {"id": "adr_cardinality_rule_present", "status": "passed" if re.search(r"existing_document_cardinality\s*=\s*0\s*\|\s*1\s*\|\s*bounded_many", adr_text) else "failed", "detail": str(adr)},
]
dirty = subprocess.check_output(
    ["git", "-C", str(repo), "status", "--porcelain=v1", "--", "apps", "crates", "migrations", "Cargo.toml", "Cargo.lock"],
    text=True,
).splitlines()
checks.append({"id": "source_clean", "status": "passed" if not dirty else "failed", "detail": dirty})
passed = all(item["status"] == "passed" for item in checks) and not drop
result = {
    "schema_version": "sylvode.flow.cardinality-result.v2", "release": "0.7.0", "since_release": "0.6",
    "source_head": head, "source_path": "apps/api/src/main.rs", "commands_found": sorted(observed),
    "new_commands_found": len(observed), "existing_document_cardinality": {"zero": sorted(observed)},
    "guest_commands": {"status": test_results[2]["status"], "new_commands_found": 0,
                       "reason": "production guest reference and preview routes fail closed"},
    "checks": checks, "tests": test_results, "test_fault": drop or None, "passed": passed,
    "unresolved": sum(item["status"] != "passed" for item in checks),
    "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
}
fd, tmp = tempfile.mkstemp(prefix=".cardinality.", dir=evidence)
with os.fdopen(fd, "w") as handle:
    json.dump(result, handle, sort_keys=True, indent=2)
    handle.write("\n")
os.replace(tmp, evidence / "cardinality-result.json")
print(json.dumps(result, sort_keys=True))
raise SystemExit(0 if passed else 1)
PY

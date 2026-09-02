#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHECKER="$ROOT_DIR/scripts/verify-flow-binary-provenance.sh"
SCHEMA="$ROOT_DIR/docs/schemas/sylvode-flow-binary-provenance-v1.schema.json"
AJV_MODULE="$ROOT_DIR/frontend/node_modules/ajv"
TMP_ROOT="$(mktemp -d)"
DIRTY_MARKER="$ROOT_DIR/apps/api/src/.openpr-provenance-test-untracked"
trap 'rm -f "$DIRTY_MARKER"; rm -rf "$TMP_ROOT"' EXIT
REPO_ROOT="$TMP_ROOT/repo"
PROBE_ROOT="$TMP_ROOT/probes"
mkdir -p "$REPO_ROOT" "$PROBE_ROOT"

for tool in cargo git jq node python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAIL: missing required command: $tool" >&2
    exit 2
  fi
done
if [[ ! -f "$AJV_MODULE/package.json" ]]; then
  echo "FAIL: schema validation requires the installed frontend Ajv module: $AJV_MODULE" >&2
  exit 2
fi

REQUESTED_CASE="${1:-all}"
case "$REQUESTED_CASE" in
  all|checker|E|B|C|D) ;;
  *) echo "Usage: $0 [all|checker|E|B|C|D]" >&2; exit 2 ;;
esac

now_ns() { date +%s%N; }
duration_ms() { echo $((($(now_ns) - $1) / 1000000)); }

git -C "$REPO_ROOT" init -q
git -C "$REPO_ROOT" config user.name provenance-test
git -C "$REPO_ROOT" config user.email provenance-test@example.invalid
printf 'fixture\n' > "$REPO_ROOT/tracked.txt"
git -C "$REPO_ROOT" add tracked.txt
git -C "$REPO_ROOT" commit -qm fixture
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
COMMITTER_DATE="$(git -C "$REPO_ROOT" show -s --format=%cI HEAD)"
OLD_HEAD="0000000000000000000000000000000000000000"
GOOD_SHA="$(printf 'a%.0s' {1..64})"

validate_schema_json() {
  local payload="$1"
  SCHEMA_PAYLOAD="$payload" node - "$SCHEMA" "$AJV_MODULE" <<'NODEEOF'
const fs = require("fs");
const schemaPath = process.argv[2];
const ajvModule = process.argv[3];
const Ajv = require(ajvModule);
const ajv = new Ajv({allErrors: true, schemaId: "auto"});
const validate = ajv.compile(JSON.parse(fs.readFileSync(schemaPath, "utf8")));
if (!validate(JSON.parse(process.env.SCHEMA_PAYLOAD))) {
  process.stderr.write("FAIL schema: " + ajv.errorsText(validate.errors, {separator: "\n"}) + "\n");
  process.exit(1);
}
NODEEOF
}

write_probe() {
  local path="$1" producer="$2" binary_available="$3" digest="$4"
  local metadata_available="$5" commit="$6" dirty="$7" committer_date="$8" source="$9" schema="${10}"
  python3 - "$path" "$producer" "$binary_available" "$digest" \
    "$metadata_available" "$commit" "$dirty" "$committer_date" "$source" "$schema" <<'PYEOF'
import json, sys
path, producer, available, digest, metadata_available, commit, dirty, committer_date, source, schema = sys.argv[1:]
metadata = None
if metadata_available == "true":
    metadata = {
        "schema_version": schema,
        "git_commit": None if commit == "null" else commit,
        "git_dirty": None if dirty == "null" else dirty == "true",
        "git_committer_date": None if committer_date == "null" else committer_date,
        "source": source,
    }
with open(path, "w", encoding="utf-8") as handle:
    json.dump({
        "producer_specified": producer == "true",
        "binary_available": available == "true",
        "binary_sha256": None if digest == "null" else digest,
        "build_metadata_available": metadata_available == "true",
        "build_metadata": metadata,
    }, handle)
PYEOF
}

assert_case() {
  local name="$1" expected_status="$2" expected_rc="$3"
  shift 3
  local probe="$PROBE_ROOT/$name.json" output rc started
  write_probe "$probe" "$@"
  started="$(now_ns)"
  set +e
  output="$($CHECKER --json --repo-root "$REPO_ROOT" --probe-json "$probe")"
  rc=$?
  set -e
  if [[ $rc -ne $expected_rc ]]; then
    echo "FAIL $name: exit=$rc expected=$expected_rc output=$output" >&2
    exit 1
  fi
  validate_schema_json "$output"
  if [[ "$(jq -r .status <<<"$output")" != "$expected_status" ]]; then
    echo "FAIL $name: status=$(jq -r .status <<<"$output") expected=$expected_status" >&2
    exit 1
  fi
  printf '%s exit=%s status=%s duration_ms=%s\n' \
    "$name" "$rc" "$expected_status" "$(duration_ms "$started")"
}

run_checker_cases() {
  assert_case normal passed 0 true true "$GOOD_SHA" true "$SOURCE_HEAD" false "$COMMITTER_DATE" git openpr.build-info.v1
  assert_case old_binary_new_head source_head_mismatch 1 true true "$GOOD_SHA" true "$OLD_HEAD" false "$COMMITTER_DATE" git openpr.build-info.v1
  assert_case build_metadata_missing build_metadata_unavailable 1 true true "$GOOD_SHA" false null null null unknown openpr.build-info.v1
  assert_case binary_missing binary_unavailable 1 true false null false null null null unknown openpr.build-info.v1
  assert_case producer_missing producer_unspecified 1 false false null false null null null unknown openpr.build-info.v1
  assert_case digest_malformed binary_digest_malformed 1 true true abc true "$SOURCE_HEAD" false "$COMMITTER_DATE" git openpr.build-info.v1
  assert_case metadata_malformed build_metadata_malformed 1 true true "$GOOD_SHA" true "$SOURCE_HEAD" false "$COMMITTER_DATE" git wrong.schema
  assert_case source_unknown build_source_unknown 1 true true "$GOOD_SHA" true null null null unknown openpr.build-info.v1
  assert_case dirty_binary source_dirty 1 true true "$GOOD_SHA" true "$SOURCE_HEAD" true "$COMMITTER_DATE" git openpr.build-info.v1

  printf 'untracked\n' > "$REPO_ROOT/untracked.txt"
  assert_case dirty_checkout source_dirty 1 true true "$GOOD_SHA" true "$SOURCE_HEAD" false "$COMMITTER_DATE" git openpr.build-info.v1
  rm "$REPO_ROOT/untracked.txt"

  local unborn_root="$TMP_ROOT/unborn" output rc started
  mkdir -p "$unborn_root"
  git -C "$unborn_root" init -q
  write_probe "$PROBE_ROOT/source_head_missing.json" true true "$GOOD_SHA" true "$SOURCE_HEAD" false "$COMMITTER_DATE" git openpr.build-info.v1
  started="$(now_ns)"
  set +e
  output="$($CHECKER --json --repo-root "$unborn_root" --probe-json "$PROBE_ROOT/source_head_missing.json")"
  rc=$?
  set -e
  validate_schema_json "$output"
  [[ $rc -eq 1 && "$(jq -r .status <<<"$output")" == source_head_missing ]] || {
    echo "FAIL source_head_missing: exit=$rc output=$output" >&2
    exit 1
  }
  printf 'source_head_missing exit=%s status=source_head_missing duration_ms=%s\n' "$rc" "$(duration_ms "$started")"

  local shim_root="$TMP_ROOT/git-shim"
  mkdir -p "$shim_root"
  cat > "$shim_root/git" <<'EOF'
#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == status ]]; then
    exit 1
  fi
done
exec /usr/bin/git "$@"
EOF
  chmod +x "$shim_root/git"
  write_probe "$PROBE_ROOT/source_clean_unproven.json" true true "$GOOD_SHA" true "$SOURCE_HEAD" false "$COMMITTER_DATE" git openpr.build-info.v1
  started="$(now_ns)"
  set +e
  output="$(PATH="$shim_root:$PATH" "$CHECKER" --json --repo-root "$REPO_ROOT" --probe-json "$PROBE_ROOT/source_clean_unproven.json")"
  rc=$?
  set -e
  validate_schema_json "$output"
  [[ $rc -eq 1 && "$(jq -r .status <<<"$output")" == source_clean_unproven ]] || {
    echo "FAIL source_clean_unproven: exit=$rc output=$output" >&2
    exit 1
  }
  printf 'source_clean_unproven exit=%s status=source_clean_unproven duration_ms=%s\n' "$rc" "$(duration_ms "$started")"
}

run_e_test() {
  local started output rc
  printf 'untracked fixture for build-info propagation\n' > "$DIRTY_MARKER"
  started="$(now_ns)"
  set +e
  output="$(cd "$ROOT_DIR" && env -u OPENPR_BUILD_GIT_COMMIT -u OPENPR_BUILD_GIT_DIRTY \
    -u OPENPR_BUILD_GIT_COMMITTER_DATE OPENPR_PROVENANCE_VERIFY_GIT_DIRTY=1 \
    cargo test -p api --bin api embedded_build_info_preserves_dirty_true -- --nocapture 2>&1)"
  rc=$?
  set -e
  rm -f "$DIRTY_MARKER"
  if [[ $rc -ne 0 ]] || ! grep -q 'test build_info_tests::embedded_build_info_preserves_dirty_true ... ok' <<<"$output"; then
    echo "FAIL E build.rs-to-main dirty constant propagation: exit=$rc" >&2
    tail -30 <<<"$output" >&2
    exit 1
  fi
  printf 'E build_info_dirty_true exit=0 test=passed duration_ms=%s\n' "$(duration_ms "$started")"
}

run_b_test() {
  local started
  started="$(now_ns)"
  python3 - "$ROOT_DIR/scripts/lib/flow_gate_v0_4_recompute.py" "$REPO_ROOT" "$PROBE_ROOT" "$SOURCE_HEAD" "$GOOD_SHA" "$OLD_HEAD" "$COMMITTER_DATE" <<'PYEOF'
import importlib.util
import json
import os
import sys

module_path, repo_root, evidence_root, source_head, digest, old_head, committer_date = sys.argv[1:]
spec = importlib.util.spec_from_file_location("flow_gate_v0_4_recompute", module_path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
path = os.path.join(evidence_root, "deployed-chain-websocket-result.json")

def artifact(commit):
    return {
        "schema_version": "sylvode.flow.deployed-chain-websocket-result.v1",
        "source_head": source_head,
        "source_dirty": False,
        "self_test": {"ran": True, "exit": 0, "duration_ms": 1},
        "passed": True,
        "gates": {
            "deployed_chain_websocket_upgrade": {
                "status": "passed",
                "reason": "forged producer pass",
            }
        },
        "checks": [{"name": "binary_provenance_matches_source_head", "passed": True}],
        "binary_provenance": {
            "schema_version": "openpr.flow.binary-provenance.v1",
            "status": "passed",
            "passed": True,
            "binary": {"available": True, "sha256": digest},
            "build_metadata_available": True,
            "build_metadata": {
                "schema_version": "openpr.build-info.v1",
                "git_commit": commit,
                "git_dirty": False,
                "git_committer_date": committer_date,
                "source": "git",
            },
        },
    }

with open(path, "w", encoding="utf-8") as handle:
    json.dump(artifact(source_head), handle)
normal = module.recompute(evidence_root, repo_root)
assert normal["hard_gates"]["deployed_chain_websocket_upgrade"] == "passed"

# Keep the producer gate and every pass bit green; mutate only the embedded commit.
with open(path, "w", encoding="utf-8") as handle:
    json.dump(artifact(old_head), handle)
forged = module.recompute(evidence_root, repo_root)
assert forged["hard_gates"]["deployed_chain_websocket_upgrade"] == "failed", forged
print("B recompute_forged_producer cli_semantics=prints_gate_map gate=failed reason=" +
      forged["reasons"]["deployed_chain_websocket_upgrade"])
PYEOF
  printf 'B recompute_wiring exit=0 assertion=passed duration_ms=%s\n' "$(duration_ms "$started")"
}

make_container_fixture() {
  CONTAINER_SHIM_ROOT="$TMP_ROOT/container-shim"
  CONTAINER_LOG="$TMP_ROOT/container-commands.log"
  DEPLOYMENT="$TMP_ROOT/deployment.json"
  mkdir -p "$CONTAINER_SHIM_ROOT"
  cat > "$CONTAINER_SHIM_ROOT/container-cli" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$PROVENANCE_SHIM_LOG"
case "$*" in
  "inspect api-container")
    printf '[{"Id":"container-id","Image":"image-id","Name":"/api-container"}]\n'
    ;;
  "exec api-container sha256sum /app/api")
    printf '%s  /app/api\n' "$PROVENANCE_SHIM_SHA"
    ;;
  "exec api-container /app/api --build-info")
    printf '{"schema_version":"openpr.build-info.v1","git_commit":"%s","git_dirty":false,"git_committer_date":"%s","source":"git"}\n' \
      "$PROVENANCE_SHIM_HEAD" "$PROVENANCE_SHIM_DATE"
    ;;
  *)
    echo "unsupported container command: $*" >&2
    exit 9
    ;;
esac
EOF
  chmod +x "$CONTAINER_SHIM_ROOT/container-cli"
  python3 - "$DEPLOYMENT" "$CONTAINER_SHIM_ROOT/container-cli" <<'PYEOF'
import json, sys
path, cli = sys.argv[1:]
with open(path, "w", encoding="utf-8") as handle:
    json.dump({
        "container_cli": cli,
        "hops": [{"name": "api", "container": "api-container", "binary_path": "/app/api"}],
    }, handle)
PYEOF
}

run_d_test() {
  local started output rc
  : > "$CONTAINER_LOG"
  started="$(now_ns)"
  set +e
  output="$(PROVENANCE_SHIM_LOG="$CONTAINER_LOG" PROVENANCE_SHIM_SHA="$GOOD_SHA" \
    PROVENANCE_SHIM_HEAD="$SOURCE_HEAD" PROVENANCE_SHIM_DATE="$COMMITTER_DATE" \
    "$CHECKER" --json --repo-root "$REPO_ROOT" --deployment "$DEPLOYMENT")"
  rc=$?
  set -e
  validate_schema_json "$output"
  if [[ $rc -ne 0 || "$(jq -r .status <<<"$output")" != passed ]] || \
     ! grep -Fxq 'exec api-container /app/api --build-info' "$CONTAINER_LOG"; then
    echo "FAIL D real container probe: exit=$rc output=$output" >&2
    sed -n '1,20p' "$CONTAINER_LOG" >&2
    exit 1
  fi
  printf 'D container_build_info_probe exit=0 status=passed duration_ms=%s\n' "$(duration_ms "$started")"
}

run_c_test() {
  local evidence_root="$TMP_ROOT/deployed-evidence" started output rc provenance artifact
  mkdir -p "$evidence_root"
  : > "$CONTAINER_LOG"
  started="$(now_ns)"
  set +e
  output="$(FLOW_BINARY_PROVENANCE_SELF_TESTED=1 \
    PROVENANCE_SHIM_LOG="$CONTAINER_LOG" PROVENANCE_SHIM_SHA="$GOOD_SHA" \
    PROVENANCE_SHIM_HEAD="$SOURCE_HEAD" PROVENANCE_SHIM_DATE="$COMMITTER_DATE" \
    "$ROOT_DIR/scripts/verify-flow-deployed-websocket-v0.4.sh" \
      --chain caddy,nginx,api --json --repo-root "$REPO_ROOT" \
      --deployment "$DEPLOYMENT" --evidence-root "$evidence_root")"
  rc=$?
  set -e
  artifact="$evidence_root/deployed-chain-websocket-result.json"
  if [[ $rc -ne 1 ]] || ! jq -e '
      .self_test == {ran: false, exit: null, duration_ms: null} and
      any(.checks[]; .name == "binary_provenance_matches_source_head" and .passed == true)
    ' "$artifact" >/dev/null; then
    echo "FAIL C deployed-gate self-test bypass trace: exit=$rc" >&2
    tail -30 <<<"$output" >&2
    exit 1
  fi
  provenance="$(jq -c .binary_provenance "$artifact")"
  validate_schema_json "$provenance"

  python3 - "$ROOT_DIR/scripts/lib/flow_gate_v0_4_recompute.py" "$REPO_ROOT" "$evidence_root" <<'PYEOF'
import importlib.util
import json
import os
import sys

module_path, repo_root, evidence_root = sys.argv[1:]
spec = importlib.util.spec_from_file_location("flow_gate_v0_4_recompute", module_path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
path = os.path.join(evidence_root, "deployed-chain-websocket-result.json")
with open(path, encoding="utf-8") as handle:
    artifact = json.load(handle)

# Isolate the authoritative self-test requirement from the expected downstream
# descriptor failure while preserving the artifact emitted through the real
# FLOW_BINARY_PROVENANCE_SELF_TESTED=1 bypass path.
artifact["passed"] = True
artifact["gates"]["deployed_chain_websocket_upgrade"] = {
    "status": "passed",
    "reason": "fixture isolates self-test recomputation",
}
with open(path, "w", encoding="utf-8") as handle:
    json.dump(artifact, handle)
recomputed = module.recompute(evidence_root, repo_root)
assert recomputed["hard_gates"]["deployed_chain_websocket_upgrade"] == "failed", recomputed
reason = recomputed["reasons"]["deployed_chain_websocket_upgrade"]
assert "binary provenance self-test did not run" in reason, reason
print("C bypass_recompute gate=failed self_test_ran=false reason=" + reason)
PYEOF
  printf 'C deployed_gate_bypass_trace exit=1 artifact_self_test_ran=false authoritative_recompute=failed duration_ms=%s\n' \
    "$(duration_ms "$started")"
}

if [[ "$REQUESTED_CASE" == all || "$REQUESTED_CASE" == checker ]]; then
  run_checker_cases
fi
if [[ "$REQUESTED_CASE" == all || "$REQUESTED_CASE" == E ]]; then
  run_e_test
fi
if [[ "$REQUESTED_CASE" == all || "$REQUESTED_CASE" == B ]]; then
  run_b_test
fi
if [[ "$REQUESTED_CASE" == all || "$REQUESTED_CASE" == C || "$REQUESTED_CASE" == D ]]; then
  make_container_fixture
fi
if [[ "$REQUESTED_CASE" == all || "$REQUESTED_CASE" == D ]]; then
  run_d_test
fi
if [[ "$REQUESTED_CASE" == all || "$REQUESTED_CASE" == C ]]; then
  run_c_test
fi

printf 'PASS: binary provenance tests case=%s\n' "$REQUESTED_CASE"

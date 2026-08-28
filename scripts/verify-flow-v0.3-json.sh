#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.3 gate verifier.
#
# Contract: /opt/working/sylvode-flow/gates/gate-commands.md ("verify" role) +
# /opt/working/sylvode-flow/decisions/ADR-0014-isolated-apply-host.md section 9.
#
# "verify" never runs product actions. It only checks: JSON Schema shape,
# artifact path/checksum, source HEAD, and the numeric relationships that
# JSON Schema structurally cannot express (dynamic comparison of measured
# values against the frozen 50 ms / 100 ms / 128 MiB isolation ceilings and
# the six v0.3-gate.yaml benchmark budgets, per candidate).
#
# Exit codes: 0 = contract satisfied, 1 = gate/check failed (drift),
# 2 = usage/tool/evidence malformed.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCHEMA_DIR="$ROOT_DIR/docs/schemas"

# Non-evidence contract docs (decisions/, contracts/, security/, testing/,
# gates/) live in the read-only spec repo, never under OpenPR. This root is
# not ambiguous: those directories do not exist under $ROOT_DIR.
CONTRACTS_ROOT="/opt/working/sylvode-flow"

# Evidence root is ambiguous between two frozen docs (see report). We resolve
# it explicitly instead of guessing: gate-commands.md's relative
# "evidence/v0.3/..." paths are joined to this root *after* stripping the
# leading "evidence/v0.3/" segment, so the default here reproduces exactly
# what versions/v0.3-foundation.md:116 pins ("证据固定写入
# /opt/working/sylvode-flow/evidence/v0.3/"). Pass --evidence-root to point
# at a different tree (e.g. ROOT_DIR/evidence/v0.3, the other legal reading).
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.3"
GATE_YAML="/opt/working/sylvode-flow/gates/v0.3-gate.yaml"
REPO_FOR_HEAD="$ROOT_DIR"

# Per-machine ITIMER_PROF / wall-watchdog overshoot calibration (ADR-0014
# "CPU 上限不是精确 50 ms"): SIGPROF delivery is not instantaneous, so a
# correctly-enforced cpu_ceiling case legitimately overshoots 50ms by the
# machine's own measured latency. That tolerance is NOT a constant this
# script may hardcode -- it must come from evidence, produced by
# `cargo run --release -q -p collab-shared --bin isolation-calibrate --
# calibrate --samples 30` and wrapped with machine/run provenance. See
# --help and the report for the wrapper shape this script expects.
CALIBRATION_PATH_OVERRIDE=""

JSON_PATH=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-v0.3-json.sh [OPTIONS] JSON_PATH

Verifies a Sylvode Flow v0.3 evidence file. Read-only: never runs product
actions, never writes evidence.

Two input modes, auto-detected from the file's "schema_version":
  sylvode.flow.gate-result.v1          Full gate verification: validates
                                        gate-result.json against
                                        docs/schemas/sylvode-flow-gate-v1.schema.json,
                                        checks every referenced artifact's
                                        path/sha256, checks source.head against
                                        the repository HEAD, then recurses into
                                        the referenced convergence-result.json
                                        and benchmark-result.json for the
                                        numeric checks below.
  sylvode.flow.convergence-result.v1   Direct numeric verification of one
                                        convergence-result.json (or a
                                        single-candidate runner file with the
                                        same "candidates" shape), without a
                                        surrounding gate-result.json. Useful
                                        for testing runners in isolation and
                                        for scripts/aggregate-flow-convergence-v0.3.sh.
  sylvode.flow.benchmark-result.v1     Direct numeric verification of one
                                        benchmark-result.json, same rationale.

What this script checks that JSON Schema structurally cannot (gate-commands.md
"schema 校验 ≠ 门禁通过"):
  - isolation cases: cpu_ms <= 50, wall_ms <= 100,
    allocated_active_peak_bytes <= 134217728 (ADR-0014 section 9), evaluated
    against expected_oracle/verdict/termination_causes consistency, for
    all five paths (completed, cpu kill, wall kill, memory kill, meter
    unavailable);
  - meter_tampered == true or kind == async_timeout => verdict must be
    "failed";
  - benchmark budgets: each candidate's six p95/single-value measurements
    against gates/v0.3-gate.yaml's budgets block, judged PER CANDIDATE (not
    merged, not "any candidate passes");
  - budget_hash recomputed from gates/v0.3-gate.yaml and compared, not
    trusted from the runner;
  - corpus_seed / case_count / budget_hash equal across candidates;
  - case_count == cases.length;
  - missing/unreadable artifacts are failures, never skips.

Options:
  --evidence-root DIR    Root that "evidence/v0.3/<name>" artifact paths in
                          gate-result.json resolve to after the literal
                          "evidence/v0.3/" prefix is stripped.
                          Default: /opt/working/sylvode-flow/evidence/v0.3
                          (see the "evidence root ambiguity" note above).
  --contracts-root DIR   Root for decisions/, contracts/, security/, testing/
                          artifact paths. Default: /opt/working/sylvode-flow
  --gate-yaml PATH        Path to v0.3-gate.yaml (source of the budgets: block
                          and required_commands:). Default:
                          /opt/working/sylvode-flow/gates/v0.3-gate.yaml
  --schema-dir DIR        Directory holding the sylvode-flow-*.schema.json
                          files. Default: <repo>/docs/schemas
  --repo-root DIR         Repository whose HEAD is compared against
                          source.head. Default: this checkout.
  --calibration PATH      Path to the per-machine isolation-calibration
                          evidence file (see "Calibration evidence" below).
                          Default: resolved the same way as other
                          evidence/v0.3/<name> artifacts, i.e.
                          <evidence-root>/isolation-calibration.json.
  -h, --help              Show this help and exit 0.

Calibration evidence (ADR-0014 "CPU 上限不是精确 50 ms"; this shape is NOT
yet part of any frozen schema -- see the report for why and what is being
proposed to the schema owner):
  SIGPROF/ITIMER_PROF delivery is not instantaneous, so a correctly-enforced
  cpu_ceiling case legitimately overshoots the 50ms ceiling by the machine's
  own measured latency, and that latency is per-machine, not a constant this
  script may hardcode. verify-flow-v0.3-json.sh therefore requires a
  calibration evidence file shaped like:
    {
      "schema_version": "sylvode.flow.isolation-calibration.v1",
      "generated_at": "RFC3339",
      "machine_id": "non-empty string identifying the calibrated machine",
      "command": "cargo run --release -q -p collab-shared --bin isolation-calibrate -- calibrate --samples N",
      "itimer_prof_resolution": { "deviation_from_50ms": { "p95": <ms>, ... } },
      "wall_watchdog_resolution": { "deviation_from_100ms": { "p95": <ms>, ... } }
    }
  ("itimer_prof_resolution" reuses isolation-calibrate's own output key
  verbatim; "wall_watchdog_resolution" is this script's own convention since
  isolation-calibrate does not produce a wall-watchdog number today.)
  A cpu_ceiling case is judged against [50, 50 + itimer_prof_resolution.
  deviation_from_50ms.p95]; a wall_ceiling case against [100, 100 +
  wall_watchdog_resolution.deviation_from_100ms.p95]. If the calibration
  file is missing, unreadable, or missing the specific field a case needs,
  that case FAILS -- missing calibration is never treated as "skip the
  upper-bound check".

Exit codes: 0 contract satisfied, 1 verification failed, 2 usage/tool/malformed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-root)
      EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"
      shift 2
      ;;
    --contracts-root)
      CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"
      shift 2
      ;;
    --gate-yaml)
      GATE_YAML="${2:?--gate-yaml requires a PATH argument}"
      shift 2
      ;;
    --schema-dir)
      SCHEMA_DIR="${2:?--schema-dir requires a DIR argument}"
      shift 2
      ;;
    --repo-root)
      REPO_FOR_HEAD="${2:?--repo-root requires a DIR argument}"
      shift 2
      ;;
    --calibration)
      CALIBRATION_PATH_OVERRIDE="${2:?--calibration requires a PATH argument}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    -*)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
    *)
      if [[ -n "$JSON_PATH" ]]; then
        echo "Unexpected extra argument: $1" >&2
        usage >&2
        exit 2
      fi
      JSON_PATH="$1"
      shift
      ;;
  esac
done

if [[ -z "$JSON_PATH" ]]; then
  echo "Missing required JSON_PATH argument." >&2
  usage >&2
  exit 2
fi

if [[ ! -f "$JSON_PATH" ]]; then
  echo "FAIL: file does not exist: $JSON_PATH" >&2
  exit 2
fi

if ! command -v jq >/dev/null 2>&1; then
  cat >&2 <<'EOF'
FAIL: missing required command: jq
Fix: sudo apt-get install -y jq   (Debian/Ubuntu)
     brew install jq              (macOS)
EOF
  exit 2
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "FAIL: missing required command: python3" >&2
  exit 2
fi

if ! jq empty "$JSON_PATH" >/dev/null 2>&1; then
  echo "FAIL: not valid JSON: $JSON_PATH" >&2
  exit 2
fi

# --- schema validator detection --------------------------------------------
# gate-commands.md requires "schema validation, done with either
# check-jsonschema or python3+jsonschema, and the script must detect which is
# available" (task instructions). We prefer the check-jsonschema CLI (it is
# the tool named first in this repo's own instructions); we fall back to the
# jsonschema Python module; we refuse to silently skip validation.
SCHEMA_TOOL=""
if command -v check-jsonschema >/dev/null 2>&1; then
  SCHEMA_TOOL="check-jsonschema"
elif python3 -c "import jsonschema" >/dev/null 2>&1; then
  SCHEMA_TOOL="python-jsonschema"
else
  cat >&2 <<'EOF'
FAIL: no JSON Schema validator available (need one of the following)
Fix (either one):
  pip install --user check-jsonschema
  pip install --user jsonschema
EOF
  exit 2
fi

validate_schema() {
  local schema_file="$1" data_file="$2" label="$3"
  if [[ ! -f "$schema_file" ]]; then
    echo "FAIL: schema file missing: $schema_file" >&2
    return 1
  fi
  case "$SCHEMA_TOOL" in
    check-jsonschema)
      if check-jsonschema --schemafile "$schema_file" "$data_file" >/tmp/verify-flow-v0.3-schema.$$ 2>&1; then
        echo "PASS: $label matches schema $(basename "$schema_file")"
        rm -f /tmp/verify-flow-v0.3-schema.$$
        return 0
      else
        echo "FAIL: $label does not match schema $(basename "$schema_file")" >&2
        sed 's/^/  /' /tmp/verify-flow-v0.3-schema.$$ >&2
        rm -f /tmp/verify-flow-v0.3-schema.$$
        return 1
      fi
      ;;
    python-jsonschema)
      if python3 - "$schema_file" "$data_file" <<'PYEOF' >/tmp/verify-flow-v0.3-schema.$$ 2>&1
import json, sys
import jsonschema
schema_path, data_path = sys.argv[1], sys.argv[2]
with open(schema_path, encoding="utf-8") as f:
    schema = json.load(f)
with open(data_path, encoding="utf-8") as f:
    data = json.load(f)
validator_cls = jsonschema.validators.validator_for(schema)
validator_cls.check_schema(schema)
validator = validator_cls(schema)
errors = sorted(validator.iter_errors(data), key=lambda e: list(e.path))
if errors:
    for e in errors:
        path = "/".join(str(p) for p in e.path) or "<root>"
        print(f"{path}: {e.message}")
    sys.exit(1)
sys.exit(0)
PYEOF
      then
        echo "PASS: $label matches schema $(basename "$schema_file")"
        rm -f /tmp/verify-flow-v0.3-schema.$$
        return 0
      else
        echo "FAIL: $label does not match schema $(basename "$schema_file")" >&2
        sed 's/^/  /' /tmp/verify-flow-v0.3-schema.$$ >&2
        rm -f /tmp/verify-flow-v0.3-schema.$$
        return 1
      fi
      ;;
  esac
}

FAILURES=0
note_fail() { FAILURES=$((FAILURES + 1)); }

sha256_of() {
  sha256sum "$1" | awk '{print $1}'
}

# Extract the flat "budgets:" mapping from v0.3-gate.yaml as compact JSON,
# without taking a PyYAML dependency: the block is a frozen flat set of
# "key: integer" lines (gates/v0.3-gate.yaml budgets:), so a small awk+jq
# extraction is sufficient and keeps the dependency set to jq/python3/
# check-jsonschema as instructed.
EXPECTED_BUDGET_KEYS="browser_engine_bundle_gzip_bytes_max cold_start_ms_p95_max apply_update_ms_p95_max bootstrap_10k_ops_ms_p95_max bootstrap_100k_ops_ms_p95_max peak_memory_100k_ops_bytes_max"

extract_budgets_json() {
  local yaml_file="$1"
  if [[ ! -f "$yaml_file" ]]; then
    echo "FAIL: --gate-yaml file missing: $yaml_file" >&2
    return 1
  fi
  local budgets_json
  budgets_json="$(awk '
    /^budgets:/ { in_block=1; next }
    in_block && /^[a-zA-Z_]/ { in_block=0 }
    in_block && /^[[:space:]]+[a-zA-Z0-9_]+:[[:space:]]*[0-9]+[[:space:]]*$/ {
      line=$0
      sub(/^[[:space:]]+/,"",line)
      split(line, kv, ":")
      key=kv[1]
      val=kv[2]
      gsub(/[[:space:]]/,"",val)
      printf "%s %s\n", key, val
    }
  ' "$yaml_file" | jq -R -n '[inputs | split(" ") | {(.[0]): (.[1] | tonumber)}] | add // {}')"
  local key_count
  key_count="$(jq 'keys | length' <<<"$budgets_json")"
  if [[ "$key_count" -ne 6 ]]; then
    echo "FAIL: extracted $key_count budgets keys from $yaml_file, expected exactly 6 ($EXPECTED_BUDGET_KEYS); the budgets: block format may have changed" >&2
    return 1
  fi
  for k in $EXPECTED_BUDGET_KEYS; do
    if [[ "$(jq --arg k "$k" 'has($k)' <<<"$budgets_json")" != "true" ]]; then
      echo "FAIL: extracted budgets from $yaml_file is missing expected key: $k" >&2
      return 1
    fi
  done
  printf '%s' "$budgets_json"
}

# Resolve an artifact-relative path (as stored in gate-result.json) to an
# absolute path on disk, per the two-root policy documented in --help.
resolve_artifact_path() {
  local rel="$1"
  case "$rel" in
    evidence/*/*)
      # strip the leading "evidence/<release>/" segment, join to EVIDENCE_ROOT
      printf '%s/%s\n' "$EVIDENCE_ROOT" "${rel#evidence/*/}"
      ;;
    *)
      printf '%s/%s\n' "$CONTRACTS_ROOT" "$rel"
      ;;
  esac
}

if ! BUDGETS_JSON="$(extract_budgets_json "$GATE_YAML")"; then
  exit 2
fi

if [[ -n "$CALIBRATION_PATH_OVERRIDE" ]]; then
  CALIBRATION_ABS="$CALIBRATION_PATH_OVERRIDE"
else
  CALIBRATION_ABS="$(resolve_artifact_path "evidence/v0.3/isolation-calibration.json")"
fi
if [[ ! -f "$CALIBRATION_ABS" ]]; then
  CALIBRATION_ABS="-"
fi

SCHEMA_VERSION="$(jq -r '.schema_version // empty' "$JSON_PATH")"
if [[ -z "$SCHEMA_VERSION" ]]; then
  echo "FAIL: input JSON has no .schema_version; cannot determine verification mode" >&2
  exit 2
fi

# --- mode: direct convergence-result.json / benchmark-result.json ----------
run_numeric_core() {
  local convergence_file="$1" benchmark_file="$2" budgets_json="$3" calibration_file="$4"
  python3 - "$convergence_file" "$benchmark_file" "$budgets_json" "$calibration_file" <<'PYEOF'
import json, sys, hashlib

convergence_path, benchmark_path, budgets_json_arg, calibration_path = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]

CPU_MS_MAX = 50
WALL_MS_MAX = 100
MEM_BYTES_MAX = 134217728
CEILING_ORACLES = {
    "cpu_ceiling", "wall_ceiling", "memory_ceiling",
    "cpu_ceiling_enforcement_failed", "address_space_backstop",
}
# Every non-"completed" oracle -- the five failure paths (cpu/wall/memory
# kill, crash, meter unavailable) -- must leave a recorded cause, not just
# the three ceiling-specific ones the schema's if/then already requires.
TERMINATION_REQUIRED_ORACLES = CEILING_ORACLES | {"crashed", "meter_unavailable"}

errors = []
passes = []

def fail(msg):
    errors.append(msg)

def ok(msg):
    passes.append(msg)

# ---- canonical budget_hash recomputation ----------------------------------
# budgets_from_yaml is extracted from gates/v0.3-gate.yaml's "budgets:" block
# by the calling bash script (extract_budgets_json), not by parsing YAML
# here, to avoid taking a PyYAML dependency.
budgets_from_yaml = None
if budgets_json_arg and budgets_json_arg != "-":
    try:
        budgets_from_yaml = json.loads(budgets_json_arg)
    except json.JSONDecodeError as exc:
        fail(f"[budget_hash] could not parse budgets JSON passed by caller: {exc}")

def canonical_budget_hash(budgets):
    canonical = json.dumps(budgets, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()

expected_budget_hash = canonical_budget_hash(budgets_from_yaml) if budgets_from_yaml else None
if expected_budget_hash:
    ok(f"[budget_hash] recomputed from gates/v0.3-gate.yaml budgets: {expected_budget_hash}")

# ---- per-machine ITIMER_PROF / wall-watchdog calibration (ADR-0014 "CPU
# 上限不是精确 50 ms") ---------------------------------------------------
# SIGPROF delivery is not instantaneous: a correctly-enforced cpu_ceiling
# case legitimately overshoots the 50ms ceiling by the machine's own
# measured ITIMER_PROF latency, produced by `cargo run --release -q -p
# collab-shared --bin isolation-calibrate -- calibrate --samples N`. That
# tolerance is per-machine and must come from evidence, never a constant
# hardcoded here. Missing/invalid calibration data is a FAILURE for any
# case that needs it, never a silently-skipped check -- an
# unbounded-overshoot cpu_ceiling case (e.g. cpu_ms=5000 "passing" because
# nothing ever checked an upper bound) is exactly the gap this closes.
cpu_overshoot_p95 = None
wall_overshoot_p95 = None
calibration_load_error = None
if calibration_path and calibration_path != "-":
    try:
        with open(calibration_path, encoding="utf-8") as f:
            calibration_doc = json.load(f)
        if calibration_doc.get("schema_version") != "sylvode.flow.isolation-calibration.v1":
            calibration_load_error = (
                f"{calibration_path}: unexpected schema_version "
                f"{calibration_doc.get('schema_version')!r} (expected "
                f"'sylvode.flow.isolation-calibration.v1')"
            )
        elif not calibration_doc.get("machine_id"):
            calibration_load_error = f"{calibration_path}: missing non-empty machine_id"
        else:
            cpu_val = calibration_doc.get("itimer_prof_resolution", {}).get("deviation_from_50ms", {}).get("p95")
            wall_val = calibration_doc.get("wall_watchdog_resolution", {}).get("deviation_from_100ms", {}).get("p95")
            if isinstance(cpu_val, (int, float)) and cpu_val >= 0:
                cpu_overshoot_p95 = cpu_val
            if isinstance(wall_val, (int, float)) and wall_val >= 0:
                wall_overshoot_p95 = wall_val
    except FileNotFoundError:
        calibration_load_error = f"calibration file not found: {calibration_path}"
    except json.JSONDecodeError as exc:
        calibration_load_error = f"calibration file not valid JSON: {calibration_path}: {exc}"
else:
    calibration_load_error = (
        "no calibration evidence resolved (expected "
        "evidence/v0.3/isolation-calibration.json; pass --calibration to override)"
    )

if cpu_overshoot_p95 is not None:
    ok(f"[calibration] cpu ITIMER_PROF p95 overshoot = {cpu_overshoot_p95} ms "
       f"(from {calibration_path})")
cpu_overshoot_missing_reason = calibration_load_error or (
    f"{calibration_path}: no itimer_prof_resolution.deviation_from_50ms.p95"
)
if wall_overshoot_p95 is not None:
    ok(f"[calibration] wall watchdog p95 overshoot = {wall_overshoot_p95} ms "
       f"(from {calibration_path})")
wall_overshoot_missing_reason = calibration_load_error or (
    f"{calibration_path}: no wall_watchdog_resolution.deviation_from_100ms.p95 "
    f"(isolation-calibrate does not produce this yet)"
)

# ---- isolation case checks -------------------------------------------------
def verify_isolation_case(case, candidate, side):
    prefix = f"[isolation {candidate}/{side}/{case.get('fixture_id', '?')}]"
    verdict = case.get("verdict")
    expected = case.get("expected_oracle")
    kind = case.get("kind")
    cpu_ms = case.get("cpu_ms")
    wall_ms = case.get("wall_ms")
    mem_active = case.get("allocated_active_peak_bytes")
    mem_attempted = case.get("allocated_attempted_peak_bytes")
    cpu_meter = case.get("cpu_meter")
    mem_meter = case.get("memory_meter")
    tampered = case.get("meter_tampered")
    termination_causes = case.get("termination_causes", [])
    causes = [c.get("cause") for c in termination_causes]
    partial_unchanged = case.get("partial_state_unchanged")

    is_native = cpu_meter == "native_itimer_prof"

    if kind == "async_timeout":
        fail(f"{prefix} kind=async_timeout: plain async timeout can never prove a CPU/memory "
             f"ceiling (limits-v1.md); this case can never be a valid isolation proof")
        if verdict != "failed":
            fail(f"{prefix} kind=async_timeout but verdict={verdict!r} (must be failed)")

    if tampered is True and verdict != "failed":
        fail(f"{prefix} meter_tampered=true but verdict={verdict!r} (must be failed; "
             f"ADR-0014 1.1 step 3)")

    if cpu_meter == "unavailable" or mem_meter == "unavailable":
        if verdict != "failed":
            fail(f"{prefix} a meter is unavailable (cpu_meter={cpu_meter!r}, "
                 f"memory_meter={mem_meter!r}) so ceiling enforcement cannot be proven; "
                 f"verdict must be failed, got {verdict!r}")

    if expected == "completed":
        if verdict != "passed":
            fail(f"{prefix} expected_oracle=completed but verdict={verdict!r}")
        if causes:
            fail(f"{prefix} expected_oracle=completed but termination_causes is non-empty: "
                 f"{causes} (a normal completion emits no termination event)")
        if is_native:
            if cpu_ms is None or cpu_ms > CPU_MS_MAX:
                fail(f"{prefix} completed case cpu_ms={cpu_ms} exceeds the "
                     f"{CPU_MS_MAX} ms isolated CPU ceiling (limits-v1.md "
                     f"decode_apply_cpu_ms_max)")
            else:
                ok(f"{prefix} cpu_ms={cpu_ms} <= {CPU_MS_MAX}")
            if mem_active is None or mem_active > MEM_BYTES_MAX:
                fail(f"{prefix} completed case allocated_active_peak_bytes={mem_active} "
                     f"exceeds the {MEM_BYTES_MAX} byte isolated memory ceiling "
                     f"(limits-v1.md isolated_apply_memory_bytes_max)")
            else:
                ok(f"{prefix} allocated_active_peak_bytes={mem_active} <= {MEM_BYTES_MAX}")
        if wall_ms is None or wall_ms > WALL_MS_MAX:
            fail(f"{prefix} completed case wall_ms={wall_ms} exceeds the "
                 f"{WALL_MS_MAX} ms wall ceiling (limits-v1.md decode_apply_wall_ms_max)")
        else:
            ok(f"{prefix} wall_ms={wall_ms} <= {WALL_MS_MAX}")
        return

    # every non-"completed" expected_oracle implies the isolated task was
    # terminated by an enforced ceiling (or crashed); the case must be failed.
    if verdict != "failed":
        fail(f"{prefix} expected_oracle={expected!r} implies enforced termination, "
             f"but verdict={verdict!r} (this is exactly the fake-green pattern "
             f"ADR-0014 section 9 exists to close)")
    if partial_unchanged is not True:
        fail(f"{prefix} verdict=failed but partial_state_unchanged={partial_unchanged!r} "
             f"(limits-v1.md: a discarded isolated instance must never write partial "
             f"state back to canonical)")

    if expected in TERMINATION_REQUIRED_ORACLES:
        if not causes:
            fail(f"{prefix} expected_oracle={expected} requires a recorded "
                 f"termination cause but termination_causes is empty")
        elif expected not in causes:
            fail(f"{prefix} expected_oracle={expected} not present in "
                 f"termination_causes={causes}")
        else:
            ok(f"{prefix} termination_causes contains {expected}")

    if expected == "cpu_ceiling":
        if is_native and (cpu_ms is None or cpu_ms < CPU_MS_MAX):
            fail(f"{prefix} expected cpu_ceiling but cpu_ms={cpu_ms} never reached "
                 f"{CPU_MS_MAX} ms")
        if is_native and cpu_ms is not None and cpu_ms >= CPU_MS_MAX:
            # Lower bound holds; now bound the overshoot. ITIMER_PROF
            # delivery is not instantaneous (ADR-0014 "CPU 上限不是精确
            # 50 ms"), so a correctly-enforced case legitimately overshoots
            # 50ms -- but only by the machine's own calibrated latency, not
            # by an arbitrary amount. Without calibration data this cannot
            # be judged, so it is a FAILURE, not a skipped check: an
            # unbounded cpu_ceiling case (cpu_ms=5000 "enforced" 5 seconds
            # late) must not read as a legitimate ITIMER delay.
            if cpu_overshoot_p95 is None:
                fail(f"{prefix} expected_oracle=cpu_ceiling requires calibrated "
                     f"ITIMER_PROF overshoot data to bound cpu_ms (ADR-0014 'CPU "
                     f"上限不是精确 50 ms'), none available: {cpu_overshoot_missing_reason}")
            else:
                cpu_ceiling_upper = CPU_MS_MAX + cpu_overshoot_p95
                if cpu_ms > cpu_ceiling_upper:
                    fail(f"{prefix} expected cpu_ceiling but cpu_ms={cpu_ms} exceeds "
                         f"the calibrated upper bound {CPU_MS_MAX} + "
                         f"{cpu_overshoot_p95} (p95 ITIMER_PROF overshoot) = "
                         f"{cpu_ceiling_upper}: enforcement was not actually prompt, "
                         f"the machine's own measured ITIMER delivery latency does "
                         f"not explain this overshoot")
                else:
                    ok(f"{prefix} cpu_ms={cpu_ms} within calibrated cpu_ceiling "
                       f"band [{CPU_MS_MAX}, {cpu_ceiling_upper}]")
    elif expected == "wall_ceiling":
        if wall_ms is None or wall_ms < WALL_MS_MAX:
            fail(f"{prefix} expected wall_ceiling but wall_ms={wall_ms} never reached "
                 f"{WALL_MS_MAX} ms")
        if is_native and cpu_ms is not None and cpu_ms >= CPU_MS_MAX:
            fail(f"{prefix} expected wall_ceiling but cpu_ms={cpu_ms} >= {CPU_MS_MAX} ms: "
                 f"per ADR-0014 1.1 this must be classified "
                 f"cpu_ceiling_enforcement_failed, not wall_ceiling")
        if wall_ms is not None and wall_ms >= WALL_MS_MAX:
            # Same reasoning as cpu_ceiling above: the wall watchdog also
            # has a real (if smaller) delivery/scheduling latency, so an
            # upper bound is required, not left open-ended. No calibrated
            # wall-watchdog number exists yet (isolation-calibrate does not
            # produce one today) -- that is a FAILURE, not an exemption.
            if wall_overshoot_p95 is None:
                fail(f"{prefix} expected_oracle=wall_ceiling requires calibrated "
                     f"wall-watchdog overshoot data to bound wall_ms, none "
                     f"available: {wall_overshoot_missing_reason}")
            else:
                wall_ceiling_upper = WALL_MS_MAX + wall_overshoot_p95
                if wall_ms > wall_ceiling_upper:
                    fail(f"{prefix} expected wall_ceiling but wall_ms={wall_ms} "
                         f"exceeds the calibrated upper bound {WALL_MS_MAX} + "
                         f"{wall_overshoot_p95} = {wall_ceiling_upper}: the "
                         f"watchdog was not actually prompt")
                else:
                    ok(f"{prefix} wall_ms={wall_ms} within calibrated wall_ceiling "
                       f"band [{WALL_MS_MAX}, {wall_ceiling_upper}]")
    elif expected == "cpu_ceiling_enforcement_failed":
        if wall_ms is None or wall_ms < WALL_MS_MAX:
            fail(f"{prefix} expected cpu_ceiling_enforcement_failed but wall_ms={wall_ms} "
                 f"never reached the {WALL_MS_MAX} ms watchdog")
        if is_native and (cpu_ms is None or cpu_ms < CPU_MS_MAX):
            fail(f"{prefix} expected cpu_ceiling_enforcement_failed but cpu_ms={cpu_ms} "
                 f"< {CPU_MS_MAX} ms (SIGPROF should have already fired)")
    elif expected == "memory_ceiling":
        if is_native:
            if mem_attempted is None or mem_attempted <= MEM_BYTES_MAX:
                fail(f"{prefix} expected memory_ceiling but "
                     f"allocated_attempted_peak_bytes={mem_attempted} never exceeded "
                     f"{MEM_BYTES_MAX}")
            if mem_active is not None and mem_active > MEM_BYTES_MAX:
                fail(f"{prefix} expected memory_ceiling but "
                     f"allocated_active_peak_bytes={mem_active} exceeds the ceiling: "
                     f"the allocator must reject before committing the peak "
                     f"(ADR-0014 section 3)")

def verify_convergence(doc):
    if doc.get("schema_version") != "sylvode.flow.convergence-result.v1":
        fail(f"[convergence] unexpected schema_version: {doc.get('schema_version')!r}")
        return

    candidates = doc.get("candidates", {})
    seeds, counts, hashes = {}, {}, {}
    for cname, cres in candidates.items():
        seeds[cname] = cres.get("corpus_seed")
        counts[cname] = cres.get("case_count")
        hashes[cname] = cres.get("budget_hash")

        cases = cres.get("cases", [])
        if counts[cname] != len(cases):
            fail(f"[convergence {cname}] case_count={counts[cname]} != "
                 f"len(cases)={len(cases)}")
        else:
            ok(f"[convergence {cname}] case_count matches cases.length ({counts[cname]})")

        if expected_budget_hash and hashes[cname] != expected_budget_hash:
            fail(f"[convergence {cname}] budget_hash={hashes[cname]!r} does not match "
                 f"recomputed canonical hash {expected_budget_hash!r} of "
                 f"gates/v0.3-gate.yaml budgets:")
        elif expected_budget_hash:
            ok(f"[convergence {cname}] budget_hash matches recomputed canonical hash")

        isolation = cres.get("isolation", {})
        for side in ("rust", "web"):
            side_res = isolation.get(side)
            if not side_res:
                fail(f"[convergence {cname}] missing isolation.{side}")
                continue
            for case in side_res.get("cases", []):
                verify_isolation_case(case, cname, side)
            status = side_res.get("status")
            case_statuses = [c.get("verdict") for c in side_res.get("cases", [])]
            if status == "passed" and "failed" in [
                c.get("verdict") for c in side_res.get("cases", [])
                if c.get("expected_oracle") != "completed"
            ]:
                pass  # a side legitimately has a mix; per-case verdict already checked above
            if not case_statuses:
                fail(f"[convergence {cname}] isolation.{side} has zero cases "
                     f"(five paths -- completed/cpu/wall/memory/meter-unavailable -- "
                     f"cannot be judged with no cases)")

    # gate-commands.md ("schema 校验 ≠ 门禁通过" item 1, and the v0.3 section:
    # "两候选 corpus seed/case count/budget hash 不同即 verify 失败"): the two
    # candidates must run the SAME corpus_seed/case_count/budget_hash so the
    # comparison is apples-to-apples -- equality is required here, not
    # difference. Schema can only prove each value exists per-candidate; it
    # cannot compare the two candidates against each other.
    if len(candidates) == 2:
        cnames = list(candidates.keys())
        a, b = cnames[0], cnames[1]
        if seeds[a] != seeds[b]:
            fail(f"[convergence] corpus_seed differs across candidates: "
                 f"{a}={seeds[a]!r} {b}={seeds[b]!r} (both candidates must run the "
                 f"same corpus for the comparison to be apples-to-apples)")
        else:
            ok(f"[convergence] corpus_seed equal across candidates ({seeds[a]!r})")
        if counts[a] != counts[b]:
            fail(f"[convergence] case_count differs across candidates: "
                 f"{a}={counts[a]} {b}={counts[b]}")
        else:
            ok(f"[convergence] case_count equal across candidates ({counts[a]})")
        if hashes[a] != hashes[b]:
            fail(f"[convergence] budget_hash differs across candidates: "
                 f"{a}={hashes[a]} {b}={hashes[b]}")
        else:
            ok("[convergence] budget_hash equal across candidates")

    for gate_name in (
        "isolated_apply_not_async_timeout",
        "rust_wasm_roundtrip",
        "snapshot_tail_rebuild_hash",
        "concurrent_tree_move_invariants",
        "offline_replay_no_accepted_loss",
        "corrupt_duplicate_out_of_order_limits",
    ):
        for cname, cres in candidates.items():
            hard_gates = cres.get("hard_gates", {})
            val = hard_gates.get(gate_name)
            if val is None:
                fail(f"[convergence {cname}] hard_gates.{gate_name} missing")

def stat_value(metric, key):
    if key == "single":
        return metric
    return metric.get(key)

BUDGET_METRIC_MAP = {
    "browser_engine_bundle_gzip_bytes_max": ("browser_engine_bundle", "total_gzip_bytes", "single"),
    "cold_start_ms_p95_max": ("cold_start_ms", None, "p95"),
    "apply_update_ms_p95_max": ("apply_update_ms", None, "p95"),
    "bootstrap_10k_ops_ms_p95_max": ("bootstrap_10k_ops_ms", None, "p95"),
    "bootstrap_100k_ops_ms_p95_max": ("bootstrap_100k_ops_ms", None, "p95"),
    "peak_memory_100k_ops_bytes_max": ("peak_memory_100k_ops_bytes", None, "single"),
}

def verify_benchmark(doc):
    if doc.get("schema_version") != "sylvode.flow.benchmark-result.v1":
        fail(f"[benchmark] unexpected schema_version: {doc.get('schema_version')!r}")
        return

    budgets = doc.get("budgets", {})
    if budgets_from_yaml:
        for key, yaml_val in budgets_from_yaml.items():
            doc_val = budgets.get(key)
            if doc_val != yaml_val:
                fail(f"[benchmark] budgets.{key}={doc_val!r} does not match "
                     f"gates/v0.3-gate.yaml budgets.{key}={yaml_val!r}")

    candidates = doc.get("candidates", {})
    per_candidate_verdict = {}
    for cname, cres in candidates.items():
        measurements = cres.get("measurements", {})
        budget_checks = cres.get("budget_checks", {})
        recomputed = {}
        for budget_key, (measure_key, nested_key, stat) in BUDGET_METRIC_MAP.items():
            budget_max = budgets.get(budget_key)
            metric = measurements.get(measure_key)
            if metric is None or budget_max is None:
                fail(f"[benchmark {cname}] missing measurement/budget for {budget_key}")
                recomputed[budget_key] = "failed"
                continue
            if nested_key:
                metric = metric.get(nested_key)
            value = stat_value(metric, stat)
            if value is None:
                fail(f"[benchmark {cname}] {budget_key}: could not read {stat} "
                     f"statistic")
                recomputed[budget_key] = "failed"
                continue
            computed_status = "passed" if value <= budget_max else "failed"
            recomputed[budget_key] = computed_status
            reported_status = budget_checks.get(budget_key)
            if reported_status != computed_status:
                fail(f"[benchmark {cname}] {budget_key}: reported "
                     f"budget_checks.{budget_key}={reported_status!r} but recomputed "
                     f"from measurements ({stat}={value}, max={budget_max}) is "
                     f"{computed_status!r}")
            else:
                ok(f"[benchmark {cname}] {budget_key} {stat}={value} vs max={budget_max} "
                   f"-> {computed_status}")

        candidate_all_passed = all(v == "passed" for v in recomputed.values())
        expected_overall = "passed" if candidate_all_passed else "failed"
        per_candidate_verdict[cname] = expected_overall
        reported_overall = cres.get("benchmark_budgets_met")
        if reported_overall != expected_overall:
            fail(f"[benchmark {cname}] benchmark_budgets_met reported "
                 f"{reported_overall!r} but recomputed (per-candidate, not merged "
                 f"across candidates) is {expected_overall!r}")
        else:
            ok(f"[benchmark {cname}] benchmark_budgets_met={expected_overall} "
               f"(judged for this candidate alone)")

        hostile = cres.get("hostile_input_safety", {})
        cpu_ms = hostile.get("decode_apply_cpu_ms")
        wall_ms = hostile.get("decode_apply_wall_ms")
        mem_bytes = hostile.get("isolated_apply_memory_bytes")
        expected_hostile_status = "passed"
        if cpu_ms is None or cpu_ms > CPU_MS_MAX:
            expected_hostile_status = "failed"
        if wall_ms is None or wall_ms > WALL_MS_MAX:
            expected_hostile_status = "failed"
        if mem_bytes is None or mem_bytes > MEM_BYTES_MAX:
            expected_hostile_status = "failed"
        if hostile.get("status") != expected_hostile_status:
            fail(f"[benchmark {cname}] hostile_input_safety.status="
                 f"{hostile.get('status')!r} but recomputed from cpu_ms={cpu_ms}, "
                 f"wall_ms={wall_ms}, isolated_apply_memory_bytes={mem_bytes} against "
                 f"50/100/{MEM_BYTES_MAX} is {expected_hostile_status!r}: "
                 f"benchmark-spec.md forbids letting a p95 pass mask a single safety "
                 f"ceiling failure")
        else:
            ok(f"[benchmark {cname}] hostile_input_safety.status="
               f"{expected_hostile_status} matches recomputed ceilings")

    if len(per_candidate_verdict) >= 2:
        ok(f"[benchmark] per-candidate verdicts (not merged): {per_candidate_verdict}")

if convergence_path and convergence_path != "-":
    try:
        with open(convergence_path, encoding="utf-8") as f:
            convergence_doc = json.load(f)
        verify_convergence(convergence_doc)
    except FileNotFoundError:
        fail(f"[convergence] required artifact missing: {convergence_path}")
    except json.JSONDecodeError as exc:
        fail(f"[convergence] not valid JSON: {convergence_path}: {exc}")

if benchmark_path and benchmark_path != "-":
    try:
        with open(benchmark_path, encoding="utf-8") as f:
            benchmark_doc = json.load(f)
        verify_benchmark(benchmark_doc)
    except FileNotFoundError:
        fail(f"[benchmark] required artifact missing: {benchmark_path}")
    except json.JSONDecodeError as exc:
        fail(f"[benchmark] not valid JSON: {benchmark_path}: {exc}")

for p in passes:
    print(f"PASS: {p}")
for e in errors:
    print(f"FAIL: {e}", file=sys.stderr)

print(f"__SUMMARY__ passed={len(passes)} failed={len(errors)}")
sys.exit(1 if errors else 0)
PYEOF
}

case "$SCHEMA_VERSION" in
  sylvode.flow.gate-result.v1)
    SCHEMA_FILE="$SCHEMA_DIR/sylvode-flow-gate-v1.schema.json"
    if ! validate_schema "$SCHEMA_FILE" "$JSON_PATH" "gate-result.json"; then
      note_fail
    fi

    # --- source HEAD / dirty -------------------------------------------------
    RECORDED_HEAD="$(jq -r '.source.head // empty' "$JSON_PATH")"
    # NOTE: deliberately not "// empty" here -- jq's // treats a real JSON
    # `false` as falsy, which would silently turn a legitimate
    # source.dirty=false into an empty string and misreport it as missing.
    RECORDED_DIRTY="$(jq -r 'if has("source") and (.source | has("dirty")) then (.source.dirty | tostring) else "" end' "$JSON_PATH")"
    if command -v git >/dev/null 2>&1 && git -C "$REPO_FOR_HEAD" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
      ACTUAL_HEAD="$(git -C "$REPO_FOR_HEAD" rev-parse HEAD)"
      if [[ -n "$(git -C "$REPO_FOR_HEAD" status --porcelain)" ]]; then
        ACTUAL_DIRTY="true"
      else
        ACTUAL_DIRTY="false"
      fi
      if [[ "$RECORDED_HEAD" != "$ACTUAL_HEAD" ]]; then
        echo "FAIL: source.head=$RECORDED_HEAD does not match actual repository HEAD=$ACTUAL_HEAD" >&2
        note_fail
      else
        echo "PASS: source.head matches actual repository HEAD ($ACTUAL_HEAD)"
      fi
      if [[ "$RECORDED_DIRTY" != "true" && "$RECORDED_DIRTY" != "false" ]]; then
        echo "FAIL: source.dirty is not a boolean: $RECORDED_DIRTY" >&2
        note_fail
      elif [[ "$RECORDED_DIRTY" != "$ACTUAL_DIRTY" ]]; then
        echo "FAIL: source.dirty=$RECORDED_DIRTY does not match actual repository state (git status --porcelain is $([ "$ACTUAL_DIRTY" == "true" ] && echo "non-empty" || echo "empty"), so dirty=$ACTUAL_DIRTY)" >&2
        note_fail
      elif [[ "$RECORDED_DIRTY" == "true" ]]; then
        echo "FAIL: source.dirty=true; strict gate must fail on a dirty tree (gate-commands.md)" >&2
        note_fail
      else
        echo "PASS: source.dirty=false and matches actual repository state"
      fi
    else
      echo "FAIL: cannot determine actual repository HEAD (not a git work tree at $REPO_FOR_HEAD)" >&2
      note_fail
    fi

    # --- artifact path/checksum verification ---------------------------------
    ARTIFACT_KEYS="$(jq -r '.artifacts | keys[]' "$JSON_PATH")"
    while IFS= read -r key; do
      [[ -z "$key" ]] && continue
      rel_path="$(jq -r --arg k "$key" '.artifacts[$k].path' "$JSON_PATH")"
      expected_sha="$(jq -r --arg k "$key" '.artifacts[$k].sha256' "$JSON_PATH")"
      abs_path="$(resolve_artifact_path "$rel_path")"
      if [[ ! -f "$abs_path" ]]; then
        echo "FAIL: artifact '$key' missing on disk: $rel_path (resolved: $abs_path)" >&2
        note_fail
        continue
      fi
      if [[ "$key" == "gate_result" ]]; then
        # This entry is self-referential: it describes the gate-result.json
        # file that IS the JSON_PATH argument. Its sha256 is inherently a
        # fixed point (the hash of a file that includes that same hash), so
        # a byte-for-byte report-time snapshot can never equal a live
        # recomputation of "this file, right now, including this field".
        # We only check that it points at the file actually being verified;
        # a real report-time consumer diffing the last two report runs (not
        # implemented here) is a better check for this one field than
        # self-hashing ever can be.
        if [[ "$abs_path" -ef "$JSON_PATH" ]]; then
          echo "PASS: artifact 'gate_result' path resolves to the file being verified ($rel_path; sha256 self-reference not checked, see script comment)"
        else
          echo "FAIL: artifact 'gate_result' path ($rel_path -> $abs_path) does not resolve to the JSON_PATH being verified ($JSON_PATH)" >&2
          note_fail
        fi
        continue
      fi
      actual_sha="$(sha256_of "$abs_path")"
      if [[ "$actual_sha" != "$expected_sha" ]]; then
        echo "FAIL: artifact '$key' sha256 mismatch: recorded=$expected_sha actual=$actual_sha ($abs_path)" >&2
        note_fail
      else
        echo "PASS: artifact '$key' checksum matches ($rel_path)"
      fi
    done <<<"$ARTIFACT_KEYS"

    # --- checks[] evidence/checksum verification ------------------------------
    CHECK_COUNT="$(jq '.checks | length' "$JSON_PATH")"
    for ((i = 0; i < CHECK_COUNT; i++)); do
      check_id="$(jq -r ".checks[$i].id" "$JSON_PATH")"
      check_status="$(jq -r ".checks[$i].status" "$JSON_PATH")"
      check_evidence="$(jq -r ".checks[$i].evidence" "$JSON_PATH")"
      check_sha="$(jq -r ".checks[$i].sha256" "$JSON_PATH")"
      abs_path="$(resolve_artifact_path "$check_evidence")"
      if [[ ! -f "$abs_path" ]]; then
        echo "FAIL: checks[$i] ('$check_id') evidence missing on disk: $check_evidence" >&2
        note_fail
        continue
      fi
      actual_sha="$(sha256_of "$abs_path")"
      if [[ "$actual_sha" != "$check_sha" ]]; then
        echo "FAIL: checks[$i] ('$check_id') sha256 mismatch: recorded=$check_sha actual=$actual_sha" >&2
        note_fail
      else
        echo "PASS: checks[$i] ('$check_id') status=$check_status evidence checksum matches"
      fi
    done

    # --- required_commands[].evidence/checksum verification -------------------
    RC_KEYS="$(jq -r '.required_commands | keys[]' "$JSON_PATH")"
    while IFS= read -r rc_key; do
      [[ -z "$rc_key" ]] && continue
      rc_evidence="$(jq -r --arg k "$rc_key" '.required_commands[$k].evidence' "$JSON_PATH")"
      rc_sha="$(jq -r --arg k "$rc_key" '.required_commands[$k].sha256' "$JSON_PATH")"
      rc_status="$(jq -r --arg k "$rc_key" '.required_commands[$k].status' "$JSON_PATH")"
      abs_path="$(resolve_artifact_path "$rc_evidence")"
      if [[ ! -f "$abs_path" ]]; then
        echo "FAIL: required_commands.$rc_key evidence missing on disk: $rc_evidence" >&2
        note_fail
        continue
      fi
      if [[ "$abs_path" -ef "$JSON_PATH" ]]; then
        # Same self-reference circularity as the "gate_result" artifact
        # entry: required_commands.{report,verify,gate,manual_signoff} for
        # this exact gate-result.json legitimately point back at the file
        # being verified right now.
        echo "PASS: required_commands.$rc_key status=$rc_status evidence self-references the file being verified (sha256 self-reference not checked)"
        continue
      fi
      actual_sha="$(sha256_of "$abs_path")"
      if [[ "$actual_sha" != "$rc_sha" ]]; then
        echo "FAIL: required_commands.$rc_key sha256 mismatch: recorded=$rc_sha actual=$actual_sha" >&2
        note_fail
      else
        echo "PASS: required_commands.$rc_key status=$rc_status evidence checksum matches"
      fi
    done <<<"$RC_KEYS"

    # --- deep numeric verification of the referenced convergence/benchmark ---
    CONVERGENCE_REL="$(jq -r '.artifacts.convergence_result.path // empty' "$JSON_PATH")"
    BENCHMARK_REL="$(jq -r '.artifacts.benchmark_result.path // empty' "$JSON_PATH")"
    CONVERGENCE_ABS="-"
    BENCHMARK_ABS="-"
    if [[ -n "$CONVERGENCE_REL" ]]; then
      CONVERGENCE_ABS="$(resolve_artifact_path "$CONVERGENCE_REL")"
      if [[ -f "$CONVERGENCE_ABS" ]]; then
        validate_schema "$SCHEMA_DIR/sylvode-flow-convergence-result-v1.schema.json" "$CONVERGENCE_ABS" "convergence-result.json" || note_fail
      else
        echo "FAIL: required artifact missing: convergence_result ($CONVERGENCE_REL)" >&2
        note_fail
        CONVERGENCE_ABS="-"
      fi
    else
      echo "FAIL: gate-result.json has no artifacts.convergence_result.path" >&2
      note_fail
    fi
    if [[ -n "$BENCHMARK_REL" ]]; then
      BENCHMARK_ABS="$(resolve_artifact_path "$BENCHMARK_REL")"
      if [[ -f "$BENCHMARK_ABS" ]]; then
        validate_schema "$SCHEMA_DIR/sylvode-flow-benchmark-result-v1.schema.json" "$BENCHMARK_ABS" "benchmark-result.json" || note_fail
      else
        echo "FAIL: required artifact missing: benchmark_result ($BENCHMARK_REL)" >&2
        note_fail
        BENCHMARK_ABS="-"
      fi
    else
      echo "FAIL: gate-result.json has no artifacts.benchmark_result.path" >&2
      note_fail
    fi

    if [[ "$CONVERGENCE_ABS" != "-" || "$BENCHMARK_ABS" != "-" ]]; then
      set +e
      NUMERIC_OUTPUT="$(run_numeric_core "$CONVERGENCE_ABS" "$BENCHMARK_ABS" "$BUDGETS_JSON" "$CALIBRATION_ABS")"
      NUMERIC_EXIT=$?
      set -e
      echo "$NUMERIC_OUTPUT" | grep -v '^__SUMMARY__' || true
      if [[ $NUMERIC_EXIT -ne 0 ]]; then
        note_fail
      fi
    fi

    # --- selected_candidate vs hard_gates.benchmark_budgets_met cross-check --
    SELECTED_CANDIDATE="$(jq -r '.selected_candidate // empty' "$JSON_PATH")"
    if [[ -n "$SELECTED_CANDIDATE" && "$BENCHMARK_ABS" != "-" ]]; then
      CANDIDATE_VERDICT="$(jq -r --arg c "$SELECTED_CANDIDATE" '.candidates[$c].benchmark_budgets_met // empty' "$BENCHMARK_ABS")"
      GATE_HARD_VERDICT="$(jq -r '.hard_gates.benchmark_budgets_met // empty' "$JSON_PATH")"
      if [[ -n "$CANDIDATE_VERDICT" && "$CANDIDATE_VERDICT" != "$GATE_HARD_VERDICT" ]]; then
        echo "FAIL: hard_gates.benchmark_budgets_met=$GATE_HARD_VERDICT but selected_candidate=$SELECTED_CANDIDATE benchmark result is $CANDIDATE_VERDICT (candidate_rule judges the selected candidate alone)" >&2
        note_fail
      elif [[ -n "$CANDIDATE_VERDICT" ]]; then
        echo "PASS: hard_gates.benchmark_budgets_met matches selected_candidate ($SELECTED_CANDIDATE) result"
      fi
    fi
    ;;

  sylvode.flow.convergence-result.v1)
    validate_schema "$SCHEMA_DIR/sylvode-flow-convergence-result-v1.schema.json" "$JSON_PATH" "convergence-result.json" || note_fail
    set +e
    NUMERIC_OUTPUT="$(run_numeric_core "$JSON_PATH" "-" "$BUDGETS_JSON" "$CALIBRATION_ABS")"
    NUMERIC_EXIT=$?
    set -e
    echo "$NUMERIC_OUTPUT" | grep -v '^__SUMMARY__' || true
    [[ $NUMERIC_EXIT -ne 0 ]] && note_fail
    ;;

  sylvode.flow.benchmark-result.v1)
    validate_schema "$SCHEMA_DIR/sylvode-flow-benchmark-result-v1.schema.json" "$JSON_PATH" "benchmark-result.json" || note_fail
    set +e
    NUMERIC_OUTPUT="$(run_numeric_core "-" "$JSON_PATH" "$BUDGETS_JSON" "$CALIBRATION_ABS")"
    NUMERIC_EXIT=$?
    set -e
    echo "$NUMERIC_OUTPUT" | grep -v '^__SUMMARY__' || true
    [[ $NUMERIC_EXIT -ne 0 ]] && note_fail
    ;;

  *)
    echo "FAIL: unsupported schema_version for verify-flow-v0.3-json.sh: $SCHEMA_VERSION" >&2
    exit 2
    ;;
esac

if [[ $FAILURES -gt 0 ]]; then
  echo "RESULT: FAIL ($FAILURES check group(s) failed)" >&2
  exit 1
fi

echo "RESULT: PASS"
exit 0

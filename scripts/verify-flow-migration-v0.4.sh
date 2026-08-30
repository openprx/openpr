#!/usr/bin/env bash
set -euo pipefail

# Sylvode Flow v0.4 migration forward/rollback strategy verifier.
#
# Contract: /opt/working/sylvode-flow/versions/v0.4-flow-alpha.md "升级与回滚"
# ("migration 只新增表。回滚先关闭 flag、拒绝新 REST/WebSocket 写入、等待
# active transaction 与 outbound broadcast drain，再回退应用；数据表保留以便
# 恢复，不自动 drop。回滚前后都运行 document head/projection integrity
# check.") plus gates/v0.4-gate.yaml's `artifacts.migration` note about the
# filename ledger, and this repository's own migration discipline
# (`apps/api/src/main.rs`: MIGRATIONS / MIGRATION_PROBES must stay in
# lockstep, or an applied migration is re-run on every boot and the API
# restart-loops -- the CHANGELOG records that exact incident).
#
# Nothing here is taken from a self-report. The static half re-derives the
# MIGRATIONS/MIGRATION_PROBES relationship from apps/api/src/main.rs by
# parsing it; the dynamic half boots the shipped `api` binary against a
# brand-new empty database and observes the real ledger.
#
# What is asserted
# ----------------
# FORWARD (static):
#   S1  MIGRATIONS and MIGRATION_PROBES list the same files in the same
#       order (recomputed here, not delegated to the Rust unit test).
#   S2  the set of files on disk equals the set MIGRATIONS embeds -- an
#       orphan .sql file is never applied, and an embedded name with no
#       file cannot compile.
#   S3  the v0.4 migration named by --migration is present in both lists
#       and `include_str!`-embedded under exactly that filename (renaming
#       it makes the already-applied ledger row look unapplied).
#   S4  every migration that DROPs a table has a probe that does not
#       resolve to the dropped relation, and no EARLIER migration's
#       `Relation(t)` probe names a table a LATER migration drops. That
#       second half is the rule the repository discipline states as "DROP
#       TABLE migrations must update MIGRATION_PROBES": leaving the old
#       `Relation` probe behind makes the runner believe the earlier file
#       was never applied.
# FORWARD (dynamic):
#   D1  a brand-new empty database, migrated by the shipped `api` binary,
#       ends with one ledger row per migration and zero rows in `failed`.
#   D2  booting the same binary a second time against that database
#       applies nothing new and changes no `applied_at` -- the migration
#       path is convergent, not a restart loop.
#   D3  the v0.4 tables the version plan names actually exist afterwards.
# ADDITIVE-ONLY (static):
#   S5  the v0.4 migration contains no DROP TABLE / DROP COLUMN / ALTER
#       ... DROP / TRUNCATE / DELETE FROM / DROP INDEX / DROP TYPE. "只新增表".
#   S6  no migration in the repository drops any of the v0.4 Flow tables
#       or the legacy `pages` table -- the rollback plan keeps data.
# ROLLBACK (dynamic, on the live binary):
#   R1  with real Flow content written, an integrity check
#       (POST .../collab/verify) passes and records the head seq.
#   R2  turning the workspace flag off makes the REST write path refuse a
#       new command, and makes the WebSocket path refuse to issue a
#       ticket -- both independently, which is what "关闭 flag、拒绝新
#       REST/WebSocket 写入" requires. A flag that only hides the UI would
#       pass neither.
#   R3  after the flag is off, every Flow table still exists and the rows
#       written in R1 are all still there: nothing is auto-dropped, so the
#       application can be rolled back and rolled forward again.
#   R4  turning the flag back on gives the same document head/projection
#       and the same integrity-check verdict as R1 -- the "回滚前后都运行
#       document head/projection integrity check" pair.
#
# What is NOT covered, stated rather than silently dropped: the drain step
# ("等待 active transaction 与 outbound broadcast drain") is an operational
# procedure around a real deployment's process supervisor, not something a
# single-process script can observe; it is reported as
# `drain_procedure: not_covered` and does not count towards a pass.
#
# Exit codes: 0 = every assertion above passed, 1 = an assertion failed,
# 2 = usage/tool/environment error.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$ROOT_DIR"
CONTRACTS_ROOT="/opt/working/sylvode-flow"
EVIDENCE_ROOT="/opt/working/sylvode-flow/evidence/v0.4"
MIGRATION_REL="migrations/0054_flow_data_layer.sql"
DATABASE_URL="${OPENPR_TEST_DATABASE_URL:-}"
JSON_MODE=0

usage() {
  cat <<'EOF'
Usage: scripts/verify-flow-migration-v0.4.sh --migration PATH --json [OPTIONS]

Re-derives the MIGRATIONS/MIGRATION_PROBES lockstep and the additive-only
property of the v0.4 migration from source, then boots the shipped `api`
binary twice against a brand-new empty database to observe the real ledger,
and exercises the documented rollback strategy (flag off -> REST and
WebSocket writes refused, tables and rows retained, integrity check equal
before and after). Writes evidence/v0.4/migration-result.json.

Options:
  --migration PATH       The v0.4 migration, relative to the repo root.
                         Default: migrations/0054_flow_data_layer.sql
  --database-url URL     Postgres DSN of a server this script may CREATE
                         DATABASE on. Default: $OPENPR_TEST_DATABASE_URL
  --repo-root DIR        Default: this checkout.
  --contracts-root DIR   Default: /opt/working/sylvode-flow
  --evidence-root DIR    Default: /opt/working/sylvode-flow/evidence/v0.4
  --json                 Required for CLI-contract compatibility.
  -h, --help             Show this help and exit 0.

Exit codes: 0 all assertions passed, 1 an assertion failed,
2 usage/tool/environment error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --migration) MIGRATION_REL="${2:?--migration requires a PATH argument}"; shift 2 ;;
    --database-url) DATABASE_URL="${2:?--database-url requires a value}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:?--repo-root requires a DIR argument}"; shift 2 ;;
    --contracts-root) CONTRACTS_ROOT="${2:?--contracts-root requires a DIR argument}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a DIR argument}"; shift 2 ;;
    --json) JSON_MODE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) echo "Unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
: "$CONTRACTS_ROOT"

if [[ $JSON_MODE -ne 1 ]]; then
  echo "FAIL: --json is required" >&2
  usage >&2
  exit 2
fi
if [[ -z "$DATABASE_URL" ]]; then
  echo "FAIL: no database URL configured (set --database-url or OPENPR_TEST_DATABASE_URL)" >&2
  exit 2
fi
for tool in jq git python3 psql curl cargo sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required command: $tool" >&2; exit 2; }
done
if [[ ! -d "$REPO_ROOT" ]] || ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "FAIL: --repo-root is not a git work tree: $REPO_ROOT" >&2
  exit 2
fi
MIGRATION_ABS="$REPO_ROOT/$MIGRATION_REL"
if [[ ! -f "$MIGRATION_ABS" ]]; then
  echo "FAIL: migration file not found: $MIGRATION_ABS" >&2
  exit 2
fi
if ! psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atc "SELECT 1" >/dev/null 2>&1; then
  echo "FAIL: database is not reachable: $DATABASE_URL" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_ROOT"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
MIGRATION_SHA="$(sha256sum "$MIGRATION_ABS" | awk '{print $1}')"
MIGRATION_BASENAME="$(basename "$MIGRATION_REL")"

VIOLATIONS=()

# ================= static half =================
echo "=== static: MIGRATIONS / MIGRATION_PROBES lockstep and additive-only scan ===" >&2
STATIC_JSON="$(python3 -c '
import json, os, re, sys

repo_root, main_rs, migrations_dir, target_basename = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
src = open(main_rs, encoding="utf-8").read()
violations = []

# --- MIGRATIONS: the const array of (name, include_str!(...)) pairs ---
mm = re.search(r"const MIGRATIONS: &\[\(&str, &str\)\] = &\[(.*?)\n\];", src, re.S)
if not mm:
    print(json.dumps({"error": "MIGRATIONS array not found in main.rs"}))
    sys.exit(0)
migrations_block = mm.group(1)
migration_names = re.findall(r"\(\s*\"([0-9A-Za-z_.\-]+\.sql)\"", migrations_block)
embedded = re.findall(r"include_str!\(\s*\"[^\"]*?/([0-9A-Za-z_.\-]+\.sql)\"\s*\)", migrations_block)

pm = re.search(r"const MIGRATION_PROBES: &\[\(&str, SchemaProbe\)\] = &\[(.*?)\n\];", src, re.S)
if not pm:
    print(json.dumps({"error": "MIGRATION_PROBES array not found in main.rs"}))
    sys.exit(0)
probes_block = pm.group(1)
# One (name, SchemaProbe::Variant(...)) entry per migration. Names and probe
# variants are captured in file order.
probe_entries = re.findall(
    r"\(\s*\"([0-9A-Za-z_.\-]+\.sql)\"\s*,\s*SchemaProbe::(\w+)((?:\([^()]*(?:\([^()]*\))?[^()]*\))?)",
    probes_block,
)
probe_names = [e[0] for e in probe_entries]
probes = {e[0]: {"variant": e[1], "args": e[2]} for e in probe_entries}

# S1
if migration_names != probe_names:
    violations.append(
        "S1: MIGRATIONS and MIGRATION_PROBES do not list the same files in the same order "
        f"(MIGRATIONS has {len(migration_names)} entries, MIGRATION_PROBES has {len(probe_names)}; "
        f"first divergence at index "
        + str(next((i for i, (a, b) in enumerate(zip(migration_names, probe_names)) if a != b), min(len(migration_names), len(probe_names))))
        + ")"
    )

# S2
on_disk = sorted(f for f in os.listdir(migrations_dir) if f.endswith(".sql"))
missing_from_array = [f for f in on_disk if f not in migration_names]
missing_on_disk = [f for f in migration_names if f not in on_disk]
for f in missing_from_array:
    violations.append(f"S2: migrations/{f} exists on disk but is not in MIGRATIONS -- it would never be applied")
for f in missing_on_disk:
    violations.append(f"S2: MIGRATIONS names {f} but there is no such file in migrations/")
if sorted(embedded) != sorted(migration_names):
    violations.append(
        "S2: the include_str! filenames do not match the MIGRATIONS names "
        f"(embedded-only: {sorted(set(embedded) - set(migration_names))}, "
        f"named-only: {sorted(set(migration_names) - set(embedded))})"
    )

# S3
if target_basename not in migration_names:
    violations.append(f"S3: {target_basename} is not listed in MIGRATIONS")
if target_basename not in probe_names:
    violations.append(f"S3: {target_basename} has no MIGRATION_PROBES entry")
if target_basename not in embedded:
    violations.append(f"S3: {target_basename} is not include_str!-embedded under that exact filename")

# --- S4/S6: what each migration drops ---
drop_re = re.compile(r"DROP\s+TABLE(?:\s+IF\s+EXISTS)?\s+([A-Za-z0-9_.\"]+)", re.IGNORECASE)
drops = {}
for name in migration_names:
    path = os.path.join(migrations_dir, name)
    if not os.path.isfile(path):
        continue
    text = open(path, encoding="utf-8", errors="replace").read()
    dropped = sorted({m.strip("\"").split(".")[-1].lower() for m in drop_re.findall(text)})
    if dropped:
        drops[name] = dropped

order = {name: i for i, name in enumerate(migration_names)}
for name, dropped in drops.items():
    probe = probes.get(name)
    if probe is None:
        continue
    args = probe["args"]
    if probe["variant"] == "Relation":
        probed = re.findall(r"\"([A-Za-z0-9_]+)\"", args)
        for t in probed:
            if t.lower() in dropped:
                violations.append(
                    f"S4: {name} drops table {t} but its own probe is Relation(\"{t}\") -- the probe can "
                    "never be satisfied, so the runner replays this file on every boot"
                )
    # any EARLIER migration whose Relation probe names a table this file drops
    for other, oprobe in probes.items():
        if order.get(other, -1) >= order.get(name, 1 << 30):
            continue
        if oprobe["variant"] != "Relation":
            continue
        for t in re.findall(r"\"([A-Za-z0-9_]+)\"", oprobe["args"]):
            if t.lower() in dropped:
                violations.append(
                    f"S4: {other} still probes Relation(\"{t}\") but {name} drops that table -- "
                    "MIGRATION_PROBES was not updated alongside the DROP TABLE migration, so an "
                    "existing database reports the earlier file as unapplied and the API refuses to start"
                )

# S5: the v0.4 migration must be additive only
target_path = os.path.join(migrations_dir, target_basename)
target_sql = open(target_path, encoding="utf-8", errors="replace").read() if os.path.isfile(target_path) else ""
# Strip -- line comments so prose in a header cannot trip the scan.
target_code = re.sub(r"(?m)--.*$", "", target_sql)
destructive_patterns = {
    "DROP TABLE": r"DROP\s+TABLE",
    "DROP COLUMN": r"DROP\s+COLUMN",
    "ALTER ... DROP": r"ALTER\s+TABLE[\s\S]{0,200}?\bDROP\b",
    "TRUNCATE": r"\bTRUNCATE\b",
    "DELETE FROM": r"\bDELETE\s+FROM\b",
    "DROP INDEX": r"DROP\s+INDEX",
    "DROP TYPE": r"DROP\s+TYPE",
    "DROP CONSTRAINT": r"DROP\s+CONSTRAINT",
}
destructive_found = {}
for label, pattern in destructive_patterns.items():
    hits = re.findall(pattern, target_code, re.IGNORECASE)
    if hits:
        destructive_found[label] = len(hits)
        violations.append(f"S5: {target_basename} contains {len(hits)} `{label}` statement(s) -- v0.4 migration must only add")

created_tables = sorted({m.lower() for m in re.findall(r"CREATE\s+TABLE(?:\s+IF\s+NOT\s+EXISTS)?\s+([A-Za-z0-9_]+)", target_code, re.IGNORECASE)})

# S6: the Flow tables and legacy `pages` must never be dropped anywhere
protected = set(created_tables) | {"pages"}
for name, dropped in drops.items():
    for t in dropped:
        if t in protected:
            violations.append(f"S6: {name} drops `{t}`, which the v0.4 rollback plan requires to be retained")

print(json.dumps({
    "migrations_count": len(migration_names),
    "probes_count": len(probe_names),
    "lockstep": migration_names == probe_names,
    "on_disk_count": len(on_disk),
    "target_migration": target_basename,
    "target_created_tables": created_tables,
    "target_destructive_statements": destructive_found,
    "migrations_that_drop_tables": drops,
    "probe_variant_histogram": {v: sum(1 for p in probes.values() if p["variant"] == v) for v in sorted({p["variant"] for p in probes.values()})},
    "violations": violations,
}))
' "$REPO_ROOT" "$REPO_ROOT/apps/api/src/main.rs" "$REPO_ROOT/migrations" "$MIGRATION_BASENAME")"

if ! jq -e . >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: static parser did not produce valid JSON" >&2
  echo "$STATIC_JSON" >&2
  exit 2
fi
if jq -e 'has("error")' >/dev/null 2>&1 <<<"$STATIC_JSON"; then
  echo "FAIL: $(jq -r '.error' <<<"$STATIC_JSON")" >&2
  exit 2
fi
while IFS= read -r v; do
  [[ -n "$v" ]] && VIOLATIONS+=("$v")
done < <(jq -r '.violations[]' <<<"$STATIC_JSON")

# ================= dynamic half =================
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
echo "=== building api binary (cargo build -p api --bin api) ===" >&2
( cd "$REPO_ROOT" && cargo build -q -p api --bin api ) || { echo "FAIL: api binary failed to build" >&2; exit 2; }
API_BIN="$TARGET_DIR/debug/api"
[[ -x "$API_BIN" ]] || { echo "FAIL: api binary not found after build: $API_BIN" >&2; exit 2; }

RUN_ID="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:8])')"
SCRATCH_DB="openpr_flow_migration_verify_$RUN_ID"
DB_PREFIX="${DATABASE_URL%/*}"
SCRATCH_URL="$DB_PREFIX/$SCRATCH_DB"
TMP_DIR="$(mktemp -d "/tmp/openpr-flow-migration-verify.XXXXXX")"
API_PORT=$((20000 + RANDOM % 20000))
API_PID=""

# shellcheck disable=SC2317  # invoked only via `trap ... EXIT`
cleanup() {
  local ec=$?
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT

echo "=== creating brand-new empty database $SCRATCH_DB ===" >&2
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS \"$SCRATCH_DB\" WITH (FORCE)" >/dev/null
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE \"$SCRATCH_DB\"" >/dev/null

APP_CONFIG="$TMP_DIR/openpr.toml"
cat > "$APP_CONFIG" <<EOF
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$SCRATCH_URL"

[auth]
jwt_secret = "flow-migration-verify-not-a-real-secret"

[logging]
filter = "api=info,openpr=info"
format = "text"
EOF

boot_api() {
  local log="$1"
  "$API_BIN" --config "$APP_CONFIG" > "$log" 2>&1 &
  API_PID=$!
  for _ in $(seq 1 90); do
    if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$API_PID" 2>/dev/null; then
      return 1
    fi
    sleep 0.5
  done
  return 1
}
stop_api() {
  if [[ -n "$API_PID" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  API_PID=""
}

# ---- D1: first boot applies every migration ----
echo "=== D1: first boot against the empty database ===" >&2
FIRST_BOOT_OK=false
if boot_api "$TMP_DIR/api-boot1.log"; then
  FIRST_BOOT_OK=true
else
  VIOLATIONS+=("D1: the api binary never became healthy against a brand-new database; see the log in the evidence")
  tail -40 "$TMP_DIR/api-boot1.log" >&2 || true
fi

LEDGER_TOTAL=0
LEDGER_FAILED=0
LEDGER_MISSING="[]"
if [[ "$FIRST_BOOT_OK" == "true" ]]; then
  LEDGER_TOTAL="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM schema_migrations")"
  LEDGER_FAILED="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM schema_migrations WHERE status='failed'")"
  EXPECTED_TOTAL="$(jq -r '.migrations_count' <<<"$STATIC_JSON")"
  if [[ "$LEDGER_TOTAL" != "$EXPECTED_TOTAL" ]]; then
    VIOLATIONS+=("D1: ledger has $LEDGER_TOTAL rows but MIGRATIONS embeds $EXPECTED_TOTAL files")
  fi
  if [[ "$LEDGER_FAILED" != "0" ]]; then
    VIOLATIONS+=("D1: $LEDGER_FAILED migration(s) recorded status=failed after a clean forward run")
  fi
  LEDGER_NAMES="$(psql "$SCRATCH_URL" -Atc "SELECT name FROM schema_migrations ORDER BY name" | jq -R . | jq -s .)"
  LEDGER_MISSING="$(python3 -c '
import json,re,sys
src=open(sys.argv[1],encoding="utf-8").read()
mm=re.search(r"const MIGRATIONS: &\[\(&str, &str\)\] = &\[(.*?)\n\];", src, re.S)
names=set(re.findall(r"\(\s*\"([0-9A-Za-z_.\-]+\.sql)\"", mm.group(1)))
ledger=set(json.loads(sys.argv[2]))
print(json.dumps(sorted(names-ledger)))
' "$REPO_ROOT/apps/api/src/main.rs" "$LEDGER_NAMES")"
  if [[ "$LEDGER_MISSING" != "[]" ]]; then
    VIOLATIONS+=("D1: migrations never recorded in the ledger: $LEDGER_MISSING")
  fi

  # ---- D3: the v0.4 tables exist ----
  MISSING_TABLES=()
  while IFS= read -r t; do
    [[ -z "$t" ]] && continue
    present="$(psql "$SCRATCH_URL" -Atc "SELECT to_regclass('$t') IS NOT NULL")"
    [[ "$present" == "t" ]] || MISSING_TABLES+=("$t")
  done < <(jq -r '.target_created_tables[]' <<<"$STATIC_JSON")
  if [[ ${#MISSING_TABLES[@]} -gt 0 ]]; then
    VIOLATIONS+=("D3: tables created by $MIGRATION_BASENAME are absent after the forward run: ${MISSING_TABLES[*]}")
  fi
fi

# ---- D2: second boot is a no-op ----
SECOND_BOOT_OK=false
LEDGER_TOTAL_2=0
APPLIED_AT_CHANGED=0
if [[ "$FIRST_BOOT_OK" == "true" ]]; then
  APPLIED_AT_BEFORE="$(psql "$SCRATCH_URL" -Atc "SELECT md5(string_agg(name || '=' || applied_at::text || '=' || status, ',' ORDER BY name)) FROM schema_migrations")"
  stop_api
  echo "=== D2: second boot against the already-migrated database ===" >&2
  if boot_api "$TMP_DIR/api-boot2.log"; then
    SECOND_BOOT_OK=true
    LEDGER_TOTAL_2="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM schema_migrations")"
    APPLIED_AT_AFTER="$(psql "$SCRATCH_URL" -Atc "SELECT md5(string_agg(name || '=' || applied_at::text || '=' || status, ',' ORDER BY name)) FROM schema_migrations")"
    [[ "$LEDGER_TOTAL_2" == "$LEDGER_TOTAL" ]] || VIOLATIONS+=("D2: the second boot changed the ledger row count ($LEDGER_TOTAL -> $LEDGER_TOTAL_2)")
    if [[ "$APPLIED_AT_BEFORE" != "$APPLIED_AT_AFTER" ]]; then
      APPLIED_AT_CHANGED=1
      VIOLATIONS+=("D2: the second boot re-applied at least one migration (name/applied_at/status digest changed) -- this is the restart-loop failure mode")
    fi
  else
    VIOLATIONS+=("D2: the api binary did not become healthy on a second boot against its own migrated database")
    tail -40 "$TMP_DIR/api-boot2.log" >&2 || true
  fi
fi

# ================= rollback strategy, on the live binary =================
ROLLBACK_JSON='{"status":"not_run"}'
if [[ "$SECOND_BOOT_OK" == "true" ]]; then
  echo "=== R1-R4: rollback strategy against the live api ===" >&2
  WORKSPACE="$(python3 -c 'import uuid; print(uuid.uuid4())')"
  OWNER_USER="$(python3 -c 'import uuid; print(uuid.uuid4())')"

  psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q <<SQL
INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type, created_at, updated_at)
VALUES ('$OWNER_USER', 'migration-verify-$RUN_ID@example.local', '', 'Migration Verify Owner', 'user', true, 'human', now(), now());
INSERT INTO workspaces (id, slug, name, created_by, created_at, updated_at)
VALUES ('$WORKSPACE', 'migration-verify-$RUN_ID', 'Migration Verify', '$OWNER_USER', now(), now());
INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
VALUES ('$WORKSPACE', '$OWNER_USER', 'owner', now());
INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level, authz_epoch, updated_at)
VALUES ('$WORKSPACE', true, 'edit', 0, now());
SQL

  # A human member's access token, minted the same way scripts/smoke-universal-forms-api.sh
  # mints one. A bot token cannot be used for these fixtures: a bot's actor id is the
  # `workspace_bots` row id, and `flow_objects.created_by` is a foreign key into `users`.
  USER_JWT="$(python3 -c '
import base64, hashlib, hmac, json, sys, time
def b64(o):
    return base64.urlsafe_b64encode(json.dumps(o, separators=(",", ":")).encode()).rstrip(b"=").decode()
secret, sub, email = sys.argv[1], sys.argv[2], sys.argv[3]
now = int(time.time())
head = b64({"alg": "HS256", "typ": "JWT"})
body = b64({"sub": sub, "email": email, "token_type": "access", "iat": now, "exp": now + 3600})
sig = base64.urlsafe_b64encode(hmac.new(secret.encode(), f"{head}.{body}".encode(), hashlib.sha256).digest()).rstrip(b"=").decode()
print(f"{head}.{body}.{sig}")
' "flow-migration-verify-not-a-real-secret" "$OWNER_USER" "migration-verify-$RUN_ID@example.local")"

  API="http://127.0.0.1:$API_PORT"
  AUTH=(-H "Authorization: Bearer $USER_JWT" -H "Content-Type: application/json")

  # A response that is not JSON at all (a proxy error page, an empty reply) must become a
  # recorded violation, never a `jq` crash that aborts the script mid-fixture.
  json_get() { jq -r "$2" <<<"$1" 2>/dev/null || printf ''; }

  CREATE_RESP="$(curl -sS -X POST "$API/api/v1/workspaces/$WORKSPACE/flow/objects" "${AUTH[@]}" \
    -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" '{object_type:"page", title:"Rollback fixture", idempotency_key:$key}')")"
  OBJECT_ID="$(json_get "$CREATE_RESP" '.data.id // .data.object.id // empty')"
  if [[ -z "$OBJECT_ID" ]]; then
    VIOLATIONS+=("R1: could not create a Flow page to exercise the rollback strategy (response: $CREATE_RESP)")
  else
    # Real content, checked: a fixture whose writes silently failed would make R3's
    # "rows retained" assertion trivially true against zero rows.
    LAST_COMMAND_RESP=""
    for i in 1 2 3; do
      LAST_COMMAND_RESP="$(curl -sS -X POST "$API/api/v1/flow/objects/$OBJECT_ID/commands" "${AUTH[@]}" \
        -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" --arg t "Rollback fixture $i" \
          '{command:{type:"set_title", payload:{title:$t}}, idempotency_key:$key}')")"
      cmd_code="$(json_get "$LAST_COMMAND_RESP" '.code // empty')"
      cmd_seq="$(json_get "$LAST_COMMAND_RESP" '.data.accepted_seq // empty')"
      if [[ "$cmd_code" != "0" ]]; then
        VIOLATIONS+=("R1: seed command $i was not accepted (response: $LAST_COMMAND_RESP)")
        break
      fi
      if [[ "$cmd_seq" != "$i" ]]; then
        VIOLATIONS+=("R1: seed command $i returned accepted_seq='$cmd_seq', expected $i")
      fi
    done

    VERIFY_BEFORE="$(curl -sS -X POST "$API/api/v1/flow/objects/$OBJECT_ID/collab/verify" "${AUTH[@]}" \
      -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" '{deep:false, idempotency_key:$key}')")"
    STATUS_BEFORE="$(json_get "$VERIFY_BEFORE" '.data.status // empty')"
    HEAD_BEFORE="$(json_get "$VERIFY_BEFORE" '.data.observed_head_seq // empty')"
    [[ "$STATUS_BEFORE" == "passed" ]] || VIOLATIONS+=("R1: pre-rollback integrity check did not pass (response: $VERIFY_BEFORE)")
    if [[ "$HEAD_BEFORE" != "3" ]]; then
      VIOLATIONS+=("R1: the pre-rollback fixture left observed_head_seq='$HEAD_BEFORE', expected 3 -- the retained-data assertions below would otherwise run against an empty document")
    fi

    UPDATES_BEFORE="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM collab_updates cu JOIN collab_documents cd ON cd.id=cu.document_id JOIN flow_objects fo ON fo.id=cd.object_id WHERE fo.workspace_id='$WORKSPACE'")"
    OBJECTS_BEFORE="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM flow_objects WHERE workspace_id='$WORKSPACE'")"
    PROJ_BEFORE="$(psql "$SCRATCH_URL" -Atc "SELECT coalesce(md5(string_agg(p.title || '=' || p.document_seq::text, ',' ORDER BY p.object_id)),'') FROM flow_object_projections p JOIN flow_objects fo ON fo.id=p.object_id WHERE fo.workspace_id='$WORKSPACE'")"

    # ---- R2: flag off refuses REST writes AND WebSocket tickets ----
    psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q -c "UPDATE flow_workspace_settings SET flow_enabled=false WHERE workspace_id='$WORKSPACE'" >/dev/null

    DOCUMENT_ID="$(psql "$SCRATCH_URL" -Atc "SELECT cd.id FROM collab_documents cd JOIN flow_objects fo ON fo.id=cd.object_id WHERE fo.id='$OBJECT_ID'")"

    REST_OFF="$(curl -sS -X POST "$API/api/v1/flow/objects/$OBJECT_ID/commands" "${AUTH[@]}" \
      -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
        '{command:{type:"set_title", payload:{title:"written after the flag went off"}}, idempotency_key:$key}')")"
    REST_OFF_CODE="$(json_get "$REST_OFF" '.code // empty')"
    # Must be the `feature_disabled` refusal error-mapping-v1.md pins to Forbidden/403, not
    # merely "anything other than success": a 500, a proxy error page or an empty reply would
    # otherwise be counted as the flag doing its job.
    if [[ "$REST_OFF_CODE" != "403" ]]; then
      VIOLATIONS+=("R2: with flow_enabled=false the REST command endpoint answered envelope code='$REST_OFF_CODE', expected 403 feature_disabled (response: $REST_OFF)")
    fi

    TICKET_OFF="$(curl -sS -X POST "$API/api/v1/collab/tickets" "${AUTH[@]}" \
      -d "$(jq -n --arg ws "$WORKSPACE" --arg doc "$DOCUMENT_ID" '{workspace_id:$ws, document_id:$doc, client_id:"rollback-verify", origin:"http://127.0.0.1"}')")"
    TICKET_OFF_CODE="$(json_get "$TICKET_OFF" '.code // empty')"
    if [[ "$TICKET_OFF_CODE" != "403" ]]; then
      VIOLATIONS+=("R2: with flow_enabled=false the WebSocket ticket endpoint answered envelope code='$TICKET_OFF_CODE', expected 403 feature_disabled -- the WebSocket path must enforce the flag independently (response: $TICKET_OFF)")
    fi

    # ---- R3: tables and rows retained ----
    DROPPED_TABLES=()
    while IFS= read -r t; do
      [[ -z "$t" ]] && continue
      present="$(psql "$SCRATCH_URL" -Atc "SELECT to_regclass('$t') IS NOT NULL")"
      [[ "$present" == "t" ]] || DROPPED_TABLES+=("$t")
    done < <(jq -r '.target_created_tables[]' <<<"$STATIC_JSON")
    if [[ ${#DROPPED_TABLES[@]} -gt 0 ]]; then
      VIOLATIONS+=("R3: turning the flag off removed Flow tables: ${DROPPED_TABLES[*]}")
    fi
    UPDATES_OFF="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM collab_updates cu JOIN collab_documents cd ON cd.id=cu.document_id JOIN flow_objects fo ON fo.id=cd.object_id WHERE fo.workspace_id='$WORKSPACE'")"
    OBJECTS_OFF="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM flow_objects WHERE workspace_id='$WORKSPACE'")"
    [[ "$UPDATES_OFF" == "$UPDATES_BEFORE" ]] || VIOLATIONS+=("R3: collab_updates row count changed when the flag went off ($UPDATES_BEFORE -> $UPDATES_OFF) -- data must be retained for recovery")
    [[ "$OBJECTS_OFF" == "$OBJECTS_BEFORE" ]] || VIOLATIONS+=("R3: flow_objects row count changed when the flag went off ($OBJECTS_BEFORE -> $OBJECTS_OFF)")

    # ---- R4: roll forward again, same head/projection, same integrity verdict ----
    psql "$SCRATCH_URL" -v ON_ERROR_STOP=1 -q -c "UPDATE flow_workspace_settings SET flow_enabled=true WHERE workspace_id='$WORKSPACE'" >/dev/null
    VERIFY_AFTER="$(curl -sS -X POST "$API/api/v1/flow/objects/$OBJECT_ID/collab/verify" "${AUTH[@]}" \
      -d "$(jq -n --arg key "$(python3 -c 'import uuid; print(uuid.uuid4())')" '{deep:false, idempotency_key:$key}')")"
    STATUS_AFTER="$(json_get "$VERIFY_AFTER" '.data.status // empty')"
    HEAD_AFTER="$(json_get "$VERIFY_AFTER" '.data.observed_head_seq // empty')"
    PROJ_AFTER="$(psql "$SCRATCH_URL" -Atc "SELECT coalesce(md5(string_agg(p.title || '=' || p.document_seq::text, ',' ORDER BY p.object_id)),'') FROM flow_object_projections p JOIN flow_objects fo ON fo.id=p.object_id WHERE fo.workspace_id='$WORKSPACE'")"
    [[ "$STATUS_AFTER" == "passed" ]] || VIOLATIONS+=("R4: post-rollback integrity check did not pass (response: $VERIFY_AFTER)")
    [[ "$HEAD_AFTER" == "$HEAD_BEFORE" ]] || VIOLATIONS+=("R4: document head seq changed across the flag-off/flag-on cycle ($HEAD_BEFORE -> $HEAD_AFTER)")
    [[ "$PROJ_AFTER" == "$PROJ_BEFORE" ]] || VIOLATIONS+=("R4: projection digest changed across the flag-off/flag-on cycle")

    ROLLBACK_JSON="$(jq -n \
      --arg object_id "$OBJECT_ID" --arg document_id "$DOCUMENT_ID" \
      --arg status_before "$STATUS_BEFORE" --arg status_after "$STATUS_AFTER" \
      --arg head_before "$HEAD_BEFORE" --arg head_after "$HEAD_AFTER" \
      --arg rest_off_code "$REST_OFF_CODE" --arg ticket_off_code "$TICKET_OFF_CODE" \
      --arg updates_before "$UPDATES_BEFORE" --arg updates_off "$UPDATES_OFF" \
      --arg objects_before "$OBJECTS_BEFORE" --arg objects_off "$OBJECTS_OFF" \
      --arg proj_before "$PROJ_BEFORE" --arg proj_after "$PROJ_AFTER" \
      '{
        status: "ran",
        object_id: $object_id, document_id: $document_id,
        integrity_before: {status: $status_before, observed_head_seq: $head_before},
        integrity_after: {status: $status_after, observed_head_seq: $head_after},
        flag_off_rest_command_envelope_code: $rest_off_code,
        flag_off_ticket_envelope_code: $ticket_off_code,
        retained_rows: {
          collab_updates_before: $updates_before, collab_updates_flag_off: $updates_off,
          flow_objects_before: $objects_before, flow_objects_flag_off: $objects_off
        },
        projection_digest: {before: $proj_before, after: $proj_after}
      }')"
  fi
fi

stop_api

PASSED=$([[ ${#VIOLATIONS[@]} -eq 0 ]] && echo true || echo false)
GATE_STATUS=$([[ "$PASSED" == "true" ]] && echo passed || echo failed)
VIOLATIONS_JSON="$(printf '%s\n' "${VIOLATIONS[@]:-}" | jq -R 'select(length>0)' | jq -s '.')"
REASON="static lockstep+additive scan and a live forward run (empty DB -> boot -> reboot) plus the flag-off/flag-on rollback cycle; $(jq 'length' <<<"$VIOLATIONS_JSON") violation(s)"

RESULT="$(jq -n \
  --arg head "$SOURCE_HEAD" --arg generated_at "$GENERATED_AT" \
  --arg migration "$MIGRATION_REL" --arg migration_sha "$MIGRATION_SHA" \
  --argjson static_check "$STATIC_JSON" \
  --argjson first_boot "$([[ "$FIRST_BOOT_OK" == "true" ]] && echo true || echo false)" \
  --argjson second_boot "$([[ "$SECOND_BOOT_OK" == "true" ]] && echo true || echo false)" \
  --arg ledger_total "$LEDGER_TOTAL" --arg ledger_total_2 "$LEDGER_TOTAL_2" --arg ledger_failed "$LEDGER_FAILED" \
  --argjson ledger_missing "$LEDGER_MISSING" \
  --argjson applied_at_changed "$APPLIED_AT_CHANGED" \
  --argjson rollback "$ROLLBACK_JSON" \
  --argjson violations "$VIOLATIONS_JSON" --argjson passed "$PASSED" \
  --arg gate_status "$GATE_STATUS" --arg reason "$REASON" \
  '{
    schema_version: "sylvode.flow.migration-result.v1",
    source_head: $head,
    generated_at: $generated_at,
    migration: {path: $migration, sha256: $migration_sha},
    static_check: $static_check,
    forward_run: {
      first_boot_healthy: $first_boot,
      second_boot_healthy: $second_boot,
      ledger_rows_after_first_boot: $ledger_total,
      ledger_rows_after_second_boot: $ledger_total_2,
      ledger_rows_failed: $ledger_failed,
      migrations_missing_from_ledger: $ledger_missing,
      second_boot_reapplied_something: ($applied_at_changed == 1)
    },
    rollback_strategy: $rollback,
    drain_procedure: {
      status: "not_covered",
      reason: "「等待 active transaction 与 outbound broadcast drain」 is an operational step around a real deployments process supervisor; a single-process verifier cannot observe it. Recorded, never counted as covered."
    },
    violations: $violations,
    passed: $passed,
    gates: {
      migration_forward_and_rollback_strategy: {status: $gate_status, reason: $reason}
    }
  }')"

OUT_PATH="$EVIDENCE_ROOT/migration-result.json"
OUT_TMP="$OUT_PATH.tmp"
printf '%s\n' "$RESULT" | jq . > "$OUT_TMP"
sync "$OUT_TMP" 2>/dev/null || true
mv -f "$OUT_TMP" "$OUT_PATH"
echo "wrote $OUT_PATH" >&2

if [[ "$PASSED" != "true" ]]; then
  jq -r '.violations[] | "  VIOLATION: " + .' <<<"$RESULT" >&2
fi

echo "$RESULT"
[[ "$PASSED" == "true" ]] && exit 0
exit 1

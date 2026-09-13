#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
: "${OPENPR_BACKUP_SOURCE_DATABASE_URL:?OPENPR_BACKUP_SOURCE_DATABASE_URL is required}"
: "${OPENPR_BACKUP_RESTORE_ADMIN_URL:?OPENPR_BACKUP_RESTORE_ADMIN_URL is required}"

RESTORE_DATABASE_NAME=${OPENPR_BACKUP_RESTORE_DATABASE_NAME:-v08_restore_drill}
RESULT_PATH=${1:-/opt/worker/.cache/openpr-v08-backup-restore/result.json}
WORK_DIR=/opt/worker/.cache/openpr-v08-backup-restore
DUMP_PATH="$WORK_DIR/source.sql"
BEFORE_PATH="$WORK_DIR/before.json"
AFTER_PATH="$WORK_DIR/after.json"
TARGET_DIR=${CARGO_TARGET_DIR:-$REPO_ROOT/target}
FINGERPRINT_BIN="$TARGET_DIR/debug/flow-document-fingerprints"

if [[ ! $RESTORE_DATABASE_NAME =~ ^v08_restore_[a-z0-9_]+$ ]]; then
  echo "restore database name must match ^v08_restore_[a-z0-9_]+$" >&2
  exit 2
fi
case "$OPENPR_BACKUP_RESTORE_ADMIN_URL" in
  */*) RESTORE_DATABASE_URL="${OPENPR_BACKUP_RESTORE_ADMIN_URL%/*}/$RESTORE_DATABASE_NAME" ;;
  *) echo "OPENPR_BACKUP_RESTORE_ADMIN_URL must be a PostgreSQL URL" >&2; exit 2 ;;
esac

cleanup() {
  psql "$OPENPR_BACKUP_RESTORE_ADMIN_URL" -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS \"$RESTORE_DATABASE_NAME\" WITH (FORCE)" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$WORK_DIR" "$(dirname "$RESULT_PATH")"
command -v pg_dump >/dev/null
command -v psql >/dev/null
command -v jq >/dev/null
command -v sha256sum >/dev/null

env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=4 \
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p api --bin flow-document-fingerprints >/dev/null

OPENPR_DATABASE_URL="$OPENPR_BACKUP_SOURCE_DATABASE_URL" "$FINGERPRINT_BIN" | jq -S . >"$BEFORE_PATH"
DOCUMENT_COUNT=$(jq 'length' "$BEFORE_PATH")
if [[ $DOCUMENT_COUNT -le 0 ]]; then
  echo "backup/restore drill requires at least one real collaboration document" >&2
  exit 1
fi

# A newer pg_dump can emit header-only SET statements unknown to the older target server. Remove
# only the PostgreSQL 17 `transaction_timeout` setting, and only before the first COPY begins;
# no data line or other restore error is filtered. The restored SQL remains the checksummed backup.
pg_dump --dbname="$OPENPR_BACKUP_SOURCE_DATABASE_URL" --format=plain --no-owner --no-acl |
  awk '
    /^COPY / { in_data = 1 }
    !in_data && $0 == "SET transaction_timeout = 0;" { next }
    { print }
  ' >"$DUMP_PATH"
DUMP_SHA256=$(sha256sum "$DUMP_PATH" | awk '{print $1}')

cleanup
psql "$OPENPR_BACKUP_RESTORE_ADMIN_URL" -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE \"$RESTORE_DATABASE_NAME\"" >/dev/null

START_NS=$(date +%s%N)
psql "$RESTORE_DATABASE_URL" -X -v ON_ERROR_STOP=1 --single-transaction \
  --file="$DUMP_PATH" >/dev/null
END_NS=$(date +%s%N)
RESTORE_NS=$((END_NS - START_NS))
RESTORE_MS=$(((RESTORE_NS + 999999) / 1000000))
RESTORE_SECONDS=$(((RESTORE_NS + 999999999) / 1000000000))

OPENPR_DATABASE_URL="$RESTORE_DATABASE_URL" "$FINGERPRINT_BIN" | jq -S . >"$AFTER_PATH"
if ! cmp -s "$BEFORE_PATH" "$AFTER_PATH"; then
  diff -u "$BEFORE_PATH" "$AFTER_PATH" >&2 || true
  echo "restored per-document head/frontier/semantic-hash/projection-seq differs" >&2
  exit 1
fi

# Determine the best observed positive tick of the exact clock used above. A restore duration at
# or below twice that resolution is reported as inconclusive rather than promoted to an RTO fact.
CLOCK_RESOLUTION_NS=1000000000
previous=$(date +%s%N)
for _ in $(seq 1 100); do
  current=$(date +%s%N)
  delta=$((current - previous))
  if [[ $delta -gt 0 && $delta -lt $CLOCK_RESOLUTION_NS ]]; then
    CLOCK_RESOLUTION_NS=$delta
  fi
  previous=$current
done
if [[ $RESTORE_NS -le $((CLOCK_RESOLUTION_NS * 2)) ]]; then
  MEASUREMENT_STATUS=inconclusive_below_instrument_resolution
else
  MEASUREMENT_STATUS=measured
fi

jq -n \
  --arg status passed \
  --arg measurement_status "$MEASUREMENT_STATUS" \
  --arg dump_sha256 "$DUMP_SHA256" \
  --argjson document_count "$DOCUMENT_COUNT" \
  --argjson restore_ns "$RESTORE_NS" \
  --argjson restore_ms "$RESTORE_MS" \
  --argjson restore_seconds "$RESTORE_SECONDS" \
  --argjson clock_resolution_ns "$CLOCK_RESOLUTION_NS" \
  '{schema:"openpr.flow.backup_restore.v0.8",status:$status,
    backup:{format:"postgresql_plain_sql",sha256:$dump_sha256},
    restore:{measurement_status:$measurement_status,elapsed_ns:$restore_ns,
      elapsed_ms:$restore_ms,elapsed_seconds_ceiling:$restore_seconds,
      clock_resolution_ns:$clock_resolution_ns},
    verification:{document_count:$document_count,executed_count:$document_count,
      per_document_fields:["head_seq","head_frontier","semantic_hash","projection_seq"],
      exact_match:true,rpo_lost_documents:0}}' >"$RESULT_PATH"

cat "$RESULT_PATH"

#!/usr/bin/env bash
set -euo pipefail
RESULT=${1:-}; KEY=${2:-}; STATUS=${3:-}; SIGNER=${4:-}; NOTE=${5:-}
[[ -f $RESULT ]] || { echo "usage: $0 GATE_RESULT backup_restore|forced_resync|operations_dry_run|security_review passed|failed SIGNER [NOTE]" >&2; exit 2; }
case "$KEY" in backup_restore|forced_resync|operations_dry_run|security_review);; *) echo 'invalid manual key' >&2; exit 2;; esac
case "$STATUS" in passed|failed);; *) echo 'invalid status' >&2; exit 2;; esac
[[ -n $SIGNER ]] || { echo 'signer required' >&2; exit 2; }
tmp=$(mktemp "$(dirname "$RESULT")/.manual.XXXXXX"); trap 'rm -f "$tmp"' EXIT
jq --arg key "$KEY" --arg status "$STATUS" --arg signer "$SIGNER" --arg note "$NOTE" --arg at "$(date -u +%FT%TZ)" \
  '.manual_signoffs[$key]={status:$status,signed_by:$signer,signed_at:$at,note:$note}
   |.accepted=(.candidate_ready and ([.manual_signoffs[].status]|all(.=="passed")))' "$RESULT" >"$tmp"
mv "$tmp" "$RESULT"; trap - EXIT; jq -c --arg key "$KEY" '.manual_signoffs[$key]' "$RESULT"

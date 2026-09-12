#!/usr/bin/env bash
set -euo pipefail
RESULT="${1:-}"; KEY="${2:-}"; STATUS="${3:-}"; SIGNER="${4:-}"; NOTE="${5:-}"
[[ -f "$RESULT" ]] || { echo "usage: $0 GATE_RESULT reference|embed_permission|lineage_no_double_write passed|failed SIGNER [NOTE]" >&2;exit 2; }
case "$KEY" in reference|embed_permission|lineage_no_double_write);;*)echo "invalid manual key" >&2;exit 2;;esac
case "$STATUS" in passed|failed);;*)echo "invalid status" >&2;exit 2;;esac
[[ -n "$SIGNER" ]] || { echo "signer required" >&2;exit 2; }
tmp="$(mktemp "$(dirname "$RESULT")/.manual.XXXXXX")";trap 'rm -f "$tmp"' EXIT
jq --arg k "$KEY" --arg s "$STATUS" --arg by "$SIGNER" --arg note "$NOTE" --arg at "$(date -u +%FT%TZ)" '.manual_signoffs[$k]={status:$s,signed_by:$by,signed_at:$at,note:$note}|.accepted=(.candidate_ready and ([.manual_signoffs[].status]|all(.=="passed")))' "$RESULT">"$tmp";mv "$tmp" "$RESULT";trap - EXIT
jq -c --arg k "$KEY" '.manual_signoffs[$k]' "$RESULT"

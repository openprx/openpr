#!/usr/bin/env bash
set -euo pipefail
RESULT=${1:-};KEY=${2:-};STATUS=${3:-};SIGNER=${4:-};NOTE=${5:-}
[[ -f $RESULT ]]||{ echo "usage: $0 GATE_RESULT KEY passed|failed SIGNER [NOTE]" >&2;exit 2;}
case "$KEY" in release_owner|rollback_owner|on_call_runbook|stable_contract_approval);;*) echo 'invalid manual key' >&2;exit 2;;esac
case "$STATUS" in passed|failed);;*) echo 'invalid status' >&2;exit 2;;esac;[[ -n $SIGNER ]]||{ echo 'signer required' >&2;exit 2;}
tmp=$(mktemp "$(dirname "$RESULT")/.manual.XXXXXX");trap 'rm -f "$tmp"' EXIT
jq --arg key "$KEY" --arg status "$STATUS" --arg signer "$SIGNER" --arg note "$NOTE" --arg at "$(date -u +%FT%TZ)" '
 .manual_signoffs[$key]={status:$status,signed_by:$signer,signed_at:$at,note:$note}
 | .blockers=([.blockers[]?|select(. != ("manual_signoff_pending:"+$key))]+(if $status=="passed" then [] else ["manual_signoff_pending:"+$key] end))
 | .accepted=(.candidate_ready and (.hard_gates.frontend_track_accepted=="passed") and ([.manual_signoffs[].status]|all(.=="passed")))' "$RESULT">"$tmp"
mv "$tmp" "$RESULT";trap - EXIT;jq -c --arg key "$KEY" '.manual_signoffs[$key]' "$RESULT"

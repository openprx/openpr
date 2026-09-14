#!/usr/bin/env bash
set -euo pipefail

RESULT=${1:-}
KEY=${2:-}
STATUS=${3:-}
SIGNER=${4:-}
NOTE=${5:-}
[[ -f $RESULT ]] || { echo "usage: $0 GATE_RESULT deprecation_matrix passed|failed SIGNER [NOTE]" >&2; exit 2; }
[[ $KEY == deprecation_matrix ]] || { echo 'invalid manual key' >&2; exit 2; }
case "$STATUS" in passed|failed) ;; *) echo 'invalid status' >&2; exit 2 ;; esac
[[ -n $SIGNER ]] || { echo 'signer required' >&2; exit 2; }
temporary=$(mktemp "$(dirname "$RESULT")/.manual.XXXXXX")
trap 'rm -f "$temporary"' EXIT
# `blockers` is recomputed alongside `accepted`. Without this the row could be signed while the
# receipt still listed `manual_signoff_pending:<key>`, leaving an artifact that says accepted=true
# and names a blocker in the same breath. A failed sign-off keeps the pending blocker: the row is
# still not satisfied, and inventing a second spelling for that would only fragment the vocabulary.
jq --arg key "$KEY" --arg status "$STATUS" --arg signer "$SIGNER" --arg note "$NOTE" --arg at "$(date -u +%FT%TZ)" \
  '.manual_signoffs[$key]={status:$status,signed_by:$signer,signed_at:$at,note:$note}
   | .blockers=(
       [.blockers[]? | select(. != ("manual_signoff_pending:" + $key))]
       + (if $status == "passed" then [] else ["manual_signoff_pending:" + $key] end)
     )
   | .accepted=(.candidate_ready and ([.manual_signoffs[].status] | all(. == "passed")))' "$RESULT" >"$temporary"
mv "$temporary" "$RESULT"
trap - EXIT
jq -c --arg key "$KEY" '.manual_signoffs[$key]' "$RESULT"

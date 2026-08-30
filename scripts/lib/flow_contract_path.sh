#!/usr/bin/env bash
# Shared resolution rule for contract-document arguments of the Flow
# verifier scripts (`--adr`, `--contract`, `--limits`, and the
# legacy-pages inventory positional).
#
# Why this exists: gates/v0.4-gate.yaml's `required_commands` and
# gates/gate-commands.md spell the exact gate commands with paths written
# relative to the CONTRACTS repo (e.g. "decisions/ADR-0013-...md"), while
# the commands themselves are run from the SOURCE repo. Taking those
# values literally made the documented "exact gate command" fail with
# exit 2 before any gate logic ran. scripts/report-flow-v0.4-json.sh, by
# contrast, expands the same arguments to absolute paths. Both call
# styles -- plus any pre-existing caller passing a path relative to its
# own working directory -- must keep working.
#
# Rule (identical in every script that sources this file):
#   1. absolute path            -> used as-is;
#   2. relative & exists vs CWD -> used as-is (backward compatible);
#   3. otherwise                -> resolved against --contracts-root;
#   4. neither exists           -> FAIL, naming BOTH attempted paths.
#
# This is path resolution only. It never decides, relaxes or skips a
# gate: an unresolvable argument still fails the caller exactly as
# before.

# flow_resolve_contract_path LABEL VALUE CONTRACTS_ROOT
# Prints the resolved path on stdout, or an explanatory FAIL line on
# stderr and returns 1.
flow_resolve_contract_path() {
  local label="$1"
  local value="$2"
  local contracts_root="${3:-}"
  local cwd_candidate root_candidate

  if [[ "$value" == /* ]]; then
    if [[ -f "$value" ]]; then
      printf '%s\n' "$value"
      return 0
    fi
    printf 'FAIL: %s file not found: %s\n' "$label" "$value" >&2
    return 1
  fi

  if [[ -f "$value" ]]; then
    printf '%s\n' "$value"
    return 0
  fi

  cwd_candidate="$PWD/$value"
  root_candidate="${contracts_root%/}/$value"
  if [[ -n "$contracts_root" && -f "$root_candidate" ]]; then
    printf '%s\n' "$root_candidate"
    return 0
  fi

  if [[ -n "$contracts_root" ]]; then
    printf 'FAIL: %s file not found: tried %s (relative to the current directory) and %s (relative to --contracts-root %s)\n' \
      "$label" "$cwd_candidate" "$root_candidate" "$contracts_root" >&2
  else
    printf 'FAIL: %s file not found: tried %s (relative to the current directory); no --contracts-root is configured for this script\n' \
      "$label" "$cwd_candidate" >&2
  fi
  return 1
}

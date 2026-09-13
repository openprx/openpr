#!/usr/bin/env bash

# Shared, side-effect-free compatibility decisions for the v0.9 Sylvode transition.
# Callers remain responsible for exporting the selected values.

sylvode_select_config() {
  local canonical="$1" legacy="$2"
  if [[ -e "$canonical" && -e "$legacy" ]]; then
    echo "Both $canonical and legacy $legacy exist; refusing silent precedence." >&2
    return 1
  fi
  if [[ -e "$canonical" || ! -e "$legacy" ]]; then
    printf '%s\n' "$canonical"
  else
    printf '%s\n' "$legacy"
  fi
}

sylvode_env_value() {
  local env_file="$1" key="$2"
  [[ -f "$env_file" ]] || return 0
  grep -E "^${key}=" "$env_file" | tail -n 1 | cut -d= -f2- || true
}

sylvode_configured_env_value() {
  local env_file="$1" key="$2"
  if [[ -v $key ]]; then
    printf '%s\n' "${!key}"
  else
    sylvode_env_value "$env_file" "$key"
  fi
}

sylvode_resolve_env() {
  local env_file="$1" canonical_key="$2" legacy_key="$3" fallback="$4"
  local canonical_value legacy_value
  canonical_value=$(sylvode_configured_env_value "$env_file" "$canonical_key")
  legacy_value=$(sylvode_configured_env_value "$env_file" "$legacy_key")
  if [[ -n "$canonical_value" && -n "$legacy_value" && "$canonical_value" != "$legacy_value" ]]; then
    echo "$canonical_key conflicts with legacy $legacy_key; refusing silent precedence." >&2
    return 1
  fi
  printf '%s\n' "${canonical_value:-${legacy_value:-$fallback}}"
}

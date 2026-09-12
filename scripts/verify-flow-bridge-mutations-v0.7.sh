#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_ROOT="${REPO_ROOT}/evidence/v0.7"
CACHE_ROOT="/opt/worker/.cache/openpr-v07-bridge-mutations"
while (($#)); do
  case "$1" in
    --repo-root) REPO_ROOT="${2:?--repo-root requires a value}"; shift 2 ;;
    --evidence-root) EVIDENCE_ROOT="${2:?--evidence-root requires a value}"; shift 2 ;;
    --json) shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[[ -n "${OPENPR_TEST_DATABASE_URL:-}" ]] || {
  echo 'FAIL: OPENPR_TEST_DATABASE_URL is required' >&2
  exit 2
}
mkdir -p "$EVIDENCE_ROOT" "$CACHE_ROOT"
RUN_ROOT="$(mktemp -d "$CACHE_ROOT/run.XXXXXX")"
TARGET_DIR="$CACHE_ROOT/target"
SOURCE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
export CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR"
declare -a WORKTREES=()

cleanup() {
  local tree
  for tree in "${WORKTREES[@]}"; do
    git -C "$REPO_ROOT" worktree remove --force "$tree" >/dev/null 2>&1 || true
  done
  rm -rf "$RUN_ROOT"
}
trap cleanup EXIT

run_test() {
  local root="$1" name="$2" test_name="$3"
  local log="$EVIDENCE_ROOT/$name.log"
  set +e
  (cd "$root" && cargo test -p api "$test_name" -- --nocapture) >"$log" 2>&1
  local exit_code=$?
  set -e
  printf '%s' "$exit_code"
}

mutate_case() {
  local root="$1" mutation="$2"
  python3 - "$root" "$mutation" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
case = sys.argv[2]

def replace(rel, old, new, count=1):
    path = root / rel
    text = path.read_text()
    observed = text.count(old)
    if observed != count:
        raise SystemExit(f"{case}: expected {count} copies in {rel}, found {observed}")
    path.write_text(text.replace(old, new))

bridge = "apps/api/src/flow/bridge.rs"
native = "apps/api/src/forms/native_create.rs"
if case == "target_guest_guard":
    replace(bridge, '''    if actor.role == "__flow_guest" {
        return Ok(None);
    }
''', '')
elif case == "preview_guest_guard":
    replace(bridge, '''    if actor.role == "__flow_guest" {
        return Err(ApiError::NotFound("bridge target not found".to_string()));
    }
''', '')
elif case == "unauthorized_placeholder":
    replace(bridge, '''        let Some(permission) = target_permission(&state.db, &actor, &target).await? else {
            continue;
        };
''', '''        let Some(permission) = target_permission(&state.db, &actor, &target).await? else {
            items.push(BridgeReferenceView::Unavailable);
            continue;
        };
''')
elif case == "unauthorized_oracle":
    replace(bridge, '''    let permission = target_permission(tx, actor, &target)
        .await?
        .ok_or_else(|| ApiError::NotFound("bridge target not found".to_string()))?;
''', '''    let permission = target_permission(tx, actor, &target)
        .await?
        .ok_or_else(|| ApiError::policy_rejected("bridge target permission denied"))?;
''')
elif case == "field_write_guard":
    replace(bridge, '''        if let Some(decision) = permission.as_ref()
            && values
                .as_object()
                .is_some_and(|object| object.keys().any(|key| !decision.field_allows(key, "write")))
        {
            return Err(ApiError::policy_rejected(
                "mapped values include a field that is not writable",
            ));
        }
''', '')
elif case == "denied_read_redaction":
    replace(bridge, '''        let summary = if row.target_type == "form_record" {
            permission
                .denied_read_fields
                .iter()
                .fold(target.summary, |mut values, key| {
                    if let Some(object) = values.as_object_mut() {
                        object.remove(key);
                    }
                    values
                })
        } else {
            target.summary
        };
''', '''        let summary = target.summary;
''')
elif case == "read_only_downgrade":
    replace(bridge, '''        let permission_state = if enabled {
            permission.state
        } else {
            read_only_state(permission.state)
        };
''', '''        let permission_state = permission.state;
''')
elif case == "reference_minimum_edit":
    replace(bridge, '''    let (source, mut actor) = source_actor(state, extensions, source_object_id, PermissionLevel::Edit).await?;
''', '''    let (source, mut actor) = source_actor(state, extensions, source_object_id, PermissionLevel::View).await?;
''')
elif case == "new_form_admin_preview_commit":
    replace(bridge, '''            if !matches!(actor.role.as_str(), "owner" | "admin")
                || !project_exists(&state.db, source.workspace_id, project_id).await?
            {
''', '''            if !project_exists(&state.db, source.workspace_id, project_id).await? {
''')
    replace(bridge, '''        if !matches!(actor.role.as_str(), "owner" | "admin")
            || !project_exists(&tx, preview.workspace_id, preview.target_project_id).await?
        {
''', '''        if !project_exists(&tx, preview.workspace_id, preview.target_project_id).await? {
''')
elif case == "flow_shrink_commit":
    replace(bridge, '''    if actor.flow_level < PermissionLevel::Edit {
''', '''    if actor.flow_level < PermissionLevel::View {
''')
elif case == "native_autonumber":
    replace(native, '''    let with_autonumber = apply_autonumber_values(tx, form, None, calculated).await?;
''', '''    let with_autonumber = calculated;
''')
elif case == "native_validator":
    replace(native, '''    run_field_validator_hooks(
        state,
        form.workspace_id,
        form.project_id,
        form.id,
        &form.key,
        &form.schema,
        &normalized,
    )
    .await?;
''', '')
elif case == "forms_default_allow":
    replace(bridge, '''                let inherit_forms_default_allow = bridge_test_mutation("br4_default_allow");
''', '''                let inherit_forms_default_allow = true;
''')
elif case == "bridge_trigger":
    path = root / "migrations/0062_flow_forms_bridge.sql"
    with path.open("a") as handle:
        handle.write('''
CREATE FUNCTION flow_bridge_mutation_noop() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RETURN NEW;
END;
$$;
CREATE TRIGGER flow_bridge_mutation_trigger
AFTER INSERT ON flow_object_lineage
FOR EACH ROW EXECUTE FUNCTION flow_bridge_mutation_noop();
''')
else:
    raise SystemExit(f"unknown mutation: {case}")
PY
}

declare -A TESTS=(
  [guest]='flow_bridge_guest_routes_hide_reference_cardinality_and_target_existence'
  [reference]='flow_bridge_reference_embed_reauthorizes_forms_policy_and_missing_policy_is_read_only'
  [native]='flow_bridge_record_conversion_runs_native_autonumber_and_validator_pipeline'
  [admin]='flow_bridge_new_form_conversion_requires_admin_at_preview_and_commit'
  [shrink]='flow_bridge_conversion_commit_rechecks_flow_permission_through_production_feature_route'
  [br4]='flow::bridge::tests::unconfigured_forms_are_read_only_and_honestly_labelled'
)
RESULTS='[]'
passed=true

for group in guest reference native admin shrink br4; do
  code="$(run_test "$REPO_ROOT" "bridge-$group-green" "${TESTS[$group]}")"
  [[ "$code" == 0 ]] || passed=false
  RESULTS="$(jq --arg id "${group}_production_green" --argjson code "$code" \
    '. + [{id:$id,expected:"green",exit_code:$code}]' <<<"$RESULTS")"
done

CASES=(
  'target_guest_guard:guest'
  'preview_guest_guard:guest'
  'unauthorized_placeholder:guest'
  'unauthorized_oracle:guest'
  'field_write_guard:native'
  'denied_read_redaction:reference'
  'read_only_downgrade:reference'
  'reference_minimum_edit:reference'
  'new_form_admin_preview_commit:admin'
  'flow_shrink_commit:shrink'
  'native_autonumber:native'
  'native_validator:native'
  'bridge_trigger:native'
  'forms_default_allow:br4'
)

for spec in "${CASES[@]}"; do
  mutation="${spec%%:*}"
  group="${spec#*:}"
  tree="$RUN_ROOT/$mutation"
  git -C "$REPO_ROOT" worktree add --detach "$tree" "$SOURCE_HEAD" >/dev/null
  WORKTREES+=("$tree")
  mutate_case "$tree" "$mutation"
  code="$(run_test "$tree" "bridge-$mutation-red" "${TESTS[$group]}")"
  if [[ "$code" == 0 ]] || ! grep -q '^test result: FAILED\.' "$EVIDENCE_ROOT/bridge-$mutation-red.log"; then
    passed=false
  fi
  RESULTS="$(jq --arg id "$mutation" --argjson code "$code" \
    '. + [{id:$id,expected:"red",exit_code:$code}]' <<<"$RESULTS")"
  git -C "$REPO_ROOT" worktree remove --force "$tree" >/dev/null
done
WORKTREES=()

jq -n \
  --arg schema_version 'sylvode.flow.bridge-mutation-result.v2' \
  --arg source_head "$SOURCE_HEAD" \
  --argjson passed "$passed" \
  --argjson cases "$RESULTS" \
  '{schema_version:$schema_version,source_head:$source_head,executed_count:($cases|length),passed:$passed,cases:$cases}' \
  | tee "$EVIDENCE_ROOT/bridge-mutation-result.json"

[[ "$passed" == true ]]

//! Flow <-> Forms bridge domain rules frozen by ADR-0019.
//!
//! This module deliberately does not call the Forms direct-access helpers: those helpers retain
//! Forms' historical "no policy row means allow" default, while BR-4 requires the bridge to treat
//! that same state as read-only and visibly unconfigured. The bridge computes one intersection
//! decision and every reference, embed and conversion path consumes it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::collab::authz::PermissionLevel;
use crate::forms::permissions::{permission_policy_field_allows, permission_policy_record_scope};

pub const BRIDGE_ACTIONS: [&str; 6] = [
    "form.view",
    "record.create",
    "record.update",
    "record.delete",
    "record.export",
    "form.design",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgePrincipal {
    WorkspaceRole,
    Guest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyConfiguration {
    Explicit,
    Unconfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeAccess {
    ReadOnly,
    Controlled,
}

/// Public permission shape. It says what the caller can do, never why another action was denied.
/// In particular it contains no target-existence bit, policy JSON, denied field names, record
/// owner identity or record-scope expression (ADR-0019 BR-3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BridgePermissionState {
    pub access: BridgeAccess,
    pub configuration: PolicyConfiguration,
    pub actions: Vec<String>,
    pub field_read_limited: bool,
    pub field_write_limited: bool,
    pub record_limited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgePermissionDecision {
    pub state: BridgePermissionState,
    denied_read_fields: BTreeSet<String>,
    denied_write_fields: BTreeSet<String>,
    record_scope: String,
}

impl BridgePermissionDecision {
    #[must_use]
    pub fn allows(&self, action: &str) -> bool {
        self.state.actions.iter().any(|allowed| allowed == action)
    }

    #[must_use]
    pub fn field_allows(&self, field_key: &str, action: &str) -> bool {
        match action {
            "read" => !self.denied_read_fields.contains(field_key),
            "write" => !self.denied_write_fields.contains(field_key),
            _ => false,
        }
    }

    #[must_use]
    pub fn record_in_scope(&self, actor_is_owner: bool) -> bool {
        self.record_scope != "owned" || actor_is_owner
    }
}

fn flow_action_ceiling(level: PermissionLevel) -> BTreeMap<&'static str, bool> {
    let mut actions = BTreeMap::new();
    for action in BRIDGE_ACTIONS {
        let allowed = match action {
            "form.view" => level >= PermissionLevel::View,
            "record.create" | "record.update" => level >= PermissionLevel::Edit,
            "record.delete" | "record.export" => level >= PermissionLevel::FullAccess,
            // BR-2: schema authority never crosses the bridge.
            "form.design" => false,
            _ => false,
        };
        actions.insert(action, allowed);
    }
    actions
}

/// Computes ADR-0019's Flow-grade x explicit-Forms-policy intersection.
///
/// `None` is the non-enumerating answer: callers must omit the reference/embed entirely. It is
/// returned for Flow `Denied`, guests, an explicit Forms denial of `form.view`, or an out-of-scope
/// record. A missing policy row is intentionally *not* passed to Forms' permissive direct-access
/// default; BR-4 narrows it to `form.view` and marks it `unconfigured`.
#[must_use]
pub fn bridge_permission(
    flow_level: PermissionLevel,
    principal: BridgePrincipal,
    forms_policy: Option<&Value>,
    record_owner_matches: Option<bool>,
) -> Option<BridgePermissionDecision> {
    if flow_level == PermissionLevel::Denied || principal == BridgePrincipal::Guest {
        return None;
    }

    let ceiling = flow_action_ceiling(flow_level);
    let (configuration, policy_actions, denied_read_fields, denied_write_fields, record_scope) = forms_policy
        .map_or_else(
            || {
                let actions = BTreeMap::from([
                    ("form.view", true),
                    ("record.create", false),
                    ("record.update", false),
                    ("record.delete", false),
                    ("record.export", false),
                    ("form.design", false),
                ]);
                (
                    PolicyConfiguration::Unconfigured,
                    actions,
                    BTreeSet::new(),
                    BTreeSet::new(),
                    "all".to_string(),
                )
            },
            |policy| {
                let actions = BRIDGE_ACTIONS
                    .into_iter()
                    .map(|action| {
                        let allowed = action != "form.design"
                            && policy
                                .get("actions")
                                .and_then(Value::as_object)
                                .and_then(|actions| actions.get(action))
                                .and_then(Value::as_bool)
                                .unwrap_or(true);
                        (action, allowed)
                    })
                    .collect::<BTreeMap<_, _>>();
                let field_keys = policy
                    .get("fields")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|fields| fields.keys())
                    .cloned()
                    .collect::<Vec<_>>();
                let denied_read = field_keys
                    .iter()
                    .filter(|key| !permission_policy_field_allows(policy, key, "read"))
                    .cloned()
                    .collect();
                let denied_write = field_keys
                    .iter()
                    .filter(|key| !permission_policy_field_allows(policy, key, "write"))
                    .cloned()
                    .collect();
                (
                    PolicyConfiguration::Explicit,
                    actions,
                    denied_read,
                    denied_write,
                    permission_policy_record_scope(policy),
                )
            },
        );

    if record_scope == "owned" && record_owner_matches == Some(false) {
        return None;
    }

    let actions = BRIDGE_ACTIONS
        .into_iter()
        .filter(|action| ceiling.get(action).copied().unwrap_or(false))
        .filter(|action| policy_actions.get(action).copied().unwrap_or(false))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !actions.iter().any(|action| action == "form.view") {
        return None;
    }
    let controlled = actions.iter().any(|action| action != "form.view");
    Some(BridgePermissionDecision {
        state: BridgePermissionState {
            access: if controlled {
                BridgeAccess::Controlled
            } else {
                BridgeAccess::ReadOnly
            },
            configuration,
            actions,
            field_read_limited: !denied_read_fields.is_empty(),
            field_write_limited: !denied_write_fields.is_empty(),
            record_limited: record_scope == "owned",
        },
        denied_read_fields,
        denied_write_fields,
        record_scope,
    })
}

#[derive(Debug, Deserialize)]
pub struct BridgeDisplay {
    #[serde(default)]
    pub mode: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{BridgeAccess, BridgePrincipal, PolicyConfiguration, bridge_permission};
    use crate::flow::collab::authz::PermissionLevel;
    use serde_json::json;

    #[test]
    fn bridge_permission_is_the_intersection_in_both_directions() {
        let forms_denies_update = json!({"actions": {"form.view": true, "record.update": false}});
        let full = bridge_permission(
            PermissionLevel::FullAccess,
            BridgePrincipal::WorkspaceRole,
            Some(&forms_denies_update),
            None,
        )
        .expect("view remains visible");
        assert!(!full.allows("record.update"), "Forms denial must beat Flow FullAccess");

        let forms_allows_delete = json!({"actions": {"form.view": true, "record.delete": true}});
        let view = bridge_permission(
            PermissionLevel::View,
            BridgePrincipal::WorkspaceRole,
            Some(&forms_allows_delete),
            None,
        )
        .expect("view remains visible");
        assert!(
            !view.allows("record.delete"),
            "Flow View must beat a Forms delete allowance"
        );
    }

    #[test]
    fn bridge_never_grants_form_design() {
        let policy = json!({"actions": {"form.view": true, "form.design": true}});
        let decision = bridge_permission(
            PermissionLevel::FullAccess,
            BridgePrincipal::WorkspaceRole,
            Some(&policy),
            None,
        )
        .expect("view remains visible");
        assert!(!decision.allows("form.design"));
    }

    #[test]
    fn unconfigured_forms_are_read_only_and_honestly_labelled() {
        let decision = bridge_permission(PermissionLevel::FullAccess, BridgePrincipal::WorkspaceRole, None, None)
            .expect("the BR-4 read-only floor remains visible");
        assert_eq!(decision.state.configuration, PolicyConfiguration::Unconfigured);
        assert_eq!(decision.state.access, BridgeAccess::ReadOnly);
        assert_eq!(decision.state.actions, ["form.view"]);
        assert!(!decision.allows("record.update"));
    }

    #[test]
    fn guests_and_flow_denied_principals_get_no_reference_shape() {
        let permissive = json!({"actions": {"form.view": true, "record.update": true}});
        assert!(
            bridge_permission(
                PermissionLevel::FullAccess,
                BridgePrincipal::Guest,
                Some(&permissive),
                None
            )
            .is_none()
        );
        assert!(
            bridge_permission(
                PermissionLevel::Denied,
                BridgePrincipal::WorkspaceRole,
                Some(&permissive),
                None
            )
            .is_none()
        );
    }

    #[test]
    fn explicit_field_and_record_restrictions_survive_the_intersection() {
        let policy = json!({
            "actions": {"form.view": true, "record.update": true},
            "fields": {"private": {"read": false, "write": false}},
            "record_scope": "owned"
        });
        assert!(
            bridge_permission(
                PermissionLevel::Edit,
                BridgePrincipal::WorkspaceRole,
                Some(&policy),
                Some(false)
            )
            .is_none(),
            "an out-of-scope record must not produce a placeholder"
        );
        let decision = bridge_permission(
            PermissionLevel::Edit,
            BridgePrincipal::WorkspaceRole,
            Some(&policy),
            Some(true),
        )
        .expect("the owned record is visible");
        assert!(!decision.field_allows("private", "read"));
        assert!(!decision.field_allows("private", "write"));
        assert!(decision.state.field_read_limited);
        assert!(decision.state.field_write_limited);
        assert!(decision.state.record_limited);
    }
}

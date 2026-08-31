//! Payload policy registry for Flow business events, mirroring
//! `apps/api/src/forms/event_redaction.rs`'s `FORM_EVENT_PAYLOAD_POLICIES` pattern
//! (`events-v1.md` "稳定 envelope 与 audit 字段": "Flow 使用独立 `flow_event_payload_policy`
//! registry, 但复用同一递归 redactor 与 withheld 行为").
//!
//! Flow does not yet have a per-reader field-level read permission model (that is v0.5+
//! authorization scope, `flow_object_grants` is reserved-but-unused in v0.4 per
//! `migrations/0054_flow_data_layer.sql`), so there is nothing analogous to Forms' "reader denied
//! field X" to redact *within* a declared payload today. What this registry does guard, right
//! now, at delivery time, is the other half of the same contract clause: "每个 Flow event type
//! 必须在 payload policy registry 中显式声明；未声明的 producer 测试失败，未知 payload 在 delivery
//! 时整体 withheld" -- an event type with no declared policy has its whole payload withheld from
//! the webhook body rather than forwarded unreviewed, and a declared payload only forwards the
//! keys the policy actually lists (dropping anything an undeclared future producer field might
//! add, the same fail-closed discipline Forms applies).

#![allow(clippy::too_long_first_doc_paragraph)]

use serde_json::{Map, Value};

/// Marker inserted into a payload whenever [`redact_flow_event_payload_for_delivery`] withheld
/// something -- same shape and same key name as `forms::event_redaction::EVENT_REDACTED_MARKER`,
/// declared independently here because that module's helpers are private to it.
pub const EVENT_REDACTED_MARKER: &str = "field_permission_redacted";

/// What a single Flow event type may expose in its delivered payload.
pub struct EventPayloadPolicy {
    /// Payload keys that carry no restricted content. Flow's v0.4 event payloads are entirely
    /// ids/enums/counts (`events-v1.md` "`semantic_summary` 只允许 action、changed counts, stable
    /// ... ids" and the "禁止进入 envelope" list), so every declared Flow policy is this shape;
    /// unlike Forms there is no `value_maps` case yet (nothing emits a raw `{field: value}` map).
    pub public_keys: &'static [&'static str],
}

const fn public_payload(public_keys: &'static [&'static str]) -> EventPayloadPolicy {
    EventPayloadPolicy { public_keys }
}

/// Declared exposure for every Flow event type actually emitted in v0.4 (`apps/api/src/flow/
/// command.rs`, `apps/api/src/flow/collab/write.rs`). Adding a producer without adding an entry
/// here fails `every_emitted_flow_event_type_declares_a_payload_policy` below, and at delivery
/// time an undeclared event type withholds its whole payload instead of leaking it.
pub const FLOW_EVENT_PAYLOAD_POLICIES: &[(&str, EventPayloadPolicy)] = &[
    (
        "flow.object.created",
        public_payload(&["object_id", "object_type", "parent_object_id"]),
    ),
    ("flow.object.archived", public_payload(&["object_id", "status"])),
    ("flow.object.restored", public_payload(&["object_id", "status"])),
    (
        "flow.content.accepted",
        public_payload(&[
            "object_id",
            "document_id",
            "accepted_seq",
            "projection_seq",
            "changed_block_ids",
            "changed_block_ids_truncated",
        ]),
    ),
    ("flow.feature.enabled", public_payload(&["workspace_id"])),
    ("flow.feature.disabled", public_payload(&["workspace_id"])),
    (
        "flow.command.rejected",
        public_payload(&["action", "error_code", "object_id", "document_id"]),
    ),
    // `ADR-0012`'s v0.5 authorization surface (`events-v1.md`'s `flow_permission` rows). The
    // declared keys are exactly the registry's column list: ids, the `principal_kind` enum and
    // the four grade names -- no principal display name, email or any other identifying field,
    // matching "无 principal 明文标识以外的信息".
    (
        "flow.permission.granted",
        public_payload(&["object_id", "principal_kind", "principal_id", "level"]),
    ),
    (
        "flow.permission.revoked",
        public_payload(&["object_id", "principal_kind", "principal_id", "old_level", "new_level"]),
    ),
    (
        "flow.permission.inheritance_changed",
        public_payload(&["object_id", "inherit_from_parent"]),
    ),
];

/// Event type prefix owned by the Flow module, for the same completeness-scan role
/// `forms::event_redaction::FORM_EVENT_TYPE_PREFIXES` plays for Forms.
pub const FLOW_EVENT_TYPE_PREFIX: &str = "flow.";

/// Event type prefix of `ADR-0012`'s authorization events (the three `flow_permission` rows in
/// `events-v1.md`'s registry). Declared here rather than at its use site in `flow::grants` for a
/// concrete reason: [`event_type_literals`] scans that module for event-type-shaped literals, and
/// a bare prefix literal there would be reported as an event type with no declared policy.
pub const FLOW_PERMISSION_EVENT_TYPE_PREFIX: &str = "flow.permission.";

pub fn flow_event_payload_policy(event_type: &str) -> Option<&'static EventPayloadPolicy> {
    FLOW_EVENT_PAYLOAD_POLICIES
        .iter()
        .find(|(declared, _)| *declared == event_type)
        .map(|(_, policy)| policy)
}

fn withheld_object() -> Value {
    let mut object = Map::new();
    object.insert(EVENT_REDACTED_MARKER.to_string(), Value::Bool(true));
    Value::Object(object)
}

/// Fail-closed delivery-time filter: an event type with no declared policy has its entire payload
/// replaced by the withheld marker; a declared type keeps only the keys its policy lists (a
/// non-object payload, which no declared Flow policy should ever produce, is treated the same as
/// "no policy" and withheld rather than forwarded verbatim).
pub fn redact_flow_event_payload_for_delivery(event_type: &str, payload: &Value) -> Value {
    let Some(policy) = flow_event_payload_policy(event_type) else {
        return withheld_object();
    };
    let Value::Object(entries) = payload else {
        return withheld_object();
    };
    let mut kept = Map::new();
    for (key, value) in entries {
        if policy.public_keys.contains(&key.as_str()) {
            kept.insert(key.clone(), value.clone());
        }
    }
    Value::Object(kept)
}

/// Collect every string literal of `source` that looks like a Flow event type, the same
/// bracket-scanning approach `forms::event_redaction::event_type_literals` uses, line-scoped the
/// same way `scripts/verify-flow-events-v0.4.sh`'s own static check is: a line containing
/// `detected_by` is a `flow_integrity_records` subject marker (`ADR-0013` §4, e.g.
/// `detected_by: "flow.command.create_object"`), not an `events-v1.md` registry event type, and is
/// excluded rather than let the shared `flow.x.y` shape produce a false undeclared-policy report.
#[cfg(test)]
fn event_type_literals(source: &str) -> std::collections::BTreeSet<String> {
    let mut literals = std::collections::BTreeSet::new();
    for line in source.lines() {
        if line.contains("detected_by") {
            continue;
        }
        let mut chars = line.chars();
        while let Some(character) = chars.next() {
            if character != '"' {
                continue;
            }
            let mut literal = String::new();
            loop {
                match chars.next() {
                    Some('\\') => {
                        if chars.next().is_none() {
                            break;
                        }
                        literal.push('\\');
                    }
                    Some('"') | None => break,
                    Some(other) => literal.push(other),
                }
            }
            if looks_like_flow_event_type(&literal) {
                literals.insert(literal);
            }
        }
    }
    literals
}

#[cfg(test)]
fn looks_like_flow_event_type(literal: &str) -> bool {
    if !literal.starts_with(FLOW_EVENT_TYPE_PREFIX) {
        return false;
    }
    literal.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_' || character == '.'
    })
}

#[cfg(test)]
mod tests {
    use super::{
        EVENT_REDACTED_MARKER, FLOW_EVENT_PAYLOAD_POLICIES, event_type_literals, flow_event_payload_policy,
        redact_flow_event_payload_for_delivery,
    };
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn declared_type_keeps_only_its_declared_keys() {
        let payload =
            json!({ "object_id": "o1", "object_type": "page", "parent_object_id": null, "future_field": "leak" });
        let filtered = redact_flow_event_payload_for_delivery("flow.object.created", &payload);
        assert_eq!(filtered.get("object_id"), Some(&json!("o1")));
        assert_eq!(filtered.get("object_type"), Some(&json!("page")));
        assert!(
            filtered.get("future_field").is_none(),
            "a key the policy does not declare must never reach the delivered body"
        );
    }

    #[test]
    fn undeclared_event_type_withholds_the_whole_payload() {
        let payload = json!({ "object_id": "o1" });
        let filtered = redact_flow_event_payload_for_delivery("flow.made_up.event", &payload);
        assert_eq!(filtered, json!({ EVENT_REDACTED_MARKER: true }));
    }

    #[test]
    fn non_object_payload_is_withheld_even_for_a_declared_type() {
        let filtered = redact_flow_event_payload_for_delivery("flow.object.created", &json!(["leak"]));
        assert_eq!(filtered, json!({ EVENT_REDACTED_MARKER: true }));
    }

    #[test]
    fn policy_table_declares_each_event_type_once() {
        let mut seen = BTreeSet::new();
        for (event_type, _) in FLOW_EVENT_PAYLOAD_POLICIES {
            assert!(seen.insert(*event_type), "duplicate policy for {event_type}");
        }
    }

    #[test]
    fn declared_policies_are_reachable() {
        assert!(flow_event_payload_policy("flow.object.created").is_some());
        assert!(flow_event_payload_policy("nope").is_none());
    }

    /// Enumerates every Flow event type literal actually emitted in production code and fails
    /// when a producer emits one this registry does not declare -- the same completeness
    /// discipline `forms::event_redaction::every_emitted_form_event_type_declares_a_payload_policy`
    /// applies to Forms, and the exact check `verify-flow-events-v0.4.sh`'s static registry check
    /// independently recomputes from outside the crate.
    #[test]
    fn every_emitted_flow_event_type_declares_a_payload_policy() {
        let command_rs = include_str!("command.rs");
        let write_rs = include_str!("collab/write.rs");
        let grants_rs = include_str!("grants.rs");
        let mut literals = BTreeSet::new();
        for source in [command_rs, write_rs, grants_rs] {
            let cut = source.find("\n#[cfg(test)]").unwrap_or(source.len());
            literals.extend(event_type_literals(&source[..cut]));
        }
        assert!(
            literals.len() >= 5,
            "event type scan found too few literals: {}",
            literals.len()
        );
        let undeclared: Vec<&String> = literals
            .iter()
            .filter(|literal| flow_event_payload_policy(literal).is_none())
            .collect();
        assert!(
            undeclared.is_empty(),
            "these event types are emitted by flow::command/flow::collab::write/flow::grants but declare no payload policy: {undeclared:?}"
        );
    }
}

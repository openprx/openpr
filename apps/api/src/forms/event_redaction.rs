//! Field level read permissions for form business events.
//!
//! Event payloads are written by many producers and embed record field values in more shapes than
//! a key name allow list can track: raw maps (`values`), rendered strings (`title`, interpolated
//! from every field of the record), bare values promoted to the top level (`order.table_changed`)
//! and audit trails (`source.signature_media`). Guessing which key holds a value has already
//! failed once, so the rule here is inverted: a reader that is denied at least one field only sees
//! the payload keys its event type explicitly declares. Everything else - including keys a future
//! producer adds - is withheld, and `every_emitted_form_event_type_declares_a_payload_policy`
//! fails the build when a producer emits an event type that has no declaration at all.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// Marker inserted into a payload, source or metadata object whenever field level read permissions
/// removed something, so a client can tell "the producer never wrote this" from "you may not see it".
pub const EVENT_REDACTED_MARKER: &str = "field_permission_redacted";

/// Event `source` keys that never carry record field values. Everything else (client supplied
/// source objects, `signature_media` audit entries) is withheld from restricted readers.
const SAFE_EVENT_SOURCE_KEYS: [&str; 3] = ["type", "actor_id", "origin_event"];

/// Event `metadata` keys written by `insert_form_event`. Metadata is generated server side, but it
/// is filtered with the same allow list so a future producer cannot widen it silently.
const SAFE_EVENT_METADATA_KEYS: [&str; 5] = ["form_id", "form_key", "record_id", "legacy_form_events", "derived_from"];

/// What a single event type may expose to a reader with field level read denials.
pub struct EventPayloadPolicy {
    /// Payload keys that carry no record field value, raw or rendered.
    pub public_keys: &'static [&'static str],
    /// Payload keys holding a `{field_key: value}` map, kept minus the denied fields.
    pub value_maps: &'static [&'static str],
}

const fn public_payload(public_keys: &'static [&'static str]) -> EventPayloadPolicy {
    EventPayloadPolicy {
        public_keys,
        value_maps: &[],
    }
}

const fn record_payload(
    public_keys: &'static [&'static str],
    value_maps: &'static [&'static str],
) -> EventPayloadPolicy {
    EventPayloadPolicy {
        public_keys,
        value_maps,
    }
}

/// Declared exposure for every event type emitted by `routes::form`.
///
/// Adding a producer without adding an entry here is a test failure, and at runtime an undeclared
/// event type withholds its whole payload from restricted readers instead of leaking it.
pub const FORM_EVENT_PAYLOAD_POLICIES: &[(&str, EventPayloadPolicy)] = &[
    ("form.created", public_payload(&["form_id", "key", "name"])),
    (
        "form.duplicated",
        public_payload(&[
            "form_id",
            "key",
            "name",
            "source_form_id",
            "source_form_key",
            "copied_view_count",
        ]),
    ),
    (
        "form.created_from_template",
        public_payload(&["form_id", "key", "name", "scenario_template_key"]),
    ),
    ("form.updated", public_payload(&["form_id", "key", "changed_fields"])),
    ("form.archived", public_payload(&["form_id", "key", "name"])),
    ("form.restored", public_payload(&["form_id", "key", "name"])),
    (
        "form.view.created",
        public_payload(&["form_id", "view_id", "key", "view_type"]),
    ),
    (
        "form.view.updated",
        public_payload(&["form_id", "view_id", "key", "view_type", "changed_fields"]),
    ),
    (
        "form.view.archived",
        public_payload(&["form_id", "view_id", "key", "view_type"]),
    ),
    (
        "form.import_mapping_template.saved",
        public_payload(&["template_id", "header_signature", "shared"]),
    ),
    (
        "form.import_mapping_template.archived",
        public_payload(&["template_id", "header_signature", "shared"]),
    ),
    (
        "form.permissions.updated",
        public_payload(&["form_id", "updated_subjects"]),
    ),
    (
        "form.attachment.created",
        public_payload(&["attachment_id", "field_key"]),
    ),
    (
        "form.attachment.archived",
        public_payload(&["attachment_id", "field_key"]),
    ),
    (
        "form.attachment.restored",
        public_payload(&["attachment_id", "field_key"]),
    ),
    ("form.record.created", record_payload(&["record_id"], &["values"])),
    ("form.record.updated", record_payload(&["record_id"], &["values"])),
    ("form.record.recalculated", record_payload(&["record_id"], &["values"])),
    (
        "form.record.child_aggregate.updated",
        record_payload(&["record_id"], &["values"]),
    ),
    // `title` is rendered from every field of the record, so it never survives a read denial.
    ("form.record.archived", public_payload(&["record_id"])),
    ("form.record.restored", public_payload(&["record_id"])),
    ("form.record.linked", public_payload(&["link_id"])),
    (
        "form.record.child_created",
        public_payload(&["link_id", "parent_record_id", "child_record_id", "relation_key"]),
    ),
    // `body` is the comment the actor typed, not a record field value.
    (
        "form.record.comment.created",
        public_payload(&["record_id", "comment_id", "body"]),
    ),
    (
        "form.record.signature.retention_deleted",
        record_payload(
            &[
                "record_id",
                "deleted_count",
                "value_deleted_count",
                "replacement_deleted_count",
            ],
            &["values"],
        ),
    ),
    // The top level `table_id` / `previous_table_id` are bare field values and are withheld; the
    // permitted part of the same information survives inside `values`.
    ("order.table_changed", record_payload(&["record_id"], &["values"])),
    ("print_job.created", record_payload(&["record_id"], &["values"])),
];

/// Event type prefixes owned by the form module. Literals in `routes::form` that look like one of
/// these and are not a permission action must be declared in the policy table.
pub const FORM_EVENT_TYPE_PREFIXES: [&str; 3] = ["form.", "order.", "print_job."];

pub fn event_payload_policy(event_type: &str) -> Option<&'static EventPayloadPolicy> {
    FORM_EVENT_PAYLOAD_POLICIES
        .iter()
        .find(|(declared, _)| *declared == event_type)
        .map(|(_, policy)| policy)
}

/// Strip everything a reader denied `denied_read_fields` may not see from an event payload.
pub fn redact_event_payload(event_type: &str, payload: Value, denied_read_fields: &BTreeSet<String>) -> Value {
    if denied_read_fields.is_empty() {
        return payload;
    }
    let Some(policy) = event_payload_policy(event_type) else {
        return withheld_object();
    };
    let Value::Object(entries) = payload else {
        return withheld_object();
    };
    let mut kept = Map::new();
    let mut redacted = false;
    for (key, value) in entries {
        if policy.public_keys.contains(&key.as_str()) {
            kept.insert(key, value);
        } else if policy.value_maps.contains(&key.as_str()) {
            match value {
                Value::Object(map) => {
                    kept.insert(key, Value::Object(map));
                }
                // A value map that is not a map has an unexpected shape; withhold it rather than
                // guess what it carries.
                _ => redacted = true,
            }
        } else {
            redacted = true;
        }
    }
    let mut payload = Value::Object(kept);
    redacted |= strip_denied_keys_deep(&mut payload, denied_read_fields);
    mark_redacted(payload, redacted)
}

/// Strip everything a restricted reader may not see from an event `source` object.
pub fn redact_event_source(source: Value, denied_read_fields: &BTreeSet<String>) -> Value {
    retain_allowed_keys(source, &SAFE_EVENT_SOURCE_KEYS, denied_read_fields)
}

/// Strip everything a restricted reader may not see from an event `metadata` object.
pub fn redact_event_metadata(metadata: Value, denied_read_fields: &BTreeSet<String>) -> Value {
    retain_allowed_keys(metadata, &SAFE_EVENT_METADATA_KEYS, denied_read_fields)
}

fn retain_allowed_keys(value: Value, allowed_keys: &[&str], denied_read_fields: &BTreeSet<String>) -> Value {
    if denied_read_fields.is_empty() {
        return value;
    }
    let Value::Object(entries) = value else {
        return withheld_object();
    };
    let mut kept = Map::new();
    let mut redacted = false;
    for (key, value) in entries {
        if allowed_keys.contains(&key.as_str()) {
            kept.insert(key, value);
        } else {
            redacted = true;
        }
    }
    let mut value = Value::Object(kept);
    redacted |= strip_denied_keys_deep(&mut value, denied_read_fields);
    mark_redacted(value, redacted)
}

fn withheld_object() -> Value {
    let mut object = Map::new();
    object.insert(EVENT_REDACTED_MARKER.to_string(), Value::Bool(true));
    Value::Object(object)
}

fn mark_redacted(mut value: Value, redacted: bool) -> Value {
    if redacted && let Some(object) = value.as_object_mut() {
        object.insert(EVENT_REDACTED_MARKER.to_string(), Value::Bool(true));
    }
    value
}

/// Remove every entry keyed by a denied field from an already pruned document. The walk is
/// iterative so a deeply nested document cannot exhaust the stack.
fn strip_denied_keys_deep(value: &mut Value, denied_read_fields: &BTreeSet<String>) -> bool {
    let mut removed = false;
    let mut pending: Vec<&mut Value> = vec![value];
    while let Some(node) = pending.pop() {
        match node {
            Value::Object(entries) => {
                for denied_key in denied_read_fields {
                    if entries.remove(denied_key).is_some() {
                        removed = true;
                    }
                }
                pending.extend(entries.values_mut());
            }
            Value::Array(items) => pending.extend(items.iter_mut()),
            _ => {}
        }
    }
    removed
}

/// Collect every string literal of `source` that looks like a form event type.
#[cfg(test)]
fn event_type_literals(source: &str) -> BTreeSet<String> {
    let mut literals = BTreeSet::new();
    let mut chars = source.chars();
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
        if looks_like_event_type(&literal) {
            literals.insert(literal);
        }
    }
    literals
}

#[cfg(test)]
fn looks_like_event_type(literal: &str) -> bool {
    if !FORM_EVENT_TYPE_PREFIXES
        .iter()
        .any(|prefix| literal.starts_with(prefix))
    {
        return false;
    }
    literal.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_' || character == '.'
    })
}

#[cfg(test)]
mod tests {
    use super::{
        EVENT_REDACTED_MARKER, FORM_EVENT_PAYLOAD_POLICIES, event_payload_policy, event_type_literals,
        redact_event_metadata, redact_event_payload, redact_event_source,
    };
    use crate::forms::permissions::FORM_PERMISSION_ACTIONS;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;

    fn denied(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    #[test]
    fn readers_without_denials_see_the_untouched_event() {
        let payload = json!({ "record_id": "r1", "title": "Alice earns 9000", "values": { "salary": 9000 } });
        let filtered = redact_event_payload("form.record.archived", payload.clone(), &BTreeSet::new());
        assert_eq!(filtered, payload);
    }

    #[test]
    fn archived_record_event_withholds_the_rendered_title() {
        let payload = json!({ "record_id": "r1", "title": "Alice earns 9000" });
        let filtered = redact_event_payload("form.record.archived", payload, &denied(&["salary"]));
        assert_eq!(filtered.get("record_id"), Some(&json!("r1")));
        assert!(filtered.get("title").is_none());
        assert_eq!(filtered.get(EVENT_REDACTED_MARKER), Some(&json!(true)));
    }

    #[test]
    fn restored_record_event_withholds_the_rendered_title() {
        let payload = json!({ "record_id": "r1", "title": "Alice earns 9000" });
        let filtered = redact_event_payload("form.record.restored", payload, &denied(&["salary"]));
        assert!(filtered.get("title").is_none());
    }

    #[test]
    fn order_table_change_withholds_top_level_field_values() {
        let payload = json!({
            "record_id": "r1",
            "previous_table_id": "T1",
            "table_id": "T2",
            "values": { "table_id": "T2", "guests": 4 }
        });
        let filtered = redact_event_payload("order.table_changed", payload, &denied(&["table_id"]));
        assert!(filtered.get("table_id").is_none());
        assert!(filtered.get("previous_table_id").is_none());
        assert_eq!(
            filtered.get("values").and_then(|values| values.get("guests")),
            Some(&json!(4))
        );
        assert!(
            filtered
                .get("values")
                .and_then(|values| values.get("table_id"))
                .is_none()
        );
    }

    #[test]
    fn signature_audit_entries_are_withheld_from_the_event_source() {
        let source = json!({
            "type": "user",
            "actor_id": "a1",
            "signature_media": {
                "entries": [{
                    "field_key": "signature",
                    "url": "/api/v1/uploads/signatures/signature-signature-1.png",
                    "previous_url": "/api/v1/uploads/signatures/signature-signature-0.png",
                    "reason": "approved by the CFO"
                }]
            }
        });
        let filtered = redact_event_source(source, &denied(&["signature"]));
        assert_eq!(filtered.get("type"), Some(&json!("user")));
        assert!(filtered.get("signature_media").is_none());
        assert_eq!(filtered.get(EVENT_REDACTED_MARKER), Some(&json!(true)));
    }

    #[test]
    fn client_supplied_source_keys_are_withheld() {
        let source = json!({ "type": "user", "imported_row": { "salary": 9000 } });
        let filtered = redact_event_source(source, &denied(&["salary"]));
        assert!(filtered.get("imported_row").is_none());
    }

    #[test]
    fn metadata_keeps_only_the_generated_keys() {
        let metadata = json!({ "form_id": "f1", "record_id": "r1", "legacy_form_events": true, "extra": "leak" });
        let filtered = redact_event_metadata(metadata, &denied(&["salary"]));
        assert_eq!(filtered.get("form_id"), Some(&json!("f1")));
        assert!(filtered.get("extra").is_none());
    }

    #[test]
    fn undeclared_event_type_withholds_the_whole_payload() {
        let payload = json!({ "record_id": "r1", "salary_snapshot": 9000 });
        let filtered = redact_event_payload("form.record.invented_by_a_new_producer", payload, &denied(&["salary"]));
        assert_eq!(filtered, json!({ EVENT_REDACTED_MARKER: true }));
    }

    #[test]
    fn value_maps_keep_permitted_fields() {
        let payload = json!({ "record_id": "r1", "values": { "salary": 9000, "name": "Alice" } });
        let filtered = redact_event_payload("form.record.updated", payload, &denied(&["salary"]));
        assert_eq!(
            filtered.get("values"),
            Some(&json!({ "name": "Alice" })),
            "permitted fields must survive"
        );
        assert_eq!(filtered.get(EVENT_REDACTED_MARKER), Some(&json!(true)));
    }

    #[test]
    fn denied_keys_are_removed_at_any_depth_of_a_kept_subtree() {
        let payload = json!({
            "record_id": "r1",
            "values": { "rows": [{ "salary": 9000, "name": "Alice" }] }
        });
        let filtered = redact_event_payload("form.record.created", payload, &denied(&["salary"]));
        let serialized = filtered.to_string();
        assert!(!serialized.contains("salary"), "denied key survived: {serialized}");
        assert!(serialized.contains("Alice"));
    }

    #[test]
    fn non_object_payloads_are_withheld() {
        let filtered = redact_event_payload("form.record.created", json!(["leak"]), &denied(&["salary"]));
        assert_eq!(filtered, json!({ EVENT_REDACTED_MARKER: true }));
    }

    #[test]
    fn policy_table_declares_each_event_type_once() {
        let mut seen = BTreeSet::new();
        for (event_type, _) in FORM_EVENT_PAYLOAD_POLICIES {
            assert!(seen.insert(*event_type), "duplicate policy for {event_type}");
        }
    }

    /// Enumerates every form event type literal in `routes::form` and fails when a producer emits
    /// one that no policy declares, so a new payload key can never be added unnoticed.
    #[test]
    fn every_emitted_form_event_type_declares_a_payload_policy() {
        let source = include_str!("../routes/form.rs");
        let literals = event_type_literals(source);
        assert!(
            literals.len() > 20,
            "event type scan found too few literals: {}",
            literals.len()
        );
        let undeclared: Vec<&String> = literals
            .iter()
            .filter(|literal| !FORM_PERMISSION_ACTIONS.contains(&literal.as_str()))
            .filter(|literal| event_payload_policy(literal).is_none())
            .collect();
        assert!(
            undeclared.is_empty(),
            "these event types are emitted by routes::form but declare no payload policy: {undeclared:?}"
        );
    }

    #[test]
    fn event_type_scan_reads_literals_and_ignores_other_strings() {
        let source = r#"
            insert_form_event(&tx, &form, None, "form.example.created", actor, source, json!({}));
            let message = "not an event type";
            let escaped = "quote \" inside";
            let action = "form.view";
        "#;
        let literals = event_type_literals(source);
        assert!(literals.contains("form.example.created"));
        assert!(literals.contains("form.view"));
        assert!(!literals.contains("not an event type"));
    }

    #[test]
    fn declared_policies_are_reachable() {
        assert!(event_payload_policy("form.record.created").is_some());
        assert!(event_payload_policy("nope").is_none());
    }

    #[test]
    fn payload_stays_an_object_after_redaction() {
        let filtered: Value =
            redact_event_payload("form.record.linked", json!({ "link_id": "l1" }), &denied(&["salary"]));
        assert!(filtered.is_object());
    }
}

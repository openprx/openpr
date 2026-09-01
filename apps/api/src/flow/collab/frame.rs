//! Wire shapes for `contracts/collab-protocol-v1.md`'s eleven control frames.
//!
//! Encoding choice (the contract leaves this to `ADR-0005`, which is engine-*encoding* scoped,
//! not transport-envelope scoped): every frame is one JSON text WebSocket message, tagged by
//! `type` (`rename_all = "snake_case"` gives exactly `hello`/`open`/`snapshot`/`update`/
//! `accepted`/`rejected`/`presence`/`ack`/`resync`/`ping`/`pong`). Opaque byte fields (`snapshot`,
//! `bytes`, `*_frontier`) are base64 (`base64::engine::general_purpose::STANDARD`), matching the
//! convention `apps/api/src/flow/projection.rs::encode_frontier` already uses for the REST
//! surface. This keeps one encoding for the whole frame regardless of which CRDT engine produced
//! the inner bytes, and is trivial to exercise from a plain WebSocket text-frame test client.
//!
//! Every frame carries `protocol_version` (contract: "所有 frame 带 `protocol_version`") and
//! `document_id`: the field table in the contract lists only the fields that differ frame to
//! frame, but a real connection needs to know which document a frame is about, so both are common
//! envelope fields here rather than being repeated as frame-specific ones. v0.4 (this package)
//! additionally scopes one WebSocket connection to exactly the one `document_id` its ticket was
//! issued for (`ADR-0007`: a ticket already carries a single `document_id`) — the
//! `presence_entries_per_connection_max = 8` ceiling in `limits-v1.md` anticipates a future
//! multi-document connection that this package does not implement; see the collab module's report
//! for that scope note.

#![allow(clippy::too_long_first_doc_paragraph)]

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// The only protocol version this server speaks. `hello.protocol_version` must equal this, or the
/// connection fails closed with `unsupported_protocol` (contract: "未知 required capability、过大、
/// 损坏或版本不兼容 frame 必须失败关闭,不得部分应用").
pub const PROTOCOL_VERSION: u32 = 1;

/// One accepted update, as embedded in a `snapshot` frame's `tail_updates`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailUpdate {
    pub seq: i64,
    pub update_id: Uuid,
    /// Base64 of the raw CRDT update bytes (`collab_updates.bytes`).
    pub bytes: String,
    pub before_frontier: String,
    pub after_frontier: String,
}

/// `rejected.code` — the frozen error-semantics vocabulary (`collab-protocol-v1.md` "错误语义").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectedCode {
    Unauthenticated,
    Forbidden,
    FeatureDisabled,
    NotFound,
    UnsupportedProtocol,
    StaleFrontier,
    InvalidUpdate,
    PolicyRejected,
    LimitExceeded,
    ResyncRequired,
    /// A deterministic, permanent server-side refusal of **this one update**
    /// (`collab-protocol-v1.md`, 2026-09-01). `recoverable` is always `false` and the connection
    /// is kept: what is permanently refused is the update, not the session.
    ServerRejected,
    ServerDraining,
}

/// The one `server_rejected.details.reason` value this package produces: the database refused the
/// write with a `SQLSTATE` that classifies as deterministic (`error::classify_sqlstate`) -- a
/// constraint violation, a data exception, or a schema error.
///
/// `error-mapping-v1.md` freezes no `details` shape for `server_rejected` beyond "只含安全的分类
/// 信息,不回显驱动错误原文", so this is a *classification*, deliberately coarse: it names the
/// family the refusal belongs to and nothing a caller could use to read back the offending data,
/// the constraint name, or the driver's message. The field name reuses the `reason` spelling
/// `server_draining` already froze (contract: "不为同一个概念造第二种拼写").
pub const SERVER_REJECTED_REASON_DATABASE: &str = "deterministic_database_refusal";

/// `rejected.write_state` — the required "did the server change anything?" discriminant
/// (`collab-protocol-v1.md`, 2026-08-30: "`recoverable` 只回答「能不能重试」，不回答「服务端状态改了
/// 没有」，而这两件事必须分开").
///
/// It is deliberately *not* derivable from `code` + `recoverable`: `stale_frontier` and
/// `server_draining{contention}` are both `{recoverable:true}` yet the first provably wrote
/// nothing while the second historically could be returned after a commit had already landed.
///
/// Producers must classify every rejection point explicitly. [`Self::Unknown`] is not a default:
/// the contract forbids using it to cover all uncertainty ("能确定未写的路径必须报 `not_applied`,
/// 否则客户端会失去可以安全重编码的能力"), because a client that sees `unknown` is obliged to retry
/// under the *same* `update_id` and must not re-encode — an obligation that costs it the ability
/// to rebase, so it may only be imposed where the server genuinely cannot tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteState {
    /// The server is certain no canonical state was written: the rejection happened before any
    /// write was issued, or the transaction that issued them was rolled back. The client may
    /// safely retry, including with re-encoded bytes and a fresh `update_id`.
    NotApplied,
    /// The rejection was produced at a point where the write may already have been durably
    /// committed. The client must retry under the same `update_id` and must not re-encode the
    /// bytes: a fresh id walks past both `(document_id,update_id)` and `(document_id,content_hash)`
    /// and applies the same operations twice.
    Unknown,
}

/// `server_draining.details.reason` (contract: "两者不得互换,缺失/未知 reason 违反协议").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrainReason {
    Drain,
    Contention,
}

/// The one shared producer value for an instance/workspace drain. REST admission, active and
/// handshake-phase WebSocket sessions, MCP/CLI (through REST), and UI consumers all derive their wire
/// payload from this value, so `reason`, retry advice, and close metadata cannot drift by surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainSignal {
    pub retry_after_ms: u64,
}

impl DrainSignal {
    #[must_use]
    pub const fn new(retry_after_ms: u64) -> Self {
        Self { retry_after_ms }
    }

    #[must_use]
    pub fn details(self) -> Value {
        serde_json::json!({
            "reason": "drain",
            "retry_after_ms": self.retry_after_ms,
        })
    }

    #[must_use]
    pub fn close_reason(self) -> String {
        format!(r#"{{"reason":"drain","retry_after_ms":{}}}"#, self.retry_after_ms)
    }
}

// `Presence.payload`/`Rejected.details` are `serde_json::Value`, which has no `Eq` impl, so this
// enum can only be `PartialEq`, not `Eq` -- test-only equality assertions (`session::database_tests`'
// backfilled-frame ordering checks) are all this derive exists for.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Hello {
        protocol_version: u32,
        capabilities: Vec<String>,
        client_id: String,
        session_id: Uuid,
    },
    Open {
        protocol_version: u32,
        document_id: Uuid,
        known_seq: Option<i64>,
        known_frontier: Option<String>,
    },
    Snapshot {
        protocol_version: u32,
        document_id: Uuid,
        snapshot_seq: i64,
        head_seq: i64,
        /// Base64 of the full document snapshot bytes.
        snapshot: String,
        tail_updates: Vec<TailUpdate>,
        head_frontier: String,
    },
    Update {
        protocol_version: u32,
        document_id: Uuid,
        update_id: Uuid,
        base_frontier: String,
        /// Base64 of the raw CRDT update bytes this client produced locally.
        bytes: String,
        idempotency_key: Option<String>,
        origin: String,
        message: Option<String>,
    },
    Accepted {
        protocol_version: u32,
        document_id: Uuid,
        update_id: Uuid,
        head_seq: i64,
        head_frontier: String,
        projection_seq: i64,
        event_id: Uuid,
    },
    Rejected {
        protocol_version: u32,
        document_id: Uuid,
        update_id: Option<Uuid>,
        code: RejectedCode,
        recoverable: bool,
        /// Required (`collab-protocol-v1.md`): never skipped on the wire, never defaulted.
        write_state: WriteState,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        current_seq: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        current_frontier: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        audit_event_id: Option<Uuid>,
    },
    Presence {
        protocol_version: u32,
        document_id: Uuid,
        session_id: Uuid,
        payload: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        ttl_seconds: Option<u32>,
    },
    Ack {
        protocol_version: u32,
        document_id: Uuid,
        seq: i64,
        frontier: String,
    },
    Resync {
        protocol_version: u32,
        document_id: Uuid,
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        minimum_snapshot_seq: Option<i64>,
    },
    Ping {
        protocol_version: u32,
        nonce: String,
    },
    Pong {
        protocol_version: u32,
        nonce: String,
    },
}

impl Frame {
    #[must_use]
    pub const fn protocol_version(&self) -> u32 {
        match self {
            Self::Hello { protocol_version, .. }
            | Self::Open { protocol_version, .. }
            | Self::Snapshot { protocol_version, .. }
            | Self::Update { protocol_version, .. }
            | Self::Accepted { protocol_version, .. }
            | Self::Rejected { protocol_version, .. }
            | Self::Presence { protocol_version, .. }
            | Self::Ack { protocol_version, .. }
            | Self::Resync { protocol_version, .. }
            | Self::Ping { protocol_version, .. }
            | Self::Pong { protocol_version, .. } => *protocol_version,
        }
    }
}

#[must_use]
pub fn encode_bytes(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

/// # Errors
/// If `raw` is not valid base64.
pub fn decode_bytes(raw: &str) -> Result<Vec<u8>, base64::DecodeError> {
    BASE64.decode(raw)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::{DrainReason, DrainSignal, Frame, PROTOCOL_VERSION, RejectedCode, decode_bytes, encode_bytes};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn hello_round_trips_and_uses_snake_case_tag() {
        let frame = Frame::Hello {
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec!["presence".to_string()],
            client_id: "client-1".to_string(),
            session_id: Uuid::new_v4(),
        };
        let json = serde_json::to_value(&frame).expect("serializes");
        assert_eq!(json["type"], "hello");
        let round_tripped: Frame = serde_json::from_value(json).expect("deserializes");
        assert_eq!(round_tripped.protocol_version(), PROTOCOL_VERSION);
    }

    #[test]
    fn rejected_code_and_drain_reason_use_the_frozen_snake_case_vocabulary() {
        assert_eq!(
            serde_json::to_value(RejectedCode::PolicyRejected).unwrap(),
            json!("policy_rejected")
        );
        assert_eq!(
            serde_json::to_value(RejectedCode::ServerDraining).unwrap(),
            json!("server_draining")
        );
        // The 2026-09-01 addition. `server_rejected` and `server_draining` share a prefix and
        // opposite recovery semantics, so a wire value that drifted onto the wrong one of the two
        // would be the single most damaging typo in this enum.
        assert_eq!(
            serde_json::to_value(RejectedCode::ServerRejected).unwrap(),
            json!("server_rejected")
        );
        assert_ne!(
            serde_json::to_value(RejectedCode::ServerRejected).unwrap(),
            serde_json::to_value(RejectedCode::ServerDraining).unwrap()
        );
        assert_eq!(
            serde_json::to_value(DrainReason::Contention).unwrap(),
            json!("contention")
        );
        let signal = DrainSignal::new(1_500);
        assert_eq!(signal.details(), json!({"reason": "drain", "retry_after_ms": 1_500}));
        assert_eq!(signal.close_reason(), r#"{"reason":"drain","retry_after_ms":1500}"#);
    }

    #[test]
    fn bytes_encoding_round_trips() {
        let original = vec![1u8, 2, 3, 250, 255];
        let encoded = encode_bytes(&original);
        let decoded = decode_bytes(&encoded).expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn an_unknown_frame_type_fails_to_deserialize_rather_than_partially_apply() {
        let raw = json!({"type": "not_a_real_frame", "protocol_version": 1});
        assert!(serde_json::from_value::<Frame>(raw).is_err());
    }
}

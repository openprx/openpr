#![allow(clippy::too_long_first_doc_paragraph)]

use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::response::{ApiResponse, OperationResponseMeta};

pub fn request_lang(headers: &HeaderMap) -> &str {
    headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("zh")
}

pub fn localize_error(msg: &str, lang: &str) -> String {
    if lang.starts_with("en") {
        match msg {
            "content is required" => "Content is required".to_string(),
            "content cannot be empty" => "Content cannot be empty".to_string(),
            "issue not found or access denied" => "Issue not found or access denied".to_string(),
            _ => msg.to_string(),
        }
    } else {
        match msg {
            "content is required" | "content cannot be empty" => "内容不能为空".to_string(),
            "issue not found or access denied" => "Issue 未找到或无权限".to_string(),
            _ => msg.to_string(),
        }
    }
}

/// `server_draining.details.reason` (`contracts/error-mapping-v1.md`: "`drain` 只用于实例/workspace
/// 正在停止接收或排空连接；`contention` 只用于 document lock wait/hold、rebase exhaustion 或
/// snapshot hard-trigger 的瞬时竞争。缺失/未知 reason 是 producer contract violation"). These two
/// carry *opposite* recovery semantics (permanent-ish drain vs. immediately-retryable contention)
/// and must never be collapsed into one terminal state -- see this package's own
/// `flow::command::map_write_rejection` for the fix this type exists to prevent from regressing:
/// folding both into a single `ApiError::Conflict` string once made an ordinary transient
/// `lock_timeout` masquerade as a permanent authorization failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerDrainingReason {
    /// The instance/workspace is actively stopping or shedding connections. Not retryable against
    /// the same server process; a client should reconnect (possibly to a different instance)
    /// after `retry_after_ms`.
    Drain,
    /// A transient, single-writer contention window (document lock wait/hold, rebase exhaustion,
    /// or a snapshot hard-trigger). Retryable almost immediately against the same server; never a
    /// sign of maintenance.
    Contention,
}

impl ServerDrainingReason {
    /// The frozen wire value (`error-mapping-v1.md`'s `details.reason`), also used as the
    /// `snake_case` value `flow::collab::frame::DrainReason` serializes to on the WebSocket wire --
    /// kept as an independent enum here (rather than importing that one) because `error.rs` sits
    /// below `flow` in the dependency graph and must not import from it.
    #[must_use]
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::Drain => "drain",
            Self::Contention => "contention",
        }
    }
}

/// Stable, transport-independent error discriminant -- one variant per frozen "Stable semantic"
/// row of `contracts/error-mapping-v1.md`'s "稳定错误的五层映射" table, plus [`Self::Unclassified`]
/// for the pre-existing string-typed [`ApiError`] variants this type is layered on top of without
/// breaking their many existing call sites.
///
/// This is the single source of truth every consumer that needs to branch on *why* a request
/// failed (rather than just its HTTP status bucket) should use instead of matching on
/// [`ApiError`]'s human-readable message string: the WebSocket close-code mapping and the CLI
/// exit-code mapping both derive their tables from this type's own [`Self::ws_close_code`] /
/// [`Self::cli_exit_code`] methods, so the frozen contract's numbers live in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    /// No stable discriminant is available -- the default [`ApiError::kind`] for legacy call
    /// sites (`BadRequest`/`Conflict`/`Internal`/`Database`) that predate this contract and have
    /// not yet been migrated to [`ApiError::typed`]. Callers must not treat this as a real
    /// `error-mapping-v1.md` row: it carries none of that table's numbers and exists only so
    /// `ApiError::kind` can be total.
    Unclassified,
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
    /// Carries the required `reason` discriminant (`error-mapping-v1.md`: "`server_draining.
    /// details.reason` 在 REST、MCP、CLI 与 WS control/close metadata 中都是 required") --
    /// deliberately part of the enum's own shape, not a side-channel string, so a caller cannot
    /// construct or match a `ServerDraining` without naming which reason it is.
    ServerDraining(ServerDrainingReason),
    ChecksumMismatch,
    UnsupportedFormat,
}

impl ApiErrorKind {
    /// The bare stable-semantic string (`error-mapping-v1.md`'s "Stable semantic" column, and the
    /// MCP/CLI JSON `error.code`/`error_code` value). `ServerDraining`'s two reasons share one
    /// stable code -- the reason itself is a separate field in the wire `details`, matching the
    /// contract's "MCP error structure" column: "recoverable=true，原样保留 required `reason`".
    #[must_use]
    pub const fn stable_code(self) -> &'static str {
        match self {
            Self::Unclassified => "unclassified",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::FeatureDisabled => "feature_disabled",
            Self::NotFound => "not_found",
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::StaleFrontier => "stale_frontier",
            Self::InvalidUpdate => "invalid_update",
            Self::PolicyRejected => "policy_rejected",
            Self::LimitExceeded => "limit_exceeded",
            Self::ResyncRequired => "resync_required",
            Self::ServerDraining(_) => "server_draining",
            Self::ChecksumMismatch => "checksum_mismatch",
            Self::UnsupportedFormat => "unsupported_format",
        }
    }

    /// The numeric envelope `code`/HTTP-status-shaped business code (`error-mapping-v1.md`'s
    /// "REST `ApiError` / envelope code / HTTP" column; REST itself always transports `HTTP 200`
    /// per that column's wire rule, this is the *body* `code`, matching the pre-existing
    /// `BadRequest`→400/`Forbidden`→403/etc. convention the legacy variants already use).
    /// [`Self::Unclassified`] returns `500` as a conservative placeholder; it is never actually
    /// read for that variant because [`ApiError::into_response`] only calls this method inside
    /// the `Typed` arm, and legacy variants compute their own status directly.
    #[must_use]
    pub const fn http_status_code(self) -> i32 {
        match self {
            Self::Unclassified => 500,
            Self::Unauthenticated => 401,
            Self::Forbidden | Self::FeatureDisabled | Self::PolicyRejected => 403,
            Self::NotFound => 404,
            Self::UnsupportedProtocol
            | Self::InvalidUpdate
            | Self::LimitExceeded
            | Self::ChecksumMismatch
            | Self::UnsupportedFormat => 400,
            Self::StaleFrontier | Self::ResyncRequired | Self::ServerDraining(_) => 409,
        }
    }

    /// A short, fixed phrase for [`OperationResponseMeta::error_summary`], matching the shape the
    /// pre-existing legacy match arms already use ("bad request", "forbidden", ...).
    #[must_use]
    pub const fn error_summary(self) -> &'static str {
        match self {
            Self::Unclassified => "unclassified error",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::FeatureDisabled => "feature disabled",
            Self::NotFound => "not found",
            Self::UnsupportedProtocol => "unsupported protocol",
            Self::StaleFrontier => "stale frontier",
            Self::InvalidUpdate => "invalid update",
            Self::PolicyRejected => "policy rejected",
            Self::LimitExceeded => "limit exceeded",
            Self::ResyncRequired => "resync required",
            Self::ServerDraining(ServerDrainingReason::Drain) => "server draining",
            Self::ServerDraining(ServerDrainingReason::Contention) => "server busy",
            Self::ChecksumMismatch => "checksum mismatch",
            Self::UnsupportedFormat => "unsupported format",
        }
    }

    /// The CLI exit code (`error-mapping-v1.md`'s "CLI 退出码全集" table). `Unclassified` returns
    /// `1` -- a generic failure code, deliberately *not* one of the table's reserved 0/2-10 values
    /// (each of which has a frozen, specific meaning a legacy/unconverted error must not claim).
    #[must_use]
    pub const fn cli_exit_code(self) -> u8 {
        match self {
            Self::Unclassified => 1,
            Self::Unauthenticated => 3,
            Self::Forbidden | Self::FeatureDisabled | Self::PolicyRejected => 4,
            Self::NotFound => 5,
            Self::StaleFrontier | Self::ResyncRequired => 6,
            Self::UnsupportedProtocol | Self::InvalidUpdate | Self::ChecksumMismatch | Self::UnsupportedFormat => 7,
            Self::LimitExceeded => 8,
            // Both reasons share exit 9 (`error-mapping-v1.md`: "两种 reason 不拆退出码，JSON 保留
            // discriminator") -- the discriminator survives in the JSON body's `details.reason`,
            // not in the exit code.
            Self::ServerDraining(_) => 9,
        }
    }

    /// The WebSocket close code this rejection uses **when the connection is actually closed**
    /// for it (`error-mapping-v1.md`'s "WS close/control" column). `None` means the wire uses a
    /// `rejected`/`resync` control frame with the connection kept open instead of a close --
    /// `stale_frontier`, `resync_required`, `invalid_update` (closes only after repeated
    /// failures, at code `4400`, outside this frozen 4401/4403/4404/4406/4408/4410 set),
    /// `server_draining` with reason `contention`, and the REST/import-export-only
    /// `checksum_mismatch`/`unsupported_format` (contract: "n/a").
    #[must_use]
    pub const fn ws_close_code(self) -> Option<u16> {
        match self {
            Self::Unauthenticated => Some(4401),
            // `forbidden` closes at handshake; `policy_rejected` closes only when permission is
            // revoked mid-session (contract: "权限撤销时 4403") -- both use the same code.
            Self::Forbidden | Self::PolicyRejected => Some(4403),
            Self::FeatureDisabled | Self::NotFound => Some(4404),
            Self::UnsupportedProtocol => Some(4406),
            // Only when connection-level flooding triggers a close (contract: "单 update 可
            // rejected，连接洪泛则 close"); a single over-limit update stays a control frame.
            Self::LimitExceeded => Some(4408),
            Self::ServerDraining(ServerDrainingReason::Drain) => Some(4410),
            Self::Unclassified
            | Self::StaleFrontier
            | Self::InvalidUpdate
            | Self::ResyncRequired
            | Self::ServerDraining(ServerDrainingReason::Contention)
            | Self::ChecksumMismatch
            | Self::UnsupportedFormat => None,
        }
    }

    /// The fixed `recoverable` value `error-mapping-v1.md`'s MCP/UI columns state explicitly.
    /// Where the contract leaves it caller/context-dependent (`policy_rejected`: "recoverable 取决
    /// 于 policy"; `limit_exceeded`: no fixed value given), this returns the conservative default
    /// (`false`) rather than guessing -- a caller with more context (e.g. a `retry_after_ms` on
    /// the concrete error) may still report `true` on the wire itself without this method lying by
    /// default.
    #[must_use]
    pub const fn recoverable(self) -> bool {
        match self {
            Self::Unauthenticated | Self::StaleFrontier | Self::ResyncRequired | Self::ServerDraining(_) => true,
            Self::Unclassified
            | Self::Forbidden
            | Self::FeatureDisabled
            | Self::NotFound
            | Self::UnsupportedProtocol
            | Self::InvalidUpdate
            | Self::PolicyRejected
            | Self::LimitExceeded
            | Self::ChecksumMismatch
            | Self::UnsupportedFormat => false,
        }
    }

    /// The UI i18n key (`error-mapping-v1.md`'s "UI key 与动作" column). `server_draining` is the
    /// one code whose key is reason-suffixed (contract: "`server_draining` 唯一 discriminator 是
    /// required `details.reason`" -- `drain`→`...drain`, `contention`→`...contention`, never a
    /// shared key a client would have to re-branch on `message` to disambiguate).
    #[must_use]
    pub const fn ui_key(self) -> &'static str {
        match self {
            Self::Unclassified => "flow.error.unclassified",
            Self::Unauthenticated => "flow.error.unauthenticated",
            Self::Forbidden => "flow.error.forbidden",
            Self::FeatureDisabled => "flow.error.feature_disabled",
            Self::NotFound => "flow.error.not_found",
            Self::UnsupportedProtocol => "flow.error.unsupported_protocol",
            Self::StaleFrontier => "flow.error.stale_frontier",
            Self::InvalidUpdate => "flow.error.invalid_update",
            Self::PolicyRejected => "flow.error.policy_rejected",
            Self::LimitExceeded => "flow.error.limit_exceeded",
            Self::ResyncRequired => "flow.error.resync_required",
            Self::ServerDraining(ServerDrainingReason::Drain) => "flow.error.server_draining.drain",
            Self::ServerDraining(ServerDrainingReason::Contention) => "flow.error.server_draining.contention",
            Self::ChecksumMismatch => "flow.error.checksum_mismatch",
            Self::UnsupportedFormat => "flow.error.unsupported_format",
        }
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("internal server error")]
    Internal,
    #[error("database error")]
    Database(#[from] sea_orm::DbErr),
    /// A rejection carrying an explicit, stable [`ApiErrorKind`] discriminant alongside a
    /// caller-safe message and optional structured `details` (`error-mapping-v1.md`'s per-code
    /// `details` shapes, e.g. `stale_frontier`'s `{current_seq,current_frontier}` or
    /// `server_draining`'s `{reason,retry_after_ms}`). New call sites that need a machine-readable
    /// reason (WS close-code mapping, CLI exit codes, `flow.command.rejected` audit payloads)
    /// should construct this via [`ApiError::typed`]/the `ApiError::policy_rejected`-style
    /// constructors below instead of the string-typed variants above, which stay exactly as they
    /// are for their hundreds of existing call sites across the rest of the app.
    #[error("{message}")]
    Typed {
        kind: ApiErrorKind,
        message: String,
        details: Option<Value>,
    },
}

impl ApiError {
    /// Constructs a [`Self::Typed`] error with no structured `details`.
    pub fn typed(kind: ApiErrorKind, message: impl Into<String>) -> Self {
        Self::Typed {
            kind,
            message: message.into(),
            details: None,
        }
    }

    fn typed_with_details(kind: ApiErrorKind, message: impl Into<String>, details: Value) -> Self {
        Self::Typed {
            kind,
            message: message.into(),
            details: Some(details),
        }
    }

    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::typed(ApiErrorKind::Unauthenticated, message)
    }

    pub fn feature_disabled(message: impl Into<String>) -> Self {
        Self::typed(ApiErrorKind::FeatureDisabled, message)
    }

    pub fn unsupported_protocol(message: impl Into<String>) -> Self {
        Self::typed(ApiErrorKind::UnsupportedProtocol, message)
    }

    pub fn invalid_update(message: impl Into<String>) -> Self {
        Self::typed(ApiErrorKind::InvalidUpdate, message)
    }

    /// `invalid_update` carrying a machine-readable `details.reason`.
    ///
    /// `error-mapping-v1.md` freezes no `details` shape for `invalid_update`, but `ADR-0013` §2.2
    /// and `rest-api-v1.md`'s `move_object` clause both name a *reason code*
    /// (`subtree_spans_multiple_projects`) that a caller has to branch on, and the same document's
    /// first rule is "禁止用英文 message 分支". A reason code that only exists inside the message
    /// string would violate exactly that. The field name mirrors `server_draining`'s
    /// `details.reason`, which is the one reason discriminator the contract has already frozen, so
    /// this does not invent a second spelling for the same idea.
    pub fn invalid_update_with_details(message: impl Into<String>, details: Value) -> Self {
        Self::typed_with_details(ApiErrorKind::InvalidUpdate, message, details)
    }

    pub fn policy_rejected(message: impl Into<String>) -> Self {
        Self::typed(ApiErrorKind::PolicyRejected, message)
    }

    /// `policy_rejected` carrying the structured `details` `rest-api-v1.md`'s self-lockout clause
    /// requires: "`details` 只含档位变化摘要，不泄漏其它 principal 的身份以外信息". The caller owns
    /// what goes in — this constructor deliberately does not assemble it, so the one place that
    /// knows which fields are safe stays the one place that decides.
    pub fn policy_rejected_with_details(message: impl Into<String>, details: Value) -> Self {
        Self::typed_with_details(ApiErrorKind::PolicyRejected, message, details)
    }

    pub fn checksum_mismatch(message: impl Into<String>) -> Self {
        Self::typed(ApiErrorKind::ChecksumMismatch, message)
    }

    pub fn unsupported_format(message: impl Into<String>) -> Self {
        Self::typed(ApiErrorKind::UnsupportedFormat, message)
    }

    /// `error-mapping-v1.md`'s `stale_frontier` `details`: `{current_seq,current_frontier}`, both
    /// optional (the wire only includes what the caller has).
    pub fn stale_frontier(
        message: impl Into<String>,
        current_seq: Option<i64>,
        current_frontier_b64: Option<&str>,
    ) -> Self {
        let mut details = Map::new();
        if let Some(seq) = current_seq {
            details.insert("current_seq".to_string(), json!(seq));
        }
        if let Some(frontier) = current_frontier_b64 {
            details.insert("current_frontier".to_string(), json!(frontier));
        }
        Self::typed_with_details(ApiErrorKind::StaleFrontier, message, Value::Object(details))
    }

    /// `error-mapping-v1.md`'s `resync_required` `details`: `{minimum_snapshot_seq}`.
    pub fn resync_required(message: impl Into<String>, minimum_snapshot_seq: Option<i64>) -> Self {
        let mut details = Map::new();
        if let Some(seq) = minimum_snapshot_seq {
            details.insert("minimum_snapshot_seq".to_string(), json!(seq));
        }
        Self::typed_with_details(ApiErrorKind::ResyncRequired, message, Value::Object(details))
    }

    /// `error-mapping-v1.md`'s `limit_exceeded` `details`: `{limit_kind,limit,observed?,
    /// retry_after_ms?}` -- `limit_kind` is the only field the contract calls out as always
    /// present ("kind 全集见 limits-v1.md，只含安全数值").
    pub fn limit_exceeded(
        message: impl Into<String>,
        limit_kind: &str,
        limit: Option<Value>,
        observed: Option<Value>,
        retry_after_ms: Option<u64>,
    ) -> Self {
        let mut details = Map::new();
        details.insert("limit_kind".to_string(), json!(limit_kind));
        if let Some(limit) = limit {
            details.insert("limit".to_string(), limit);
        }
        if let Some(observed) = observed {
            details.insert("observed".to_string(), observed);
        }
        if let Some(retry_after_ms) = retry_after_ms {
            details.insert("retry_after_ms".to_string(), json!(retry_after_ms));
        }
        Self::typed_with_details(ApiErrorKind::LimitExceeded, message, Value::Object(details))
    }

    /// `error-mapping-v1.md`'s `server_draining` `details`: `{reason,retry_after_ms}`, both
    /// required -- `reason` lives in [`ApiErrorKind::ServerDraining`] itself (not a side field) so
    /// it can never be omitted or misspelled independently of the discriminant.
    pub fn server_draining(reason: ServerDrainingReason, retry_after_ms: u64, message: impl Into<String>) -> Self {
        Self::typed_with_details(
            ApiErrorKind::ServerDraining(reason),
            message,
            json!({ "retry_after_ms": retry_after_ms }),
        )
    }

    /// The best-effort [`ApiErrorKind`] for any [`ApiError`], including the legacy string-typed
    /// variants that predate this contract. [`Self::Typed`] returns its own exact discriminant;
    /// `Unauthorized`/`Forbidden`/`NotFound` return the one [`ApiErrorKind`] their HTTP status
    /// unambiguously means regardless of which part of the app raised them (401/403/404 each have
    /// exactly one meaning in `error-mapping-v1.md`'s table). `BadRequest`/`Conflict`/`Internal`/
    /// `Database` return [`ApiErrorKind::Unclassified`] rather than guess among that bucket's
    /// several possible stable codes (`error-mapping-v1.md`'s 400 bucket alone covers
    /// `unsupported_protocol`/`invalid_update`/`limit_exceeded`/`checksum_mismatch`/
    /// `unsupported_format`) -- a wrong guess here would be worse than an honest "unknown" for a
    /// consumer branching on the result.
    #[must_use]
    pub const fn kind(&self) -> ApiErrorKind {
        match self {
            Self::Typed { kind, .. } => *kind,
            Self::Unauthorized(_) => ApiErrorKind::Unauthenticated,
            Self::Forbidden(_) => ApiErrorKind::Forbidden,
            Self::NotFound(_) => ApiErrorKind::NotFound,
            Self::BadRequest(_) | Self::Conflict(_) | Self::Internal | Self::Database(_) => ApiErrorKind::Unclassified,
        }
    }
}

impl ApiError {
    /// The pre-existing legacy envelope shape (`ApiResponse::error`), unchanged from before
    /// [`ApiErrorKind`] existed. Factored out so [`IntoResponse::into_response`] can be one flat
    /// match over every [`ApiError`] variant instead of matching twice over the same enum (which
    /// would need an unreachable, panic-capable arm for [`ApiError::Typed`] in the second match --
    /// banned in production code by this repo's iron rules).
    fn legacy_response(code: i32, message: &str, error_summary: &'static str) -> axum::response::Response {
        let mut response = (StatusCode::OK, ApiResponse::error(code, message)).into_response();
        response.extensions_mut().insert(OperationResponseMeta {
            business_code: code,
            error_summary: Some(error_summary),
        });
        response
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Typed { kind, message, details } => {
                let code = kind.http_status_code();
                let mut merged_details = if let ApiErrorKind::ServerDraining(reason) = kind {
                    let mut reason_only = Map::new();
                    reason_only.insert("reason".to_string(), json!(reason.wire_value()));
                    Some(reason_only)
                } else {
                    None
                };
                if let Some(Value::Object(extra_map)) = details {
                    let merged = merged_details.get_or_insert_with(Map::new);
                    for (key, value) in extra_map {
                        merged.insert(key, value);
                    }
                }

                // Same envelope shape every other response uses (`ApiResponse`), not a parallel
                // hand-built JSON body: `data` stays explicitly `null` (never omitted, unlike the
                // legacy string-typed variants below, which skip it entirely on error) so this
                // does not change the wire shape callers already depend on.
                let body = ApiResponse::<Value> {
                    code,
                    message,
                    data: Some(Value::Null),
                    error_code: Some(kind.stable_code()),
                    details: merged_details.map(Value::Object),
                };
                let mut response = (StatusCode::OK, Json(body)).into_response();
                response.extensions_mut().insert(OperationResponseMeta {
                    business_code: code,
                    error_summary: Some(kind.error_summary()),
                });
                response
            }
            Self::BadRequest(msg) => Self::legacy_response(400, &msg, "bad request"),
            Self::Unauthorized(msg) => Self::legacy_response(401, &msg, "unauthorized"),
            Self::Forbidden(msg) => Self::legacy_response(403, &msg, "forbidden"),
            Self::NotFound(msg) => Self::legacy_response(404, &msg, "not found"),
            Self::Conflict(msg) => Self::legacy_response(409, &msg, "conflict"),
            Self::Internal => Self::legacy_response(500, "internal server error", "internal server error"),
            Self::Database(err) => {
                tracing::error!(error = %err, "database error");
                Self::legacy_response(500, "database error", "database error")
            }
        }
    }
}

//! Typed outcome and exit codes for `sylvode`, per `error-mapping-v1.md`'s "CLI 退出码全集".
//!
//! `apps/api/src/error.rs`'s `ApiErrorKind` is the single source of truth for the stable
//! nine-way (plus `server_draining`'s two reasons) business taxonomy this module maps onto
//! process exit codes: [`CliError::from_structured`] reconstructs the server's own
//! `ApiErrorKind` from the wire `error_code`/`details` a
//! [`super::api_client::StructuredApiError`] carries, then calls that type's own
//! `cli_exit_code()`/`recoverable()` — it never invents a second, parallel mapping. An
//! endpoint that has not migrated to `ApiError::typed` yet (`error_code` absent) falls back
//! to the coarser numeric-envelope-code mapping this module used exclusively before
//! `ApiErrorKind` existed, so those endpoints keep behaving exactly as before.

use api::error::ApiErrorKind;
use serde_json::{Value, json};

use super::api_client::StructuredApiError;

/// `error-mapping-v1.md` "CLI 退出码全集".
pub mod exit {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 2;
    pub const UNAUTHENTICATED: i32 = 3;
    pub const FORBIDDEN: i32 = 4;
    pub const NOT_FOUND: i32 = 5;
    pub const CONFLICT: i32 = 6;
    pub const INVALID: i32 = 7;
    pub const LIMIT: i32 = 8;
    pub const TEMPORARY: i32 = 9;
    pub const MISMATCH: i32 = 10;
}

/// A typed `sylvode` failure: the stable `code`/`message`/`recoverable`/`details` the JSON
/// envelope carries, plus the process exit status it maps to.
#[derive(Debug, Clone)]
pub struct CliError {
    pub code: &'static str,
    pub message: String,
    pub recoverable: bool,
    pub details: Value,
    pub exit: i32,
}

impl CliError {
    /// A local argument/config/file error: nothing was sent to the API
    /// (`error-mapping-v1.md`: "2 | 本地参数/JSON/文件错误，未发请求").
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            code: "usage_error",
            message: message.into(),
            recoverable: false,
            details: json!({}),
            exit: exit::USAGE,
        }
    }

    /// A transport-level failure: the request never reached a business decision at all (DNS,
    /// connect, timeout, or a response that is not the `{code,message,data}` envelope the API
    /// always answers with). Grouped under `server_draining`/exit 9, the "temporary service
    /// failure" bucket, because from the caller's point of view both are "retry later", and
    /// `error-mapping-v1.md` reserves no separate code for "could not even reach the server".
    pub fn network(message: impl Into<String>) -> Self {
        Self {
            code: "server_draining",
            message: message.into(),
            recoverable: true,
            details: json!({}),
            exit: exit::TEMPORARY,
        }
    }

    /// Classifies a [`StructuredApiError`] (`client::OpenPrClient::get_structured`/
    /// `post_structured`/`put_structured`) into a [`CliError`].
    ///
    /// `error_code` present means the failure is a real `ApiError::Typed` response
    /// (`apps/api/src/error.rs`): its stable code is matched back onto the exact
    /// [`ApiErrorKind`] variant it came from and [`Self::from_kind`] consumes that variant's
    /// own `cli_exit_code()`/`recoverable()` — never a second hand-written mapping.
    /// `error_code` absent falls back to [`Self::from_legacy_numeric_code`] (untyped
    /// `ApiError::BadRequest`/`Conflict`/`Internal`/`Database`, or a transport-level failure
    /// with no envelope `code` at all).
    pub fn from_structured(error: StructuredApiError) -> Self {
        let StructuredApiError {
            code,
            message,
            error_code,
            details,
        } = error;

        let Some(error_code) = error_code.as_deref() else {
            return match code {
                Some(code) => Self::from_legacy_numeric_code(code, message),
                None => Self::network(message),
            };
        };

        match error_code {
            "unauthenticated" => Self::from_kind(ApiErrorKind::Unauthenticated, message, details),
            "forbidden" => Self::from_kind(ApiErrorKind::Forbidden, message, details),
            "feature_disabled" => Self::from_kind(ApiErrorKind::FeatureDisabled, message, details),
            "not_found" => Self::from_kind(ApiErrorKind::NotFound, message, details),
            "unsupported_protocol" => Self::from_kind(ApiErrorKind::UnsupportedProtocol, message, details),
            "stale_frontier" => Self::from_kind(ApiErrorKind::StaleFrontier, message, details),
            "invalid_update" => Self::from_kind(ApiErrorKind::InvalidUpdate, message, details),
            "policy_rejected" => Self::from_kind(ApiErrorKind::PolicyRejected, message, details),
            "limit_exceeded" => Self::from_kind(ApiErrorKind::LimitExceeded, message, details),
            "resync_required" => Self::from_kind(ApiErrorKind::ResyncRequired, message, details),
            "server_draining" => Self::from_server_draining(&message, details),
            "checksum_mismatch" => Self::from_kind(ApiErrorKind::ChecksumMismatch, message, details),
            "unsupported_format" => Self::from_kind(ApiErrorKind::UnsupportedFormat, message, details),
            // `"unclassified"` (the API's own placeholder for a not-yet-migrated `Typed`
            // error) or any stable code this binary does not recognise (a newer server):
            // fall back to the numeric code when one is available rather than guess a
            // specific `ApiErrorKind` that was not actually named on the wire.
            _ => match code {
                Some(code) => Self::from_legacy_numeric_code(code, message),
                None => Self::network(message),
            },
        }
    }

    /// `server_draining`'s `reason` discriminator is `required` on the wire
    /// (`error-mapping-v1.md`: "`server_draining.details.reason` 在 REST、MCP、CLI 与 WS
    /// control/close metadata 中都是 required") and carries opposite recovery semantics per
    /// reason: `drain` means retrying against this instance will not help, `contention` means
    /// retry almost immediately. Both still exit `9` — "两种 reason 不拆退出码，JSON 保留
    /// discriminator" — but the *message* a human reading `--format table` sees, and the
    /// `details.reason`/`retry_after_ms` a `--format json` caller reads, must never collapse
    /// the two into one generic "temporary" report the way this CLI did before this change.
    ///
    /// A missing or unrecognised `reason` is a producer contract violation
    /// (`error-mapping-v1.md`: "缺失/未知 reason 是 producer contract violation，测试必须失
    /// 败；consumer 仍按 exit 9/临时错误安全退避，但不得猜测为维护或竞争、不得用 message 补
    /// 判"): this still exits `9` and stays `recoverable`, but it must not guess which of the
    /// two it is, so the message says exactly that instead of picking one.
    fn from_server_draining(message: &str, details: Option<Value>) -> Self {
        let details = details.unwrap_or_else(|| json!({}));
        let reason = details.get("reason").and_then(Value::as_str);
        let retry_after_ms = details.get("retry_after_ms").and_then(Value::as_u64);

        let annotated_message = match reason {
            Some("drain") => format!(
                "{message} (server_draining/drain: this instance is draining connections and will not accept a \
                 retry; reconnect to a different instance)"
            ),
            Some("contention") => retry_after_ms.map_or_else(
                || format!("{message} (server_draining/contention: transient contention, retry shortly)"),
                |ms| format!("{message} (server_draining/contention: transient contention, retry after {ms}ms)"),
            ),
            _ => format!(
                "{message} (server_draining: producer contract violation -- details.reason was {} instead of the \
                 required 'drain' or 'contention'; treating as a temporary failure without guessing which)",
                reason.map_or_else(|| "missing".to_string(), |value| format!("'{value}'"))
            ),
        };

        Self {
            // `stable_code()` is the same string for either `ServerDrainingReason` (the
            // discriminator lives in `details.reason`, not in a forked stable code) -- see
            // `ApiErrorKind::stable_code`'s own doc comment.
            code: "server_draining",
            message: annotated_message,
            recoverable: true,
            details,
            exit: exit::TEMPORARY,
        }
    }

    /// Builds a [`CliError`] straight from an [`ApiErrorKind`] this binary already resolved,
    /// consuming that type's own `stable_code()`/`recoverable()`/`cli_exit_code()` rather than
    /// re-deriving them.
    fn from_kind(kind: ApiErrorKind, message: String, details: Option<Value>) -> Self {
        Self {
            code: kind.stable_code(),
            message,
            recoverable: kind.recoverable(),
            details: details.unwrap_or_else(|| json!({})),
            exit: i32::from(kind.cli_exit_code()),
        }
    }

    /// The mapping [`Self::from_structured`] used exclusively before the API grew a stable
    /// `error_code`: an untyped `ApiError::BadRequest`/`Unauthorized`/`Forbidden`/`NotFound`/
    /// `Conflict` speaks only in HTTP-status-shaped numeric envelope codes, so this reads the
    /// numeric code back and maps it onto the nearest exit code the table reserves, without
    /// inventing a stable `code` string the backend never actually decided between (`forbidden`
    /// vs `feature_disabled` are both HTTP 403 on this path, for instance).
    fn from_legacy_numeric_code(code: i64, message: String) -> Self {
        match code {
            401 => Self {
                code: "unauthenticated",
                message,
                recoverable: true,
                details: json!({}),
                exit: exit::UNAUTHENTICATED,
            },
            403 => Self {
                code: "forbidden",
                message,
                recoverable: false,
                details: json!({}),
                exit: exit::FORBIDDEN,
            },
            404 => Self {
                code: "not_found",
                message,
                recoverable: false,
                details: json!({}),
                exit: exit::NOT_FOUND,
            },
            409 => Self {
                code: "stale_frontier",
                message,
                recoverable: true,
                details: json!({}),
                exit: exit::CONFLICT,
            },
            400 => Self {
                code: "invalid_update",
                message,
                recoverable: false,
                details: json!({}),
                exit: exit::INVALID,
            },
            _ => Self::network(message),
        }
    }

    /// `collab verify` completed (a `200`/`code:0` `OperationReceipt`) but found an
    /// inconsistency (`error-mapping-v1.md`: "10 | verify/integrity 命令执行成功但发现不一致").
    /// Not an API error at all — the call succeeded — so this is built directly from the
    /// receipt, never from [`Self::from_structured`].
    pub fn integrity_mismatch(details: Value) -> Self {
        Self {
            code: "integrity_mismatch",
            message: "collab verify completed but found an inconsistency between the head, \
                       snapshot, tail, and projection"
                .to_string(),
            recoverable: false,
            details,
            exit: exit::MISMATCH,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CliError, exit};
    use crate::cli_app::api_client::StructuredApiError;
    use serde_json::json;

    fn typed(code: i64, error_code: &str, details: Option<serde_json::Value>) -> StructuredApiError {
        StructuredApiError {
            code: Some(code),
            message: "no".to_string(),
            error_code: Some(error_code.to_string()),
            details,
        }
    }

    #[test]
    fn from_structured_maps_every_stable_code_to_the_frozen_exit_table() {
        assert_eq!(
            CliError::from_structured(typed(401, "unauthenticated", None)).exit,
            exit::UNAUTHENTICATED
        );
        assert_eq!(
            CliError::from_structured(typed(403, "forbidden", None)).exit,
            exit::FORBIDDEN
        );
        assert_eq!(
            CliError::from_structured(typed(403, "feature_disabled", None)).exit,
            exit::FORBIDDEN
        );
        assert_eq!(
            CliError::from_structured(typed(404, "not_found", None)).exit,
            exit::NOT_FOUND
        );
        assert_eq!(
            CliError::from_structured(typed(409, "stale_frontier", None)).exit,
            exit::CONFLICT
        );
        assert_eq!(
            CliError::from_structured(typed(409, "resync_required", None)).exit,
            exit::CONFLICT
        );
        assert_eq!(
            CliError::from_structured(typed(400, "invalid_update", None)).exit,
            exit::INVALID
        );
        assert_eq!(
            CliError::from_structured(typed(400, "unsupported_protocol", None)).exit,
            exit::INVALID
        );
        assert_eq!(
            CliError::from_structured(typed(400, "checksum_mismatch", None)).exit,
            exit::INVALID
        );
        assert_eq!(
            CliError::from_structured(typed(400, "unsupported_format", None)).exit,
            exit::INVALID
        );
        assert_eq!(
            CliError::from_structured(typed(403, "policy_rejected", None)).exit,
            exit::FORBIDDEN
        );
        assert_eq!(
            CliError::from_structured(typed(400, "limit_exceeded", None)).exit,
            exit::LIMIT
        );
    }

    #[test]
    fn from_structured_falls_back_to_numeric_code_for_untyped_errors() {
        let untyped = StructuredApiError {
            code: Some(404),
            message: "issue not found".to_string(),
            error_code: None,
            details: None,
        };
        assert_eq!(CliError::from_structured(untyped).exit, exit::NOT_FOUND);
    }

    #[test]
    fn from_structured_falls_back_to_temporary_for_transport_failures() {
        let transport = StructuredApiError {
            code: None,
            message: "Request failed: connection refused".to_string(),
            error_code: None,
            details: None,
        };
        assert_eq!(CliError::from_structured(transport).exit, exit::TEMPORARY);
    }

    #[test]
    fn server_draining_drain_and_contention_share_exit_9_but_never_the_same_message() {
        let drain = CliError::from_structured(typed(409, "server_draining", Some(json!({ "reason": "drain" }))));
        let contention = CliError::from_structured(typed(
            409,
            "server_draining",
            Some(json!({ "reason": "contention", "retry_after_ms": 250 })),
        ));

        assert_eq!(drain.exit, exit::TEMPORARY);
        assert_eq!(contention.exit, exit::TEMPORARY);
        assert_eq!(drain.code, "server_draining");
        assert_eq!(contention.code, "server_draining");

        // The two reasons must never be reported identically to a human: drain means
        // retrying is pointless, contention means retry almost immediately.
        assert_ne!(drain.message, contention.message);
        assert!(drain.message.contains("drain"));
        assert!(drain.message.contains("will not accept a retry"));
        assert!(contention.message.contains("contention"));
        assert!(contention.message.contains("retry after 250ms"));

        // `details.reason` is preserved verbatim -- a JSON consumer must branch on this, never
        // on the human message.
        assert_eq!(drain.details.get("reason").and_then(|v| v.as_str()), Some("drain"));
        assert_eq!(
            contention.details.get("reason").and_then(|v| v.as_str()),
            Some("contention")
        );
    }

    #[test]
    fn server_draining_with_missing_or_unknown_reason_stays_temporary_without_guessing() {
        let missing = CliError::from_structured(typed(409, "server_draining", None));
        assert_eq!(missing.exit, exit::TEMPORARY);
        assert!(missing.recoverable);
        assert!(missing.message.contains("producer contract violation"));
        assert!(!missing.message.contains("drain:"));
        assert!(!missing.message.contains("contention:"));

        let unknown = CliError::from_structured(typed(
            409,
            "server_draining",
            Some(json!({ "reason": "something_else" })),
        ));
        assert_eq!(unknown.exit, exit::TEMPORARY);
        assert!(unknown.message.contains("producer contract violation"));
    }

    #[test]
    fn unrecognised_stable_code_falls_back_to_numeric_mapping() {
        let future_code = typed(404, "some_future_stable_code", None);
        assert_eq!(CliError::from_structured(future_code).exit, exit::NOT_FOUND);
    }
}

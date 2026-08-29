//! Typed outcome and exit codes for `sylvode`, per `error-mapping-v1.md`'s "CLI 退出码全集".
//!
//! `error-mapping-v1.md`'s nine-way business taxonomy (`unauthenticated`, `forbidden`,
//! `feature_disabled`, `not_found`, `stale_frontier`, `invalid_update`, `policy_rejected`,
//! `limit_exceeded`, `resync_required`, `server_draining`, `checksum_mismatch`,
//! `unsupported_format`) is derived server-side from typed Flow errors that do not exist in
//! this codebase yet: `apps/api/src/error.rs`'s `ApiError` today carries only the five
//! generic HTTP-status-shaped variants that predate Flow (`BadRequest`/400,
//! `Unauthorized`/401, `Forbidden`/403, `NotFound`/404, `Conflict`/409), and none of the v0.4
//! Flow endpoints this binary calls emit anything finer than that. [`CliError::from_api_error`]
//! is therefore the coarsest-but-honest mapping the *current* backend supports: it reads the
//! numeric envelope `code` `OpenPrClient` already surfaces in its error text and maps it onto
//! the nearest exit code the table below reserves, without inventing a stable `code` string the
//! backend has not actually decided between (`forbidden` vs `feature_disabled` are both HTTP
//! 403 today, for instance). This gap is called out again in the delivery report.

use serde_json::{Value, json};

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

    /// Classifies an `OpenPrClient::get`/`post`/`put` error string (`format!("API error {code} \
    /// from {path}: {message}")` — `client::check_response_envelope`) by its envelope `code`.
    /// See the module doc comment for what this mapping does and does not claim.
    pub fn from_api_error(message: String) -> Self {
        match envelope_code(&message) {
            Some(401) => Self {
                code: "unauthenticated",
                message,
                recoverable: true,
                details: json!({}),
                exit: exit::UNAUTHENTICATED,
            },
            Some(403) => Self {
                code: "forbidden",
                message,
                recoverable: false,
                details: json!({}),
                exit: exit::FORBIDDEN,
            },
            Some(404) => Self {
                code: "not_found",
                message,
                recoverable: false,
                details: json!({}),
                exit: exit::NOT_FOUND,
            },
            Some(409) => Self {
                code: "stale_frontier",
                message,
                recoverable: true,
                details: json!({}),
                exit: exit::CONFLICT,
            },
            Some(400) => Self {
                code: "invalid_update",
                message,
                recoverable: false,
                details: json!({}),
                exit: exit::INVALID,
            },
            Some(_) | None => Self::network(message),
        }
    }

    /// `collab verify` completed (a `200`/`code:0` `OperationReceipt`) but found an
    /// inconsistency (`error-mapping-v1.md`: "10 | verify/integrity 命令执行成功但发现不一致").
    /// Not an API error at all — the call succeeded — so this is built directly from the
    /// receipt, never from [`Self::from_api_error`].
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

/// Extracts the numeric envelope `code` from an `API error <code> from <path>: <message>`
/// string. `None` for every other shape the client produces (`Request failed: ...`,
/// `Malformed response from ...`, `HTTP <status> from ...`), all of which are transport level.
fn envelope_code(message: &str) -> Option<u16> {
    let rest = message.strip_prefix("API error ")?;
    let (code, _) = rest.split_once(' ')?;
    code.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{CliError, exit};

    #[test]
    fn from_api_error_maps_known_envelope_codes() {
        assert_eq!(
            CliError::from_api_error("API error 401 from /x: no".to_string()).exit,
            exit::UNAUTHENTICATED
        );
        assert_eq!(
            CliError::from_api_error("API error 403 from /x: no".to_string()).exit,
            exit::FORBIDDEN
        );
        assert_eq!(
            CliError::from_api_error("API error 404 from /x: no".to_string()).exit,
            exit::NOT_FOUND
        );
        assert_eq!(
            CliError::from_api_error("API error 409 from /x: no".to_string()).exit,
            exit::CONFLICT
        );
        assert_eq!(
            CliError::from_api_error("API error 400 from /x: no".to_string()).exit,
            exit::INVALID
        );
    }

    #[test]
    fn from_api_error_falls_back_to_temporary_for_unmapped_or_transport_failures() {
        assert_eq!(
            CliError::from_api_error("API error 500 from /x: no".to_string()).exit,
            exit::TEMPORARY
        );
        assert_eq!(
            CliError::from_api_error("Request failed: connection refused".to_string()).exit,
            exit::TEMPORARY
        );
        assert_eq!(
            CliError::from_api_error("Malformed response from /x: expected object".to_string()).exit,
            exit::TEMPORARY
        );
    }
}

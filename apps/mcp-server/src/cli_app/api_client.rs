//! `OpenPrClient` request methods that surface a structured, typed failure instead of the
//! formatted `String` every `apps/mcp-server/src/tools/*.rs` handler consumes.
//!
//! Lives under `cli_app`, not `client`, on purpose. `apps/mcp-server/src/main.rs` (the
//! `mcp-server` binary) declares its own private `mod client;`/`mod server;`/`mod tools;`
//! tree rather than depending on the `mcp_server` library crate, so anything added to
//! `client/mod.rs` is physically compiled twice: once for the `mcp_server` library (which
//! `cli_app` and the `sylvode` binary use) and once again for the `mcp-server` binary's own
//! private copy, which declares no `mod cli_app;` at all. A `pub` item in a `bin` crate gets
//! no "part of the public API" exemption from the dead-code lint the way a `lib` crate's does
//! (a binary has no downstream consumer), so anything under `client/` that only `cli_app`
//! calls would report as dead code in that duplicate copy — CLAUDE.md's "zero dead code"/"zero
//! warnings" rule with no honest way to silence it locally (`#[allow(dead_code)]` is banned by
//! the same rule). Placing the structured-error surface here instead means the `mcp-server`
//! binary's private module tree, which never declares `cli_app`, never compiles this file at
//! all, so it never sees these items as unreachable in the first place.
//!
//! `OpenPrClient::client`/`base_url`/`operation_headers`/`authorization` are `pub(crate)`
//! (`client/mod.rs`) so this module can build requests the exact same way
//! `OpenPrClient::send` does, without a second private copy of that wiring. The `{code,...}`
//! envelope classification itself (`envelope_outcome` below) is a second, independent copy of
//! what `client::check_response_envelope` already does, for the same reason: sharing that
//! classification would mean sharing a type whose `error_code`/`details` fields only this
//! module's copy ever reads, which is exactly the "field never read" duplicate-compilation
//! trap this module's own doc comment above describes for whole items. Two small, independent
//! implementations of the same `{code,message,error_code,details}` shape is the honest
//! trade-off; both are exercised by this crate's own tests.

use crate::client::{OpenPrClient, rejected_request_error};
use reqwest::RequestBuilder;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// What kind of JSON value a malformed response body turned out to be, for the same
/// diagnostic `client::check_response_envelope` reports.
const fn describe_json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "an empty body",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// This module's own copy of `client::check_response_envelope`'s classification, returning the
/// full `{code,message,error_code,details}` shape instead of a formatted string. See this
/// module's doc comment for why it is not shared.
fn envelope_outcome(payload: &Value, path: &str) -> Result<(), StructuredApiError> {
    let Some(envelope) = payload.as_object() else {
        return Err(StructuredApiError::transport(format!(
            "Malformed response from {path}: expected a {{code, message, data}} envelope, got {}",
            describe_json_kind(payload)
        )));
    };
    match envelope.get("code").and_then(Value::as_i64) {
        Some(0) => Ok(()),
        Some(code) => {
            let message = envelope
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown API error")
                .to_string();
            let error_code = envelope.get("error_code").and_then(Value::as_str).map(str::to_string);
            let details = envelope.get("details").cloned();
            Err(StructuredApiError {
                code: Some(code),
                message,
                error_code,
                details,
            })
        }
        None => Err(StructuredApiError::transport(format!(
            "Malformed response from {path}: the {{code, message, data}} envelope carries no integer `code`"
        ))),
    }
}

/// Structured API-call failure.
///
/// Additive next to the String-returning `get`/`post`/`put` `apps/mcp-server/src/tools/*.rs`
/// keeps using. `code` is `None` when the failure never reached a business decision at all
/// (transport failure, a non-2xx status from something in front of the API, a non-JSON body,
/// or a payload that is not the `{code,...}` envelope shape) — the same bucket
/// `cli_app::error::CliError::network` reports. `error_code` and `details` are only ever
/// populated from a real `{code,...}` envelope and mirror `apps/api/src/error.rs`'s
/// `ApiErrorKind::stable_code()`/`ApiError::Typed`'s `details` verbatim; an endpoint that has
/// not migrated to `ApiError::typed` yet answers with `error_code: None`, which callers must
/// treat as "no stable discriminant available", not as success.
#[derive(Debug, Clone)]
pub struct StructuredApiError {
    pub code: Option<i64>,
    pub message: String,
    pub error_code: Option<String>,
    pub details: Option<Value>,
}

impl StructuredApiError {
    const fn transport(message: String) -> Self {
        Self {
            code: None,
            message,
            error_code: None,
            details: None,
        }
    }
}

impl OpenPrClient {
    /// Same request/response handling as `OpenPrClient::send` (`client/mod.rs`), but surfaces
    /// [`StructuredApiError`] instead of collapsing it to a formatted string — see that type's
    /// doc comment. Used only by `cli_app`'s `sylvode` dispatch today; every other call site
    /// keeps going through `OpenPrClient::get`/`post`/`put`.
    async fn send_structured<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        path: &str,
    ) -> Result<T, StructuredApiError> {
        let resp = self
            .operation_headers(request)
            .header(
                "Authorization",
                self.authorization().map_err(StructuredApiError::transport)?,
            )
            .send()
            .await
            .map_err(|e| StructuredApiError::transport(format!("Request failed: {e}")))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| StructuredApiError::transport(format!("Failed to read response body from {path}: {e}")))?;

        if !status.is_success() {
            return Err(StructuredApiError::transport(rejected_request_error(
                status, path, &body,
            )));
        }

        let payload: Value = if body.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&body).map_err(|e| {
                StructuredApiError::transport(format!("Failed to deserialize response from {path}: {e}"))
            })?
        };

        envelope_outcome(&payload, path)?;
        serde_json::from_value(payload)
            .map_err(|e| StructuredApiError::transport(format!("Failed to deserialize response from {path}: {e}")))
    }

    pub async fn get_structured<T: DeserializeOwned>(&self, path: &str) -> Result<T, StructuredApiError> {
        let url = format!("{}{path}", self.base_url);
        self.send_structured(self.client.get(&url), path).await
    }

    pub async fn post_structured<T: DeserializeOwned, B: Serialize + Sync>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, StructuredApiError> {
        let url = format!("{}{path}", self.base_url);
        self.send_structured(self.client.post(&url).json(body), path).await
    }

    pub async fn put_structured<T: DeserializeOwned, B: Serialize + Sync>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, StructuredApiError> {
        let url = format!("{}{path}", self.base_url);
        self.send_structured(self.client.put(&url).json(body), path).await
    }
}

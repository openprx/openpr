//! The server-side origin every Flow command stamps onto the events it produces.
//!
//! `events-v1.md`'s envelope `source:{surface,session?,tool?,request?,client_id?,service?}` plus
//! the two causal-chain fields `correlation_id` / `causation_id`.
//!
//! # Why this is a type and not a `json!` literal at each producer
//!
//! `events-v1.md` freezes: "`source` 由服务端按 Web/REST/MCP/CLI/worker 覆盖，沿用 MCP origin
//! contract" and "首个用户请求生成 `correlation_id`；由 command、job、worker 或 retry 导出的下一
//! 事件把直接父 event id 写 `causation_id` 并继承 correlation".
//!
//! Both clauses are about the *caller*, not about the producer. A producer that writes
//! `json!({ "surface": "rest" })` inline is not implementing them — it is asserting that its only
//! caller will ever be REST. The moment `mcp-surface-v1.md`'s three transports or
//! `cli-surface-v1.md`'s two reuse the same command path (they are contractually required to:
//! "MCP、CLI、Web 必须调用本契约，不得增加 DB 直连或私有 endpoint"), that assertion becomes a
//! silent lie in the audit stream — no error, no rejection, just every operation recorded as REST
//! forever. Audit data cannot be repaired after the fact, so the caller has to be able to *say*
//! its surface before those callers exist, not after.
//!
//! # Deliberately not `Deserialize`
//!
//! `rest-api-v1.md`: "服务端忽略 caller 自报 actor，以认证 context 生成 actor/origin/causation",
//! and `events-v1.md`: "不适用时省略且**不能填 caller 自报值**". Nothing here may be built from a
//! request body, so none of these types derive `Deserialize` and none is reachable from a wire
//! shape. Every value is produced by trusted server code at the transport boundary.

use serde_json::{Map, Value};
use uuid::Uuid;

/// `events-v1.md`: "`source.surface` 全集为 `web|rest|mcp_http|mcp_sse|mcp_stdio|cli|
/// cli_tools_call|worker|system`".
///
/// An enum rather than a `&str` so the frozen set is the only inhabitable one: a producer cannot
/// stamp a typo, a caller-supplied string, or a surface the contract never registered. The same
/// nine spellings are already the `collab_updates_origin_surface_check` CHECK constraint's value
/// list (`migrations/0054_flow_data_layer.sql`), so [`Self::as_wire`] is simultaneously the
/// envelope spelling and the column spelling — one vocabulary, not two that can drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSurface {
    Web,
    Rest,
    McpHttp,
    McpSse,
    McpStdio,
    Cli,
    CliToolsCall,
    Worker,
    System,
}

impl EventSurface {
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Rest => "rest",
            Self::McpHttp => "mcp_http",
            Self::McpSse => "mcp_sse",
            Self::McpStdio => "mcp_stdio",
            Self::Cli => "cli",
            Self::CliToolsCall => "cli_tools_call",
            Self::Worker => "worker",
            Self::System => "system",
        }
    }

    /// The surface a **remote client** is allowed to declare at the authentication boundary,
    /// parsed from the transport label the MCP/CLI client sends.
    ///
    /// `events-v1.md` (2026-09-01): "`source` 必须由认证/传输边界推导，不得由调用方自报……surface、
    /// server session、exact registered tool 全部取自中间件已解析出的可信上下文". This is the
    /// allow-list half of that rule, and it lives here — beside `as_wire`, its exact inverse — so
    /// the vocabulary cannot drift between what is written and what is accepted.
    ///
    /// **Deliberately partial.** Only the five *client* transports are parseable:
    ///
    /// | accepted | rejected | why the rejection matters |
    /// |---|---|---|
    /// | `mcp_http`, `mcp_sse`, `mcp_stdio`, `cli`, `cli_tools_call` | `rest` | the boundary's own default, never something a client talks its way into |
    /// | | `web` | a browser session is proven by the WebSocket ticket handshake, not asserted by a header |
    /// | | `worker`, `system` | background work has no remote caller at all; accepting these would let a token mint events that look like the server talking to itself |
    ///
    /// Anything unrecognized is `None`, and the caller ([`middleware::bot_auth`]) falls back to
    /// [`Self::Rest`] — default-deny, so a new surface added to the enum is not silently
    /// declarable by clients until it is added here on purpose.
    ///
    /// [`middleware::bot_auth`]: crate::middleware::bot_auth
    #[must_use]
    pub fn from_client_transport_label(label: &str) -> Option<Self> {
        match label {
            "mcp_http" => Some(Self::McpHttp),
            "mcp_sse" => Some(Self::McpSse),
            "mcp_stdio" => Some(Self::McpStdio),
            "cli" => Some(Self::Cli),
            "cli_tools_call" => Some(Self::CliToolsCall),
            _ => None,
        }
    }
}

/// Serialized as its wire spelling and nothing else.
///
/// Written by hand rather than derived so that [`EventSurface::as_wire`] stays the single
/// vocabulary: a `#[serde(rename_all = "snake_case")]` derive would produce a *second* spelling of
/// the same nine values, and the day someone adds a variant whose `snake_case` name differs from its
/// wire name, the envelope and the `collab_updates.origin_surface` check constraint drift apart
/// silently. Needed because `middleware::bot_auth::BotAuthContext` is `Serialize`.
///
/// There is deliberately **no** `Deserialize`: `rest-api-v1.md` fixes that the server "忽略 caller
/// 自报 actor，以认证 context 生成 actor/origin/causation", so a surface must never be
/// constructible from a request body. The only parse into this type is
/// [`EventSurface::from_client_transport_label`], at the authentication boundary, against an
/// allow-list.
impl serde::Serialize for EventSurface {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_wire())
    }
}

/// The envelope's `source` object.
///
/// # `Option` vs required, and why omission is expressible
///
/// `events-v1.md` writes every key but `surface` as optional (`source:{surface,session?,tool?,
/// request?,client_id?,service?}`) and states the rule for the rest explicitly: "不适用时**省略**
/// 且不能填 caller 自报值". So the distinction the type must be able to express is *omitted* vs
/// *present*, and [`Self::to_json`] expresses it the only way JSON can: a `None` field contributes
/// no key at all, and a `Some(value)` field contributes the key verbatim — including an empty
/// string, which stays a present-but-empty value rather than collapsing into "omitted".
///
/// That last clause is load-bearing and is asserted by
/// [`tests::an_empty_string_stays_a_present_key_and_does_not_collapse_into_omitted`]. Folding
/// `Some(String::new())` into an absent key would be exactly the "缺省与空值不可区分" defect
/// `grants::SetInheritanceInput::initial_grants` documents on its own `Option`: two different
/// facts ("this surface has no session" and "this surface reported an empty session id") would
/// become one indistinguishable output, and no reader of the audit stream could tell them apart.
///
/// `surface` is required because no surface is ever inapplicable: every event is produced by
/// *some* transport, and the contract gives `worker`/`system` as the answers for the ones with no
/// human caller.
#[derive(Debug, Clone)]
pub struct EventSource {
    pub surface: EventSurface,
    /// `mcp-surface-v1.md`: "session id 由服务端生成" — an MCP transport's server-generated
    /// session id, or a WebSocket collab session id. `None` for a plain REST request, which has
    /// no session concept at all.
    pub session: Option<String>,
    /// `mcp-surface-v1.md`: "tool 保存 exact name". `None` for any surface that is not a tool
    /// call.
    pub tool: Option<String>,
    /// The server's own id for this request (a JSON-RPC id for MCP, a per-request UUID for REST).
    pub request: Option<String>,
    /// `events-v1.md`: "另允许 Web `client_id`". The collab client-id handshake's value.
    pub client_id: Option<String>,
    /// `events-v1.md`: "只有无用户发起的 scheduled maintenance 才允许 `actor_id=null,
    /// surface=system`，同时必须有 service identity 写入 `source.service`".
    pub service: Option<String>,
}

impl EventSource {
    /// A surface with every optional key omitted.
    #[must_use]
    pub const fn new(surface: EventSurface) -> Self {
        Self {
            surface,
            session: None,
            tool: None,
            request: None,
            client_id: None,
            service: None,
        }
    }

    #[must_use]
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session = Some(session.into());
        self
    }

    #[must_use]
    pub fn with_tool(mut self, tool: impl Into<String>) -> Self {
        self.tool = Some(tool.into());
        self
    }

    #[must_use]
    pub fn with_request(mut self, request: impl Into<String>) -> Self {
        self.request = Some(request.into());
        self
    }

    #[must_use]
    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = Some(client_id.into());
        self
    }

    #[must_use]
    pub fn with_service(mut self, service: impl Into<String>) -> Self {
        self.service = Some(service.into());
        self
    }

    /// The `business_events.source` JSON. Absent keys are absent; present keys are written
    /// verbatim (see this type's doc comment).
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("surface".to_string(), Value::String(self.surface.as_wire().to_string()));
        for (key, value) in [
            ("session", self.session.as_ref()),
            ("tool", self.tool.as_ref()),
            ("request", self.request.as_ref()),
            ("client_id", self.client_id.as_ref()),
            ("service", self.service.as_ref()),
        ] {
            if let Some(value) = value {
                map.insert(key.to_string(), Value::String(value.clone()));
            }
        }
        Value::Object(map)
    }
}

/// One command's complete event provenance: where it came from, and where it sits in the causal
/// chain.
///
/// `correlation_id` is **not** an `Option`. `events-v1.md` says "首个用户请求生成
/// `correlation_id`", and every construction site of this type *is* a request (or a job derived
/// from one), so there is no state in which a command legitimately has no correlation. Making it
/// required means the "所有事件共享同一 correlation" invariant cannot be violated by forgetting to
/// fill it — only by deliberately minting a second one, which is what
/// `move_object`'s correlation test mutates to prove the assertion bites.
///
/// `causation_id` *is* an `Option`, and its `None` is meaningful rather than missing: it is the
/// contract's "首个请求" — the root of a causal chain, an event nothing else caused.
#[derive(Debug, Clone)]
pub struct CommandOrigin {
    pub source: EventSource,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
}

impl CommandOrigin {
    /// A first user request: a freshly generated correlation id, and no parent event
    /// (`events-v1.md`: "首个用户请求生成 `correlation_id`").
    #[must_use]
    pub fn first_request(source: EventSource) -> Self {
        Self {
            source,
            correlation_id: Uuid::new_v4(),
            causation_id: None,
        }
    }

    /// Shorthand for [`Self::first_request`] over a surface with nothing optional to declare —
    /// no session, no tool, no request id, no client id, no service. A real transport that *has*
    /// one of those must fill it (`routes::flow::request_origin` fills `request`; the WebSocket path
    /// fills `session` and `client_id`); this is for the surfaces and fixtures that genuinely
    /// have none.
    #[must_use]
    pub fn first_request_from(surface: EventSurface) -> Self {
        Self::first_request(EventSource::new(surface))
    }

    /// The origin for an event **derived** from `parent_event_id` within this same command:
    /// `events-v1.md` — "由 command、job、worker 或 retry 导出的下一事件把直接父 event id 写
    /// `causation_id` 并继承 correlation".
    ///
    /// The surface is carried through unchanged: a derived event was produced by the same
    /// transport as the command that caused it, so re-deriving it (or defaulting it) could only
    /// disagree with the parent.
    #[must_use]
    pub fn derived_from(&self, parent_event_id: Uuid) -> Self {
        Self {
            source: self.source.clone(),
            correlation_id: self.correlation_id,
            causation_id: Some(parent_event_id),
        }
    }

    #[must_use]
    pub fn source_json(&self) -> Value {
        self.source.to_json()
    }

    #[must_use]
    pub const fn surface(&self) -> EventSurface {
        self.source.surface
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::{CommandOrigin, EventSource, EventSurface};
    use uuid::Uuid;

    /// `events-v1.md`'s frozen `source.surface` value set, and
    /// `migrations/0054_flow_data_layer.sql`'s `collab_updates_origin_surface_check`. If a new
    /// surface is ever added to one, this test is where the other has to follow.
    #[test]
    fn every_frozen_surface_spelling_matches_the_contract_and_the_check_constraint() {
        let all = [
            (EventSurface::Web, "web"),
            (EventSurface::Rest, "rest"),
            (EventSurface::McpHttp, "mcp_http"),
            (EventSurface::McpSse, "mcp_sse"),
            (EventSurface::McpStdio, "mcp_stdio"),
            (EventSurface::Cli, "cli"),
            (EventSurface::CliToolsCall, "cli_tools_call"),
            (EventSurface::Worker, "worker"),
            (EventSurface::System, "system"),
        ];
        for (surface, wire) in all {
            assert_eq!(surface.as_wire(), wire);
        }
    }

    #[test]
    fn an_omitted_optional_key_is_absent_from_the_json_entirely() {
        let json = EventSource::new(EventSurface::Rest).to_json();
        let object = json.as_object().expect("source is a JSON object");
        assert_eq!(object.get("surface").and_then(serde_json::Value::as_str), Some("rest"));
        for key in ["session", "tool", "request", "client_id", "service"] {
            assert!(
                !object.contains_key(key),
                "an omitted optional key must contribute no key at all, but '{key}' was present"
            );
        }
    }

    /// The "缺省与空值不可区分" guard. `None` and `Some("")` are two different facts and must
    /// produce two different JSON shapes; a `to_json` that folded the empty string into an absent
    /// key would make them one, and this assertion is what stops that.
    #[test]
    fn an_empty_string_stays_a_present_key_and_does_not_collapse_into_omitted() {
        let omitted = EventSource::new(EventSurface::McpStdio).to_json();
        let empty = EventSource::new(EventSurface::McpStdio).with_session("").to_json();
        assert_ne!(
            omitted, empty,
            "an omitted session and an empty-string session must not serialize identically"
        );
        assert_eq!(
            empty.get("session").and_then(serde_json::Value::as_str),
            Some(""),
            "a present-but-empty session must be written verbatim, not dropped"
        );
    }

    #[test]
    fn every_optional_key_is_written_verbatim_when_present() {
        let json = EventSource::new(EventSurface::McpHttp)
            .with_session("sess-1")
            .with_tool("objects.patch")
            .with_request("json-rpc-7")
            .with_client_id("client-9")
            .with_service("flow.snapshot")
            .to_json();
        assert_eq!(json["surface"], "mcp_http");
        assert_eq!(json["session"], "sess-1");
        assert_eq!(json["tool"], "objects.patch");
        assert_eq!(json["request"], "json-rpc-7");
        assert_eq!(json["client_id"], "client-9");
        assert_eq!(json["service"], "flow.snapshot");
    }

    #[test]
    fn a_first_request_has_no_causation_and_a_derived_event_inherits_the_correlation() {
        let root = CommandOrigin::first_request(EventSource::new(EventSurface::Cli).with_tool("objects.move"));
        assert!(
            root.causation_id.is_none(),
            "a first user request is the root of its causal chain"
        );

        let parent_event_id = Uuid::new_v4();
        let derived = root.derived_from(parent_event_id);
        assert_eq!(
            derived.correlation_id, root.correlation_id,
            "a derived event must inherit the correlation, not mint a new one"
        );
        assert_eq!(
            derived.causation_id,
            Some(parent_event_id),
            "a derived event's causation must name its direct parent event"
        );
        assert_eq!(
            derived.source_json(),
            root.source_json(),
            "a derived event was produced by the same transport as its parent"
        );
    }

    #[test]
    fn two_first_requests_do_not_share_a_correlation() {
        let a = CommandOrigin::first_request(EventSource::new(EventSurface::Rest));
        let b = CommandOrigin::first_request(EventSource::new(EventSurface::Rest));
        assert_ne!(
            a.correlation_id, b.correlation_id,
            "each first request roots its own correlation"
        );
    }
}

//! The write paths this package ships: `POST .../flow/objects` (object creation) and
//! `POST .../flow/objects/{id}/commands` (`set_title|insert_block|update_block|delete_block|
//! move_block|semantic_patch|archive|restore`, `rest-api-v1.md` "v0.4 Flow Alpha").
//!
//! The six content types share the *exact* write path `flow::collab::write::accept_update` and
//! the WebSocket `update` frame use (hydrate/isolated-apply outside any lock, the per-document
//! coordinator, the commit-time `authz_epoch` fence, the document row lock) — this module only
//! turns a command payload into the same shape of CRDT update bytes a WebSocket client would have
//! produced locally, then calls the identical function. `archive`/`restore` never advance a
//! document head (`existing_document_cardinality = 0`, `rest-api-v1.md`'s `move_object`/`link`
//! commentary on the same rule), so they take no document coordinator and no document row lock.
//! They lock the actual server-derived `flow_objects` impact set (the root and its subtree),
//! re-derive that set inside the transaction, and still take the commit-time
//! `authz_epoch` fence (`ADR-0012` §3.1, `authz::fence_epoch_for_share`)
//! inside that transaction: the fence's job is authorization freshness, not document-head
//! consistency, so "no document head to advance" does not imply "no epoch to re-verify" — see
//! `execute_lifecycle_command`'s own doc comment for the TOCTOU window this closes.

#![allow(clippy::too_long_first_doc_paragraph)]

use collab_core::{CollabEngine, CollabError, LoroCollabEngine, NodeId, NodeKind, Operation};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{ApiError, ApiErrorKind, ServerDrainingReason};
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};

use super::collab::{authz, bootstrap, frame, limits as collab_limits, runtime, write};
use super::event_origin::CommandOrigin;
use super::model::{AcceptedChange, FlowFeatureUpdateView, FlowObjectView};
use super::move_object::GovernanceCommandType;
use super::projection;
use super::query::feature_view_from_row;
use super::repository::{self, NewCollabDocument, NewFlowObject, NewProjection};

/// Registered `object_type` values (`domain-model-v1.md` "Object types": v0.4 ships `navigator`
/// and `page`; `collection`/`record` are v0.6). Not a `flow_objects_object_type_check` mirror by
/// accident — the two must never drift, and the CHECK constraint is the actual enforcement; this
/// list only lets the handler reject early with a typed `invalid_update` instead of a DB error.
const REGISTERED_OBJECT_TYPES: &[&str] = &["page", "navigator"];

/// Not frozen by `limits-v1.md` (only `message` at 500 chars and `idempotency_key` at 1-128 bytes
/// are). Chosen to match the one limit the contract *does* freeze for a similar caller-supplied
/// string, pending that gap being closed upstream.
const TITLE_MAX_CHARS: usize = 500;
const MESSAGE_MAX_CHARS: usize = 500;
const IDEMPOTENCY_KEY_MIN_BYTES: usize = 1;
const IDEMPOTENCY_KEY_MAX_BYTES: usize = 128;

/// Loro document format tag stored in `collab_documents.format_version`. Not frozen by any
/// contract file (`export-package-v1.md` only freezes an analogous `wire_format_version` for the
/// export/import package shape, not for `collab_documents` itself) — versioned independently of
/// the `loro` crate's own semver so a future encoding change can be detected without conflating it
/// with a dependency bump.
const DOCUMENT_FORMAT_VERSION: &str = "loro-1";

/// `ADR-0013` §"Gate 接线" (`command_contended_document_cardinality`): every write command must
/// machine-verifiably declare how many *already-existing* documents its execution contends a head
/// advance on (the "contended existing document set", not the full set of rows a command writes —
/// a brand-new document a command also creates in the same transaction never counts here).
///
/// - `Zero`: no existing document's head is advanced — pure `PostgreSQL` governance/metadata
///   (`ADR-0013` §1's first table row: archive/restore, feature flag, authz change,
///   relation link/unlink).
/// - `One`: exactly one existing document's head is advanced — v0.4's content commands, and
///   (from v0.5) a parented create that only touches its navigator's ordering.
/// - `BoundedMany(n)`: `n` existing documents contend a head advance in one command — `ADR-0013`
///   §2's multi-document lock-order path, first introduced by v0.5's cross-project `move_object`.
///   **No v0.4-registered command may declare this** (`ADR-0013` §1: "v0.4 的竞争文档集合恒 ≤ 1");
///   `v0_4_command_cardinality_registry`'s own test below is the machine gate for that bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingDocumentCardinality {
    Zero,
    One,
    BoundedMany(u8),
}

impl ExistingDocumentCardinality {
    pub const fn count(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
            Self::BoundedMany(n) => n,
        }
    }
}

/// `create_object`'s declared `existing_document_cardinality`: `Zero`, matching what this
/// function actually does today — it only ever inserts a brand-new `flow_objects` row and a
/// brand-new `collab_documents` row (see `create_object`'s own "no existing row to lock" doc
/// comment above); a `parent_object_id` only sets `flow_objects.parent_id` (a plain `PostgreSQL` FK
/// column), it does not touch the parent navigator's own CRDT document. This is *not* yet the
/// "带 parent 的 create 只锁 navigator" cardinality-1 case `ADR-0013` §1's table describes for a
/// future navigator-ordering feature — that feature does not exist in this package, so declaring
/// `One` here would assert a lock this code never takes.
pub const CREATE_OBJECT_CARDINALITY: ExistingDocumentCardinality = ExistingDocumentCardinality::Zero;

/// `set_flow_feature`'s declared `existing_document_cardinality`: `Zero` — a
/// `flow_workspace_settings` row transition, no `collab_documents` row involved at all.
pub const SET_FLOW_FEATURE_CARDINALITY: ExistingDocumentCardinality = ExistingDocumentCardinality::Zero;

/// Every v0.4-registered write command, by its wire `command.type`/endpoint name, alongside its
/// declared [`ExistingDocumentCardinality`] — the machine-checkable registry
/// `command_contended_document_cardinality` asserts over (this module's own test below, and any
/// future `verify-flow-cardinality-v0.4.sh` gate script that wants the same facts from Rust rather
/// than re-deriving them from prose).
pub fn v0_4_command_cardinality_registry() -> Vec<(&'static str, ExistingDocumentCardinality)> {
    let mut registry = vec![
        ("create_object", CREATE_OBJECT_CARDINALITY),
        ("set_flow_feature", SET_FLOW_FEATURE_CARDINALITY),
    ];
    for content in [
        ContentCommandType::SetTitle,
        ContentCommandType::InsertBlock,
        ContentCommandType::UpdateBlock,
        ContentCommandType::DeleteBlock,
        ContentCommandType::MoveBlock,
        ContentCommandType::SemanticPatch,
    ] {
        registry.push((content.wire_name(), content.existing_document_cardinality()));
    }
    for lifecycle in [LifecycleCommandType::Archive, LifecycleCommandType::Restore] {
        registry.push((lifecycle.wire_name(), lifecycle.existing_document_cardinality()));
    }
    registry
}

/// Every command registered at **v0.5**, by wire name, with its declared
/// [`ExistingDocumentCardinality`].
///
/// `ADR-0013` §1 R16: "不得把 v0.5 的清单外推到 v0.8 …… 改为由机器字段持续证明：command registry
/// 每个命令必须声明 `existing_document_cardinality`，逐版 gate 扫描所有新增 command variant" —
/// so this is a superset of [`v0_4_command_cardinality_registry`], not a replacement, and the v0.4
/// bound keeps being asserted against the v0.4 list alone.
pub fn v0_5_command_cardinality_registry() -> Vec<(&'static str, ExistingDocumentCardinality)> {
    let mut registry = v0_4_command_cardinality_registry();
    registry.extend([
        ("grants_set", ExistingDocumentCardinality::Zero),
        ("inheritance_set", ExistingDocumentCardinality::Zero),
    ]);
    for governance in [
        GovernanceCommandType::MoveObject,
        GovernanceCommandType::Link,
        GovernanceCommandType::Unlink,
    ] {
        registry.push((governance.wire_name(), governance.existing_document_cardinality()));
    }
    registry
}

/// The value a `REFERENCES users(id)` column may take for this actor: the actor's own id when it
/// is a user, `None` when it is a bot.
///
/// `flow::grants` has always done this (`Caller::granted_by`, and `actor_id: if caller.is_bot()`),
/// which is exactly why `PUT .../grants` was the one bot-reachable Flow write route that worked.
/// Every other route wrote the bot id straight into a users FK and returned a 500 — measured, not
/// inferred; see the report's probe section.
///
/// **The bot's identity is not lost by writing `NULL` here.** `events-v1.md` freezes no envelope
/// field for a bot principal, so inventing one would be worse than the hole it fills; instead the
/// event's `source.request` is the *same* `request_id` the middleware wrote to
/// `bot_operation_logs.request_id` for the same call, so `business_events.source->>'request'` joins
/// straight to the row that names the bot. That join only exists because the two were deliberately
/// made the same value (`middleware::bot_auth::bot_auth_context`).
#[must_use]
pub const fn actor_user_id(actor_id: Uuid, actor_is_bot: bool) -> Option<Uuid> {
    if actor_is_bot { None } else { Some(actor_id) }
}

pub struct CreateObjectInput {
    pub workspace_id: Uuid,
    pub actor_id: Uuid,
    /// Whether [`Self::actor_id`] is a `workspace_bots` id rather than a `users` id.
    ///
    /// Every "who did this" column on the Flow write paths — `flow_objects.created_by`/
    /// `updated_by`, `flow_workspace_settings.updated_by`, `collab_updates.actor_id`,
    /// `business_events.actor_id` — is `REFERENCES users(id)`, while
    /// `middleware::bot_auth::require_workspace_access_from_auth` returns the **bot id** as the
    /// actor for a bot token. Writing that id into any of those columns is a foreign-key
    /// violation, which is why this flag has to travel with the actor rather than be inferred: the
    /// surface cannot tell you (a bot may legitimately present as `rest`), and the id itself
    /// cannot tell you.
    ///
    /// See [`actor_user_id`].
    pub actor_is_bot: bool,
    pub object_type: String,
    pub project_id: Option<Uuid>,
    pub parent_object_id: Option<Uuid>,
    pub title: String,
    pub idempotency_key: String,
    pub message: Option<String>,
    /// Where this command came from, declared by the transport that accepted it — see
    /// [`CommandOrigin`]. The producer below reads `source`/`correlation_id`/`causation_id` off
    /// this value; it does not decide any of them itself.
    pub origin: CommandOrigin,
}

/// The semantic request body bound to a create idempotency key. The generated object id and the
/// transport origin are deliberately absent: neither is caller-controlled create intent. The
/// normalized title is stored so harmless surrounding whitespace has the same meaning on replay.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct CreateObjectIdempotencyBody {
    object_type: String,
    project_id: Option<Uuid>,
    parent_object_id: Option<Uuid>,
    title: String,
    message: Option<String>,
}

impl CreateObjectIdempotencyBody {
    fn from_input(input: &CreateObjectInput, normalized_title: &str) -> Self {
        Self {
            object_type: input.object_type.clone(),
            project_id: input.project_id,
            parent_object_id: input.parent_object_id,
            title: normalized_title.to_string(),
            message: input.message.clone(),
        }
    }
}

fn validate(input: &CreateObjectInput) -> Result<(), ApiError> {
    if !REGISTERED_OBJECT_TYPES.contains(&input.object_type.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "object_type must be one of {REGISTERED_OBJECT_TYPES:?}"
        )));
    }
    let title = input.title.trim();
    if title.is_empty() {
        return Err(ApiError::BadRequest("title must not be empty".to_string()));
    }
    if title.chars().count() > TITLE_MAX_CHARS {
        return Err(ApiError::BadRequest(format!(
            "title must be at most {TITLE_MAX_CHARS} characters"
        )));
    }
    let key_bytes = input.idempotency_key.len();
    if !(IDEMPOTENCY_KEY_MIN_BYTES..=IDEMPOTENCY_KEY_MAX_BYTES).contains(&key_bytes) {
        return Err(ApiError::BadRequest(format!(
            "idempotency_key must be {IDEMPOTENCY_KEY_MIN_BYTES}-{IDEMPOTENCY_KEY_MAX_BYTES} bytes"
        )));
    }
    if let Some(message) = &input.message
        && message.chars().count() > MESSAGE_MAX_CHARS
    {
        return Err(ApiError::BadRequest(format!(
            "message must be at most {MESSAGE_MAX_CHARS} characters"
        )));
    }
    Ok(())
}

/// A caller-supplied `project_id`/`parent_object_id` that resolves to a *real* row, but one that
/// lives in a different workspace than the request's own `workspace_id`. `rest-api-v1.md`
/// ("RelationView"): "数据库约束外出现跨 workspace relation 时整次请求 fail closed 为
/// `invalid_update`、记录 integrity alert" — `v0.4-flow-alpha.md` names this exact check as the
/// v0.4-scoped instance of that rule (no `flow_relations` table exists before v0.5; a create's
/// `project_id`/`parent_object_id` are v0.4's only cross-object references). Records the drift in
/// `flow_integrity_records` (`ADR-0013` §4) before failing closed — the id existing at all but in
/// the wrong workspace is never a plain "not found" typo, so it gets a paper trail a genuine
/// missing-row `BadRequest` does not.
pub(super) async fn record_cross_workspace_relation_and_fail_closed(
    state: &AppState,
    requesting_workspace_id: Uuid,
    subject_kind: &str,
    referenced_id: Uuid,
    referenced_workspace_id: Uuid,
    detected_by: &str,
) -> ApiError {
    if let Err(err) = repository::insert_integrity_record(
        &state.db,
        repository::IntegrityRecordInput {
            workspace_id: requesting_workspace_id,
            kind: "cross_workspace_relation",
            subject_kind,
            subject_id: &referenced_id.to_string(),
            detected_by,
            details_redacted: json!({
                "referenced_workspace_id": referenced_workspace_id,
                "requesting_workspace_id": requesting_workspace_id,
            }),
        },
    )
    .await
    {
        tracing::error!(error = %err, "failed to record integrity alert for a cross-workspace relation");
    }
    ApiError::BadRequest("invalid_update".to_string())
}

/// Resolves an already-committed `flow.object.created` event for `idempotency_key` into the
/// [`AcceptedChange`] a replay of that creation must return — the *canonical* object named by
/// `business_events.aggregate_id`, never a caller- or attempt-local id.
///
/// Used twice by [`create_object`], on purpose: once as the ordinary pre-transaction replay guard,
/// and once after the transaction's own `business_events` insert loses the unique-index race. Both
/// are the same question ("has this key already created an object?") asked at the only two points
/// where it can be answered, so they must produce the same answer rather than two near-copies that
/// can drift.
///
/// `Ok(None)` means the key is unused and the caller should go ahead and create.
///
/// # Errors
/// `Conflict` if the key was already used for a different operation, if any semantic request-body
/// field drifts, or if an older event has no complete request identity to compare; `Internal` if
/// the event names an object that cannot be read back.
async fn replay_created_object(
    state: &AppState,
    workspace_id: Uuid,
    idempotency_key: &str,
    requested_body: &CreateObjectIdempotencyBody,
) -> Result<Option<AcceptedChange>, ApiError> {
    let Some(existing) = repository::find_idempotent_event(&state.db, workspace_id, idempotency_key).await? else {
        return Ok(None);
    };
    if existing.event_type != "flow.object.created" {
        return Err(ApiError::Conflict(
            "idempotency_key was already used for a different operation".to_string(),
        ));
    }
    let Some(stored_body) = existing.metadata.get("idempotency_body") else {
        return Err(ApiError::Conflict(
            "idempotency_key belongs to a create event whose complete request body cannot be verified".to_string(),
        ));
    };
    let stored_body: CreateObjectIdempotencyBody = serde_json::from_value(stored_body.clone()).map_err(|_| {
        ApiError::Conflict("idempotency_key belongs to a create event with an invalid request identity".to_string())
    })?;
    if stored_body != *requested_body {
        return Err(ApiError::Conflict(
            "idempotency_key was already used with a different create request body".to_string(),
        ));
    }
    let object_id = Uuid::parse_str(&existing.aggregate_id).map_err(|_| ApiError::Internal)?;
    let view = repository::fetch_object_view(&state.db, object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok(Some(accepted_change_from_row(view, existing.id)))
}

/// `details.reason` on the `invalid_update` a create that *explicitly declares* a `project_id`
/// other than its parent's gets. Frozen by contract, not chosen here: `rest-api-v1.md` "v0.5 起,
/// `POST /workspaces/{workspace_id}/flow/objects` 携带 `parent_object_id` 时" spells this exact
/// string for the create side, and `ADR-0013` §2.2 R17's first step repeats it. The earlier note
/// here — that only `move_object`'s `subtree_spans_multiple_projects` was frozen and that this
/// spelling was the implementation's own, pending ratification — is obsolete and was wrong from
/// the contract's side: it is ratified.
///
/// The same two contract sentences also bound *when* this reason may be produced: only for a
/// declared-and-different scope. An **omitted** `project_id` is not a declaration of the
/// unprojected scope — the server inherits the parent's own value, `NULL` included — so it never
/// reaches this rejection. See `create_object`'s parent branch.
pub const CHILD_PROJECT_MUST_MATCH_PARENT: &str = "child_project_must_match_parent";

pub async fn create_object(state: &AppState, input: CreateObjectInput) -> Result<AcceptedChange, ApiError> {
    runtime::runtime().ensure_workspace_accepting(input.workspace_id)?;
    validate(&input)?;
    let title = input.title.trim().to_string();
    let idempotency_body = CreateObjectIdempotencyBody::from_input(&input, &title);

    // Idempotent replay: a caller retrying the exact same `idempotency_key` gets back the
    // original result instead of a unique-violation `Conflict`
    // (`business_events_idempotency` is unique on `(workspace_id, idempotency_key)`).
    if let Some(replay) =
        replay_created_object(state, input.workspace_id, &input.idempotency_key, &idempotency_body).await?
    {
        return Ok(replay);
    }

    if let Some(project_id) = input.project_id {
        let project_workspace = repository::fetch_project_workspace(&state.db, project_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest("project not found".to_string()))?;
        if project_workspace != input.workspace_id {
            return Err(record_cross_workspace_relation_and_fail_closed(
                state,
                input.workspace_id,
                "project",
                project_id,
                project_workspace,
                "flow.command.create_object", // detected_by: an ADR-0013 §4 integrity-record producer, not an events-v1 type
            )
            .await);
        }
    }

    // The scope the row is actually written with. It equals `input.project_id` for a root object
    // and for a child that declared its parent's scope; for a child that *omitted* `project_id`
    // the parent branch below replaces it with the parent's own value (`NULL` included), which is
    // the inheritance `rest-api-v1.md` and `ADR-0013` §2.2 R17 require. Nothing between here and
    // `insert_flow_object` may go back to reading `input.project_id` for the stored scope.
    let mut effective_project_id = input.project_id;
    let mut effective_parent_id = input.parent_object_id;

    if effective_parent_id.is_none() && input.object_type != "navigator" {
        effective_parent_id = Some(repository::ensure_workspace_navigator_root(&state.db, input.workspace_id).await?);
    } else if effective_parent_id.is_none()
        && input.object_type == "navigator"
        && input.project_id.is_none()
        && repository::fetch_workspace_navigator_root(&state.db, input.workspace_id)
            .await?
            .is_some()
    {
        return Err(ApiError::Conflict(
            "workspace already has a canonical navigator root".to_string(),
        ));
    }

    if let Some(parent_id) = effective_parent_id {
        let parent = repository::fetch_parent_object(&state.db, parent_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest("parent_object_id not found".to_string()))?;
        if parent.workspace_id != input.workspace_id {
            return Err(record_cross_workspace_relation_and_fail_closed(
                state,
                input.workspace_id,
                "flow_object",
                parent_id,
                parent.workspace_id,
                "flow.command.create_object", // detected_by: an ADR-0013 §4 integrity-record producer, not an events-v1 type
            )
            .await);
        }
        if parent.lifecycle_status == "archived" {
            return Err(ApiError::BadRequest("parent_object_id is archived".to_string()));
        }
        // `ADR-0013` §2.2 R17: a non-root object sits in its parent's project scope. Until this
        // check existed, `project_id` and `parent_object_id` were validated *independently* -- each
        // only against the workspace -- so `P1 root -> P2 child -> P3 grandchild` was a fully legal
        // shape to create, and a cross-project `move_object` of such a subtree would have to touch
        // four navigator documents against a declared `BoundedMany(2)` ceiling.
        //
        // `flow_objects_parent_project_fk` (migration `0056`) now rejects the same shape at the
        // database, but a foreign-key violation surfaces as an opaque 500. This is the decidable
        // answer: the caller learns which of the two fields to change, and the constraint stays
        // what it should be -- the backstop, not the error message.
        //
        // Declaring vs. omitting are two different requests and the contract gives them two
        // different answers (`rest-api-v1.md` "v0.5 起 ...": 显式携带且与父级不一致 → reject;
        // 省略 → 由服务端继承父级的值，包括父级为 `NULL` 的未投影 scope):
        //
        // - **Declared and different** is the rejection. `NULL` is a real scope on the declared
        //   side too, so `Some(project)` under an unprojected parent is just as much a mismatch as
        //   `Some(other_project)` under a projected one — the child's ordering entry would land in
        //   a different navigator document from its parent's, which is exactly the two-navigator
        //   subtree the invariant exists to prevent.
        // - **Declared and equal** passes untouched.
        // - **Omitted** inherits. Reading omission as "the caller declared the unprojected scope"
        //   would reject the most ordinary request there is ("new child page under this page") and
        //   would be the server picking a scope the caller never named — `rest-api-v1.md`'s "不得
        //   把省略解释成另一个 scope" forbids precisely that.
        //
        // `project_id: Option<Uuid>` cannot separate an omitted field from an explicit JSON
        // `null` (serde folds both to `None`, and `CreateFlowObjectRequest` has the same shape),
        // so an explicit `null` is treated as omission and inherits. The contract only ever
        // speaks of 省略/omission and freezes no distinct answer for a declared `null`, so no
        // frozen behaviour is lost; widening the type would change `CreateObjectInput` for every
        // caller, and is left as the thing to do if the contract ever splits the two.
        //
        // Inheritance keeps `flow_objects_parent_project_fk` satisfied by construction rather than
        // relaxing it: the row goes in carrying the parent's own scope, so the *stored* values
        // still compare equal and the constraint (which is deliberately blind to how the value was
        // obtained) never sees a difference.
        match input.project_id {
            Some(declared) if parent.project_id != Some(declared) => {
                return Err(ApiError::invalid_update_with_details(
                    "project_id must equal the parent object's project_id: a non-root object sits in \
                     its parent's project scope (omit project_id to inherit the parent's scope)",
                    json!({ "reason": CHILD_PROJECT_MUST_MATCH_PARENT }),
                ));
            }
            Some(_) => {}
            None => effective_project_id = parent.project_id,
        }
        // The write-side half of `ADR-0012` §3's depth/cycle rule. The read side already fails
        // closed on an over-deep, cyclic, or incomplete inheritance chain
        // (`authz::fetch_chain`), but nothing stopped such a chain from being *created*: the only
        // schema guard on `parent_id` is `flow_objects_parent_not_self_check` (migration `0054`),
        // which rejects `parent_id = id` and nothing else. Without this call a child of a parent
        // already at `tree_depth_max` lands in the table and is then permanently unauthorizable
        // for every non-admin -- the read side denies it and its whole subtree forever, and v0.4
        // ships no re-parent command to repair it.
        authz::ensure_parent_can_adopt_child(&state.db, input.workspace_id, parent_id).await?;
    }

    // Build the document outside any lock, matching `v0.4-flow-alpha.md`'s "hydrate/apply
    // outside the row lock" rule (there is no existing row to lock for a brand-new document
    // anyway: `contended_existing_document_set` is 0 for this command, see
    // `domain-model-v1.md` "Command boundaries").
    let mut engine = LoroCollabEngine::new_empty(rand::random());
    engine.set_title(&title).map_err(|err| {
        tracing::error!(error = %err, "collab-core: set_title failed on a brand-new document");
        ApiError::Internal
    })?;
    let snapshot = engine.export_snapshot().map_err(|err| {
        tracing::error!(error = %err, "collab-core: export_snapshot failed on a brand-new document");
        ApiError::Internal
    })?;
    let frontier = engine.frontier();
    let semantic = engine.semantic_snapshot().map_err(|err| {
        tracing::error!(error = %err, "collab-core: semantic_snapshot failed on a brand-new document");
        ApiError::Internal
    })?;
    let state_json = projection::state_json(&semantic).map_err(|_| ApiError::Internal)?;
    let plain_text = projection::plain_text(&semantic);

    let object_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let frontier_bytes = frontier.as_bytes().to_vec();

    let tx = state.db.begin().await?;

    repository::insert_flow_object(
        &tx,
        &NewFlowObject {
            id: object_id,
            workspace_id: input.workspace_id,
            project_id: effective_project_id,
            object_type: input.object_type.clone(),
            parent_id: effective_parent_id,
            created_by: actor_user_id(input.actor_id, input.actor_is_bot),
        },
    )
    .await?;

    repository::insert_collab_document(
        &tx,
        &NewCollabDocument {
            id: document_id,
            object_id,
            format_version: DOCUMENT_FORMAT_VERSION.to_string(),
            snapshot,
            frontier: frontier_bytes.clone(),
        },
    )
    .await?;

    repository::insert_projection(
        &tx,
        &NewProjection {
            object_id,
            document_seq: 0,
            document_frontier: frontier_bytes.clone(),
            title: title.clone(),
            state: state_json.clone(),
            plain_text,
        },
    )
    .await?;

    let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;
    let outcome = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id: input.workspace_id,
            // The event's scope is the object's scope, and after inheritance those are
            // `effective_project_id`, not the possibly-omitted request field. A `flow.object.created`
            // row filed under the unprojected scope for an object stored in a project's scope would
            // make every project-filtered event read disagree with `flow_objects` itself.
            project_id: effective_project_id,
            event_type: "flow.object.created".to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: object_id.to_string(),
            actor_id: actor_user_id(input.actor_id, input.actor_is_bot),
            source: input.origin.source_json(),
            payload: json!({
                "object_id": object_id,
                "object_type": input.object_type,
                "parent_object_id": effective_parent_id,
            }),
            metadata: json!({
                "message": input.message,
                "idempotency_body": idempotency_body.clone(),
            }),
            correlation_id: Some(input.origin.correlation_id),
            causation_id: input.origin.causation_id,
            idempotency_key: Some(input.idempotency_key.clone()),
        },
        Some(FlowDispatchSpec {
            max_attempts: dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?;

    // Race lost. `insert_flow_event` resolves its `ON CONFLICT ... DO NOTHING` by *reading back*
    // the winner's row rather than erroring, so `was_new == false` means a concurrent request
    // with this same key already committed the whole aggregate — and the object/document/
    // projection rows staged above are now a second, unreferenced aggregate for one logical
    // creation. Committing them would answer the caller with a `object_id` that no
    // `business_events` row names and that no replay of this key will ever return again: a
    // phantom success. The pre-transaction guard above cannot cover this, because it necessarily
    // runs before the winner commits; the database's own unique index is the only point at which
    // the race is decided, so the answer has to be recovered here.
    //
    // Roll back, then re-read the winner's committed aggregate and return *that*. This is not a
    // retry: the caller's create already happened (under the other request), so the correct
    // response for this request is the canonical object, produced inside this same call.
    if !outcome.was_new {
        // A rollback failure means the connection died with the transaction still open; the
        // server has no way to make the staged rows visible in that case either, so the caller's
        // answer is still the winner's committed object.
        let _ = tx.rollback().await;
        return replay_created_object(state, input.workspace_id, &input.idempotency_key, &idempotency_body)
            .await?
            // `was_new == false` is only reachable once the conflicting row is committed and
            // visible (`ON CONFLICT DO NOTHING` waits out an in-flight speculative insertion
            // before deciding), so the event this just conflicted with must be readable here.
            .ok_or(ApiError::Internal);
    }
    let event_id = outcome.event_id;

    tx.commit().await?;

    let object = FlowObjectView {
        id: object_id,
        workspace_id: input.workspace_id,
        // Same reason as the event above: this view is the caller's copy of the row that was just
        // committed, so it reports the inherited scope, not the omitted request field.
        project_id: effective_project_id,
        parent_id: effective_parent_id,
        object_type: input.object_type,
        lifecycle_status: "active".to_string(),
        governance_metadata: json!({}),
        title,
        semantic_content: state_json.clone(),
        document_id,
        document_seq: 0,
        frontier: projection::encode_frontier(&frontier),
        projection_seq: 0,
        projection_lag: 0,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        archived_at: None,
    };

    Ok(AcceptedChange {
        accepted_seq: 0,
        head_frontier: object.frontier.clone(),
        projection_seq: 0,
        semantic_diff: state_json,
        affected_object_ids: vec![object_id],
        event_id,
        command_result: None,
        object,
    })
}

/// `flow_object_grants.level` / `flow_workspace_settings.default_member_level`'s four values
/// (`domain-model-v1.md`), reused here so `set_flow_feature` rejects an unknown level the same way
/// the database `CHECK` constraint would, instead of surfacing a `Database` 500.
const MEMBER_LEVELS: &[&str] = &["full_access", "edit", "comment", "view"];

pub struct SetFlowFeatureInput {
    pub workspace_id: Uuid,
    pub actor_id: Uuid,
    /// Whether [`Self::actor_id`] is a `workspace_bots` id rather than a `users` id.
    ///
    /// Every "who did this" column on the Flow write paths — `flow_objects.created_by`/
    /// `updated_by`, `flow_workspace_settings.updated_by`, `collab_updates.actor_id`,
    /// `business_events.actor_id` — is `REFERENCES users(id)`, while
    /// `middleware::bot_auth::require_workspace_access_from_auth` returns the **bot id** as the
    /// actor for a bot token. Writing that id into any of those columns is a foreign-key
    /// violation, which is why this flag has to travel with the actor rather than be inferred: the
    /// surface cannot tell you (a bot may legitimately present as `rest`), and the id itself
    /// cannot tell you.
    ///
    /// See [`actor_user_id`].
    pub actor_is_bot: bool,
    pub enabled: Option<bool>,
    pub default_member_level: Option<String>,
    pub idempotency_key: String,
    /// See [`CreateObjectInput::origin`].
    pub origin: CommandOrigin,
}

fn validate_set_flow_feature(input: &SetFlowFeatureInput) -> Result<(), ApiError> {
    if input.enabled.is_none() && input.default_member_level.is_none() {
        return Err(ApiError::BadRequest(
            "at least one of enabled or default_member_level must be supplied".to_string(),
        ));
    }
    if let Some(level) = input.default_member_level.as_deref()
        && !MEMBER_LEVELS.contains(&level)
    {
        return Err(ApiError::BadRequest(format!(
            "default_member_level must be one of {MEMBER_LEVELS:?}"
        )));
    }
    let key_bytes = input.idempotency_key.len();
    if !(IDEMPOTENCY_KEY_MIN_BYTES..=IDEMPOTENCY_KEY_MAX_BYTES).contains(&key_bytes) {
        return Err(ApiError::BadRequest(format!(
            "idempotency_key must be {IDEMPOTENCY_KEY_MIN_BYTES}-{IDEMPOTENCY_KEY_MAX_BYTES} bytes"
        )));
    }
    Ok(())
}

/// `PUT /api/v1/workspaces/{workspace_id}/features/flow`.
///
/// A feature-flag transition emits `flow.feature.enabled|disabled`; a baseline transition emits
/// `flow.permission.baseline_changed`. Either authorization-affecting transition advances
/// `authz_epoch`, and all settings update in this one transaction. When both change, the baseline
/// event owns the request idempotency key and the feature event is causally derived from it. A
/// no-op still stamps the updater but emits no event.
pub async fn set_flow_feature(state: &AppState, input: SetFlowFeatureInput) -> Result<FlowFeatureUpdateView, ApiError> {
    validate_set_flow_feature(&input)?;

    if let Some(existing) =
        repository::find_idempotent_event(&state.db, input.workspace_id, &input.idempotency_key).await?
    {
        if existing.event_type != "flow.feature.enabled"
            && existing.event_type != "flow.feature.disabled"
            && existing.event_type != "flow.permission.baseline_changed"
        {
            return Err(ApiError::Conflict(
                "idempotency_key was already used for a different operation".to_string(),
            ));
        }
        let row = repository::fetch_flow_settings(&state.db, input.workspace_id).await?;
        return Ok(FlowFeatureUpdateView {
            feature: feature_view_from_row(row),
            event_id: Some(existing.id),
        });
    }

    let tx = state.db.begin().await?;

    repository::ensure_flow_settings_row(&tx, input.workspace_id).await?;
    let current = repository::fetch_flow_settings_for_update(&tx, input.workspace_id)
        .await?
        .ok_or(ApiError::Internal)?;

    let new_enabled = input.enabled.unwrap_or(current.flow_enabled);
    let transition = input.enabled.filter(|&enabled| enabled != current.flow_enabled);
    let new_member_level = input
        .default_member_level
        .as_deref()
        .unwrap_or(&current.default_member_level);
    let baseline_transition = (new_member_level != current.default_member_level)
        .then(|| (current.default_member_level.clone(), new_member_level.to_string()));
    let authorization_transition = baseline_transition.is_some() || transition.is_some();
    let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;

    let baseline_event_id = if let Some((old_level, new_level)) = &baseline_transition {
        let outcome = insert_flow_event(
            &tx,
            BusinessEventInput {
                workspace_id: input.workspace_id,
                project_id: None,
                event_type: "flow.permission.baseline_changed".to_string(),
                aggregate_type: "flow_permission".to_string(),
                aggregate_id: input.workspace_id.to_string(),
                actor_id: actor_user_id(input.actor_id, input.actor_is_bot),
                source: input.origin.source_json(),
                payload: json!({
                    "workspace_id": input.workspace_id,
                    "old_level": old_level,
                    "new_level": new_level,
                }),
                metadata: json!({}),
                correlation_id: Some(input.origin.correlation_id),
                causation_id: input.origin.causation_id,
                idempotency_key: Some(input.idempotency_key.clone()),
            },
            Some(FlowDispatchSpec {
                max_attempts: dispatch_max_attempts,
                document_id: None,
                accepted_seq: None,
            }),
        )
        .await?;
        if !outcome.was_new {
            tx.rollback().await?;
            let row = repository::fetch_flow_settings(&state.db, input.workspace_id).await?;
            return Ok(FlowFeatureUpdateView {
                feature: feature_view_from_row(row),
                event_id: Some(outcome.event_id),
            });
        }
        Some(outcome.event_id)
    } else {
        None
    };

    let feature_event_id = if let Some(enabled) = transition {
        let event_type = if enabled {
            "flow.feature.enabled"
        } else {
            "flow.feature.disabled"
        };
        let feature_origin = baseline_event_id.map_or_else(|| input.origin.clone(), |id| input.origin.derived_from(id));
        let outcome = insert_flow_event(
            &tx,
            BusinessEventInput {
                workspace_id: input.workspace_id,
                project_id: None,
                event_type: event_type.to_string(),
                aggregate_type: "flow_feature".to_string(),
                aggregate_id: input.workspace_id.to_string(),
                actor_id: actor_user_id(input.actor_id, input.actor_is_bot),
                source: feature_origin.source_json(),
                payload: json!({ "workspace_id": input.workspace_id }),
                metadata: json!({}),
                correlation_id: Some(feature_origin.correlation_id),
                causation_id: feature_origin.causation_id,
                idempotency_key: baseline_event_id.is_none().then(|| input.idempotency_key.clone()),
            },
            Some(FlowDispatchSpec {
                max_attempts: dispatch_max_attempts,
                document_id: None,
                accepted_seq: None,
            }),
        )
        .await?;
        if !outcome.was_new {
            tx.rollback().await?;
            let row = repository::fetch_flow_settings(&state.db, input.workspace_id).await?;
            return Ok(FlowFeatureUpdateView {
                feature: feature_view_from_row(row),
                event_id: Some(outcome.event_id),
            });
        }
        Some(outcome.event_id)
    } else {
        None
    };

    let committed_epoch = if authorization_transition {
        Some(authz::advance_epoch(&tx, input.workspace_id).await?)
    } else {
        None
    };

    repository::update_flow_settings(
        &tx,
        input.workspace_id,
        new_enabled,
        new_member_level,
        actor_user_id(input.actor_id, input.actor_is_bot),
    )
    .await?;
    tx.commit().await?;

    if let Some(committed_epoch) = committed_epoch {
        super::collab::permission_cache::invalidate_workspace_after_commit(state, input.workspace_id);
        let revocation_stats = if new_enabled {
            super::collab::revocation::revalidate_workspace_after_commit(state, input.workspace_id, committed_epoch)
                .await
        } else {
            super::collab::revocation::disconnect_workspace_for_disabled_feature(input.workspace_id, committed_epoch)
        };
        tracing::debug!(workspace_id = %input.workspace_id, ?revocation_stats, "workspace sessions handled after Flow feature change");
    }

    let updated = repository::fetch_flow_settings(&state.db, input.workspace_id).await?;
    Ok(FlowFeatureUpdateView {
        feature: feature_view_from_row(updated),
        event_id: baseline_event_id.or(feature_event_id),
    })
}

/// Reconstructs the `AcceptedChange` an idempotent replay returns: the object's current view,
/// with `accepted_seq`/`projection_seq` read back from storage rather than re-derived, and
/// `event_id` pointing at the original event row rather than minting a new one — a replay must
/// report the same fact that already happened, not a second event.
pub(super) fn accepted_change_from_row(row: repository::ObjectViewRow, event_id: Uuid) -> AcceptedChange {
    let view = super::query::object_view_from_row(row);
    AcceptedChange {
        accepted_seq: view.document_seq,
        head_frontier: view.frontier.clone(),
        projection_seq: view.projection_seq,
        semantic_diff: view.semantic_content.clone(),
        affected_object_ids: vec![view.id],
        event_id,
        command_result: None,
        object: view,
    }
}

// ---- `POST .../flow/objects/{object_id}/commands` ----

/// `NodeId`/`client_id`-style caller-supplied string bound (matches
/// `ticket::validate_client_id`'s convention for the same class of opaque caller-chosen string).
const NODE_ID_MAX_BYTES: usize = 256;

/// The five v0.4 content command types (`rest-api-v1.md`): each maps to one or more
/// [`collab_core::Operation`]s (or, for `set_title`, [`LoroCollabEngine::set_title`]) applied to an
/// isolated fork before being exported as CRDT update bytes and handed to the exact same
/// [`write::accept_update`] the WebSocket write path uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentCommandType {
    SetTitle,
    InsertBlock,
    UpdateBlock,
    DeleteBlock,
    MoveBlock,
    SemanticPatch,
}

impl ContentCommandType {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "set_title" => Some(Self::SetTitle),
            "insert_block" => Some(Self::InsertBlock),
            "update_block" => Some(Self::UpdateBlock),
            "delete_block" => Some(Self::DeleteBlock),
            "move_block" => Some(Self::MoveBlock),
            "semantic_patch" => Some(Self::SemanticPatch),
            _ => None,
        }
    }

    const fn wire_name(self) -> &'static str {
        match self {
            Self::SetTitle => "set_title",
            Self::InsertBlock => "insert_block",
            Self::UpdateBlock => "update_block",
            Self::DeleteBlock => "delete_block",
            Self::MoveBlock => "move_block",
            Self::SemanticPatch => "semantic_patch",
        }
    }

    /// `ADR-0013` §1's table: "所有内容编辑" → cardinality 1. Every content command loads and
    /// advances exactly the one existing `collab_documents` row `execute_content_command` reads
    /// via `bootstrap::load(&state.db, document_id)` — never zero (there is always a document to
    /// edit) and never more than one (v0.4 has no cross-document content command). Matched on
    /// `self` (rather than a bare constant) so a future content command variant with a different
    /// cardinality must edit this match, not silently inherit `One`.
    const fn existing_document_cardinality(self) -> ExistingDocumentCardinality {
        match self {
            Self::SetTitle
            | Self::InsertBlock
            | Self::UpdateBlock
            | Self::DeleteBlock
            | Self::MoveBlock
            | Self::SemanticPatch => ExistingDocumentCardinality::One,
        }
    }
}

/// The two v0.4 lifecycle command types. Neither advances a document head
/// (`existing_document_cardinality = 0`): a plain `flow_objects.lifecycle_status` transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleCommandType {
    Archive,
    Restore,
}

impl LifecycleCommandType {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "archive" => Some(Self::Archive),
            "restore" => Some(Self::Restore),
            _ => None,
        }
    }

    const fn wire_name(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Restore => "restore",
        }
    }

    /// `ADR-0013` §1's table, first row: pure `flow_objects.lifecycle_status` governance, no
    /// `collab_documents` row touched at all.
    const fn existing_document_cardinality(self) -> ExistingDocumentCardinality {
        match self {
            Self::Archive | Self::Restore => ExistingDocumentCardinality::Zero,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LifecyclePlan {
    rows: Vec<repository::LifecycleScopeRow>,
    cascade: bool,
}

impl LifecyclePlan {
    fn derive(object_id: Uuid, rows: Vec<repository::LifecycleScopeRow>) -> Result<Self, ApiError> {
        if rows.is_empty() {
            return Err(ApiError::NotFound("flow object not found".to_string()));
        }
        if rows.iter().any(|row| row.invalid_tree) {
            return Err(ApiError::invalid_update(
                "the lifecycle impact set is cyclic or exceeds tree_depth_max",
            ));
        }
        if !rows.iter().any(|row| row.id == object_id) {
            return Err(ApiError::invalid_update(
                "the lifecycle impact set does not contain its requested root",
            ));
        }
        // `rest-api-v1.md` freezes the v0.4 `archive|restore` payload without a cascade
        // operation. Unknown payload fields remain tolerated for wire compatibility, but cannot
        // change semantics. A future cascade must be a separately registered request semantic;
        // the mere presence of descendants is not such a request.
        let cascade = false;
        Ok(Self { rows, cascade })
    }

    fn root(&self, object_id: Uuid) -> Result<&repository::LifecycleScopeRow, ApiError> {
        self.rows
            .iter()
            .find(|row| row.id == object_id)
            .ok_or_else(|| ApiError::invalid_update("the lifecycle impact root drifted"))
    }

    fn affected_object_ids(&self) -> Vec<Uuid> {
        self.rows.iter().map(|row| row.id).collect()
    }

    fn required_permission_level(&self, object_id: Uuid) -> Result<authz::PermissionLevel, ApiError> {
        let root = self.root(object_id)?;
        Ok(lifecycle_required_permission_level(LifecycleTierFacts {
            object_type: &root.object_type,
            parent_id: root.parent_id,
            affected_count: if self.cascade { self.rows.len() } else { 1 },
            has_shared_descendants: root.has_shared_descendants,
            durability: LifecycleDurability::Reversible,
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleDurability {
    Reversible,
    Irreversible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LifecycleTierFacts<'a> {
    object_type: &'a str,
    parent_id: Option<Uuid>,
    affected_count: usize,
    has_shared_descendants: bool,
    durability: LifecycleDurability,
}

fn lifecycle_required_permission_level(facts: LifecycleTierFacts<'_>) -> authz::PermissionLevel {
    if facts.object_type == "navigator"
        || facts.object_type == "collection"
        || facts.parent_id.is_none()
        || facts.affected_count > 1
        || facts.has_shared_descendants
        || facts.durability == LifecycleDurability::Irreversible
    {
        authz::PermissionLevel::FullAccess
    } else {
        authz::PermissionLevel::Edit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Content(ContentCommandType),
    Lifecycle(LifecycleCommandType),
    /// v0.5's governance family (`super::move_object`). Declared in that module rather than here
    /// so this module stays the v0.4-frozen registry `verify-flow-authz-baseline-v0.4.sh`'s static
    /// check reads, and so a `bounded_many` command's wire name lives next to the multi-document
    /// machinery that makes it legal.
    Governance(GovernanceCommandType),
}

impl CommandKind {
    fn parse(raw: &str) -> Option<Self> {
        ContentCommandType::parse(raw)
            .map(Self::Content)
            .or_else(|| LifecycleCommandType::parse(raw).map(Self::Lifecycle))
            .or_else(|| GovernanceCommandType::parse(raw).map(Self::Governance))
    }

    /// The `events-v1.md` primary event type this command produces on success — also the
    /// expected `business_events.event_type` an idempotent replay of this `idempotency_key` must
    /// match (a caller reusing the same key for a different command type is `Conflict`, not a
    /// silent replay of the wrong operation).
    const fn event_type(self) -> &'static str {
        match self {
            Self::Content(_) => "flow.content.accepted",
            Self::Lifecycle(LifecycleCommandType::Archive) => "flow.object.archived",
            Self::Lifecycle(LifecycleCommandType::Restore) => "flow.object.restored",
            Self::Governance(kind) => kind.event_type(),
        }
    }

    /// The generic permission level for commands whose scope is known from their variant alone.
    /// Lifecycle permission is deliberately not decided here: its tier depends on the actual tree
    /// impact and sharing facts in `LifecyclePlan`.
    const fn required_permission_level(self) -> authz::PermissionLevel {
        match self {
            // `ADR-0012` §4: "被移动对象需 `full_access`（移动会改变它的继承）". This is only the
            // source side of the double-sided rule; `move_object::execute` owns the target side
            // (`edit` on the new parent), which is a different authorization domain whenever the
            // two sides sit under different boundaries.
            Self::Governance(GovernanceCommandType::MoveObject) => authz::PermissionLevel::FullAccess,
            Self::Governance(GovernanceCommandType::Link | GovernanceCommandType::Unlink) => {
                authz::PermissionLevel::Edit
            }
            // Lifecycle commands never reach this generic path; their actual tier comes from a
            // `LifecyclePlan`. The value here preserves the v0.4 registry's ordinary-page baseline.
            Self::Content(_) | Self::Lifecycle(_) => authz::PermissionLevel::Edit,
        }
    }
}

pub struct ExecuteCommandInput {
    pub object_id: Uuid,
    pub actor_id: Uuid,
    /// `"user"` or `"bot"` (matches `flow_object_grants.principal_kind`/
    /// `authz::effective_permission`'s `principal_kind` parameter).
    pub principal_kind: String,
    /// The caller's `workspace_members.role` (or bot-synthesized role) — see
    /// `middleware::bot_auth::require_workspace_access`.
    pub role: String,
    pub command_type: String,
    pub payload: Value,
    /// Base64 `Frontier` bytes, when the caller wants strict optimistic-concurrency locking
    /// instead of the CRDT's default commutative merge (`write::UpdateRequest::expected_frontier`).
    /// Only valid for the six content command types; `archive`/`restore` reject a non-`None` value
    /// (they never advance a document head, so it could never be honored).
    pub expected_frontier: Option<String>,
    pub idempotency_key: String,
    pub message: Option<String>,
    /// A caller identity string stamped onto `collab_updates.origin_client_id` and onto the
    /// `update` frame relayed to any WebSocket sessions with this document open — this surface has
    /// no `client_id` handshake like the WebSocket ticket flow, so the REST layer synthesizes one
    /// (see `routes::flow::post_flow_object_command`).
    pub origin_client_id: String,
    /// Where this command came from, declared by the transport that accepted it — see
    /// [`CommandOrigin`]. Every producer this command reaches (`execute_content_command`'s
    /// `flow.content.accepted`, `execute_lifecycle_command`'s archive/restore,
    /// `move_object`'s `flow.object.moved` plus its derived `flow.content.accepted` rows, and
    /// `record_command_rejected`'s audit-only row) reads its `source`/`correlation_id`/
    /// `causation_id` from here rather than deciding any of them itself.
    pub origin: CommandOrigin,
}

impl ExecuteCommandInput {
    /// Whether [`Self::actor_id`] is a `workspace_bots` id rather than a `users` id.
    ///
    /// Derived from [`Self::principal_kind`] rather than carried as a second field: two
    /// representations of one fact can disagree, and this one already exists and is already the
    /// value `flow::grants` judges principals by (`Caller::is_bot`). See [`actor_user_id`] for why
    /// the distinction has to reach the write sites at all.
    #[must_use]
    pub fn actor_is_bot(&self) -> bool {
        self.principal_kind == "bot"
    }
}

fn validate_execute_command_input(input: &ExecuteCommandInput) -> Result<(), ApiError> {
    let key_bytes = input.idempotency_key.len();
    if !(IDEMPOTENCY_KEY_MIN_BYTES..=IDEMPOTENCY_KEY_MAX_BYTES).contains(&key_bytes) {
        return Err(ApiError::BadRequest(format!(
            "idempotency_key must be {IDEMPOTENCY_KEY_MIN_BYTES}-{IDEMPOTENCY_KEY_MAX_BYTES} bytes"
        )));
    }
    if let Some(message) = &input.message
        && message.chars().count() > MESSAGE_MAX_CHARS
    {
        return Err(ApiError::BadRequest(format!(
            "message must be at most {MESSAGE_MAX_CHARS} characters"
        )));
    }
    Ok(())
}

/// Writes the audit-only `flow.command.rejected` event (`events-v1.md` "Event type registry":
/// `aggregate_type=flow_command`, "命令在创建 job 前被拒绝时只产生 audit-only `flow.command.
/// rejected`"). `insert_flow_event`'s `dispatch: None` is the `delivery_class=audit_only` half of
/// that helper: the row is written but never gets an `event_dispatch` row, so it never reaches a
/// webhook subscriber ("永不产生 dispatch work 与投递行、不触发 webhook").
///
/// Always called against `state.db` directly, never inside the rejected command's own
/// transaction: `events-v1.md` — "失败的 domain transaction 不得留下 success event 与 dispatch
/// work...；rejected/audit-only row 在失败 transaction 回滚后以安全摘要单独写入" — every caller
/// below has already rolled back (or never opened) that transaction by the time this runs.
///
/// Only called once `workspace_id` is known, i.e. after the target object has been resolved: a
/// rejection that happens before that point (malformed request shape, an unregistered
/// `command.type`, or the object simply not existing) has no workspace to scope an audit row
/// under, and the REST error response itself remains the caller's complete signal for those.
///
/// Never surfaces a failure to the caller — `events-v1.md` requires "写 audit 失败必须告警", not
/// that a command's own (already-decided) rejection be replaced by a second, unrelated database
/// error — so a failure here is only logged.
#[allow(clippy::too_many_arguments)]
async fn record_command_rejected(
    state: &AppState,
    workspace_id: Uuid,
    actor_id: Uuid,
    // Same `users(id)` FK as every other producer, and the same reason it has to be told rather
    // than infer: see [`actor_user_id`]. Missing it here was worse than elsewhere, because this
    // function **swallows its own failure** — the insert is logged and dropped, so a bot-triggered
    // rejection did not 500, it simply left no audit row at all. A silently missing audit row is
    // the one failure mode an audit stream cannot survive.
    actor_is_bot: bool,
    origin: &CommandOrigin,
    action: &str,
    error: &ApiError,
    object_id: Uuid,
    document_id: Uuid,
) {
    let error_code = error.kind().stable_code();
    let result = insert_flow_event(
        &state.db,
        BusinessEventInput {
            workspace_id,
            project_id: None,
            event_type: "flow.command.rejected".to_string(),
            aggregate_type: "flow_command".to_string(),
            // `events-v1.md`: "aggregate id 是服务端 request_id 或 update_id" — this command never
            // minted one before being rejected, so a fresh id is synthesized for this audit row
            // (never the caller's `idempotency_key`, which stays reserved for a future successful
            // retry of the same request).
            aggregate_id: Uuid::new_v4().to_string(),
            actor_id: actor_user_id(actor_id, actor_is_bot),
            source: origin.source_json(),
            payload: json!({
                "action": action,
                "error_code": error_code,
                "object_id": object_id,
                "document_id": document_id,
            }),
            metadata: json!({}),
            // A rejected command is its own command's only event, so it inherits the command's
            // own place in the chain: the request's correlation, and whatever caused the command
            // itself (`None` for a first user request). It is not "derived from" a primary event,
            // because a rejected command never produced one.
            correlation_id: Some(origin.correlation_id),
            causation_id: origin.causation_id,
            idempotency_key: None,
        },
        None,
    )
    .await;
    if let Err(err) = result {
        tracing::error!(
            error = %err,
            action,
            error_code,
            "failed to record flow.command.rejected audit event"
        );
    }
}

/// `POST /api/v1/flow/objects/{object_id}/commands`.
///
/// # Errors
/// `NotFound` if the object does not exist; `BadRequest` for an unregistered `command.type` or a
/// malformed payload (`invalid_update` on the wire); a typed `policy_rejected` for insufficient
/// permission or a commit-time `authz_epoch` mismatch; `Conflict` for an `idempotency_key` reused
/// with a different command or a redundant `archive`; a typed `stale_frontier`/`resync_required`/
/// `server_draining`/`limit_exceeded`/`invalid_update` rejection from the shared write path.
/// Propagates a database failure otherwise. Every rejection reached once `object_id` resolves to a
/// real object also records an audit-only `flow.command.rejected` event (see
/// [`record_command_rejected`]).
pub async fn execute_command(state: &AppState, input: ExecuteCommandInput) -> Result<AcceptedChange, ApiError> {
    validate_execute_command_input(&input)?;

    let kind = CommandKind::parse(&input.command_type).ok_or_else(|| {
        ApiError::invalid_update(format!(
            "command.type '{}' is not a registered v0.4 command",
            input.command_type
        ))
    })?;
    if matches!(kind, CommandKind::Content(ContentCommandType::SemanticPatch)) {
        check_semantic_patch_json_bytes(&input.payload)?;
    }

    let view_row = repository::fetch_object_view(&state.db, input.object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let workspace_id = view_row.workspace_id;
    let document_id = view_row.document_id;
    let object_type = view_row.object_type.clone();
    runtime::runtime().ensure_workspace_accepting(workspace_id)?;

    match execute_command_authorized(state, &input, kind, workspace_id, document_id, &object_type).await {
        Ok(change) => Ok(change),
        Err(err) => {
            record_command_rejected(
                state,
                workspace_id,
                input.actor_id,
                input.actor_is_bot(),
                &input.origin,
                &input.command_type,
                &err,
                input.object_id,
                document_id,
            )
            .await;
            Err(err)
        }
    }
}

/// The idempotency-replay check, permission check, and command dispatch that make up
/// `execute_command`'s body once `workspace_id`/`document_id`/`object_type` are known — split out
/// so [`execute_command`] can wrap every `Err` this returns with [`record_command_rejected`]
/// without duplicating that wrapping at each individual early return.
async fn execute_command_authorized(
    state: &AppState,
    input: &ExecuteCommandInput,
    kind: CommandKind,
    workspace_id: Uuid,
    document_id: Uuid,
    _object_type: &str,
) -> Result<AcceptedChange, ApiError> {
    // Relation commands have a relation id (not the source object id) as their event aggregate,
    // and their replay identity also includes the target/relation id from the payload. Let their
    // module perform that richer replay check before the generic object/document aggregate check.
    if let CommandKind::Governance(kind @ (GovernanceCommandType::Link | GovernanceCommandType::Unlink)) = kind {
        let checked_epoch = authz::read_epoch(&state.db, workspace_id).await?;
        return super::relations::execute_command(state, input, workspace_id, checked_epoch, kind).await;
    }

    if let CommandKind::Lifecycle(lifecycle_kind) = kind {
        return execute_lifecycle_command(state, input, workspace_id, lifecycle_kind).await;
    }

    if let Some(existing) = repository::find_idempotent_event(&state.db, workspace_id, &input.idempotency_key).await? {
        if existing.event_type != kind.event_type() {
            return Err(ApiError::Conflict(
                "idempotency_key was already used for a different operation".to_string(),
            ));
        }
        // Same event type, different target. `business_events`' idempotency index is
        // *workspace*-scoped, so without this a key reused across two objects would return the
        // first object's event id while reporting the second object's current state -- and
        // silently apply nothing. `aggregate_id` is the document for content commands
        // (`flow.content.accepted`) and the object for lifecycle ones.
        let expected_aggregate = match kind {
            CommandKind::Content(_) => document_id,
            CommandKind::Lifecycle(_) | CommandKind::Governance(_) => input.object_id,
        };
        if existing.aggregate_id != expected_aggregate.to_string() {
            return Err(ApiError::Conflict(
                "idempotency_key was already used for a different operation".to_string(),
            ));
        }
        let current = repository::fetch_object_view(&state.db, input.object_id)
            .await?
            .ok_or(ApiError::Internal)?;
        return Ok(accepted_change_from_row(current, existing.id));
    }

    // `ADR-0012` §3.1's fence compares the epoch this command *checked permission against* with
    // the epoch still in force at commit, so `checked_epoch` has to be taken no later than the
    // permission read it is fencing. It used to be read afterwards (inside
    // `execute_content_command` / `execute_lifecycle_command`), which inverted the barrier: an
    // authorization change committing in the window between the permission read and the epoch
    // read got *baked into* `checked_epoch`, so the commit-time `FOR SHARE` compared the new
    // epoch against itself, matched, and let the already-stale permission land. The fence only
    // ever compares epochs — it never recomputes permission (`authz::fence_epoch_for_share`) —
    // so this ordering is the whole of the protection. Reading it first is also strictly safe in
    // the other direction: a revocation that commits *before* this read is seen by
    // `effective_permission` below, and one that commits after moves the epoch past this value
    // and is caught by the fence.
    let checked_epoch = authz::read_epoch(&state.db, workspace_id).await?;

    let principal_kind = if input.principal_kind == "bot" { "bot" } else { "user" };
    let level = authz::effective_permission(
        &state.db,
        workspace_id,
        input.object_id,
        principal_kind,
        input.actor_id,
        &input.role,
    )
    .await?;
    if level < kind.required_permission_level() {
        return Err(ApiError::policy_rejected("insufficient permission for this command"));
    }

    match kind {
        CommandKind::Content(content_kind) => {
            execute_content_command(state, input, workspace_id, document_id, checked_epoch, content_kind).await
        }
        CommandKind::Lifecycle(_)
        | CommandKind::Governance(GovernanceCommandType::Link | GovernanceCommandType::Unlink) => {
            Err(ApiError::Internal)
        }
        // The `full_access` check above is an early rejection (and the one that produces the
        // `flow.command.rejected` audit row); `move_object` re-decides both sides of
        // `ADR-0012` §4's double-sided rule inside its own transaction, on the snapshot it
        // commits, so it takes no permission level from here.
        CommandKind::Governance(GovernanceCommandType::MoveObject) => {
            super::move_object::execute(state, input, workspace_id, checked_epoch).await
        }
    }
}

#[derive(Debug, Deserialize)]
struct SetTitlePayload {
    title: String,
}

#[derive(Debug, Deserialize)]
struct InsertBlockPayload {
    block_id: String,
    #[serde(default)]
    parent_block_id: Option<String>,
    #[serde(default)]
    index: u32,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateBlockPayload {
    block_id: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    properties: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct DeleteBlockPayload {
    block_id: String,
}

#[derive(Debug, Deserialize)]
struct MoveBlockPayload {
    block_id: String,
    #[serde(default)]
    parent_block_id: Option<String>,
    #[serde(default)]
    index: u32,
}

#[derive(Debug, Deserialize)]
struct SemanticPatchPayload {
    operations: Vec<Operation>,
}

/// Enforces the serialized semantic-patch payload ceiling before the target object is read and,
/// crucially, before any canonical/audit/event row can be written. The parsed JSON value is
/// re-serialized compactly because every REST/MCP/CLI/Web producer reaches this application
/// service after JSON decoding; insignificant transport whitespace is not semantic patch data.
fn check_semantic_patch_json_bytes(payload: &Value) -> Result<(), ApiError> {
    let bytes =
        serde_json::to_vec(payload).map_err(|_| ApiError::invalid_update("semantic_patch is not valid JSON"))?;
    let observed = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if observed > collab_limits::SEMANTIC_PATCH_JSON_BYTES_MAX {
        return Err(ApiError::limit_exceeded(
            "semantic patch JSON exceeds the fixed byte ceiling",
            "semantic_patch_bytes",
            Some(json!(collab_limits::SEMANTIC_PATCH_JSON_BYTES_MAX)),
            Some(json!(observed)),
            None,
        ));
    }
    Ok(())
}

fn parse_node_id(raw: &str, field: &str) -> Result<NodeId, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > NODE_ID_MAX_BYTES {
        return Err(ApiError::BadRequest(format!(
            "{field} must be 1-{NODE_ID_MAX_BYTES} non-whitespace-only characters"
        )));
    }
    Ok(NodeId::from(trimmed))
}

fn parse_payload<T: serde::de::DeserializeOwned>(command_type: &str, payload: &Value) -> Result<T, ApiError> {
    serde_json::from_value(payload.clone())
        .map_err(|err| ApiError::BadRequest(format!("invalid payload for {command_type}: {err}")))
}

/// Maps a [`CollabError`] to the typed `invalid_update`/`limit_exceeded` [`ApiErrorKind`]s
/// (`error-mapping-v1.md`: both REST `BadRequest`/400, but distinct stable codes — never folded
/// into one string-typed `BadRequest` the caller would have to substring-match, which is exactly
/// the anti-pattern this typed discriminant replaces). Never a raw `{:?}` dump of engine
/// internals — each arm produces a caller-safe message naming only the logical node id or limit
/// kind, and `limit_exceeded` carries its `limit_kind`/`limit`/`observed` as structured `details`
/// instead of interpolated into the message text.
pub(super) fn map_collab_error(err: &CollabError) -> ApiError {
    match err {
        CollabError::UnknownNode { id } => ApiError::invalid_update(format!("unknown node id '{id}'")),
        CollabError::DuplicateNode { id } => ApiError::invalid_update(format!("duplicate node id '{id}'")),
        CollabError::CycleRejected { id } => {
            ApiError::invalid_update(format!("move of node '{id}' rejected: would create a cycle"))
        }
        // `CollabError::limit_kind()` special-cases this exact shape as the `update_bytes` limit
        // (an oversized snapshot/update byte slice); every other `InputTooLarge`/`EmptyInput`/
        // `DecodeFailed` shape falls through to the generic `invalid_update` arm below.
        CollabError::InputTooLarge {
            input: "update",
            actual_bytes,
            max_bytes,
        } => ApiError::limit_exceeded(
            "limit_exceeded: update_bytes",
            "update_bytes",
            Some(json!(max_bytes)),
            Some(json!(actual_bytes)),
            None,
        ),
        CollabError::EmptyInput { .. } | CollabError::InputTooLarge { .. } | CollabError::DecodeFailed { .. } => {
            ApiError::invalid_update("invalid_update")
        }
        CollabError::OperationFailed { reason } => ApiError::invalid_update(format!("invalid_update: {reason}")),
        CollabError::LimitExceeded {
            limit_kind,
            limit,
            observed,
        } => ApiError::limit_exceeded(
            format!("limit_exceeded: {limit_kind}"),
            limit_kind,
            Some(json!(limit)),
            Some(json!(observed)),
            None,
        ),
    }
}

/// Applies one content command's payload to `engine` (already forked, mutated in place). Every
/// operation this dispatches through is `apply_operation`'s shared vocabulary
/// (`crates/collab-core/src/engine.rs`) — no command type invents a second mutation path.
///
/// `set_title` aside (which never goes through the `Operation` vocabulary at all —
/// [`LoroCollabEngine::set_title`] is a document-level field, not a tree op, and `TITLE_MAX_CHARS`
/// already bounds it), every other command type first materializes the *complete, ordered* list
/// of [`Operation`]s the payload requires — `update_block`'s caller-supplied `properties` map is
/// the one case in this handler whose operation count is not fixed by the command shape itself,
/// so it is exactly the case `semantic_patch_operations_max` exists to bound. That full list is
/// checked with [`collab_core::limits::check_operation_batch_count`] *before* a single operation
/// is applied to `engine` — `limits-v1.md`'s "reject the whole patch atomically, never a partial
/// prefix" — then each operation is checked with [`collab_core::limits::check_operation`] against
/// the snapshot immediately before it (so a `create_node` that would make a second `create_node`
/// in the same batch exceed `container_count`, for example, is still caught) right before it is
/// applied. `engine` here is always a fresh, request-local fork (`hydrate_and_apply`'s caller in
/// `execute_content_command`) that is simply dropped on any `Err` return — a batch-count or
/// per-operation rejection therefore never reaches `engine.export_from`/`write::accept_update`, so
/// it can never advance a document head or produce a business event/`event_dispatch` row.
fn apply_content_command(
    engine: &mut LoroCollabEngine,
    kind: ContentCommandType,
    payload: &Value,
) -> Result<(), ApiError> {
    let ops: Vec<Operation> = match kind {
        ContentCommandType::SetTitle => {
            let payload: SetTitlePayload = parse_payload("set_title", payload)?;
            let title = payload.title.trim();
            if title.is_empty() {
                return Err(ApiError::BadRequest("title must not be empty".to_string()));
            }
            if title.chars().count() > TITLE_MAX_CHARS {
                return Err(ApiError::BadRequest(format!(
                    "title must be at most {TITLE_MAX_CHARS} characters"
                )));
            }
            return engine.set_title(title).map_err(|err| map_collab_error(&err));
        }
        ContentCommandType::InsertBlock => {
            let payload: InsertBlockPayload = parse_payload("insert_block", payload)?;
            let id = parse_node_id(&payload.block_id, "block_id")?;
            let parent = payload
                .parent_block_id
                .as_deref()
                .map(|raw| parse_node_id(raw, "parent_block_id"))
                .transpose()?;
            let mut ops = vec![Operation::CreateNode {
                id: id.clone(),
                parent,
                index: payload.index,
                kind: NodeKind::Block,
            }];
            if let Some(text) = payload.text.filter(|text| !text.is_empty()) {
                ops.push(Operation::InsertText { id, index: 0, text });
            }
            ops
        }
        ContentCommandType::UpdateBlock => {
            let payload: UpdateBlockPayload = parse_payload("update_block", payload)?;
            let id = parse_node_id(&payload.block_id, "block_id")?;
            if payload.text.is_none() && payload.properties.is_empty() {
                return Err(ApiError::BadRequest(
                    "update_block requires at least one of text or properties".to_string(),
                ));
            }
            let mut ops = Vec::new();
            if let Some(text) = payload.text {
                let existing = engine.semantic_snapshot().map_err(|err| map_collab_error(&err))?;
                let node = existing
                    .nodes
                    .get(&id)
                    .ok_or_else(|| ApiError::BadRequest(format!("unknown node id '{id}'")))?;
                let old_len = u32::try_from(node.text.len())
                    .map_err(|_| ApiError::BadRequest("block text too large to update".to_string()))?;
                if old_len > 0 {
                    ops.push(Operation::DeleteText {
                        id: id.clone(),
                        index: 0,
                        len: old_len,
                    });
                }
                if !text.is_empty() {
                    ops.push(Operation::InsertText {
                        id: id.clone(),
                        index: 0,
                        text,
                    });
                }
            }
            for (key, value) in payload.properties {
                ops.push(Operation::SetProperty {
                    id: id.clone(),
                    key,
                    value,
                });
            }
            ops
        }
        ContentCommandType::DeleteBlock => {
            let payload: DeleteBlockPayload = parse_payload("delete_block", payload)?;
            let id = parse_node_id(&payload.block_id, "block_id")?;
            vec![Operation::DeleteNode { id }]
        }
        ContentCommandType::MoveBlock => {
            let payload: MoveBlockPayload = parse_payload("move_block", payload)?;
            let id = parse_node_id(&payload.block_id, "block_id")?;
            let new_parent = payload
                .parent_block_id
                .as_deref()
                .map(|raw| parse_node_id(raw, "parent_block_id"))
                .transpose()?;
            vec![Operation::MoveNode {
                id,
                new_parent,
                index: payload.index,
            }]
        }
        ContentCommandType::SemanticPatch => {
            let payload: SemanticPatchPayload = parse_payload("semantic_patch", payload)?;
            if payload.operations.is_empty() {
                return Err(ApiError::invalid_update(
                    "semantic_patch requires at least one operation",
                ));
            }
            payload.operations
        }
    };

    let limits = collab_limits::document_limits();
    collab_core::limits::check_operation_batch_count(ops.len(), &limits)
        .map_err(|violation| map_collab_error(&CollabError::from(violation)))?;

    for op in ops {
        let snapshot = engine.semantic_snapshot().map_err(|err| map_collab_error(&err))?;
        collab_core::limits::check_operation(&snapshot, &op, &limits)
            .map_err(|violation| map_collab_error(&CollabError::from(violation)))?;
        engine.apply_operation(&op).map_err(|err| map_collab_error(&err))?;
    }
    Ok(())
}

/// `error-mapping-v1.md`'s stable-code → REST mapping, applied to a rejection from the shared
/// write path (`write::accept_update`) exactly the way the WebSocket layer would report it on the
/// wire, just carried through `ApiError` instead of a `Frame::Rejected`.
///
/// Every arm now carries an exact [`ApiErrorKind`] discriminant instead of a plain `BadRequest`/
/// `Conflict` string — the same class of gap the module's own doc comment on
/// [`ApiError::Typed`]-style constructors calls out: a caller previously had to substring-match
/// `"limit_exceeded: ..."`/`"server_draining"` out of the message text, which could not
/// distinguish `server_draining`'s `drain` from `contention` reason at all (both collapsed into
/// one `Conflict("server_draining")`). `rejected.details`' structured fields (`limit_kind`/
/// `limit`/`observed`/`retry_after_ms`/`minimum_snapshot_seq`) are now carried through as typed
/// `details` rather than interpolated into the message.
pub(super) fn map_write_rejection(rejected: &write::Rejected) -> ApiError {
    use super::collab::frame::RejectedCode;

    // Hoisted once so every arm below reads the same `Option<&Value>` instead of each
    // re-deriving `rejected.details.as_ref()` independently -- this is also the only real
    // caller-facing REST site that ever inspects `Rejected::details` at all, so it is where
    // `error-mapping-v1.md`'s "REST 必须能读出 limit_kind/limit/observed" requirement is either
    // honored or silently dropped.
    let details = rejected.details.as_ref();

    match rejected.code {
        RejectedCode::Unauthenticated => ApiError::unauthenticated("unauthenticated"),
        RejectedCode::Forbidden => ApiError::typed(ApiErrorKind::Forbidden, "forbidden"),
        RejectedCode::FeatureDisabled => ApiError::feature_disabled("feature_disabled"),
        RejectedCode::NotFound => ApiError::NotFound("not_found".to_string()),
        RejectedCode::UnsupportedProtocol => ApiError::unsupported_protocol("unsupported_protocol"),
        RejectedCode::InvalidUpdate => ApiError::invalid_update("invalid_update"),
        RejectedCode::PolicyRejected => ApiError::policy_rejected("policy_rejected"),
        RejectedCode::StaleFrontier => {
            let current_frontier = rejected.current_frontier.as_deref().map(frame::encode_bytes);
            ApiError::stale_frontier("stale_frontier", rejected.current_seq, current_frontier.as_deref())
        }
        RejectedCode::ResyncRequired => {
            let minimum_snapshot_seq = details
                .and_then(|details| details.get("minimum_snapshot_seq"))
                .and_then(Value::as_i64);
            ApiError::resync_required("resync_required", minimum_snapshot_seq)
        }
        RejectedCode::LimitExceeded => {
            let (limit_kind, limit, observed, retry_after_ms) =
                details.map_or(("unknown", None, None, None), |details| {
                    (
                        details.get("limit_kind").and_then(Value::as_str).unwrap_or("unknown"),
                        details.get("limit").cloned(),
                        details.get("observed").cloned(),
                        details.get("retry_after_ms").and_then(Value::as_u64),
                    )
                });
            ApiError::limit_exceeded(
                format!("limit_exceeded: {limit_kind}"),
                limit_kind,
                limit,
                observed,
                retry_after_ms,
            )
        }
        // `error-mapping-v1.md`'s `server_rejected` row: `Internal` / 500 / HTTP 200. The
        // `details.reason` the write path classified is carried through verbatim rather than
        // re-derived, and a rejection that somehow arrives without one still must not be reported
        // as retryable -- the whole point of this code is that it is not.
        RejectedCode::ServerRejected => ApiError::server_rejected(
            details
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("unclassified"),
        ),
        RejectedCode::ServerDraining => {
            let reason = details
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str);
            let retry_after_ms = details
                .and_then(|details| details.get("retry_after_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            match reason {
                Some("drain") => {
                    ApiError::server_draining(ServerDrainingReason::Drain, retry_after_ms, "server_draining")
                }
                Some("contention") => {
                    ApiError::server_draining(ServerDrainingReason::Contention, retry_after_ms, "server_draining")
                }
                // `error-mapping-v1.md`: "缺失/未知 reason 是 producer contract violation... 不得
                // 猜测为维护或竞争、不得用 message 补判" -- the write path never producing a
                // recognized reason here is a bug in that path, not something to paper over with a
                // guessed discriminant; the caller still gets a safe, generically-retryable
                // rejection instead of a fabricated `ServerDraining` reason.
                other => {
                    tracing::error!(
                        reason = ?other,
                        "write path rejected with server_draining but no recognized reason; this is a producer contract violation"
                    );
                    ApiError::Conflict("server_draining".to_string())
                }
            }
        }
    }
}

/// Resolves an already-committed `flow.content.accepted` event for `idempotency_key` on
/// `document_id` into the [`AcceptedChange`] a replay of that content write must return.
///
/// `Ok(None)` means no such event exists — the key is unused, or it belongs to a different
/// operation or a different document — and the caller must keep its own error rather than
/// substitute someone else's result. The cross-target case is already refused as a `Conflict` by
/// [`execute_command_authorized`]'s guard whenever it is visible to it, so returning `None`
/// here leaves that decision where it belongs instead of duplicating it.
///
/// # Errors
/// `Internal` if the event exists but the object it belongs to cannot be read back.
async fn replay_content_command(
    state: &AppState,
    workspace_id: Uuid,
    document_id: Uuid,
    object_id: Uuid,
    idempotency_key: &str,
) -> Result<Option<AcceptedChange>, ApiError> {
    let Some(existing) = repository::find_idempotent_event(&state.db, workspace_id, idempotency_key).await? else {
        return Ok(None);
    };
    if existing.event_type != "flow.content.accepted" || existing.aggregate_id != document_id.to_string() {
        return Ok(None);
    }
    let view = repository::fetch_object_view(&state.db, object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok(Some(accepted_change_from_row(view, existing.id)))
}

/// Builds the CRDT update bytes for one content command (isolated fork, outside any lock — matches
/// `write::hydrate_and_apply`'s own discipline for exactly this reason: neither path may hold a
/// lock across a CRDT apply), then submits them through [`write::accept_update`] — the identical
/// function `flow::collab::session::handle_client_frame` calls for a WebSocket `update` frame.
/// `accept_update` itself broadcasts the resulting `update`+`accepted` frame pair to this
/// document's WebSocket sessions (`collab-protocol-v1.md`: "复用 ADR-0010 的 ordered egress /
/// invalidation 通道，不另起一套" — this call and the WebSocket path share that one broadcast call
/// site inside `accept_update`, not a second one here).
async fn execute_content_command(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    document_id: Uuid,
    // The epoch `execute_command_authorized` read *before* computing this caller's permission --
    // see the comment there for why it must not be re-read here.
    checked_epoch: i64,
    kind: ContentCommandType,
) -> Result<AcceptedChange, ApiError> {
    let expected_frontier = input
        .expected_frontier
        .as_deref()
        .map(frame::decode_bytes)
        .transpose()
        .map_err(|_| ApiError::BadRequest("expected_frontier is not valid base64".to_string()))?;

    let boot = bootstrap::load(&state.db, document_id).await?;
    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|err| map_collab_error(&err))?;
    for tail in &boot.tail_updates {
        engine
            .import_update(&tail.bytes)
            .map_err(|err| map_collab_error(&err))?;
    }
    let base_frontier = engine.frontier();
    if let Err(err) = apply_content_command(&mut engine, kind, &input.payload) {
        // The concurrent same-key double submit `rest-api-v1.md` registers as a known residual
        // window: this request's `find_idempotent_event` guard (in `execute_command_authorized`)
        // ran *before* the other request committed and so missed, while the `bootstrap::load`
        // above ran *after* it and already sees that request's effect — so `insert_block` fails
        // `DuplicateNode` here, before `update_id` deduplication in `write::accept_update` is ever
        // reached, and the caller is told `invalid_update` for a command that in fact succeeded.
        //
        // Asking the guard's question again, now, closes it: the effect that made the apply fail
        // was read out of committed state, and `write::stage_locked_writes` writes the
        // `flow.content.accepted` event in the very same transaction as the `collab_updates` row
        // that carries it, so if that effect is visible its event under this key is visible too.
        // The recovery is therefore decided by committed rows, not by timing. It only fires when
        // an event already exists under *this* caller's key on *this* document; a `DuplicateNode`
        // caused by anything else (a block some other key created, a malformed payload) finds no
        // such event and still fails, unchanged.
        if let Some(replay) = replay_content_command(
            state,
            workspace_id,
            document_id,
            input.object_id,
            &input.idempotency_key,
        )
        .await?
        {
            return Ok(replay);
        }
        return Err(err);
    }
    let update_bytes = engine
        .export_from(&base_frontier)
        .map_err(|err| map_collab_error(&err))?;

    let collab = runtime::runtime();
    // Never `Uuid::new_v4()`: this function is re-entered from scratch on every REST retry of one
    // logical command, so a freshly minted id here is a *different* id on every attempt, and the
    // whole of `write::accept_update`'s replay dedup keys off it. See
    // `write::replay_stable_update_id`.
    let update_id = write::replay_stable_update_id(document_id, &input.idempotency_key);

    let outcome = write::accept_update(
        &state.db,
        &collab.cache,
        &collab.coordinator,
        &collab.registry,
        &collab.snapshot,
        crate::config::runtime().flow.dispatch_max_attempts,
        None,
        write::UpdateRequest {
            document_id,
            update_id,
            bytes: update_bytes,
            idempotency_key: Some(input.idempotency_key.clone()),
            // Records this command's `flow.content.accepted` event under the caller's key, so the
            // `find_idempotent_event` replay guard in `execute_command_authorized` covers content
            // commands the way it already covers create/feature/lifecycle. Content commands were
            // the one family that wrote `None` here, which made that guard unreachable for all six
            // of them (`write::UpdateRequest::event_idempotency_key`).
            event_idempotency_key: Some(input.idempotency_key.clone()),
            origin_client_id: Some(input.origin_client_id.clone()),
            message: input.message.clone(),
            actor_id: input.actor_id,
            actor_is_bot: input.actor_is_bot(),
            workspace_id,
            checked_epoch,
            expected_frontier,
            // A content command's `flow.content.accepted` **is** this command's primary event —
            // there is no other event for it to be derived from — so the command's own origin
            // goes through unchanged. Before this existed, `write::stage_locked_writes` stamped
            // the literal `"web"` on every caller of `accept_update`, which meant a REST content
            // command was recorded in `business_events.source` and in
            // `collab_updates.origin_surface` as a WebSocket write.
            origin: input.origin.clone(),
        },
    )
    .await?;

    let accepted = match outcome {
        write::AcceptOutcome::Accepted(accepted) => accepted,
        write::AcceptOutcome::Rejected(rejected) => return Err(map_write_rejection(&rejected)),
    };
    if accepted.should_advance_snapshot {
        crate::flow::collab::snapshot::spawn_background(&collab.snapshot, state.db.clone(), document_id);
    }
    // `write::accept_update` already broadcast the `update`+`accepted` frame pair to every
    // WebSocket session with `document_id` open (this REST caller has no session_id of its own to
    // exclude, so `None` above means every open session receives it) -- see that function's doc
    // comment for why this is its one broadcast call site, not a second one here.

    let view = repository::fetch_object_view(&state.db, input.object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let object = super::query::object_view_from_row(view);
    Ok(AcceptedChange {
        accepted_seq: accepted.head_seq,
        head_frontier: object.frontier.clone(),
        projection_seq: accepted.projection_seq,
        semantic_diff: object.semantic_content.clone(),
        affected_object_ids: vec![input.object_id],
        event_id: accepted.event_id,
        command_result: None,
        object,
    })
}

async fn lifecycle_plan<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
) -> Result<LifecyclePlan, ApiError> {
    let depth_max = i64::try_from(collab_limits::TREE_DEPTH_MAX).unwrap_or(i64::MAX);
    let rows = repository::lifecycle_scope(conn, workspace_id, object_id, depth_max).await?;
    LifecyclePlan::derive(object_id, rows)
}

async fn authorize_lifecycle_plan<C: ConnectionTrait>(
    conn: &C,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    plan: &LifecyclePlan,
) -> Result<(), ApiError> {
    let required = plan.required_permission_level(input.object_id)?;
    let principal_kind = if input.principal_kind == "bot" { "bot" } else { "user" };
    for object_id in plan.affected_object_ids() {
        let level = authz::effective_permission(
            conn,
            workspace_id,
            object_id,
            principal_kind,
            input.actor_id,
            &input.role,
        )
        .await?;
        if level < required {
            return Err(ApiError::policy_rejected(
                "insufficient permission for this lifecycle impact set",
            ));
        }
    }
    Ok(())
}

fn lifecycle_replay_affected_ids(metadata: &Value, object_id: Uuid) -> Vec<Uuid> {
    let Some(values) = metadata.get("affected_object_ids").and_then(Value::as_array) else {
        return vec![object_id];
    };
    let mut ids: Vec<Uuid> = values
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|raw| Uuid::parse_str(raw).ok())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() { vec![object_id] } else { ids }
}

/// `archive`/`restore` mutates no document head and the frozen v0.4 request means a reversible,
/// non-cascading transition of the addressed object. The payload remains deliberately loose for
/// wire compatibility, but no unknown payload field can invent the separate cascade operation
/// that the current command registry does not expose. The one-row impact set and its permission
/// tier are prepared outside the transaction, then locked and re-derived under the commit-time
/// epoch fence. Any drift rolls the transaction back.
async fn execute_lifecycle_command(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    kind: LifecycleCommandType,
) -> Result<AcceptedChange, ApiError> {
    if input.expected_frontier.is_some() {
        return Err(ApiError::BadRequest(
            "expected_frontier is not accepted for archive/restore: this command never advances a document head"
                .to_string(),
        ));
    }

    let checked_epoch = authz::read_epoch(&state.db, workspace_id).await?;
    let prepared = lifecycle_plan(&state.db, workspace_id, input.object_id).await?;
    authorize_lifecycle_plan(&state.db, input, workspace_id, &prepared).await?;

    let (event_type, target_status) = match kind {
        LifecycleCommandType::Archive => ("flow.object.archived", "archived"),
        LifecycleCommandType::Restore => ("flow.object.restored", "active"),
    };

    if let Some(existing) = repository::find_idempotent_event(&state.db, workspace_id, &input.idempotency_key).await? {
        if existing.event_type != event_type || existing.aggregate_id != input.object_id.to_string() {
            return Err(ApiError::Conflict(
                "idempotency_key was already used for a different operation".to_string(),
            ));
        }
        let current = repository::fetch_object_view(&state.db, input.object_id)
            .await?
            .ok_or(ApiError::Internal)?;
        let mut change = accepted_change_from_row(current, existing.id);
        change.affected_object_ids = lifecycle_replay_affected_ids(&existing.metadata, input.object_id);
        return Ok(change);
    }

    let tx = state.db.begin().await?;

    // Commit-time fencing barrier (`ADR-0012` §3.1), held to commit -- previously entirely
    // missing for archive/restore (this package's own doc comment above used to justify that gap
    // by "archive/restore never advance a document head, so they take no `authz_epoch` fence", but
    // the fence's job is not document-head consistency, it is authorization freshness: without
    // it, a `full_access` grant revoked by a concurrent authorization change after this
    // function's caller already checked `effective_permission` could still land as a committed
    // `archived`/`restored` transition). Reuses `fence_epoch_for_share` exactly as
    // `write::run_locked_phase` does for content commands: only a genuine epoch mismatch
    // (`ApiError::Conflict`) means the caller's permission is stale; any other error (a
    // `lock_timeout` hit while waiting on the `FOR SHARE`, a dropped connection, ...) says
    // nothing about authorization and must propagate as a real `Err` rather than being folded
    // into a permanent, non-recoverable rejection -- the exact class of bug `12700c4` fixed for
    // the content write path, which this lifecycle path must not reintroduce.
    match authz::fence_epoch_for_share(&tx, workspace_id, checked_epoch).await {
        Ok(()) => {}
        Err(ApiError::Conflict(_)) => {
            let _ = tx.rollback().await;
            return Err(ApiError::policy_rejected(
                "authz_epoch advanced since permission was checked; command rejected",
            ));
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(err);
        }
    }

    let affected_object_ids = prepared.affected_object_ids();
    let locked = repository::lock_objects_for_update(&tx, &affected_object_ids).await?;
    if locked.len() != affected_object_ids.len() {
        let _ = tx.rollback().await;
        return Err(ApiError::Conflict(
            "the lifecycle impact set changed while it was being locked; retry".to_string(),
        ));
    }

    let rechecked = lifecycle_plan(&tx, workspace_id, input.object_id).await?;
    if rechecked != prepared {
        let _ = tx.rollback().await;
        return Err(ApiError::Conflict(
            "the lifecycle impact set changed after authorization; retry".to_string(),
        ));
    }
    if let Err(err) = authorize_lifecycle_plan(&tx, input, workspace_id, &rechecked).await {
        let _ = tx.rollback().await;
        return Err(err);
    }

    let root = rechecked.root(input.object_id)?;

    if kind == LifecycleCommandType::Archive && root.lifecycle_status == "archived" {
        let _ = tx.rollback().await;
        return Err(ApiError::Conflict("object is already archived".to_string()));
    }

    let archived_at = if target_status == "archived" {
        Some(chrono::Utc::now())
    } else {
        None
    };
    let changed = repository::set_objects_lifecycle(
        &tx,
        &affected_object_ids,
        target_status,
        archived_at,
        actor_user_id(input.actor_id, input.actor_is_bot()),
    )
    .await?;
    if changed != u64::try_from(affected_object_ids.len()).unwrap_or(u64::MAX) {
        let _ = tx.rollback().await;
        return Err(ApiError::Conflict(
            "the lifecycle impact set changed while it was being updated; retry".to_string(),
        ));
    }

    let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;
    let event_id = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id,
            project_id: root.project_id,
            event_type: event_type.to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: input.object_id.to_string(),
            actor_id: actor_user_id(input.actor_id, input.actor_is_bot()),
            source: input.origin.source_json(),
            payload: json!({ "object_id": input.object_id, "status": target_status }),
            metadata: json!({
                "message": input.message,
                "cascade": prepared.cascade,
                "affected_object_ids": affected_object_ids,
            }),
            correlation_id: Some(input.origin.correlation_id),
            causation_id: input.origin.causation_id,
            idempotency_key: Some(input.idempotency_key.clone()),
        },
        Some(FlowDispatchSpec {
            max_attempts: dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?
    .event_id;

    tx.commit().await?;

    let view = repository::fetch_object_view(&state.db, input.object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let mut change = accepted_change_from_row(view, event_id);
    change.affected_object_ids = affected_object_ids;
    Ok(change)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod cardinality_gate_tests {
    use super::{ExistingDocumentCardinality, v0_4_command_cardinality_registry, v0_5_command_cardinality_registry};

    /// `command_contended_document_cardinality` (`ADR-0013` §1, v0.4): "v0.4 的竞争文档集合恒
    /// ≤ 1". Every command this package registers — content, lifecycle, and the two
    /// non-`CommandKind` write paths (`create_object`, `set_flow_feature`) — must declare a
    /// cardinality of at most 1; a future command that needs `BoundedMany` must fail this test
    /// until it also ships the `ADR-0013` §2 multi-document lock-order machinery, not slip in
    /// silently.
    #[test]
    fn v0_4_command_set_existing_document_cardinality_is_always_at_most_one() {
        let registry = v0_4_command_cardinality_registry();
        assert!(!registry.is_empty(), "the v0.4 command registry must not be empty");
        for (name, cardinality) in registry {
            assert!(
                cardinality.count() <= 1,
                "command '{name}' declares existing_document_cardinality={cardinality:?} \
                 (count={}), violating ADR-0013's v0.4 bound of <= 1",
                cardinality.count()
            );
        }
    }

    #[test]
    fn v0_4_registry_covers_every_registered_command_name() {
        let registry = v0_4_command_cardinality_registry();
        let names: Vec<&str> = registry.iter().map(|(name, _)| *name).collect();
        for expected in [
            "create_object",
            "set_flow_feature",
            "set_title",
            "insert_block",
            "update_block",
            "delete_block",
            "move_block",
            "semantic_patch",
            "archive",
            "restore",
        ] {
            assert!(
                names.contains(&expected),
                "command '{expected}' is missing from the cardinality registry"
            );
        }
        assert_eq!(names.len(), 10, "registry must not silently gain or lose commands");
    }

    #[test]
    fn v0_5_registry_covers_every_registered_command_name_and_cardinality() {
        let registry = v0_5_command_cardinality_registry();
        assert_eq!(
            registry.len(),
            15,
            "the v0.5 registry must not silently gain or lose commands"
        );
        for (expected_name, expected_cardinality) in [
            ("create_object", ExistingDocumentCardinality::Zero),
            ("set_flow_feature", ExistingDocumentCardinality::Zero),
            ("set_title", ExistingDocumentCardinality::One),
            ("insert_block", ExistingDocumentCardinality::One),
            ("update_block", ExistingDocumentCardinality::One),
            ("delete_block", ExistingDocumentCardinality::One),
            ("move_block", ExistingDocumentCardinality::One),
            ("semantic_patch", ExistingDocumentCardinality::One),
            ("archive", ExistingDocumentCardinality::Zero),
            ("restore", ExistingDocumentCardinality::Zero),
            ("grants_set", ExistingDocumentCardinality::Zero),
            ("inheritance_set", ExistingDocumentCardinality::Zero),
            ("move_object", ExistingDocumentCardinality::BoundedMany(2)),
            ("link", ExistingDocumentCardinality::Zero),
            ("unlink", ExistingDocumentCardinality::Zero),
        ] {
            assert_eq!(
                registry
                    .iter()
                    .find(|(name, _)| *name == expected_name)
                    .map(|(_, cardinality)| *cardinality),
                Some(expected_cardinality),
                "command '{expected_name}' is missing or has the wrong v0.5 cardinality"
            );
        }
    }

    #[test]
    fn cardinality_count_matches_each_variant() {
        assert_eq!(ExistingDocumentCardinality::Zero.count(), 0);
        assert_eq!(ExistingDocumentCardinality::One.count(), 1);
        assert_eq!(ExistingDocumentCardinality::BoundedMany(3).count(), 3);
    }
}

#[cfg(test)]
mod lifecycle_tier_tests {
    use super::{LifecycleDurability, LifecycleTierFacts, authz::PermissionLevel, lifecycle_required_permission_level};
    use uuid::Uuid;

    #[test]
    fn lifecycle_tier_is_classified_from_scope_not_only_object_type() {
        assert_eq!(
            lifecycle_required_permission_level(LifecycleTierFacts {
                object_type: "page",
                parent_id: Some(Uuid::nil()),
                affected_count: 1,
                has_shared_descendants: false,
                durability: LifecycleDurability::Reversible,
            }),
            PermissionLevel::Edit,
            "the v0.4 non-root leaf-page baseline must stay edit"
        );
        for (label, object_type, parent_id, affected_count, shared, durability) in [
            (
                "navigator",
                "navigator",
                None,
                1,
                false,
                LifecycleDurability::Reversible,
            ),
            ("root page", "page", None, 1, false, LifecycleDurability::Reversible),
            (
                "collection",
                "collection",
                Some(Uuid::nil()),
                1,
                false,
                LifecycleDurability::Reversible,
            ),
            (
                "page cascade",
                "page",
                Some(Uuid::nil()),
                2,
                false,
                LifecycleDurability::Reversible,
            ),
            (
                "shared descendants",
                "page",
                Some(Uuid::nil()),
                1,
                true,
                LifecycleDurability::Reversible,
            ),
            (
                "retention/permanent cleanup",
                "page",
                Some(Uuid::nil()),
                1,
                false,
                LifecycleDurability::Irreversible,
            ),
        ] {
            assert_eq!(
                lifecycle_required_permission_level(LifecycleTierFacts {
                    object_type,
                    parent_id,
                    affected_count,
                    has_shared_descendants: shared,
                    durability,
                }),
                PermissionLevel::FullAccess,
                "{label} must never fall through to the ordinary page edit tier"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod typed_error_mapping_tests {
    use collab_core::CollabError;
    use serde_json::{Value, json};

    use super::{check_semantic_patch_json_bytes, map_collab_error, map_write_rejection};
    use crate::error::{ApiError, ApiErrorKind, ServerDrainingReason};
    use crate::flow::collab::frame::{RejectedCode, WriteState};
    use crate::flow::collab::write::Rejected;

    fn rejected(code: RejectedCode, details: Option<Value>) -> Rejected {
        Rejected {
            update_id: None,
            code,
            recoverable: false,
            write_state: WriteState::NotApplied,
            details,
            current_seq: None,
            current_frontier: None,
        }
    }

    fn semantic_patch_payload_with_serialized_bytes(target: u64) -> Value {
        let empty = json!({"operations": [], "padding": ""});
        let base = u64::try_from(serde_json::to_vec(&empty).expect("serializes").len()).expect("fits");
        let padding = usize::try_from(target - base).expect("target fits usize");
        json!({"operations": [], "padding": "x".repeat(padding)})
    }

    #[test]
    fn semantic_patch_bytes_exact_boundary_is_accepted_and_plus_one_is_rejected_before_writes() {
        let limit = crate::flow::collab::limits::SEMANTIC_PATCH_JSON_BYTES_MAX;
        let exact = semantic_patch_payload_with_serialized_bytes(limit);
        assert_eq!(
            u64::try_from(serde_json::to_vec(&exact).expect("serializes").len()).expect("fits"),
            limit
        );
        assert!(check_semantic_patch_json_bytes(&exact).is_ok());

        let plus_one = semantic_patch_payload_with_serialized_bytes(limit + 1);
        let err = check_semantic_patch_json_bytes(&plus_one).expect_err("one byte over must reject");
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = err else {
            panic!("limit rejection must be typed");
        };
        let details = details.expect("limit rejection carries details");
        assert_eq!(details["limit_kind"], "semantic_patch_bytes");
        assert_eq!(details["limit"], limit);
        assert_eq!(details["observed"], limit + 1);
    }

    /// `server_rejected` and `server_draining` are the two codes that both mean "the server, not
    /// you" and differ only in whether a retry can ever work — so the REST mapping getting them
    /// the wrong way round is the failure mode with the highest cost and the lowest visibility.
    #[test]
    fn server_rejected_maps_to_a_permanent_500_and_never_to_a_retryable_draining() {
        let mapped = map_write_rejection(&rejected(
            RejectedCode::ServerRejected,
            Some(json!({ "reason": "deterministic_database_refusal" })),
        ));

        assert_eq!(mapped.kind(), ApiErrorKind::ServerRejected);
        assert_eq!(mapped.kind().stable_code(), "server_rejected");
        assert_eq!(mapped.kind().http_status_code(), 500);
        assert_eq!(mapped.kind().cli_exit_code(), 11);
        assert_ne!(
            mapped.kind().cli_exit_code(),
            ApiErrorKind::ServerDraining(ServerDrainingReason::Contention).cli_exit_code(),
            "exit 9 means temporary; a permanent refusal must not borrow it"
        );
        assert_ne!(
            mapped.kind().cli_exit_code(),
            10,
            "exit 10 is the verify/integrity mismatch; a permanent refusal must not borrow it either"
        );
        assert!(
            !mapped.kind().recoverable(),
            "`server_rejected` must never be advertised as retryable"
        );
        assert_ne!(
            mapped.kind(),
            ApiErrorKind::ServerDraining(ServerDrainingReason::Contention),
            "a permanent refusal must not collapse onto the transient-contention discriminant"
        );

        let ApiError::Typed { details, .. } = mapped else {
            panic!("`server_rejected` must be a typed rejection, not a legacy string one");
        };
        let details = details.expect("`server_rejected` carries its classification");
        assert_eq!(details["reason"], "deterministic_database_refusal");
    }

    /// The fallback nobody looks at until it is wrong.
    ///
    /// A `server_rejected` whose producer supplied no `details` still has to render a `reason`,
    /// and the value chosen there is a wire value like any other: if it ever reads `contention` or
    /// `drain`, the REST envelope carries a permanent refusal that *names itself* as transient,
    /// and a consumer branching on `details.reason` — which is the one field
    /// `error-mapping-v1.md` freezes for this purpose — retries forever. The `error_code` being
    /// correct does not save it, because the reason is the finer-grained thing clients read.
    #[test]
    fn a_server_rejected_without_details_falls_back_to_a_reason_that_is_not_a_retry_hint() {
        let mapped = map_write_rejection(&rejected(RejectedCode::ServerRejected, None));

        assert_eq!(mapped.kind(), ApiErrorKind::ServerRejected);
        let ApiError::Typed { details, .. } = mapped else {
            panic!("`server_rejected` must stay typed even without producer details");
        };
        let details = details.expect("the fallback still carries a reason");
        let reason = details["reason"].as_str().expect("reason is a string");
        assert_eq!(reason, "unclassified");
        for transient in ["contention", "drain"] {
            assert_ne!(
                reason, transient,
                "a permanent refusal must never describe itself with a `server_draining` reason"
            );
        }
    }

    /// And the same rule for the value the write path actually produces: it must not collide with
    /// `server_draining`'s frozen discriminator set either.
    #[test]
    fn the_produced_server_rejected_reason_is_not_a_server_draining_discriminator() {
        let mapped = map_write_rejection(&rejected(
            RejectedCode::ServerRejected,
            Some(json!({ "reason": crate::flow::collab::frame::SERVER_REJECTED_REASON_DATABASE })),
        ));
        let ApiError::Typed { details, .. } = mapped else {
            panic!("must stay typed");
        };
        let details = details.expect("carries a reason");
        let reason = details["reason"].as_str().expect("reason is a string");
        assert_eq!(reason, "deterministic_database_refusal");
        for transient in ["contention", "drain"] {
            assert_ne!(reason, transient);
        }
    }

    /// `error-mapping-v1.md`'s central invariant this package's typed discriminant exists to
    /// enforce: `server_draining`'s two reasons must never collapse into the same `ApiErrorKind`
    /// -- the exact regression `12700c4` fixed once for the epoch-fence path.
    #[test]
    fn server_draining_drain_and_contention_map_to_distinct_kinds() {
        let drain = map_write_rejection(&rejected(
            RejectedCode::ServerDraining,
            Some(json!({ "reason": "drain", "retry_after_ms": 500 })),
        ));
        let contention = map_write_rejection(&rejected(
            RejectedCode::ServerDraining,
            Some(json!({ "reason": "contention", "retry_after_ms": 200 })),
        ));

        assert_eq!(drain.kind(), ApiErrorKind::ServerDraining(ServerDrainingReason::Drain));
        assert_eq!(
            contention.kind(),
            ApiErrorKind::ServerDraining(ServerDrainingReason::Contention)
        );
        assert_ne!(drain.kind(), contention.kind());

        // The two reasons share one CLI exit code (contract: "两种 reason 不拆退出码") but must
        // use different WS close-code behavior: `drain` closes the connection (4410), `contention`
        // never does (a `rejected` control frame keeps the connection open).
        assert_eq!(drain.kind().cli_exit_code(), 9);
        assert_eq!(contention.kind().cli_exit_code(), 9);
        assert_eq!(drain.kind().ws_close_code(), Some(4410));
        assert_eq!(contention.kind().ws_close_code(), None);
        assert_ne!(drain.kind().ui_key(), contention.kind().ui_key());
    }

    /// `error-mapping-v1.md`: "缺失/未知 reason 是 producer contract violation... 不得猜测为维护或
    /// 竞争". A missing/unrecognized `reason` must never be silently coerced into `Drain` or
    /// `Contention` -- it must fall back to a generic, `Unclassified`-kind rejection instead of
    /// fabricating a discriminant the producer never actually sent.
    #[test]
    fn server_draining_with_missing_reason_never_fabricates_a_discriminant() {
        let missing_reason = map_write_rejection(&rejected(RejectedCode::ServerDraining, None));
        let unknown_reason = map_write_rejection(&rejected(
            RejectedCode::ServerDraining,
            Some(json!({ "reason": "maintenance_window" })),
        ));

        assert!(matches!(missing_reason, ApiError::Conflict(_)));
        assert!(matches!(unknown_reason, ApiError::Conflict(_)));
        assert_eq!(missing_reason.kind(), ApiErrorKind::Unclassified);
        assert_eq!(unknown_reason.kind(), ApiErrorKind::Unclassified);
    }

    #[test]
    fn limit_exceeded_rejection_carries_structured_details() {
        let err = map_write_rejection(&rejected(
            RejectedCode::LimitExceeded,
            Some(json!({ "limit_kind": "document_block_count", "limit": 5000, "observed": 5001 })),
        ));
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = err else {
            panic!("expected a Typed limit_exceeded error");
        };
        let details = details.expect("limit_exceeded must carry details");
        assert_eq!(details.get("limit_kind"), Some(&json!("document_block_count")));
        assert_eq!(details.get("limit"), Some(&json!(5000)));
        assert_eq!(details.get("observed"), Some(&json!(5001)));
    }

    #[test]
    fn stale_frontier_rejection_carries_current_seq() {
        let mut with_seq = rejected(RejectedCode::StaleFrontier, None);
        with_seq.current_seq = Some(42);
        let err = map_write_rejection(&with_seq);
        assert_eq!(err.kind(), ApiErrorKind::StaleFrontier);
        let ApiError::Typed { details, .. } = err else {
            panic!("expected a Typed stale_frontier error");
        };
        assert_eq!(details.expect("details").get("current_seq"), Some(&json!(42)));
    }

    #[test]
    fn policy_rejected_from_write_path_is_distinguishable_from_forbidden() {
        let policy = map_write_rejection(&rejected(RejectedCode::PolicyRejected, None));
        let forbidden = map_write_rejection(&rejected(RejectedCode::Forbidden, None));
        assert_eq!(policy.kind(), ApiErrorKind::PolicyRejected);
        assert_eq!(forbidden.kind(), ApiErrorKind::Forbidden);
        assert_ne!(policy.kind(), forbidden.kind());
        // Both share the same HTTP/CLI bucket (403 / exit 4) but are still distinct stable codes.
        assert_eq!(policy.kind().http_status_code(), forbidden.kind().http_status_code());
        assert_ne!(policy.kind().stable_code(), forbidden.kind().stable_code());
    }

    #[test]
    fn collab_limit_exceeded_maps_to_typed_limit_exceeded_with_numeric_details() {
        let err = map_collab_error(&CollabError::LimitExceeded {
            limit_kind: "tree_depth",
            limit: 32,
            observed: 33,
        });
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = err else {
            panic!("expected a Typed limit_exceeded error");
        };
        let details = details.expect("details");
        assert_eq!(details.get("limit_kind"), Some(&json!("tree_depth")));
        assert_eq!(details.get("limit"), Some(&json!(32)));
        assert_eq!(details.get("observed"), Some(&json!(33)));
    }

    #[test]
    fn collab_oversized_update_maps_to_update_bytes_limit_kind() {
        let err = map_collab_error(&CollabError::InputTooLarge {
            input: "update",
            actual_bytes: 999,
            max_bytes: 512,
        });
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = err else {
            panic!("expected a Typed limit_exceeded error");
        };
        assert_eq!(
            details.expect("details").get("limit_kind"),
            Some(&json!("update_bytes"))
        );
    }

    #[test]
    fn collab_unknown_node_maps_to_invalid_update() {
        let err = map_collab_error(&CollabError::UnknownNode { id: "n1".to_string() });
        assert_eq!(err.kind(), ApiErrorKind::InvalidUpdate);
    }
}

// ---- Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`) ----
//
// Same throwaway-scratch-database convention as `super::collab::write`'s and
// `super::collab::authz`'s database tests. These cover two properties that are only observable
// against a real PostgreSQL instance with real concurrent sessions:
//
//  1. `ADR-0012` §3.1's fencing barrier is only sound if `checked_epoch` is taken no *later* than
//     the permission read it fences (`authz_epoch_is_read_before_permission_...`);
//  2. `ADR-0012` §3's `tree_depth_max` and the "继承链成环 fail closed" rule have to be enforced on
//     the *write* path, not only when the chain is later read
//     (`create_object_rejects_...`).
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::items_after_statements,
    clippy::struct_field_names,
    clippy::too_many_lines
)]
mod database_tests {
    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };
    use sea_orm::{
        ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
    };
    use std::time::Duration;
    use uuid::Uuid;

    use super::{
        CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput, create_object, execute_command, set_flow_feature,
    };
    use crate::error::{ApiError, ApiErrorKind};
    use crate::flow::collab::authz;
    use crate::flow::event_origin::{CommandOrigin, EventSource, EventSurface};
    use crate::flow::model::AcceptedChange;
    use crate::flow::repository;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    /// `limits-v1.md`'s `tree_depth_max = 32` with the root at depth 0, written as a literal so
    /// these fixtures pin the *contract* rather than sliding along with any implementation
    /// constant: a legal chain spans depths `0..=32`, i.e. 33 nodes joined by 32 hops.
    const DEEPEST_LEGAL_CHAIN_NODES: usize = 33;

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

        let name = format!("openpr_flow_command_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));

        let (prefix, _) = admin_url.rsplit_once('/')?;
        let url = format!("{prefix}/{name}");
        let db = Database::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));

        migrate(&db).await;

        Some(Scratch { db, name, admin_url })
    }

    async fn migrate(db: &DatabaseConnection) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no migration file was found in {dir}");
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "flow-command-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-command-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }

    async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    struct Fixture {
        workspace_id: Uuid,
        owner_id: Uuid,
        member_id: Uuid,
    }

    async fn seed_workspace(db: &DatabaseConnection) -> Fixture {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        let member_id = Uuid::new_v4();
        for user_id in [owner_id, member_id] {
            exec(
                db,
                "INSERT INTO users (id, email, password_hash, name, role, is_active) \
                 VALUES ($1, $2, '!', 'test', 'user', true)",
                vec![user_id.into(), format!("{user_id}@command.test").into()],
            )
            .await;
        }
        exec(
            db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'command test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        for (user_id, role) in [(owner_id, "owner"), (member_id, "member")] {
            exec(
                db,
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
                vec![workspace_id.into(), user_id.into(), role.into()],
            )
            .await;
        }
        exec(
            db,
            "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level) \
             VALUES ($1, true, 'edit')",
            vec![workspace_id.into()],
        )
        .await;
        Fixture {
            workspace_id,
            owner_id,
            member_id,
        }
    }

    async fn create_page(state: &AppState, fx: &Fixture, parent: Option<Uuid>) -> Result<Uuid, ApiError> {
        create_object(
            state,
            CreateObjectInput {
                origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                    crate::flow::event_origin::EventSurface::Rest,
                ),
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: parent,
                title: "Command Path Test Page".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .map(|accepted| accepted.object.id)
    }

    async fn create_typed_object(state: &AppState, fx: &Fixture, object_type: &str, parent: Option<Uuid>) -> Uuid {
        create_object(
            state,
            CreateObjectInput {
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                actor_is_bot: false,
                object_type: object_type.to_string(),
                project_id: None,
                parent_object_id: parent,
                title: format!("Lifecycle {object_type}"),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .expect("lifecycle fixture object is created")
        .object
        .id
    }

    async fn lifecycle_command(
        state: &AppState,
        _fx: &Fixture,
        object_id: Uuid,
        actor_id: Uuid,
        role: &str,
        command_type: &str,
        legacy_cascade_hint: bool,
        idempotency_key: String,
    ) -> Result<AcceptedChange, ApiError> {
        execute_command(
            state,
            ExecuteCommandInput {
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
                object_id,
                actor_id,
                principal_kind: "user".to_string(),
                role: role.to_string(),
                command_type: command_type.to_string(),
                // Both fields are intentionally outside the lifecycle wire contract. v0.4's
                // loose payload accepted them, and the server must continue to accept but ignore
                // them: neither value is allowed to control the derived impact set.
                payload: serde_json::json!({
                    "cascade": legacy_cascade_hint,
                    "v0_4_extension": "still accepted"
                }),
                expected_frontier: None,
                idempotency_key,
                message: None,
                origin_client_id: format!("lifecycle-test:{actor_id}"),
            },
        )
        .await
    }

    async fn lifecycle_statuses(db: &DatabaseConnection, object_ids: &[Uuid]) -> Vec<(Uuid, String)> {
        #[derive(FromQueryResult)]
        struct Row {
            id: Uuid,
            lifecycle_status: String,
        }
        let ids = object_ids.iter().map(Uuid::to_string).collect::<Vec<_>>().join(",");
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, lifecycle_status FROM flow_objects \
             WHERE id = ANY(string_to_array($1, ',')::uuid[]) ORDER BY id",
            vec![ids.into()],
        ))
        .all(db)
        .await
        .expect("lifecycle statuses query runs")
        .into_iter()
        .map(|row| (row.id, row.lifecycle_status))
        .collect()
    }

    async fn event_metadata(db: &DatabaseConnection, event_id: Uuid) -> serde_json::Value {
        #[derive(FromQueryResult)]
        struct Row {
            metadata: serde_json::Value,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT metadata FROM business_events WHERE id = $1",
            vec![event_id.into()],
        ))
        .one(db)
        .await
        .expect("event metadata query runs")
        .expect("event exists")
        .metadata
    }

    async fn insert_raw_object(db: &DatabaseConnection, workspace_id: Uuid, parent_id: Option<Uuid>) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO flow_objects (id, workspace_id, object_type, parent_id) \
             VALUES ($1, $2, CASE WHEN $3::uuid IS NULL THEN 'navigator' ELSE 'page' END, $3)",
            vec![id.into(), workspace_id.into(), parent_id.into()],
        )
        .await;
        id
    }

    /// Root-to-leaf chain of `nodes` objects written straight to `flow_objects` (its leaf sits at
    /// depth `nodes - 1`), returned root-first.
    async fn build_chain(db: &DatabaseConnection, workspace_id: Uuid, nodes: usize) -> Vec<Uuid> {
        let mut ids = Vec::with_capacity(nodes);
        let mut parent = None;
        for _ in 0..nodes {
            let id = insert_raw_object(db, workspace_id, parent).await;
            ids.push(id);
            parent = Some(id);
        }
        ids
    }

    async fn count_collab_updates(db: &DatabaseConnection, document_id: Uuid) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            n: i64,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1",
            vec![document_id.into()],
        ))
        .one(db)
        .await
        .expect("count query runs")
        .expect("count query returns a row");
        row.n
    }

    async fn document_of(db: &DatabaseConnection, object_id: Uuid) -> Uuid {
        #[derive(FromQueryResult)]
        struct Row {
            id: Uuid,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE object_id = $1",
            vec![object_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("the object has a document")
        .id
    }

    fn assert_limit_exceeded_tree_depth(err: &ApiError, what: &str) {
        assert_eq!(
            err.kind(),
            ApiErrorKind::LimitExceeded,
            "{what}: wrong error kind ({err:?})"
        );
        let ApiError::Typed { details, .. } = err else {
            panic!("{what}: expected a Typed limit_exceeded error, got {err:?}");
        };
        assert_eq!(
            details.as_ref().and_then(|d| d.get("limit_kind")),
            Some(&serde_json::json!("tree_depth")),
            "{what}: limit_kind must be the frozen `tree_depth`"
        );
    }

    /// `archive_tier_by_object_scope` plus the v0.4 baseline fixture named by the gate. The member
    /// has only `default_member_level=edit`; the ordinary non-root Page must succeed through both
    /// transitions even when it has a child, because tree shape does not request a cascade.
    #[tokio::test]
    async fn lifecycle_tier_uses_the_real_impact_set_and_preserves_the_edit_baseline() {
        let scratch = scratch_or_skip!("lifecycle_tiers");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let navigator = create_typed_object(&state, &fx, "navigator", None).await;

        // The non-minimal v0.4 fixture: default-member edit and an ordinary non-root Page that
        // already has a child. Even a loose extension field named `cascade` cannot invent request
        // semantics absent from the frozen command payload.
        let ordinary_page = create_typed_object(&state, &fx, "page", Some(navigator)).await;
        let child = create_typed_object(&state, &fx, "page", Some(ordinary_page)).await;
        let archived = lifecycle_command(
            &state,
            &fx,
            ordinary_page,
            fx.member_id,
            "member",
            "archive",
            true,
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("an edit member keeps the v0.4 non-cascading Page archive capability");
        assert_eq!(archived.affected_object_ids, vec![ordinary_page]);
        assert_eq!(
            event_metadata(&scratch.db, archived.event_id).await["affected_object_ids"],
            serde_json::json!([ordinary_page])
        );
        assert_eq!(event_metadata(&scratch.db, archived.event_id).await["cascade"], false);
        let statuses = lifecycle_statuses(&scratch.db, &[ordinary_page, child]).await;
        assert_eq!(
            statuses
                .iter()
                .find(|(id, _)| *id == ordinary_page)
                .map(|(_, status)| status.as_str()),
            Some("archived")
        );
        assert_eq!(
            statuses
                .iter()
                .find(|(id, _)| *id == child)
                .map(|(_, status)| status.as_str()),
            Some("active"),
            "non-cascading archive must not mutate a child"
        );

        lifecycle_command(
            &state,
            &fx,
            ordinary_page,
            fx.member_id,
            "member",
            "restore",
            false,
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("the same edit member keeps the v0.4 restore capability");
        lifecycle_command(
            &state,
            &fx,
            ordinary_page,
            fx.member_id,
            "member",
            "restore",
            false,
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("restore is idempotent even with a fresh request key");
        assert_eq!(lifecycle_statuses(&scratch.db, &[ordinary_page]).await[0].1, "active");

        // ADR-0018 NR-3 removes the old synthetic-root conflict: omission resolves to the
        // materialized navigator root, so this second top-level Page is non-root and the edit
        // baseline can archive it. The actual navigator root still requires full_access.
        let default_parent_page = create_typed_object(&state, &fx, "page", None).await;
        let parent = repository::fetch_movable_object(&scratch.db, default_parent_page)
            .await
            .expect("default-parent lookup runs")
            .expect("default-parent page exists")
            .parent_id;
        assert_eq!(parent, Some(navigator), "omitted parent must resolve to root_object_id");
        lifecycle_command(
            &state,
            &fx,
            default_parent_page,
            fx.member_id,
            "member",
            "archive",
            false,
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("an edit member can archive a default-parent top-level Page");

        let err = lifecycle_command(
            &state,
            &fx,
            navigator,
            fx.member_id,
            "member",
            "archive",
            false,
            Uuid::new_v4().to_string(),
        )
        .await
        .expect_err("navigator root still requires full_access");
        assert_eq!(err.kind(), ApiErrorKind::PolicyRejected, "navigator: {err:?}");

        scratch.drop_self().await;
    }

    /// Descendant growth cannot silently widen a non-cascading request. A child inserted after the
    /// prepare read remains active while the addressed Page completes its one-row transition.
    #[tokio::test]
    async fn lifecycle_descendant_growth_does_not_turn_a_plain_archive_into_a_cascade() {
        let scratch = scratch_or_skip!("lifecycle_drift");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let navigator = create_typed_object(&state, &fx, "navigator", None).await;
        let root = create_typed_object(&state, &fx, "page", Some(navigator)).await;

        let db_url = scratch
            .admin_url
            .rsplit_once('/')
            .map(|(prefix, _)| format!("{prefix}/{}", scratch.name))
            .expect("database url");
        let db_b = Database::connect(&db_url).await.expect("B connects independently");
        let tx_b = db_b.begin().await.expect("B begins");
        tx_b.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_objects WHERE id = $1 FOR UPDATE",
            vec![root.into()],
        ))
        .await
        .expect("B locks the root");

        let state_a = state_for(scratch.db.clone());
        let actor = fx.owner_id;
        let workspace = fx.workspace_id;
        let a_task = tokio::spawn(async move {
            execute_command(
                &state_a,
                ExecuteCommandInput {
                    origin: CommandOrigin::first_request_from(EventSurface::Rest),
                    object_id: root,
                    actor_id: actor,
                    principal_kind: "user".to_string(),
                    role: "owner".to_string(),
                    command_type: "archive".to_string(),
                    payload: serde_json::json!({ "cascade": true }),
                    expected_frontier: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    origin_client_id: "lifecycle-drift-a".to_string(),
                },
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(!a_task.is_finished(), "A must be waiting for B's root row lock");
        let child = Uuid::new_v4();
        tx_b.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_objects (id, workspace_id, object_type, parent_id) \
                 VALUES ($1, $2, 'page', $3)",
            vec![child.into(), workspace.into(), root.into()],
        ))
        .await
        .expect("B grows the subtree while A holds only its unlocked plan");
        tx_b.commit()
            .await
            .expect("B commits the new child and releases the root");

        let archived = a_task
            .await
            .expect("A joins")
            .expect("descendant growth does not alter the one-object request");
        assert_eq!(archived.affected_object_ids, vec![root]);
        let statuses = lifecycle_statuses(&scratch.db, &[root, child]).await;
        assert_eq!(
            statuses
                .iter()
                .find(|(id, _)| *id == root)
                .map(|(_, status)| status.as_str()),
            Some("archived")
        );
        assert_eq!(
            statuses
                .iter()
                .find(|(id, _)| *id == child)
                .map(|(_, status)| status.as_str()),
            Some("active")
        );
        assert_eq!(
            scalar_i64(
                &scratch.db,
                "SELECT count(*)::bigint AS value FROM business_events \
                 WHERE aggregate_id = $1 AND event_type = 'flow.object.archived'",
                vec![root.to_string().into()],
            )
            .await,
            1,
            "the addressed-object transition emits exactly one lifecycle event"
        );

        scratch.drop_self().await;
    }

    /// The lifecycle transaction's epoch `FOR SHARE` is held through commit. A revocation that
    /// reaches its conflicting epoch update while archive is parked on the object row must wait;
    /// archive linearizes first, then the revocation advances the epoch.
    #[tokio::test]
    async fn lifecycle_holds_the_current_authz_epoch_fence_until_its_commit() {
        let scratch = scratch_or_skip!("lifecycle_epoch_fence");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let navigator = create_typed_object(&state, &fx, "navigator", None).await;
        let object_id = create_typed_object(&state, &fx, "page", Some(navigator)).await;
        exec(
            &scratch.db,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![object_id.into()],
        )
        .await;
        exec(
            &scratch.db,
            "INSERT INTO flow_object_grants \
             (workspace_id, object_id, principal_kind, principal_id, level) \
             VALUES ($1, $2, 'user', $3, 'edit')",
            vec![fx.workspace_id.into(), object_id.into(), fx.member_id.into()],
        )
        .await;
        let original_epoch = authz::read_epoch(&scratch.db, fx.workspace_id)
            .await
            .expect("epoch reads");

        let db_url = scratch
            .admin_url
            .rsplit_once('/')
            .map(|(prefix, _)| format!("{prefix}/{}", scratch.name))
            .expect("database url");
        let blocker_db = Database::connect(&db_url).await.expect("blocker connects");
        let blocker = blocker_db.begin().await.expect("blocker begins");
        blocker
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM flow_objects WHERE id = $1 FOR UPDATE",
                vec![object_id.into()],
            ))
            .await
            .expect("blocker locks the object");

        let state_a = state_for(scratch.db.clone());
        let member_id = fx.member_id;
        let workspace_id = fx.workspace_id;
        let a_task = tokio::spawn(async move {
            execute_command(
                &state_a,
                ExecuteCommandInput {
                    origin: CommandOrigin::first_request_from(EventSurface::Rest),
                    object_id,
                    actor_id: member_id,
                    principal_kind: "user".to_string(),
                    role: "member".to_string(),
                    command_type: "archive".to_string(),
                    payload: serde_json::json!({}),
                    expected_frontier: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    origin_client_id: "lifecycle-epoch-a".to_string(),
                },
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(!a_task.is_finished(), "archive must be parked on the object lock");

        let revoker_db = Database::connect(&db_url).await.expect("revoker connects");
        let (deleted_tx, deleted_rx) = tokio::sync::oneshot::channel();
        let revoker = tokio::spawn(async move {
            let tx = revoker_db.begin().await.expect("revoker begins");
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM flow_object_grants WHERE workspace_id = $1 AND principal_id = $2",
                vec![workspace_id.into(), member_id.into()],
            ))
            .await
            .expect("revoker deletes the grant in its uncommitted transaction");
            deleted_tx.send(()).expect("test still waits for the delete");
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_workspace_settings SET authz_epoch = authz_epoch + 1 WHERE workspace_id = $1",
                vec![workspace_id.into()],
            ))
            .await
            .expect("epoch update resumes after archive commits");
            tx.commit().await.expect("revocation commits");
        });
        deleted_rx.await.expect("revoker reached the epoch update");
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            !revoker.is_finished(),
            "the revocation must be blocked by archive's held epoch FOR SHARE"
        );

        blocker.commit().await.expect("release the object lock");
        a_task
            .await
            .expect("archive task joins")
            .expect("archive commits before the later revocation");
        revoker.await.expect("revoker task joins");
        assert_eq!(lifecycle_statuses(&scratch.db, &[object_id]).await[0].1, "archived");
        assert_eq!(
            authz::read_epoch(&scratch.db, fx.workspace_id)
                .await
                .expect("epoch reads"),
            original_epoch + 1
        );

        scratch.drop_self().await;
    }

    // ---- 1. the `authz_epoch` fence's own TOCTOU: the two reads must not be inverted ----

    /// ★ `ADR-0012` §3.1's commit-time fence compares "the epoch permission was checked against"
    /// with the epoch still in force at commit. That comparison is worthless if `checked_epoch` is
    /// read *after* the permission it is supposed to fence: an authorization change committing in
    /// between is then folded into `checked_epoch` itself, the fence compares the new epoch with
    /// itself, matches, and the already-stale permission lands. `fence_epoch_for_share` never
    /// recomputes permission — it only compares epochs — so the order of these two reads is the
    /// entire barrier.
    ///
    /// The interleaving is made deterministic with a real lock, not a sleep: `B` takes
    /// `LOCK TABLE flow_workspace_settings IN ACCESS EXCLUSIVE MODE`, which blocks even a plain
    /// `SELECT` on that table — so `A` parks precisely on its `authz::read_epoch` round trip and
    /// nowhere else. The fixture object carries its own authorization boundary
    /// (`inherit_from_parent = false`) plus an explicit `full_access` grant, so
    /// `effective_permission` resolves entirely out of `flow_objects` + `flow_object_grants` and
    /// never touches the locked table itself; that is what makes "blocked" mean "blocked at the
    /// epoch read".
    ///
    /// With the reads in the wrong order this test observes the exact defect: `A` computes
    /// `full_access` from the grant, then blocks; `B` deletes the grant and advances the epoch;
    /// `A` reads the *post-revocation* epoch as its `checked_epoch`, the fence matches, and the
    /// update commits after the revocation.
    #[tokio::test]
    async fn authz_epoch_is_read_before_permission_so_a_revocation_between_them_cannot_be_fenced_out() {
        let scratch = scratch_or_skip!("epoch_read_order");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let object_id = create_page(&state, &fx, None).await.expect("page is created");
        let document_id = document_of(&scratch.db, object_id).await;

        // An authorization boundary on the object itself + an explicit grant to the member. Two
        // consequences, both load-bearing: (a) `effective_permission` takes the boundary branch
        // and never reads `flow_workspace_settings`, so the table lock below isolates the epoch
        // read; (b) deleting the grant is a *real* revocation — with the baseline cut off, the
        // member drops straight to `view`.
        exec(
            &scratch.db,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![object_id.into()],
        )
        .await;
        exec(
            &scratch.db,
            "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level) \
             VALUES ($1, $2, 'user', $3, 'full_access')",
            vec![fx.workspace_id.into(), object_id.into(), fx.member_id.into()],
        )
        .await;

        let original_epoch = authz::read_epoch(&scratch.db, fx.workspace_id)
            .await
            .expect("epoch reads");
        let updates_before = count_collab_updates(&scratch.db, document_id).await;

        // `B` on its own connection: a genuinely separate session, so the table lock is
        // cross-transaction rather than a self-block.
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).expect("checked by scratch_or_skip! above");
        let db_url = admin_url
            .rsplit_once('/')
            .map(|(prefix, _)| format!("{prefix}/{}", scratch.name))
            .expect("db url");
        let db_b = Database::connect(&db_url).await.expect("B connects independently");

        let (b_holding_tx, b_holding_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_b_tx, release_b_rx) = tokio::sync::oneshot::channel::<()>();
        let workspace_id = fx.workspace_id;
        let member_id = fx.member_id;

        let b_task = tokio::spawn(async move {
            use sea_orm::TransactionTrait;
            let tx = db_b.begin().await.expect("B begins");
            tx.execute_unprepared("LOCK TABLE flow_workspace_settings IN ACCESS EXCLUSIVE MODE")
                .await
                .expect("B locks the epoch table");
            b_holding_tx.send(()).expect("A is still waiting to receive this");

            release_b_rx.await.expect("A releases B");
            // The authorization change itself: revoke the grant and advance the epoch in one
            // transaction, exactly as `ADR-0012` §3.1 point 1 requires.
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM flow_object_grants WHERE workspace_id = $1 AND principal_id = $2",
                vec![workspace_id.into(), member_id.into()],
            ))
            .await
            .expect("B revokes the grant");
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_workspace_settings SET authz_epoch = authz_epoch + 1 WHERE workspace_id = $1",
                vec![workspace_id.into()],
            ))
            .await
            .expect("B advances the epoch");
            tx.commit().await.expect("B commits, releasing the table lock");
        });

        b_holding_rx.await.expect("B signals it holds the lock");

        let state_for_a = state_for(scratch.db.clone());
        let a_task = tokio::spawn(async move {
            execute_command(
                &state_for_a,
                ExecuteCommandInput {
                    origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                        crate::flow::event_origin::EventSurface::Rest,
                    ),
                    object_id,
                    actor_id: member_id,
                    principal_kind: "user".to_string(),
                    role: "member".to_string(),
                    command_type: "set_title".to_string(),
                    payload: serde_json::json!({ "title": "written after the revocation" }),
                    expected_frontier: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    origin_client_id: "test-client-a".to_string(),
                },
            )
            .await
        });

        // `A` must genuinely be parked on the locked table, not merely slow: nothing else in this
        // command path reads `flow_workspace_settings`, so still-unfinished here means "waiting on
        // the epoch read". No `lock_timeout` is in force on that read (`run_locked_phase`'s is a
        // `SET LOCAL` inside its own, later transaction), so the wait is unbounded and this is a
        // barrier rather than a race with a timer.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            !a_task.is_finished(),
            "A must still be blocked on B's table lock at this point -- the interleaving this test \
             depends on did not happen"
        );

        release_b_tx.send(()).expect("B is still waiting to receive this");
        b_task.await.expect("B task joins");

        let a_result = a_task.await.expect("A task joins");
        let Err(err) = a_result else {
            panic!(
                "a command whose permission was revoked while it sat between its own authorization \
                 reads was accepted -- `checked_epoch` was taken after the permission read, so the \
                 fence compared the post-revocation epoch with itself"
            );
        };
        assert_eq!(
            err.kind(),
            ApiErrorKind::PolicyRejected,
            "the revoked command must be rejected as policy_rejected, got {err:?}"
        );

        // The decisive assertion, independent of what the in-memory result claimed: nothing was
        // persisted for a caller whose permission had already been taken away.
        assert_eq!(
            count_collab_updates(&scratch.db, document_id).await,
            updates_before,
            "a content write landed after the revocation committed"
        );
        assert_eq!(
            authz::read_epoch(&scratch.db, fx.workspace_id)
                .await
                .expect("epoch reads"),
            original_epoch + 1,
            "B's revocation must still have taken effect"
        );

        scratch.drop_self().await;
    }

    // ---- 3. the write path must enforce `tree_depth_max` and refuse a corrupted chain ----

    /// A parent already sitting at `tree_depth_max` (depth 32) cannot adopt a child: the child
    /// would sit at depth 33, which `authz::fetch_chain` refuses to evaluate for anyone but a
    /// workspace admin — so accepting the row would mint an object that is permanently
    /// unauthorizable, for it and for everything ever created beneath it, with no v0.4 command
    /// able to re-parent it back.
    ///
    /// Nailed from both sides: depth 32 is legal and must still be creatable, and the object
    /// created there must still resolve on the read path.
    #[tokio::test]
    async fn create_object_rejects_a_parent_that_is_already_at_the_frozen_tree_depth_limit() {
        let scratch = scratch_or_skip!("create_depth");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;

        // Root-first, so `chain[i]` sits at depth `i`.
        let chain = build_chain(&scratch.db, fx.workspace_id, DEEPEST_LEGAL_CHAIN_NODES).await;
        let at_limit = chain[DEEPEST_LEGAL_CHAIN_NODES - 1]; // depth 32
        let one_below_limit = chain[DEEPEST_LEGAL_CHAIN_NODES - 2]; // depth 31

        // A child of the depth-31 node lands at depth 32 -- the deepest legal position -- and must
        // be accepted *and* still resolve on the read path.
        let legal_child = create_page(&state, &fx, Some(one_below_limit))
            .await
            .expect("a child at exactly tree_depth_max must be creatable");
        let level = authz::effective_permission(
            &scratch.db,
            fx.workspace_id,
            legal_child,
            "user",
            fx.member_id,
            "member",
        )
        .await
        .expect("an object at exactly tree_depth_max must still be authorizable");
        assert_eq!(
            level,
            authz::PermissionLevel::Edit,
            "no boundary ⇒ the edit baseline applies"
        );

        // One deeper is not creatable at all.
        let err = create_page(&state, &fx, Some(at_limit))
            .await
            .expect_err("a child at depth 33 must be rejected by the write path");
        assert_limit_exceeded_tree_depth(&err, "a child one past tree_depth_max");

        // ...and nothing was written: no `flow_objects` row past the limit.
        #[derive(FromQueryResult)]
        struct OrphanCount {
            n: i64,
        }
        let orphans = OrphanCount::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM flow_objects WHERE parent_id = $1",
            vec![at_limit.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("count runs")
        .expect("count row")
        .n;
        assert_eq!(orphans, 0, "the rejected create must not have left a row behind");

        scratch.drop_self().await;
    }

    /// `flow_objects_parent_not_self_check` (migration `0054`) only forbids `parent_id = id`, so
    /// `A -> B -> A` is a legal row pair as far as `PostgreSQL` is concerned. The read path fails
    /// closed on such a chain; the write path must refuse to attach anything new underneath it
    /// rather than mint another permanently unauthorizable row.
    #[tokio::test]
    async fn create_object_rejects_a_parent_whose_chain_is_cyclic() {
        let scratch = scratch_or_skip!("create_cycle");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;

        let a = insert_raw_object(&scratch.db, fx.workspace_id, None).await;
        let b = insert_raw_object(&scratch.db, fx.workspace_id, Some(a)).await;
        exec(
            &scratch.db,
            "UPDATE flow_objects SET parent_id = $1 WHERE id = $2",
            vec![b.into(), a.into()],
        )
        .await;

        let err = create_page(&state, &fx, Some(b))
            .await
            .expect_err("a parent inside a parent_id cycle must be rejected");
        assert_eq!(
            err.kind(),
            ApiErrorKind::InvalidUpdate,
            "a cyclic parent chain must fail closed as invalid_update, got {err:?}"
        );

        scratch.drop_self().await;
    }

    // ---- 5. `project_id` inheritance from `parent_object_id` (`rest-api-v1.md` v0.5 create) ----

    async fn seed_project(db: &DatabaseConnection, workspace_id: Uuid, key: &str) -> Uuid {
        let project_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) \
             VALUES ($1, $2, $3, $3, (SELECT created_by FROM workspaces WHERE id = $2))",
            vec![project_id.into(), workspace_id.into(), key.into()],
        )
        .await;
        project_id
    }

    async fn try_create(
        state: &AppState,
        fx: &Fixture,
        object_type: &str,
        project_id: Option<Uuid>,
        parent: Option<Uuid>,
    ) -> Result<AcceptedChange, ApiError> {
        create_object(
            state,
            CreateObjectInput {
                origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                    crate::flow::event_origin::EventSurface::Rest,
                ),
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                actor_is_bot: false,
                object_type: object_type.to_string(),
                project_id,
                parent_object_id: parent,
                title: format!("Scope Test {object_type}"),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
    }

    /// The committed `flow_objects.project_id`, read back as its own nullable column so a `NULL`
    /// scope stays distinguishable from any UUID (including the nil UUID that `0056`'s generated
    /// `project_scope_id` normalises `NULL` onto — that normalisation belongs to the constraint
    /// key, never to the stored `project_id`).
    async fn stored_project_id(db: &DatabaseConnection, object_id: Uuid) -> Option<Uuid> {
        #[derive(FromQueryResult)]
        struct Row {
            project_id: Option<Uuid>,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT project_id FROM flow_objects WHERE id = $1",
            vec![object_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("the object row is committed and readable")
        .project_id
    }

    async fn created_event_project_id(db: &DatabaseConnection, object_id: Uuid) -> Option<Uuid> {
        #[derive(FromQueryResult)]
        struct Row {
            project_id: Option<Uuid>,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT project_id FROM business_events \
             WHERE event_type = 'flow.object.created' AND aggregate_id = $1",
            vec![object_id.to_string().into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("the creation event is committed and readable")
        .project_id
    }

    async fn scalar_i64(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            value: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .one(db)
            .await
            .expect("count query runs")
            .expect("count query returns a row")
            .value
    }

    fn reason_of(err: &ApiError) -> Option<String> {
        let ApiError::Typed { details, .. } = err else {
            return None;
        };
        details
            .as_ref()
            .and_then(|d| d.get("reason"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }

    /// ★ `rest-api-v1.md` ("v0.5 起 ... 请求**省略** `project_id` 时由服务端继承父级的值") and
    /// `ADR-0013` §2.2 R17 ("省略时继承父级 scope"): omitting `project_id` under a projected parent
    /// is the most ordinary create there is — "a new child page under this page" — and the server
    /// answers it by writing the *parent's* scope. Reading omission as a declaration of the
    /// unprojected scope and rejecting it is the defect this pins: it turns a legal request into
    /// `child_project_must_match_parent` and leaves callers no way to spell "same scope as my
    /// parent" except by re-deriving it client-side.
    ///
    /// The assertion is deliberately on the **committed row**, not on the response body: a
    /// response echoing the request field would look right while the row went in unprojected.
    /// The event's scope is checked for the same reason.
    #[tokio::test]
    async fn create_object_inherits_a_projected_parents_scope_when_project_id_is_omitted() {
        let scratch = scratch_or_skip!("scope_inherit_projected");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let project = seed_project(&scratch.db, fx.workspace_id, "SCOPEA").await;

        let parent = try_create(&state, &fx, "navigator", Some(project), None)
            .await
            .expect("a projected root is created")
            .object;
        assert_eq!(
            stored_project_id(&scratch.db, parent.id).await,
            Some(project),
            "fixture precondition: the parent really is in the project scope"
        );

        let accepted = try_create(&state, &fx, "page", None, Some(parent.id))
            .await
            .expect("omitting project_id under a projected parent is a legal request");

        assert_eq!(
            stored_project_id(&scratch.db, accepted.object.id).await,
            Some(project),
            "the committed row must carry the inherited scope, not the omitted request field"
        );
        assert_eq!(
            created_event_project_id(&scratch.db, accepted.object.id).await,
            Some(project),
            "flow.object.created must be filed under the same scope the row was written with"
        );
        assert_eq!(
            accepted.object.project_id,
            Some(project),
            "the response must report the scope the object actually has"
        );

        scratch.drop_self().await;
    }

    /// ★ The `NULL` half of the same rule: `rest-api-v1.md` says the inherited value includes
    /// "父级为 `NULL` 的未投影 scope". Written so it can fail: the parent is created unprojected and
    /// *asserted* to be unprojected first, and the child's stored scope is read back as a nullable
    /// column, so an implementation that inherited through `0056`'s nil-UUID scope normalisation
    /// (`project_scope_id`) — or that supplied any other placeholder — fails here even though the
    /// projected-parent test above would still pass.
    #[tokio::test]
    async fn create_object_inherits_an_unprojected_parents_null_scope_when_project_id_is_omitted() {
        let scratch = scratch_or_skip!("scope_inherit_null");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        // A project exists but is not the parent's: nothing may drift into it by accident.
        let unrelated = seed_project(&scratch.db, fx.workspace_id, "SCOPEU").await;

        let parent = try_create(&state, &fx, "navigator", None, None)
            .await
            .expect("an unprojected root is created")
            .object;
        assert_eq!(
            stored_project_id(&scratch.db, parent.id).await,
            None,
            "fixture precondition: the parent really is unprojected"
        );

        let accepted = try_create(&state, &fx, "page", None, Some(parent.id))
            .await
            .expect("omitting project_id under an unprojected parent is a legal request");

        let stored = stored_project_id(&scratch.db, accepted.object.id).await;
        assert_eq!(
            stored, None,
            "an unprojected parent's scope is NULL and must be inherited as NULL, not as the nil \
             UUID and not as any project (got {stored:?}, unrelated project is {unrelated})"
        );
        assert_eq!(
            scalar_i64(
                &scratch.db,
                "SELECT count(*)::bigint AS value FROM flow_objects WHERE id = $1 AND project_id IS NULL",
                vec![accepted.object.id.into()],
            )
            .await,
            1,
            "the column itself must be SQL NULL"
        );
        assert_eq!(
            created_event_project_id(&scratch.db, accepted.object.id).await,
            None,
            "flow.object.created must be filed unprojected too"
        );
        assert_eq!(accepted.object.project_id, None, "the response must report NULL");

        scratch.drop_self().await;
    }

    /// The committed-row count of objects that violate "a non-root object sits in its parent's
    /// project scope". Backed by `flow_object_project_scope_violations` (migration `0056`), the
    /// same monitor view `move_object`'s own suite asserts against, so both write paths are held
    /// to one definition of the invariant instead of two hand-rolled queries.
    async fn scope_violation_count(db: &DatabaseConnection) -> i64 {
        crate::flow::repository::project_scope_violation_count(db)
            .await
            .expect("the invariant monitor view is queryable")
    }

    /// ★ The write path answers `ADR-0013` §2.2 R17's invariant with a decidable error rather than
    /// letting `flow_objects_parent_project_fk` surface as a 500, and writes nothing when it
    /// refuses.
    ///
    /// This test used to live in `move_object.rs`'s suite, next to the migration-`0056` constraint
    /// tests it shares an invariant with. It is about `create_object`, so it belongs here.
    ///
    /// It covers the *declared* half only. `rest-api-v1.md` ("v0.5 起 ... 请求**显式携带**
    /// `project_id` 且与父级不一致 ... 请求**省略** `project_id` 时由服务端继承父级的值") and
    /// `ADR-0013` §2.2 R17 give omission the opposite answer, and an earlier revision of this test
    /// asserted the rejection for omission too — it was wrong, and the inheritance assertions
    /// below (plus the two dedicated tests above) are what replaced it. The rejections that remain
    /// are the ones the contract actually freezes a reason code for: a `project_id` the caller
    /// spelled out that is not the parent's, in both directions.
    #[tokio::test]
    async fn create_object_refuses_a_parent_in_a_different_project_scope() {
        let scratch = scratch_or_skip!("scope_create");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let project_a = seed_project(&scratch.db, fx.workspace_id, "SCOPEDA").await;
        let project_b = seed_project(&scratch.db, fx.workspace_id, "SCOPEDB").await;

        let root_a = try_create(&state, &fx, "navigator", Some(project_a), None)
            .await
            .expect("a projected root is created")
            .object
            .id;
        let root_unprojected = try_create(&state, &fx, "navigator", None, None)
            .await
            .expect("an unprojected root is created")
            .object
            .id;
        let before_objects = scalar_i64(
            &scratch.db,
            "SELECT count(*)::bigint AS value FROM flow_objects WHERE workspace_id = $1",
            vec![fx.workspace_id.into()],
        )
        .await;
        let before_events = scalar_i64(
            &scratch.db,
            "SELECT count(*)::bigint AS value FROM business_events \
             WHERE workspace_id = $1 AND event_type = 'flow.object.created'",
            vec![fx.workspace_id.into()],
        )
        .await;

        for (declared, parent, label) in [
            (Some(project_b), root_a, "a declared project that is not the parent's"),
            (
                Some(project_a),
                root_unprojected,
                "a declared project under an unprojected parent",
            ),
        ] {
            let err = match try_create(&state, &fx, "page", declared, Some(parent)).await {
                Ok(accepted) => panic!("{label} must be refused, but object {} was created", accepted.object.id),
                Err(err) => err,
            };
            assert_eq!(
                err.kind(),
                ApiErrorKind::InvalidUpdate,
                "{label} must be a decidable invalid_update, not a database 500: {err:?}"
            );
            assert_eq!(
                reason_of(&err).as_deref(),
                Some(super::CHILD_PROJECT_MUST_MATCH_PARENT),
                "{label}: {err:?}"
            );
        }

        // The refusals wrote nothing: no object row, no document, no projection, no event.
        assert_eq!(
            scalar_i64(
                &scratch.db,
                "SELECT count(*)::bigint AS value FROM flow_objects WHERE workspace_id = $1",
                vec![fx.workspace_id.into()],
            )
            .await,
            before_objects,
            "a refused create must not leave a flow_objects row behind"
        );
        assert_eq!(
            scalar_i64(
                &scratch.db,
                "SELECT count(*)::bigint AS value FROM business_events \
                 WHERE workspace_id = $1 AND event_type = 'flow.object.created'",
                vec![fx.workspace_id.into()],
            )
            .await,
            before_events,
            "a refused create must not emit flow.object.created"
        );
        assert_eq!(scope_violation_count(&scratch.db).await, 0);

        // The legal shapes still work, so the check is a rule and not a blanket refusal.
        let same = try_create(&state, &fx, "page", Some(project_a), Some(root_a))
            .await
            .expect("a child in its parent's project is legal")
            .object
            .id;
        assert_eq!(stored_project_id(&scratch.db, same).await, Some(project_a));
        let root_b = try_create(&state, &fx, "navigator", Some(project_b), None)
            .await
            .expect("a project-scoped navigator root is legal")
            .object
            .id;
        let root = try_create(&state, &fx, "page", Some(project_b), Some(root_b))
            .await
            .expect("a projected Page starts below its compatible navigator")
            .object
            .id;
        assert_eq!(stored_project_id(&scratch.db, root).await, Some(project_b));

        // Omission is inheritance, not a third scope. The assertion is on the **committed row**,
        // because that is the value `flow_objects_parent_project_fk` and every project-scoped read
        // go on to use; a response body echoing the request field would look identical here and be
        // wrong. Reverting `create_object` to write `input.project_id` makes this line fail.
        let inherited = try_create(&state, &fx, "page", None, Some(root_a))
            .await
            .expect("omitting project_id under a projected parent inherits, it is not a rejection")
            .object
            .id;
        assert_eq!(
            stored_project_id(&scratch.db, inherited).await,
            Some(project_a),
            "an omitted project_id must be filled in from the parent, not stored as NULL"
        );

        // The `NULL` half of the same sentence ("包括父级为 `NULL` 的未投影 scope"). Note what this
        // one can and cannot catch: inheriting `NULL` from an unprojected parent and never
        // inheriting at all are indistinguishable *here*, so this is a regression guard rather
        // than the case that would have caught the original defect. It is not vacuous, though —
        // it fails if the inherited value is materialised as anything other than SQL `NULL` (for
        // instance as `0056`'s nil-UUID `project_scope_id` sentinel), and it fails if the omitted
        // branch is ever made to fail closed on an unprojected parent.
        let unprojected = try_create(&state, &fx, "page", None, Some(root_unprojected))
            .await
            .expect("an unprojected child of an unprojected parent is legal")
            .object
            .id;
        assert_eq!(stored_project_id(&scratch.db, unprojected).await, None);
        assert_eq!(
            scalar_i64(
                &scratch.db,
                "SELECT count(*)::bigint AS value FROM flow_objects WHERE id = $1 AND project_id IS NULL",
                vec![unprojected.into()],
            )
            .await,
            1,
            "the inherited NULL must be SQL NULL, not a sentinel UUID"
        );

        // Every shape created above still satisfies the invariant the constraint enforces.
        assert_eq!(scope_violation_count(&scratch.db).await, 0);

        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // `events-v1.md` origin: every producer in this module reads its surface from the caller
    // -----------------------------------------------------------------------------------------

    /// `events-v1.md`: "`source` 由服务端按 Web/REST/MCP/CLI/worker 覆盖".
    ///
    /// This module has five producers — `create_object`, the shared content write,
    /// `execute_lifecycle_command`, `set_flow_feature`, and the audit-only
    /// `record_command_rejected` — and every one of them used to write the literal
    /// `{"surface":"rest"}`. Run here from `surface=cli`, which REST can never produce, so any
    /// producer that still decides its own surface fails on its own row rather than hiding behind
    /// the four that were fixed.
    #[tokio::test]
    async fn every_producer_in_this_module_stamps_the_callers_surface_not_a_literal() {
        #[derive(Debug, FromQueryResult)]
        struct EventRow {
            event_type: String,
            source: serde_json::Value,
            correlation_id: Option<Uuid>,
            /// Read, not merely selected: each of these producers writes exactly one event per
            /// command, so every row here is a first user request's primary event and its
            /// causation must be `NULL`. `flow.command.rejected` in particular once filled this
            /// with a fresh `Uuid::new_v4()` — a parent id naming an event that never existed —
            /// and no test noticed, because no row type read the column.
            causation_id: Option<Uuid>,
        }

        let scratch = scratch_or_skip!("origin_surface_per_producer");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;

        let cli_origin =
            || CommandOrigin::first_request(EventSource::new(EventSurface::Cli).with_tool("sylvode objects create"));

        // (1) create_object
        let created = create_object(
            &state,
            CreateObjectInput {
                origin: cli_origin(),
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: None,
                title: "CLI Origin Page".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .expect("create succeeds");
        let object_id = created.object.id;

        let command_input = |command_type: &str, payload: serde_json::Value| ExecuteCommandInput {
            origin: cli_origin(),
            object_id,
            actor_id: fx.owner_id,
            principal_kind: "user".to_string(),
            role: "owner".to_string(),
            command_type: command_type.to_string(),
            payload,
            expected_frontier: None,
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
            origin_client_id: "cli-origin-test".to_string(),
        };

        // (2) the shared content write (`write::stage_one_document`)
        execute_command(
            &state,
            command_input("set_title", serde_json::json!({ "title": "Renamed by CLI" })),
        )
        .await
        .expect("set_title succeeds");

        // (3) the lifecycle producer
        execute_command(&state, command_input("archive", serde_json::json!({})))
            .await
            .expect("archive succeeds");

        // (4) the audit-only rejection producer: a second archive is a real `Conflict`, and every
        // rejection reached after the object resolves records `flow.command.rejected`.
        let rejected = execute_command(&state, command_input("archive", serde_json::json!({})))
            .await
            .expect_err("archiving an already-archived object is a conflict");
        assert!(matches!(rejected, ApiError::Conflict(_)), "{rejected:?}");

        // (5) the workspace feature flag producer
        set_flow_feature(
            &state,
            SetFlowFeatureInput {
                origin: cli_origin(),
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                actor_is_bot: false,
                enabled: Some(false),
                default_member_level: None,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("set_flow_feature succeeds");

        let rows = EventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT event_type, source, correlation_id, causation_id FROM business_events \
             WHERE workspace_id = $1 ORDER BY created_at, id",
            vec![fx.workspace_id.into()],
        ))
        .all(&scratch.db)
        .await
        .expect("business_events query runs");

        let types: Vec<&str> = rows.iter().map(|row| row.event_type.as_str()).collect();
        for expected in [
            "flow.object.created",
            "flow.content.accepted",
            "flow.object.archived",
            "flow.command.rejected",
            "flow.feature.disabled",
        ] {
            assert!(types.contains(&expected), "'{expected}' was not written; got {types:?}");
        }

        let expected_source = serde_json::json!({ "surface": "cli", "tool": "sylvode objects create" });
        for row in &rows {
            assert_eq!(
                row.source, expected_source,
                "'{}' was written with source {:?}, but every caller here declared {:?}",
                row.event_type, row.source, expected_source
            );
            assert_eq!(
                row.causation_id, None,
                "'{}' is the only event of a first user request, so it roots the chain; a minted \
                 causation here is an edge pointing at nothing",
                row.event_type
            );
            assert!(
                row.correlation_id.is_some(),
                "'{}' must carry the correlation its request generated, not NULL",
                row.event_type
            );
        }

        scratch.drop_self().await;
    }
}

// ---- Real-database idempotency-race tests (opt-in via `OPENPR_TEST_DATABASE_URL`) ----
//
// Same scratch-database convention as `flow::collab::write::database_tests`: own throwaway
// database per run, migrated from `migrations/*.sql` on disk, dropped on the way out.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::items_after_statements,
    clippy::too_many_lines
)]
mod idempotency_race_database_tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };
    use sea_orm::{
        ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement,
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        ContentCommandType, CreateObjectInput, ExecuteCommandInput, create_object, execute_command,
        execute_content_command,
    };
    use crate::flow::collab::authz;
    use crate::flow::repository;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

        let name = format!("openpr_flow_idem_race_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));

        let (prefix, _) = admin_url.rsplit_once('/')?;
        let url = format!("{prefix}/{name}");
        // Every racing request below holds one connection for the whole of its transaction --
        // including the wait `INSERT ... ON CONFLICT DO NOTHING` performs on the winner's
        // uncommitted speculative insertion -- so the pool has to be wider than the race.
        let mut opts = ConnectOptions::new(url);
        opts.max_connections(24).connect_timeout(Duration::from_secs(30));
        let db = Database::connect(opts)
            .await
            .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));

        migrate(&db).await;

        Some(Scratch { db, name, admin_url })
    }

    async fn migrate(db: &DatabaseConnection) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no migration file was found in {dir}");
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "flow-idem-race-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-idem-race-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }

    async fn exec(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) {
        state
            .db
            .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    async fn seed_workspace(state: &AppState) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            state,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@flow-idem.test").into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'flow idem race test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), owner_id.into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
            vec![workspace_id.into()],
        )
        .await;
        (workspace_id, owner_id)
    }

    async fn count(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            n: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .one(&state.db)
            .await
            .expect("count query runs")
            .expect("count query returns a row")
            .n
    }

    /// Eight concurrent creations under one `idempotency_key`. The database layer was already
    /// clean before the fix -- exactly one `business_events` row, one aggregate reachable *through*
    /// that row -- and the defect lived entirely in the answers: the losers of the unique-index
    /// race each committed and returned their own attempt-local `object_id`, so one logical
    /// creation reported several different objects, only one of which the key will ever replay to.
    ///
    /// The assertions therefore check the *responses* against the canonical row, and then check
    /// that the workspace holds exactly one object at all -- the second half is what makes a
    /// returned id "real" rather than merely equal to its siblings.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_creates_under_one_idempotency_key_all_answer_with_the_one_canonical_object() {
        let scratch = scratch_or_skip!("create_phantom_id");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;

        const RACERS: usize = 8;
        let key = format!("phantom-race-{}", Uuid::new_v4());
        let title = "one canonical object".to_string();

        let mut handles = Vec::with_capacity(RACERS);
        for _ in 0..RACERS {
            let state = state.clone();
            let key = key.clone();
            let title = title.clone();
            handles.push(tokio::spawn(async move {
                create_object(
                    &state,
                    CreateObjectInput {
                        origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                            crate::flow::event_origin::EventSurface::Rest,
                        ),
                        workspace_id,
                        actor_id: owner_id,
                        actor_is_bot: false,
                        object_type: "page".to_string(),
                        project_id: None,
                        parent_object_id: None,
                        title,
                        idempotency_key: key,
                        message: None,
                    },
                )
                .await
            }));
        }

        let mut object_ids = BTreeSet::new();
        let mut event_ids = BTreeSet::new();
        for handle in handles {
            let accepted = handle
                .await
                .expect("racing create task does not panic")
                .expect("every racer under one idempotency key must succeed");
            object_ids.insert(accepted.object.id);
            event_ids.insert(accepted.event_id);
            assert_eq!(accepted.affected_object_ids, vec![accepted.object.id]);
        }

        #[derive(FromQueryResult)]
        struct EventRow {
            id: Uuid,
            aggregate_id: String,
        }
        let canonical = EventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, aggregate_id FROM business_events \
             WHERE workspace_id = $1 AND idempotency_key = $2",
            vec![workspace_id.into(), key.clone().into()],
        ))
        .all(&state.db)
        .await
        .expect("canonical event query runs");
        assert_eq!(canonical.len(), 1, "one key must record exactly one creation event");
        let canonical_object_id =
            Uuid::parse_str(&canonical[0].aggregate_id).expect("the event names a UUID aggregate");

        assert_eq!(
            object_ids.iter().copied().collect::<Vec<_>>(),
            vec![canonical_object_id],
            "every successful response must identify the one canonical object; \
             a response carrying any other id is a phantom success"
        );
        assert_eq!(
            event_ids.iter().copied().collect::<Vec<_>>(),
            vec![canonical[0].id],
            "every successful response must report the one committed event"
        );

        // The returned id is not merely equal across responses -- it resolves to a committed,
        // readable object with the requested title.
        let view = repository::fetch_object_view(&state.db, canonical_object_id)
            .await
            .expect("object view query runs")
            .expect("the id every response returned must resolve to a committed object");
        assert_eq!(view.id, canonical_object_id);
        assert_eq!(view.projection_title, title);

        // And no loser left a second, unreferenced aggregate behind: the counts here are scoped to
        // the workspace, not joined through `business_events`, so an orphaned object *is* visible.
        assert_eq!(
            count(
                &state,
                "SELECT count(*) AS n FROM flow_objects WHERE workspace_id = $1 AND object_type = 'page'",
                vec![workspace_id.into()],
            )
            .await,
            1,
            "the losing transactions must leave no orphaned flow_objects row behind"
        );
        assert_eq!(
            count(
                &state,
                "SELECT count(*) AS n FROM collab_documents cd \
                 JOIN flow_objects fo ON fo.id = cd.object_id \
                 WHERE fo.workspace_id = $1 AND fo.object_type = 'page'",
                vec![workspace_id.into()],
            )
            .await,
            1,
        );
        assert_eq!(
            count(
                &state,
                "SELECT count(*) AS n FROM flow_object_projections p \
                 JOIN flow_objects fo ON fo.id = p.object_id \
                 WHERE fo.workspace_id = $1 AND fo.object_type = 'page'",
                vec![workspace_id.into()],
            )
            .await,
            1,
        );
        assert_eq!(
            count(
                &state,
                "SELECT count(*) AS n FROM event_dispatch WHERE event_id = $1",
                vec![canonical[0].id.into()],
            )
            .await,
            1,
            "exactly one dispatch row for the one committed creation event"
        );

        scratch.drop_self().await;
    }

    /// `rest-api-v1.md`'s registered residual window, reproduced deterministically: a content
    /// command whose replay guard missed (here: bypassed, which is exactly what a guard read
    /// before the winner's COMMIT observes) but whose `bootstrap::load` already sees the winner's
    /// committed block. `apply_content_command` then fails `DuplicateNode` before `update_id`
    /// deduplication is ever reached.
    ///
    /// The window's defining state -- winner committed, this request's guard blind to it -- is
    /// fully determined by committed rows, so driving `execute_content_command` directly
    /// reproduces it without depending on an interleaving that cannot be scheduled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_content_command_racing_its_own_committed_same_key_write_replays_it() {
        let scratch = scratch_or_skip!("content_residual_window");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;

        let created = create_object(
            &state,
            CreateObjectInput {
                origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                    crate::flow::event_origin::EventSurface::Rest,
                ),
                workspace_id,
                actor_id: owner_id,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: None,
                title: "residual window page".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .expect("object creation succeeds");
        let object_id = created.object.id;
        let document_id = created.object.document_id;

        let key = format!("residual-{}", Uuid::new_v4());
        let payload = json!({ "block_id": "block-under-race", "text": "hello" });
        let input = |key: &str| ExecuteCommandInput {
            origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                crate::flow::event_origin::EventSurface::Rest,
            ),
            object_id,
            actor_id: owner_id,
            principal_kind: "user".to_string(),
            role: "owner".to_string(),
            command_type: "insert_block".to_string(),
            payload: payload.clone(),
            expected_frontier: None,
            idempotency_key: key.to_string(),
            message: None,
            origin_client_id: "residual-window-test".to_string(),
        };

        // The winner: a complete, committed `insert_block` under `key`.
        let winner = execute_command(&state, input(&key))
            .await
            .expect("the winning insert_block commits");
        assert_eq!(winner.accepted_seq, 1);

        let checked_epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");

        // The loser, entering the write path with a guard read that predates that commit.
        let loser = execute_content_command(
            &state,
            &input(&key),
            workspace_id,
            document_id,
            checked_epoch,
            ContentCommandType::InsertBlock,
        )
        .await
        .expect(
            "a same-key content command whose apply collides with its own already-committed \
             effect must replay that effect, not fail invalid_update",
        );

        assert_eq!(
            loser.event_id, winner.event_id,
            "the replay must report the committed event, not a new one"
        );
        assert_eq!(loser.object.id, object_id);
        assert_eq!(
            loser.accepted_seq, winner.accepted_seq,
            "the replay must report the committed head, not advance it"
        );
        assert_eq!(
            count(
                &state,
                "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1",
                vec![document_id.into()],
            )
            .await,
            1,
            "the replay must not have written a second update"
        );
        assert_eq!(
            count(
                &state,
                "SELECT count(*) AS n FROM business_events \
                 WHERE workspace_id = $1 AND idempotency_key = $2",
                vec![workspace_id.into(), key.clone().into()],
            )
            .await,
            1,
        );

        // A `DuplicateNode` that is *not* this caller's own committed write still fails: same
        // block id, a key that never wrote anything.
        let unrelated = execute_content_command(
            &state,
            &input("residual-unused-key"),
            workspace_id,
            document_id,
            checked_epoch,
            ContentCommandType::InsertBlock,
        )
        .await;
        let err = unrelated.expect_err("a duplicate block under an unused key is still invalid_update");
        assert!(
            format!("{err:?}").contains("duplicate node"),
            "expected the original duplicate-node rejection, got {err:?}"
        );

        scratch.drop_self().await;
    }
}

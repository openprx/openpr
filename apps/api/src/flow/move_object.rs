//! `move_object` — the cross-parent (and cross-project) governance command, and the **first and
//! only** command in the frozen v0.5 set whose *contended existing document set* can exceed one
//! (`ADR-0013` §1's third table row). Everything in `ADR-0013` §2 that v0.4 wrote down but never
//! executed runs here for the first time.
//!
//! What it is, in one paragraph: `parent_id` is `PostgreSQL`'s (`ADR-0012` §1), so re-parenting is
//! a governance row write, not a CRDT edit — but the navigator documents that hold *ordering*
//! (`ADR-0012` §1: "navigator CRDT document 此后只持有排序（position key）与显示元数据") must be
//! rewritten on both sides of the move, and those are canonical CRDT documents with heads. So one
//! transaction advances up to two existing document heads, rewrites two `flow_objects` governance
//! columns, writes the events, and bumps `authz_epoch` — atomically, with no compensation
//! (`ADR-0013` §3: single `PostgreSQL` transaction, explicitly no saga/2PC).
//!
//! ## The two lock layers (`ADR-0013` §2.1, as corrected by R16)
//!
//! ```text
//! layer 0 (before the transaction, in-process): the document coordinator, ascending document_id
//! layer 1 (in transaction): flow_workspace_settings.authz_epoch          FOR UPDATE
//! layer 2 (in transaction): flow_objects (moved object + target chain)   FOR UPDATE, ascending id
//! layer 3 (in transaction): collab_documents (the contended set)         FOR UPDATE, ascending id
//! layer 4 (in transaction): business_events / event_dispatch
//! ```
//!
//! Both ordered layers derive their order from the *same* function,
//! [`super::collab::coordinator::ascending_document_lock_order`] — R16's correction is that
//! ordering only the database rows is not enough, because the instance-local coordinator is taken
//! first and a `(A,B)` / `(B,A)` pair there deadlocks in **tokio**, where `PostgreSQL`'s deadlock
//! detector cannot see it. Sharing one ordering function is what makes "coordinator order ==
//! database order" a property of the code rather than a rule to remember.
//!
//! ## Why layer 1 is `FOR UPDATE`, and why the barrier is re-verification rather than a fence
//!
//! A content write fences with `FOR SHARE` (`authz::fence_epoch_for_share`) so writes do not
//! serialize against each other. `move_object` cannot use that lock: it changes `parent_id`, which
//! `ADR-0012` §3.1 point 1 lists as an authorization change, so it must also *advance* the epoch
//! before committing — and `FOR SHARE` followed by an `UPDATE` of the same row is a lock upgrade
//! that two such transactions deadlock on. It takes `authz::lock_epoch_for_update` instead, as the
//! first statement of the transaction, exactly like `flow::grants`.
//!
//! Holding the conflicting lock makes an epoch *comparison* the wrong barrier as well as an
//! unnecessary one. `rest-api-v1.md` spells the alternative out — "再取文档行锁并**在锁内重验有效
//! 权限**" — and that is what happens here: both sides of `ADR-0012` §4's double-sided rule are
//! re-evaluated on this transaction's own snapshot, after the epoch row is held. A comparison
//! would reject whenever *anything* in the workspace changed the epoch first (including an
//! unrelated move); re-verification decides on the truth instead, which is strictly stronger and
//! never manufactures a false `policy_rejected`. Both the epoch this command checked permission
//! against outside the transaction and the epoch it committed are still reported in
//! `command_result`, which is what the `authz_linearization_no_escalation` gate artifact asks for.
//!
//! **A consequence worth stating out loud, because it changes what the lock-order gate can
//! observe:** since every `move_object` in a workspace takes that one row exclusively as its first
//! locked action, two concurrent moves in the same workspace are already fully serialized *before*
//! either reaches layer 3. The reversed-order document deadlock the ADR warns about is therefore
//! unreachable between two moves at the database layer — the reachable one is the layer-0
//! coordinator deadlock, which happens before any transaction exists. That is exactly the case
//! R16 added, and it is the one this package's tests construct deterministically.

#![allow(clippy::too_long_first_doc_paragraph)]

use std::sync::Arc;

use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};
use platform::app::AppState;
use sea_orm::{DatabaseTransaction, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};

use super::collab::coordinator::ascending_document_lock_order;
use super::collab::limits::MAX_REBASE_ATTEMPTS;
use super::collab::runtime::CollabRuntime;
use super::collab::{authz, bootstrap, frame, limits as collab_limits, runtime, snapshot, write};
use super::command::{
    ExecuteCommandInput, ExistingDocumentCardinality, accepted_change_from_row, map_collab_error, map_write_rejection,
};
use super::model::AcceptedChange;
use super::repository::{self, MovableObjectRow};

/// `ADR-0013` §1's "`bounded_many` 一律走第 2 节的排序锁路径并**设文档数上限**".
///
/// Two: the navigator of the scope the object leaves, and the navigator of the scope it joins.
/// There is no third — an object belongs to exactly one scope before the move and exactly one
/// after, and this command never touches a descendant's scope.
///
/// **Why a descendant's scope is not touched**: a move rewrites the moved object's own
/// `project_id` to its new parent's and deliberately does **not** cascade that to its descendants.
/// `ADR-0013` §1 requires a `bounded_many` command to carry a document-count ceiling, and a
/// cascade would make the contended set proportional to subtree size — unbounded, and therefore
/// outside what this command's own contract can promise. The visible consequence is that a moved
/// subtree's descendants keep their old `project_id` while their `parent_id` chain now leads into
/// another project; authorization is unaffected (it follows `parent_id`, `ADR-0012` §1), navigator
/// scoping is. That needs a contract ruling and is recorded as an open item in this package's
/// delivery report rather than hidden behind an implementation detail.
///
/// The locked phase asserts the derived set never exceeds this ceiling, and refuses rather than
/// silently locking more.
pub const MOVE_OBJECT_CONTENDED_DOCUMENT_MAX: u8 = 2;

/// `ADR-0012` §3's inheritance-chain depth ceiling (`limits-v1.md`'s frozen `tree_depth_max`),
/// counted in `parent_id` hops with a root at depth 0 — the same counting
/// `authz::ensure_parent_can_adopt_child` uses.
const TREE_DEPTH_MAX: usize = 32;

/// The v0.5 governance command family: commands that change `flow_objects` governance columns and
/// may advance document heads while doing so.
///
/// Deliberately declared here rather than in `flow::command`: the wire name lives with the
/// implementation, and `flow::command` stays the v0.4-frozen content/lifecycle registry it already
/// is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernanceCommandType {
    MoveObject,
}

impl GovernanceCommandType {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "move_object" => Some(Self::MoveObject),
            _ => None,
        }
    }

    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::MoveObject => "move_object",
        }
    }

    /// `events-v1.md`'s registry row for this command.
    #[must_use]
    pub const fn event_type(self) -> &'static str {
        match self {
            Self::MoveObject => "flow.object.moved",
        }
    }

    /// `ADR-0013` §1's machine-checkable declaration. `BoundedMany`, and the *only* one in the
    /// frozen v0.5 set — `command_contended_document_cardinality` scans every command variant and
    /// fails closed on anything undeclared.
    #[must_use]
    pub const fn existing_document_cardinality(self) -> ExistingDocumentCardinality {
        match self {
            Self::MoveObject => ExistingDocumentCardinality::BoundedMany(MOVE_OBJECT_CONTENDED_DOCUMENT_MAX),
        }
    }
}

/// `rest-api-v1.md` "v0.5 Collaboration": "跨 object command payload 必须含 `target_object_id`".
#[derive(Debug, Deserialize)]
struct MoveObjectPayload {
    /// The new parent. Required — `mcp-surface-v1.md` marks `objects.move`'s `new_parent_id` as
    /// mandatory, and `ADR-0012` §4's double-sided rule ("目标父级需 `edit`") has no defined
    /// answer when there is no target object to evaluate, so "move to the workspace root" is not
    /// a v0.5 operation rather than an operation with an invented permission rule.
    target_object_id: Uuid,
    /// Place the moved entry immediately after this sibling entry in the target navigator's
    /// ordering (`cli-surface-v1.md`'s `--after ID`). Absent: append at the end.
    #[serde(default)]
    after_id: Option<Uuid>,
    /// Optimistic-concurrency guard for the **target navigator** document
    /// (`rest-api-v1.md`: "只有真正推进 target 文档 canonical head 的命令才携带
    /// `expected_target_frontier?`"). Base64, same encoding as `expected_frontier`.
    #[serde(default)]
    expected_target_frontier: Option<String>,
    /// `ADR-0012` §4.1 point 1, applied to the one other way a caller can lock themselves out:
    /// moving an object under an authorization boundary they hold nothing beneath.
    #[serde(default)]
    confirm_self_lockout: bool,
}

// ---------------------------------------------------------------------------------------------
// Prepare phase (no lock held, no transaction open)
// ---------------------------------------------------------------------------------------------

/// One navigator document's participation in this move.
///
/// Both variants are *locked* in layer 3; only [`Self::Advance`] stages a head advance. A document
/// that has no ordering entry to remove still belongs to the contended set for locking purposes:
/// the set's membership is derived structurally from `project_id` (`ADR-0013` §2.2), not from
/// whether an entry happens to exist right now, and a concurrent writer could add one between the
/// unlocked prepare and the commit. Its `observed_head_seq` is re-verified under the lock exactly
/// like an advancing document's, so that race turns into a rebase rather than a lost update.
enum DocumentPlan {
    Locked {
        document_id: Uuid,
        observed_head_seq: i64,
    },
    Advance {
        document_id: Uuid,
        request: Box<write::UpdateRequest>,
        prepared: Box<write::Prepared>,
    },
}

impl DocumentPlan {
    const fn document_id(&self) -> Uuid {
        match self {
            Self::Locked { document_id, .. } | Self::Advance { document_id, .. } => *document_id,
        }
    }

    const fn observed_head_seq(&self) -> i64 {
        match self {
            Self::Locked { observed_head_seq, .. } => *observed_head_seq,
            Self::Advance { prepared, .. } => prepared.observed.head_seq,
        }
    }
}

/// The invocation-wide handles and caller identity every phase of one move needs, bundled so the
/// phase functions stay readable instead of threading six unrelated parameters each.
struct MoveContext<'a> {
    state: &'a AppState,
    collab: &'a CollabRuntime,
    input: &'a ExecuteCommandInput,
    workspace_id: Uuid,
    /// The `authz_epoch` the caller's permission was first checked against, outside any
    /// transaction. Reported as the gate artifact's `checked_epoch`; the *decision* is re-made
    /// inside the transaction, so this value is evidence, not a barrier.
    checked_epoch: i64,
}

/// Everything the locked phase must re-verify, captured from the unlocked prepare phase.
struct MovePlan {
    workspace_id: Uuid,
    object_id: Uuid,
    target_object_id: Uuid,
    /// The moved object's `parent_id`/`project_id` as read outside the lock.
    source_parent_id: Option<Uuid>,
    source_project_id: Option<Uuid>,
    /// The target's `project_id` as read outside the lock — the object's `project_id` after the
    /// move.
    target_project_id: Option<Uuid>,
    /// The target's ancestor chain, leaf-first, as read outside the lock. Both the cycle rule and
    /// the layer-2 lock set come from it, and the locked phase re-derives it to detect drift.
    target_chain: Vec<Uuid>,
    /// Ascending `document_id` — the contended existing document set's lock order, layer 0 and
    /// layer 3 alike.
    document_lock_order: Vec<Uuid>,
    /// The navigator *objects* (not documents) touched, for `affected_object_ids`.
    navigator_object_ids: Vec<Uuid>,
}

/// The ordering entries an object may occupy inside a navigator document all start with the
/// object's UUID: `"<uuid>"` for its first placement there, `"<uuid>#1"`, `"<uuid>#2"`, ... for
/// each later one.
///
/// **Why generations exist at all.** A CRDT delete is a tombstone: the engine keeps the id in its
/// `id_to_tree` map forever, so re-creating the same node id fails `DuplicateNode`, and the loro
/// adapter refuses to `mov_to` a deleted node (`"TreeID … is deleted or does not exist"`) — so a
/// tombstoned entry can be neither re-created nor resurrected. Without generations, moving an
/// object out of a scope and later back into it works exactly once and then fails permanently.
/// That is not hypothetical: it is the bug this package's atomicity test caught on its third
/// move.
///
/// The consequence, recorded honestly: a navigator document accumulates one tombstone per removed
/// entry. `collab_core::limits`' `container_count_max` counts *live* nodes, so tombstones do not
/// consume that ceiling, but they do grow the document. `limits-v1.md` has no number for it and
/// this package does not invent one; the delivery report carries it as an open contract item.
fn entry_prefix(object_id: Uuid) -> String {
    object_id.to_string()
}

/// Whether `id` is one of `object_id`'s ordering entries (any generation).
fn is_entry_of(id: &str, prefix: &str) -> bool {
    id == prefix
        || id
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('#'))
            .is_some_and(|generation| !generation.is_empty() && generation.bytes().all(|b| b.is_ascii_digit()))
}

/// The object's currently *live* ordering entry in this navigator, if it has one.
fn live_entry_of(snapshot: &collab_core::SemanticSnapshot, object_id: Uuid) -> Option<NodeId> {
    let prefix = entry_prefix(object_id);
    snapshot
        .nodes
        .iter()
        .find(|(id, node)| !node.deleted && is_entry_of(id, &prefix))
        .map(|(id, _)| id.clone())
}

/// A never-before-used entry id for a *new* placement: the bare UUID if this navigator has never
/// held one, else the first free `#n` generation.
///
/// The search is bounded by the snapshot's own node count: with `n` nodes present, at most `n + 1`
/// candidate ids can be taken, so the first free one is always found inside that many probes.
/// There is no arbitrary constant here and therefore no invented limit.
fn fresh_entry_id(snapshot: &collab_core::SemanticSnapshot, object_id: Uuid) -> Result<NodeId, ApiError> {
    let prefix = entry_prefix(object_id);
    let bare: NodeId = Arc::from(prefix.as_str());
    if !snapshot.nodes.contains_key(&bare) {
        return Ok(bare);
    }
    for generation in 1..=snapshot.nodes.len() {
        let candidate: NodeId = Arc::from(format!("{prefix}#{generation}").as_str());
        if !snapshot.nodes.contains_key(&candidate) {
            return Ok(candidate);
        }
    }
    // Unreachable: `nodes.len() + 1` candidates were probed against `nodes.len()` entries.
    tracing::error!(%object_id, nodes = snapshot.nodes.len(), "move_object: no free navigator entry id");
    Err(ApiError::Internal)
}

/// Builds the CRDT update bytes that put `object_id`'s ordering entry where this move says it
/// belongs, or `None` when this document needs no change at all.
///
/// `remove` is the source-navigator case (drop the entry); otherwise the entry is repositioned if
/// it is already live here, or created under a fresh generation if it is not. "No change at all"
/// is decided by the *frontier*, not by guessing which operations are no-ops: a `MoveNode` that
/// lands an entry exactly where it already sat may produce no operation in the engine, and
/// shipping an empty update would advance a head with nothing in it.
fn build_navigator_update(
    engine: &mut LoroCollabEngine,
    object_id: Uuid,
    remove: bool,
    after_id: Option<Uuid>,
) -> Result<Option<Vec<u8>>, ApiError> {
    let base_frontier = engine.frontier();
    let snapshot = engine.semantic_snapshot().map_err(|err| map_collab_error(&err))?;
    let live = live_entry_of(&snapshot, object_id);

    let operation = if remove {
        let Some(entry) = live else { return Ok(None) };
        Operation::DeleteNode { id: entry }
    } else {
        // Root-level position: one past the requested predecessor, or the end of the list. The
        // engine clamps an out-of-range index itself (`LoroCollabEngine::clamp_index`), so a
        // stale `after_id` degrades to "append" rather than failing the command.
        match live {
            Some(entry) => Operation::MoveNode {
                // Computed over the siblings *excluding this entry*, which is what makes a
                // same-parent reposition legal: loro's `mov_to` takes the node out of the list
                // before re-inserting it, so the valid range is `0..=len-1`, while
                // `LoroCollabEngine::clamp_index` only clamps to `len` — passing `len` fails with
                // "The index(n) should be <= the length of children (n-1)". Excluding the entry
                // makes both the create and the move case use one formula.
                index: position_after(&snapshot, after_id, Some(&entry)),
                id: entry,
                new_parent: None,
            },
            None => Operation::CreateNode {
                id: fresh_entry_id(&snapshot, object_id)?,
                parent: None,
                index: position_after(&snapshot, after_id, None),
                kind: NodeKind::NavigatorNode,
            },
        }
    };

    let limits = collab_limits::document_limits();
    collab_core::limits::check_operation(&snapshot, &operation, &limits)
        .map_err(|violation| map_collab_error(&collab_core::CollabError::from(violation)))?;
    engine
        .apply_operation(&operation)
        .map_err(|err| map_collab_error(&err))?;

    if engine.frontier().as_bytes() == base_frontier.as_bytes() {
        // The engine merged the operation into nothing — the entry was already exactly here.
        return Ok(None);
    }
    let bytes = engine
        .export_from(&base_frontier)
        .map_err(|err| map_collab_error(&err))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(bytes))
}

/// Index for a new/moved root-level navigator entry: immediately after `after_id`'s entry, or at
/// the end of the root list.
///
/// `exclude` is the entry being repositioned, if it is already in this list — see
/// [`build_navigator_update`] for why leaving it in produces an out-of-range index for a
/// same-parent move.
fn position_after(snapshot: &collab_core::SemanticSnapshot, after_id: Option<Uuid>, exclude: Option<&NodeId>) -> u32 {
    let mut roots: Vec<(&str, &NodeId)> = snapshot
        .nodes
        .iter()
        .filter(|(id, node)| node.parent.is_none() && !node.deleted && Some(*id) != exclude)
        .map(|(id, node)| (node.order_key.as_str(), id))
        .collect();
    roots.sort_unstable();
    let end = u32::try_from(roots.len()).unwrap_or(u32::MAX);
    let Some(after_id) = after_id else { return end };
    let Some(wanted) = live_entry_of(snapshot, after_id) else {
        return end;
    };
    roots
        .iter()
        .position(|(_, id)| **id == wanted)
        .and_then(|index| u32::try_from(index.saturating_add(1)).ok())
        .unwrap_or(end)
}

/// Hydrates one navigator document outside any lock, computes its ordering change, and prepares
/// the write through the *same* [`write::hydrate_and_apply`] every content write uses — isolated
/// apply, resource ceilings, projection prepare and all.
async fn plan_document(
    ctx: &MoveContext<'_>,
    document_id: Uuid,
    object_id: Uuid,
    remove: bool,
    after_id: Option<Uuid>,
    expected_frontier: Option<&[u8]>,
) -> Result<DocumentPlan, ApiError> {
    let input = ctx.input;
    let boot = bootstrap::load(&ctx.state.db, document_id).await?;
    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|err| map_collab_error(&err))?;
    for tail in &boot.tail_updates {
        engine
            .import_update(&tail.bytes)
            .map_err(|err| map_collab_error(&err))?;
    }

    let Some(bytes) = build_navigator_update(&mut engine, object_id, remove, after_id)? else {
        // Locked but not advanced. `boot.head_seq` is the head the "no change" conclusion was
        // drawn against, so the locked phase can still detect a concurrent writer.
        return Ok(DocumentPlan::Locked {
            document_id,
            observed_head_seq: boot.head_seq,
        });
    };

    // Never `Uuid::new_v4()`: this whole function is re-entered on every REST retry and on every
    // bounded rebase, and `write`'s replay dedup keys off `update_id`
    // (`write::replay_stable_update_id`).
    let update_id = write::replay_stable_update_id(document_id, &input.idempotency_key);
    let request = write::UpdateRequest {
        document_id,
        update_id,
        bytes,
        idempotency_key: Some(input.idempotency_key.clone()),
        // The move's own `flow.object.moved` event carries the caller's key; `business_events`'
        // idempotency index is `(workspace_id, idempotency_key)` and admits exactly one row per
        // key, so the per-document `flow.content.accepted` events must not also claim it.
        event_idempotency_key: None,
        origin_client_id: Some(input.origin_client_id.clone()),
        message: input.message.clone(),
        actor_id: input.actor_id,
        workspace_id: ctx.workspace_id,
        checked_epoch: ctx.checked_epoch,
        expected_frontier: expected_frontier.map(<[u8]>::to_vec),
    };

    match write::hydrate_and_apply(
        &ctx.state.db,
        &ctx.collab.cache,
        document_id,
        update_id,
        &request.bytes,
        request.expected_frontier.as_deref(),
    )
    .await?
    {
        write::HydrateOutcome::Prepared(prepared) => Ok(DocumentPlan::Advance {
            document_id,
            request: Box::new(request),
            prepared,
        }),
        write::HydrateOutcome::Rejected(write::AcceptOutcome::Rejected(rejected)) => {
            Err(map_write_rejection(&rejected))
        }
        write::HydrateOutcome::Rejected(write::AcceptOutcome::Accepted(_)) => {
            // `hydrate_and_apply` never returns `Rejected(Accepted(..))`; treating it as an
            // internal error is the fail-closed reading of an impossible state.
            tracing::error!(%document_id, "move_object: hydrate_and_apply returned an accepted outcome in a rejection");
            Err(ApiError::Internal)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Locked phase
// ---------------------------------------------------------------------------------------------

/// What one attempt at the locked phase concluded.
enum LockedOutcome {
    Committed {
        event_id: Uuid,
        advanced: Vec<AdvancedDocument>,
        /// `authz_epoch` after this transaction's own bump — the gate artifact's
        /// `committed_epoch`.
        committed_epoch: i64,
    },
    /// The in-transaction re-verification of `ADR-0012` §4's double-sided rule failed: whatever
    /// the caller held when `execute_command` checked, they do not hold it now. Rolled back,
    /// nothing written — the "撤权后零 accepted write" half of
    /// `authz_linearization_no_escalation`.
    PermissionRevoked(&'static str),
    /// The contended set, the target's ancestor chain, or a document head moved between prepare
    /// and lock (`ADR-0013` §2.2: "集合漂移即 rollback 重来，不得沿用旧集合提交"). Rolled back;
    /// the caller re-prepares from scratch.
    Drift(&'static str),
    /// A concurrent request already committed this exact `idempotency_key`.
    AlreadyCommitted,
}

struct AdvancedDocument {
    document_id: Uuid,
    accepted: write::Accepted,
}

/// The whole transaction. Nothing here opens an engine, touches the warm cache, or awaits
/// anything but the database — the same discipline `write::stage_locked_writes` holds itself to.
#[allow(clippy::too_many_lines)]
async fn run_locked_phase(
    tx: &DatabaseTransaction,
    ctx: &MoveContext<'_>,
    plan: &MovePlan,
    payload: &MoveObjectPayload,
    plans: &[DocumentPlan],
    observed_lock_order: &mut Vec<Uuid>,
) -> Result<LockedOutcome, ApiError> {
    let input = ctx.input;
    write::set_locked_phase_statement_budgets(tx).await?;

    // [layer 1] the conflicting epoch lock, held to commit. Taken first, so any in-flight content
    // write holding `FOR SHARE` on this row has either committed or is blocked before this
    // transaction reads a single permission, and every row locked below is acquired after it.
    authz::lock_epoch_for_update(tx, plan.workspace_id).await?;

    // [layer 2] the moved object and the target's whole ancestor chain, ascending `id`. The chain
    // is included because the cycle and depth rules are statements about it, and an unlocked
    // ancestor could be re-parented by a concurrent move between this check and the commit
    // (`ADR-0012` §3.1 point 3: "move 锁住被移动对象、目标对象及必要祖先/授权边界行").
    let mut object_lock_order: Vec<Uuid> = plan.target_chain.clone();
    object_lock_order.push(plan.object_id);
    object_lock_order.sort_unstable();
    object_lock_order.dedup();
    let mut locked_rows: Vec<(Uuid, MovableObjectRow)> = Vec::with_capacity(object_lock_order.len());
    for object_id in &object_lock_order {
        let Some(row) = repository::lock_movable_object(tx, *object_id).await? else {
            return Ok(LockedOutcome::Drift("an object in the move's lock set disappeared"));
        };
        locked_rows.push((*object_id, row));
    }
    let row_for = |id: Uuid| locked_rows.iter().find(|(row_id, _)| *row_id == id).map(|(_, row)| row);

    let (Some(source), Some(target)) = (row_for(plan.object_id), row_for(plan.target_object_id)) else {
        return Ok(LockedOutcome::Drift("the moved object or its target left the lock set"));
    };
    if source.lifecycle_status != "active" || target.lifecycle_status != "active" {
        return Ok(LockedOutcome::Drift(
            "the moved object or its target was archived concurrently",
        ));
    }
    if source.parent_id != plan.source_parent_id || source.project_id != plan.source_project_id {
        return Ok(LockedOutcome::Drift("the moved object was re-parented concurrently"));
    }
    if target.project_id != plan.target_project_id {
        return Ok(LockedOutcome::Drift("the target's project changed concurrently"));
    }

    // The cycle/depth rules, re-derived on the locked rows.
    let locked_chain = authz::inheritance_chain(tx, plan.workspace_id, plan.target_object_id).await?;
    if locked_chain.ids != plan.target_chain {
        return Ok(LockedOutcome::Drift("the target's ancestor chain changed concurrently"));
    }
    // A concurrent move that made this one illegal is a real rejection, not a retry. Read on
    // `tx`, not on a second connection: it must see the same snapshot the locked rows above came
    // from, or the rule is being checked against a state this transaction is not committing.
    check_cycle_and_depth(tx, plan, &locked_chain.ids).await?;
    // Re-decided under the locks, for the same reason the cycle/depth rules are: the unlocked
    // pre-check ran before this transaction existed. `lock_movable_object`'s `FOR UPDATE` on the
    // moved object conflicts with the `FOR KEY SHARE` a concurrent child insert takes on its
    // parent, so from here the subtree cannot gain a member behind this command's back.
    ensure_move_keeps_the_subtree_in_one_project(tx, plan.workspace_id, plan.object_id, target.project_id).await?;

    // `ADR-0012` §4's double-sided rule, re-decided on this transaction's own snapshot rather
    // than trusted from the unlocked pre-check. `caller_before` is therefore also the level the
    // §4.1 self-lockout summary compares against, which is what makes that summary describe this
    // commit rather than an earlier reading of the world.
    let principal_kind = principal_kind_of(input);
    let caller_before = authz::effective_permission(
        tx,
        plan.workspace_id,
        plan.object_id,
        principal_kind,
        input.actor_id,
        &input.role,
    )
    .await?;
    if caller_before < authz::PermissionLevel::FullAccess {
        return Ok(LockedOutcome::PermissionRevoked(
            "full_access on the moved object is required to move it",
        ));
    }
    let target_level = authz::effective_permission(
        tx,
        plan.workspace_id,
        plan.target_object_id,
        principal_kind,
        input.actor_id,
        &input.role,
    )
    .await?;
    if target_level < authz::PermissionLevel::Edit {
        return Ok(LockedOutcome::PermissionRevoked(
            "edit on the target parent is required to move an object under it",
        ));
    }

    // The contended set's *membership* re-verified from the locked rows (`ADR-0013` §2.2).
    let relocked = derive_contended_set(tx, plan.workspace_id, source.project_id, target.project_id).await?;
    if relocked.lock_order != plan.document_lock_order {
        return Ok(LockedOutcome::Drift("the contended document set changed concurrently"));
    }

    // [layer 3] the contended documents, ascending `document_id`, one explicit `FOR UPDATE` per
    // id. A single `WHERE id = ANY(..) ORDER BY id FOR UPDATE` would also lock in sorted order,
    // but only because `PostgreSQL` places `LockRows` above `Sort`; doing it one row at a time
    // makes the order a property of this loop instead of a property of a query plan, and lets a
    // test record the order that was actually taken.
    for document_id in &plan.document_lock_order {
        observed_lock_order.push(*document_id);
        let Some(head_seq) = lock_document_head(tx, *document_id).await? else {
            return Ok(LockedOutcome::Drift("a contended navigator document disappeared"));
        };
        let Some(entry) = plans.iter().find(|entry| entry.document_id() == *document_id) else {
            return Ok(LockedOutcome::Drift(
                "the contended set and the prepared plans disagree",
            ));
        };
        if head_seq != entry.observed_head_seq() {
            return Ok(LockedOutcome::Drift("a contended navigator document head moved"));
        }
    }

    // [layer 3, writes] each advancing document, still ascending, through the identical statements
    // a single-document content write uses.
    let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;
    let mut advanced = Vec::with_capacity(plans.len());
    for document_id in &plan.document_lock_order {
        let Some(DocumentPlan::Advance { request, prepared, .. }) =
            plans.iter().find(|entry| entry.document_id() == *document_id)
        else {
            continue;
        };
        match write::stage_one_document(tx, request, prepared, dispatch_max_attempts, "rest").await? {
            write::StagedOutcome::Ready(staged) => advanced.push(AdvancedDocument {
                document_id: *document_id,
                accepted: write::Accepted {
                    update_id: request.update_id,
                    head_seq: staged.new_head_seq,
                    head_frontier: staged.after_frontier,
                    projection_seq: staged.new_head_seq,
                    event_id: staged.event_id,
                    before_frontier: prepared.observed.head_frontier.clone(),
                    should_advance_snapshot: false,
                },
            }),
            write::StagedOutcome::Rebase => {
                return Ok(LockedOutcome::Drift(
                    "a contended navigator document head moved under the lock",
                ));
            }
            // Unreachable: `stage_one_document` does not fence (this transaction already holds
            // the exclusive epoch lock and re-verified permission above). Treated as drift rather
            // than assumed away.
            write::StagedOutcome::EpochMismatch => {
                return Ok(LockedOutcome::Drift("the epoch moved under the exclusive lock"));
            }
        }
    }

    // The governance write itself.
    repository::set_object_parent(
        tx,
        plan.object_id,
        plan.target_object_id,
        target.project_id,
        input.actor_id,
    )
    .await?;

    // `ADR-0012` §4.1, applied to the move: the caller may hold `full_access` only through the
    // *old* parent chain, and land the object under a boundary they hold nothing beneath. Computed
    // on this transaction's own post-`UPDATE` snapshot, so the "after" level is the level this
    // commit would actually produce.
    let caller_after = authz::effective_permission(
        tx,
        plan.workspace_id,
        plan.object_id,
        principal_kind,
        input.actor_id,
        &input.role,
    )
    .await?;
    if caller_after < authz::PermissionLevel::FullAccess && !payload.confirm_self_lockout {
        return Err(ApiError::policy_rejected_with_details(
            "this move would remove your own full_access on the object; \
             resend with confirm_self_lockout=true to proceed",
            json!({
                "action": "move_self_lockout",
                "caller": {
                    "before_level": caller_before.as_wire(),
                    "after_level": caller_after.as_wire(),
                    "loses_full_access": true,
                },
            }),
        ));
    }

    // [layer 4] the governance event, carrying the caller's idempotency key.
    let outcome = insert_flow_event(
        tx,
        BusinessEventInput {
            workspace_id: plan.workspace_id,
            project_id: target.project_id,
            event_type: GovernanceCommandType::MoveObject.event_type().to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: plan.object_id.to_string(),
            actor_id: Some(input.actor_id),
            source: json!({ "surface": "rest" }),
            payload: json!({
                "object_id": plan.object_id,
                "old_parent_id": plan.source_parent_id,
                "new_parent_id": plan.target_object_id,
                "position_key": payload.after_id,
            }),
            metadata: json!({ "message": input.message }),
            correlation_id: None,
            causation_id: None,
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
        // A concurrent request with this same key already committed the whole aggregate. Every
        // row staged above belongs to a second, unreferenced copy of one logical move — the same
        // race `create_object` resolves by rolling back and returning the winner's result.
        return Ok(LockedOutcome::AlreadyCommitted);
    }

    // `ADR-0012` §3.1 point 1: `parent_id` is an authorization change, so this transaction
    // advances the epoch. Any content write that checked permission before this commit now fails
    // its own `FOR SHARE` fence.
    let committed_epoch = authz::advance_epoch(tx, plan.workspace_id).await?;

    Ok(LockedOutcome::Committed {
        event_id: outcome.event_id,
        advanced,
        committed_epoch,
    })
}

/// One contended document's head, under `FOR UPDATE`.
async fn lock_document_head(tx: &DatabaseTransaction, document_id: Uuid) -> Result<Option<i64>, ApiError> {
    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        head_seq: i64,
    }
    let row = <Row as sea_orm::FromQueryResult>::find_by_statement(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT head_seq FROM collab_documents WHERE id = $1 FOR UPDATE",
        vec![document_id.into()],
    ))
    .one(tx)
    .await?;
    Ok(row.map(|r| r.head_seq))
}

// ---------------------------------------------------------------------------------------------
// Shared validation
// ---------------------------------------------------------------------------------------------

const fn principal_kind_of(input: &ExecuteCommandInput) -> &'static str {
    if matches!(input.principal_kind.as_bytes(), b"bot") {
        "bot"
    } else {
        "user"
    }
}

/// The contended existing document set for a move between two scopes.
///
/// `ADR-0013` §2.2: the set is derived from `project_id`, which is exactly why the locked phase
/// re-derives it from the locked rows and rolls back on any difference.
struct ContendedSet {
    /// Ascending `document_id`, de-duplicated: the layer-0 and layer-3 lock order alike.
    lock_order: Vec<Uuid>,
    /// The navigator *objects* behind it, for `affected_object_ids`.
    navigator_object_ids: Vec<Uuid>,
    /// The navigator the object is leaving, if that scope has one.
    source_document_id: Option<Uuid>,
    /// The navigator the object is joining, if that scope has one. Equal to
    /// [`Self::source_document_id`] for a move within one scope.
    target_document_id: Option<Uuid>,
}

async fn derive_contended_set<C: sea_orm::ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    source_project_id: Option<Uuid>,
    target_project_id: Option<Uuid>,
) -> Result<ContendedSet, ApiError> {
    let source = repository::fetch_navigator_document(conn, workspace_id, source_project_id).await?;
    let target = repository::fetch_navigator_document(conn, workspace_id, target_project_id).await?;
    let documents: Vec<Uuid> = source.iter().chain(target.iter()).map(|row| row.document_id).collect();
    let mut navigator_object_ids: Vec<Uuid> = source.iter().chain(target.iter()).map(|row| row.object_id).collect();
    navigator_object_ids.sort_unstable();
    navigator_object_ids.dedup();
    Ok(ContendedSet {
        lock_order: ascending_document_lock_order(&documents),
        navigator_object_ids,
        source_document_id: source.map(|row| row.document_id),
        target_document_id: target.map(|row| row.document_id),
    })
}

/// The move-specific half of the write-side structural rules `create_object` cannot express.
///
/// Two things a create can never do and a move does easily:
/// * **a cycle** — the target is the object itself or one of its own descendants, which
///   `flow_objects_parent_not_self_check` (the only schema guard) does not catch beyond one hop,
///   and which permanently un-authorizes the whole ring (`authz::fetch_chain` fails closed on it);
/// * **an over-deep subtree** — a create adds one node of height 0, a move relocates a subtree of
///   arbitrary height, so the rule is `depth(target) + 1 + height(subtree) <= tree_depth_max`,
///   not the parent-only check `ensure_parent_can_adopt_child` performs.
async fn check_cycle_and_depth<C: sea_orm::ConnectionTrait>(
    conn: &C,
    plan: &MovePlan,
    target_chain: &[Uuid],
) -> Result<(), ApiError> {
    if target_chain.contains(&plan.object_id) {
        return Err(ApiError::invalid_update(
            "target_object_id is the object itself or one of its descendants; the move would create a parent_id cycle",
        ));
    }
    // `target_chain` is leaf-first and root-terminated, so its length is `depth(target) + 1`,
    // which is also the moved object's own depth after the move.
    let object_depth_after = target_chain.len();
    let probe = i64::try_from(TREE_DEPTH_MAX.saturating_add(1)).unwrap_or(i64::MAX);
    let height = repository::subtree_height(conn, plan.workspace_id, plan.object_id, probe).await?;
    let deepest = object_depth_after.saturating_add(usize::try_from(height).unwrap_or(usize::MAX));
    if deepest > TREE_DEPTH_MAX {
        return Err(ApiError::limit_exceeded(
            "the moved subtree would sit deeper than the frozen tree depth limit",
            "tree_depth",
            Some(json!(TREE_DEPTH_MAX)),
            Some(json!(deepest)),
            None,
        ));
    }
    Ok(())
}

/// `details.reason` on the transitional refusal `ADR-0013` §2.2 R17 and `rest-api-v1.md`'s
/// `move_object` clause both spell out by name.
pub const SUBTREE_SPANS_MULTIPLE_PROJECTS: &str = "subtree_spans_multiple_projects";

/// The transitional fail-closed rule for `project_id` cascading (`ADR-0013` §2.2 R17).
///
/// The ruling has two steps and this is the gap between them. Step one -- migration `0056`'s
/// `flow_objects_parent_project_fk` -- makes "a subtree belongs to one project scope" a database
/// invariant. Step two, cascading the subtree's `project_id` and ordering entries with the moved
/// object, is blocked on `limits-v1.md`'s `move_subtree_nodes_max`, which is still
/// `status: unset`. Until it is frozen, this command must refuse anything the non-cascading
/// implementation would answer wrongly, because both alternatives are explicitly rejected by the
/// ADR: silently not cascading is a known data inconsistency, and silently cascading breaks the
/// declared `BoundedMany(2)` document ceiling.
///
/// The rule is one predicate over the post-move subtree, computed **without** cascading: the moved
/// object would land in `target_project_id` while every descendant keeps the scope it has, so the
/// set of scopes that subtree would span is `{target_project_id} ∪ {descendant scopes}`. More than
/// one entry means this move cannot be completed correctly today. That covers both reachable
/// causes with one code:
///
/// * **a cross-project move of an object that has descendants** -- the common case, and after
///   `0056` also the case the database itself would refuse: rewriting the moved row's
///   `project_id` while its children still reference the old scope violates the foreign key, so
///   without this check the caller gets a 500 instead of a decision;
/// * **a subtree that already spans scopes** -- pre-`0056` data. Unreachable for anything created
///   after the constraint lands, kept because defence in depth is the point: the check must not
///   depend on the constraint having been applied to the database it is running against.
///
/// `None` participates as a scope value, matching the invariant's own NULL semantics.
async fn ensure_move_keeps_the_subtree_in_one_project<C: sea_orm::ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
    target_project_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let probe = i64::try_from(TREE_DEPTH_MAX.saturating_add(1)).unwrap_or(i64::MAX);
    let mut scopes: std::collections::BTreeSet<Option<Uuid>> =
        repository::descendant_project_scopes(conn, workspace_id, object_id, probe)
            .await?
            .into_iter()
            .collect();
    scopes.insert(target_project_id);
    if scopes.len() > 1 {
        return Err(ApiError::invalid_update_with_details(
            "this move would leave the subtree spanning more than one project scope; cascading \
             project_id across a subtree is gated on the move_subtree_nodes_max limit, which is \
             not frozen yet",
            json!({ "reason": SUBTREE_SPANS_MULTIPLE_PROJECTS }),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

/// `POST /api/v1/flow/objects/{object_id}/commands` with `command.type = "move_object"`.
///
/// Called by `flow::command::execute_command_authorized`, which has already resolved the
/// workspace, taken `checked_epoch` *before* reading permission, and verified `full_access` on the
/// moved object (`ADR-0012` §4's source-side rule). This function owns the target-side `edit`
/// check, the structural rules, and the whole `ADR-0013` §2 multi-document path.
///
/// # Errors
/// `invalid_update` for a malformed payload, a cycle, an archived participant, or a
/// `navigator`/root object as the move's subject; `limit_exceeded{tree_depth}` past the depth
/// ceiling; `policy_rejected` without `edit` on the target, on an unconfirmed self-lockout, or when
/// the in-transaction re-verification finds the caller no longer holds what they held;
/// `server_draining{contention}` when the coordinator or the bounded rebase gives up. Propagates
/// database failures.
pub async fn execute(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    checked_epoch: i64,
) -> Result<AcceptedChange, ApiError> {
    execute_on(state, runtime::runtime(), input, workspace_id, checked_epoch).await
}

/// [`execute`] against an explicit collab runtime.
///
/// The runtime is a parameter rather than a `runtime::runtime()` call inside the body for one
/// reason that matters to this command specifically: the coordinator is **instance-local**
/// (`ADR-0010` 第 0 层), so "two API instances contending the same two documents" is only
/// expressible as "two `CollabRuntime`s". A test that cannot construct that cannot tell the
/// layer-0 and layer-3 orders apart at all.
///
/// # Errors
/// See [`execute`].
#[allow(clippy::too_many_lines)]
pub async fn execute_on(
    state: &AppState,
    collab: &CollabRuntime,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    checked_epoch: i64,
) -> Result<AcceptedChange, ApiError> {
    let ctx = MoveContext {
        state,
        collab,
        input,
        workspace_id,
        checked_epoch,
    };
    if input.expected_frontier.is_some() {
        return Err(ApiError::invalid_update(
            "expected_frontier is not accepted for move_object: this command advances navigator heads, \
             not the moved object's own document; use payload.expected_target_frontier",
        ));
    }
    let payload: MoveObjectPayload = serde_json::from_value(input.payload.clone())
        .map_err(|err| ApiError::invalid_update(format!("invalid move_object payload: {err}")))?;
    let expected_target_frontier = payload
        .expected_target_frontier
        .as_deref()
        .map(frame::decode_bytes)
        .transpose()
        .map_err(|_| ApiError::invalid_update("expected_target_frontier is not valid base64"))?;

    if payload.target_object_id == input.object_id {
        return Err(ApiError::invalid_update("an object cannot be moved under itself"));
    }

    let source = repository::fetch_movable_object(&state.db, input.object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    if source.workspace_id != workspace_id {
        return Err(ApiError::NotFound("flow object not found".to_string()));
    }
    if source.lifecycle_status != "active" {
        return Err(ApiError::invalid_update("an archived object cannot be moved"));
    }
    if source.object_type == "navigator" {
        // A navigator *is* the root of its scope's ordering; giving it a parent would make the
        // structure it orders contain itself.
        return Err(ApiError::invalid_update(
            "a navigator object cannot be moved under a parent",
        ));
    }

    let target = repository::fetch_movable_object(&state.db, payload.target_object_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("target_object_id not found".to_string()))?;
    if target.workspace_id != workspace_id {
        // Cross-workspace: fail closed and leave an integrity record, exactly as `create_object`
        // does for a cross-workspace parent (`rest-api-v1.md`'s "RelationView" rule).
        return Err(super::command::record_cross_workspace_relation_and_fail_closed(
            state,
            workspace_id,
            "flow_object",
            payload.target_object_id,
            target.workspace_id,
            "flow.command.move_object", // detected_by: an ADR-0013 §4 integrity-record producer, not an events-v1 type
        )
        .await);
    }
    if target.lifecycle_status != "active" {
        return Err(ApiError::invalid_update("target_object_id is archived"));
    }

    // `ADR-0012` §4: "被移动对象需 `full_access`（移动会改变它的继承），目标父级需 `edit`". The
    // first half was checked by the caller; this is the second, and it is a *separate*
    // authorization domain whenever the two sides sit under different boundaries — which is
    // precisely what a cross-project move is.
    let target_level = authz::effective_permission(
        &state.db,
        workspace_id,
        payload.target_object_id,
        principal_kind_of(input),
        input.actor_id,
        &input.role,
    )
    .await?;
    if target_level < authz::PermissionLevel::Edit {
        return Err(ApiError::policy_rejected(
            "edit on the target parent is required to move an object under it",
        ));
    }

    let target_chain = authz::inheritance_chain(&state.db, workspace_id, payload.target_object_id)
        .await?
        .ids;
    let ContendedSet {
        lock_order: document_lock_order,
        navigator_object_ids,
        source_document_id: source_navigator,
        target_document_id: target_navigator,
    } = derive_contended_set(&state.db, workspace_id, source.project_id, target.project_id).await?;
    if document_lock_order.len() > MOVE_OBJECT_CONTENDED_DOCUMENT_MAX as usize {
        // Unreachable with two scopes; asserted rather than assumed, because `ADR-0013` §1 makes
        // the ceiling part of the command's declaration and a silent overrun would be a
        // `bounded_many` command quietly becoming unbounded.
        tracing::error!(
            observed = document_lock_order.len(),
            "move_object: contended document set exceeded its declared ceiling"
        );
        return Err(ApiError::Internal);
    }

    let plan = MovePlan {
        workspace_id,
        object_id: input.object_id,
        target_object_id: payload.target_object_id,
        source_parent_id: source.parent_id,
        source_project_id: source.project_id,
        target_project_id: target.project_id,
        target_chain,
        document_lock_order,
        navigator_object_ids,
    };
    check_cycle_and_depth(&state.db, &plan, &plan.target_chain).await?;
    ensure_move_keeps_the_subtree_in_one_project(&state.db, workspace_id, plan.object_id, target.project_id).await?;

    // [layer 0] every contended document's admission slot, ascending, before anything else.
    let Ok(_permits) = collab.coordinator.acquire_many(&plan.document_lock_order).await else {
        return Err(ApiError::server_draining(
            crate::error::ServerDrainingReason::Contention,
            200,
            "server_draining",
        ));
    };

    let mut attempts = 0u32;
    loop {
        attempts += 1;

        let mut plans: Vec<DocumentPlan> = Vec::with_capacity(plan.document_lock_order.len());
        for document_id in &plan.document_lock_order {
            // The source navigator loses the entry; the target navigator gains it. When both
            // scopes share one navigator the single document is the target case — a reposition,
            // not a remove-then-add.
            let is_target = target_navigator == Some(*document_id);
            let is_source_only = !is_target && source_navigator == Some(*document_id);
            let expected = if is_target {
                expected_target_frontier.as_deref()
            } else {
                None
            };
            plans.push(
                plan_document(
                    &ctx,
                    *document_id,
                    plan.object_id,
                    is_source_only,
                    payload.after_id,
                    expected,
                )
                .await?,
            );
        }

        let tx = state.db.begin().await?;
        let mut observed_lock_order = Vec::with_capacity(plan.document_lock_order.len());
        let outcome = run_locked_phase(&tx, &ctx, &plan, &payload, &plans, &mut observed_lock_order).await;

        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(err) => {
                // `ADR-0013` §2.3: atomicity comes from the database. Any failure at any point
                // rolls the whole transaction back — no partial parent change, no half-advanced
                // navigator, no orphan event.
                let _ = tx.rollback().await;
                return Err(err);
            }
        };

        match outcome {
            LockedOutcome::Committed {
                event_id,
                advanced,
                committed_epoch,
            } => {
                tx.commit().await?;
                finish(&ctx, &mut plans, &advanced);
                return build_response(&ctx, &plan, event_id, &advanced, &observed_lock_order, committed_epoch).await;
            }
            LockedOutcome::PermissionRevoked(reason) => {
                let _ = tx.rollback().await;
                return Err(ApiError::policy_rejected(reason));
            }
            LockedOutcome::AlreadyCommitted => {
                let _ = tx.rollback().await;
                return replay(state, workspace_id, input).await?.ok_or(ApiError::Internal);
            }
            LockedOutcome::Drift(reason) => {
                let _ = tx.rollback().await;
                if attempts >= MAX_REBASE_ATTEMPTS {
                    tracing::warn!(reason, "move_object: giving up after bounded rebase attempts");
                    return Err(ApiError::server_draining(
                        crate::error::ServerDrainingReason::Contention,
                        200,
                        "server_draining",
                    ));
                }
            }
        }
    }
}

/// Post-commit cache seeding and ordered broadcast for every advanced navigator document, still
/// inside the coordinator permits this command holds — the same `write::finish_committed` the
/// single-document path uses, so a navigator ordering change reaches open sessions through one
/// broadcast call site rather than a second one written here.
fn finish(ctx: &MoveContext<'_>, plans: &mut Vec<DocumentPlan>, advanced: &[AdvancedDocument]) {
    for entry in std::mem::take(plans) {
        let DocumentPlan::Advance {
            document_id,
            request,
            prepared,
        } = entry
        else {
            continue;
        };
        let Some(committed) = advanced.iter().find(|done| done.document_id == document_id) else {
            continue;
        };
        write::finish_committed(
            &ctx.collab.cache,
            &ctx.collab.registry,
            None,
            &request,
            prepared,
            committed.accepted.clone(),
            false,
        );
        // Tail-shape bookkeeping is deliberately post-commit and non-blocking here: a navigator
        // ordering change is a handful of bytes, and `write::accept_update`'s pre-write hard
        // trigger exists to stop a *content* tail from growing without a checkpoint.
        snapshot::spawn_background(&ctx.collab.snapshot, ctx.state.db.clone(), document_id);
    }
}

/// The response, including the machine-readable evidence
/// `multi_document_lock_order_and_atomicity` needs: which documents were contended and in what
/// order they were locked.
async fn build_response(
    ctx: &MoveContext<'_>,
    plan: &MovePlan,
    event_id: Uuid,
    advanced: &[AdvancedDocument],
    observed_lock_order: &[Uuid],
    committed_epoch: i64,
) -> Result<AcceptedChange, ApiError> {
    let view = repository::fetch_object_view(&ctx.state.db, plan.object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let mut change = accepted_change_from_row(view, event_id);
    let mut affected = vec![plan.object_id, plan.target_object_id];
    affected.extend(plan.navigator_object_ids.iter().copied());
    affected.dedup();
    change.affected_object_ids = affected;
    change.command_result = Some(json!({
        "command": GovernanceCommandType::MoveObject.wire_name(),
        "existing_document_cardinality": "bounded_many",
        "existing_document_cardinality_max": MOVE_OBJECT_CONTENDED_DOCUMENT_MAX,
        "contended_existing_document_set": plan.document_lock_order,
        "document_lock_order": observed_lock_order,
        "old_parent_id": plan.source_parent_id,
        "new_parent_id": plan.target_object_id,
        // `ADR-0012` §3.1 point 2: "gate artifact 必须记录每次写的 checked_epoch 与
        // committed_epoch".
        "checked_epoch": ctx.checked_epoch,
        "committed_epoch": committed_epoch,
        "advanced_documents": advanced
            .iter()
            .map(|done| json!({ "document_id": done.document_id, "accepted_seq": done.accepted.head_seq }))
            .collect::<Vec<Value>>(),
    }));
    Ok(change)
}

/// A repeated `idempotency_key` returns the original event id and the object's current state.
async fn replay(
    state: &AppState,
    workspace_id: Uuid,
    input: &ExecuteCommandInput,
) -> Result<Option<AcceptedChange>, ApiError> {
    let Some(existing) = repository::find_idempotent_event(&state.db, workspace_id, &input.idempotency_key).await?
    else {
        return Ok(None);
    };
    if existing.event_type != GovernanceCommandType::MoveObject.event_type()
        || existing.aggregate_id != input.object_id.to_string()
    {
        return Ok(None);
    }
    let view = repository::fetch_object_view(&state.db, input.object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok(Some(accepted_change_from_row(view, existing.id)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::{GovernanceCommandType, MOVE_OBJECT_CONTENDED_DOCUMENT_MAX, position_after};
    use crate::flow::command::ExistingDocumentCardinality;
    use collab_core::{NodeKind, SemanticNode, SemanticSnapshot};
    use std::sync::Arc;
    use uuid::Uuid;

    fn node(order_key: &str) -> SemanticNode {
        SemanticNode {
            parent: None,
            order_key: order_key.to_string(),
            kind: NodeKind::NavigatorNode,
            text: String::new(),
            properties: std::collections::BTreeMap::new(),
            deleted: false,
        }
    }

    #[test]
    fn move_object_declares_bounded_many_at_its_frozen_ceiling() {
        let declared = GovernanceCommandType::MoveObject.existing_document_cardinality();
        assert_eq!(
            declared,
            ExistingDocumentCardinality::BoundedMany(MOVE_OBJECT_CONTENDED_DOCUMENT_MAX)
        );
        assert_eq!(declared.count(), 2, "the two navigators, and never a third");
    }

    #[test]
    fn governance_command_parses_only_its_own_wire_name() {
        assert_eq!(
            GovernanceCommandType::parse("move_object"),
            Some(GovernanceCommandType::MoveObject)
        );
        assert_eq!(GovernanceCommandType::parse("move_block"), None);
        assert_eq!(GovernanceCommandType::parse("create_child"), None);
        assert_eq!(GovernanceCommandType::MoveObject.wire_name(), "move_object");
        assert_eq!(GovernanceCommandType::MoveObject.event_type(), "flow.object.moved");
    }

    #[test]
    fn position_after_appends_when_the_predecessor_is_absent_or_unset() {
        let mut snapshot = SemanticSnapshot::default();
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        snapshot.nodes.insert(Arc::from(first.to_string()), node("0000000000"));
        snapshot.nodes.insert(Arc::from(second.to_string()), node("0000000001"));

        assert_eq!(position_after(&snapshot, None, None), 2, "no predecessor means append");
        assert_eq!(position_after(&snapshot, Some(first), None), 1);
        assert_eq!(position_after(&snapshot, Some(second), None), 2);
        assert_eq!(
            position_after(&snapshot, Some(Uuid::from_u128(99)), None),
            2,
            "a stale after_id degrades to append rather than failing the command"
        );

        // Repositioning an existing entry: the entry itself must not be counted, or the index
        // lands one past what a same-parent `MoveNode` accepts.
        let moving: super::NodeId = Arc::from(second.to_string());
        assert_eq!(
            position_after(&snapshot, None, Some(&moving)),
            1,
            "appending a repositioned entry must index into the list without it"
        );
        assert_eq!(position_after(&snapshot, Some(first), Some(&moving)), 1);
    }
}

// ---------------------------------------------------------------------------------------------
// Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`)
// ---------------------------------------------------------------------------------------------
//
// Everything asserted below is a rule that only exists once rows can be written and two callers
// can race, so none of it is expressible as a pure function test. Same throwaway-database-per-test
// convention as `super::grants`'s and `super::collab::authz`'s suites.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]
mod database_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use platform::app::AppState;
    use platform::config::{AppConfig, Secret};
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::execute_on;
    use crate::error::{ApiError, ApiErrorKind};
    use crate::flow::collab::authz::{self, PermissionLevel};
    use crate::flow::collab::coordinator::ascending_document_lock_order;
    use crate::flow::collab::runtime::CollabRuntime;
    use crate::flow::command::{CreateObjectInput, ExecuteCommandInput, create_object};
    use crate::flow::model::AcceptedChange;

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
        let name = format!("openpr_flow_move_{label}");
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
                app_name: "flow-move-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-move-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
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
        project_a: Uuid,
        project_b: Uuid,
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
                vec![user_id.into(), format!("{user_id}@move.test").into()],
            )
            .await;
        }
        exec(
            db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'move test', $3)",
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
        let project_a = Uuid::new_v4();
        let project_b = Uuid::new_v4();
        for (project_id, key) in [(project_a, "PA"), (project_b, "PB")] {
            exec(
                db,
                "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, $3, $3, $4)",
                vec![project_id.into(), workspace_id.into(), key.into(), owner_id.into()],
            )
            .await;
        }
        Fixture {
            workspace_id,
            owner_id,
            member_id,
            project_a,
            project_b,
        }
    }

    async fn create(
        state: &AppState,
        fx: &Fixture,
        object_type: &str,
        project_id: Option<Uuid>,
        parent: Option<Uuid>,
    ) -> Uuid {
        create_object(
            state,
            CreateObjectInput {
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                object_type: object_type.to_string(),
                project_id,
                parent_object_id: parent,
                title: "Move Fixture".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .expect("object is created")
        .object
        .id
    }

    fn move_input(object_id: Uuid, actor_id: Uuid, role: &str, payload: Value) -> ExecuteCommandInput {
        ExecuteCommandInput {
            object_id,
            actor_id,
            principal_kind: "user".to_string(),
            role: role.to_string(),
            command_type: "move_object".to_string(),
            payload,
            expected_frontier: None,
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
            origin_client_id: "move-test".to_string(),
        }
    }

    /// Runs `move_object` through its real entry point against an explicit runtime, taking the
    /// `checked_epoch` the same way `flow::command::execute_command_authorized` does.
    async fn run_move(
        state: &AppState,
        collab: &CollabRuntime,
        fx: &Fixture,
        input: &ExecuteCommandInput,
    ) -> Result<AcceptedChange, ApiError> {
        let checked_epoch = authz::read_epoch(&state.db, fx.workspace_id).await?;
        execute_on(state, collab, input, fx.workspace_id, checked_epoch).await
    }

    async fn scalar_i64(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            value: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .one(db)
            .await
            .expect("query runs")
            .expect("query returns a row")
            .value
    }

    async fn head_seq(db: &DatabaseConnection, document_id: Uuid) -> i64 {
        scalar_i64(
            db,
            "SELECT head_seq AS value FROM collab_documents WHERE id = $1",
            vec![document_id.into()],
        )
        .await
    }

    async fn head_frontier(db: &DatabaseConnection, document_id: Uuid) -> String {
        #[derive(FromQueryResult)]
        struct Row {
            head_frontier: Vec<u8>,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE id = $1",
            vec![document_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("document exists");
        crate::flow::collab::frame::encode_bytes(&row.head_frontier)
    }

    /// The navigator's live root ordering entries, in `order_key` order, mapped back to the
    /// object each one stands for — read out of `flow_object_projections.state`, which is the
    /// committed projection of the navigator document, not anything this command reported about
    /// itself.
    async fn navigator_order(db: &DatabaseConnection, navigator_object_id: Uuid) -> Vec<Uuid> {
        #[derive(FromQueryResult)]
        struct Row {
            state: Value,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM flow_object_projections WHERE object_id = $1",
            vec![navigator_object_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("projection exists");
        let nodes = row.state["nodes"].as_object().expect("state has a nodes map").clone();
        let mut live: Vec<(String, Uuid)> = nodes
            .iter()
            .filter(|(_, node)| node["deleted"] == json!(false) && node["parent"].is_null())
            .map(|(id, node)| {
                let order_key = node["order_key"].as_str().expect("order_key").to_string();
                // Entry ids are `<uuid>` or `<uuid>#n`; both start with the object's uuid.
                let uuid = Uuid::parse_str(id.get(..36).unwrap_or(id)).expect("entry id starts with a uuid");
                (order_key, uuid)
            })
            .collect();
        live.sort();
        live.into_iter().map(|(_, id)| id).collect()
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
        .expect("document exists")
        .id
    }

    async fn parent_of(db: &DatabaseConnection, object_id: Uuid) -> Option<Uuid> {
        #[derive(FromQueryResult)]
        struct Row {
            parent_id: Option<Uuid>,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT parent_id FROM flow_objects WHERE id = $1",
            vec![object_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("object exists")
        .parent_id
    }

    async fn project_of(db: &DatabaseConnection, object_id: Uuid) -> Option<Uuid> {
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
        .expect("object exists")
        .project_id
    }

    async fn epoch_of(db: &DatabaseConnection, workspace_id: Uuid) -> i64 {
        scalar_i64(
            db,
            "SELECT authz_epoch AS value FROM flow_workspace_settings WHERE workspace_id = $1",
            vec![workspace_id.into()],
        )
        .await
    }

    async fn moved_event_count(db: &DatabaseConnection, workspace_id: Uuid) -> i64 {
        scalar_i64(
            db,
            "SELECT count(*)::bigint AS value FROM business_events \
             WHERE workspace_id = $1 AND event_type = 'flow.object.moved'",
            vec![workspace_id.into()],
        )
        .await
    }

    async fn update_count(db: &DatabaseConnection, document_id: Uuid) -> i64 {
        scalar_i64(
            db,
            "SELECT count(*)::bigint AS value FROM collab_updates WHERE document_id = $1",
            vec![document_id.into()],
        )
        .await
    }

    async fn level_for(
        db: &DatabaseConnection,
        fx: &Fixture,
        object_id: Uuid,
        user_id: Uuid,
        role: &str,
    ) -> PermissionLevel {
        authz::effective_permission(db, fx.workspace_id, object_id, "user", user_id, role)
            .await
            .unwrap_or_else(|err| panic!("effective_permission failed: {err:?}"))
    }

    fn command_result(change: &AcceptedChange) -> &Value {
        change
            .command_result
            .as_ref()
            .expect("move_object always reports its multi-document evidence")
    }

    fn uuid_list(value: &Value) -> Vec<Uuid> {
        value
            .as_array()
            .expect("array")
            .iter()
            .map(|entry| Uuid::parse_str(entry.as_str().expect("uuid string")).expect("uuid"))
            .collect()
    }

    // -----------------------------------------------------------------------------------------
    // 1. The multi-document path itself: two heads, one transaction, ascending lock order.
    // -----------------------------------------------------------------------------------------

    #[tokio::test]
    async fn cross_project_move_advances_both_navigator_heads_in_one_ascending_ordered_transaction() {
        let scratch = scratch_or_skip!("cross_project");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let doc_a = document_of(&scratch.db, nav_a).await;
        let doc_b = document_of(&scratch.db, nav_b).await;

        // First move materialises the ordering entry in B's navigator: A has no entry to remove
        // yet, so only one head advances. This is the honest shape of a first move, and it is the
        // setup that makes the *second* move a genuine two-document command.
        let first = run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("first move succeeds");
        assert_eq!(head_seq(&scratch.db, doc_b).await, 1, "B's navigator gained the entry");
        assert_eq!(head_seq(&scratch.db, doc_a).await, 0, "A had no entry to remove");
        assert_eq!(project_of(&scratch.db, page).await, Some(fx.project_b));
        assert_eq!(parent_of(&scratch.db, page).await, Some(nav_b));

        let before_a = head_seq(&scratch.db, doc_a).await;
        let before_b = head_seq(&scratch.db, doc_b).await;
        let epoch_before = epoch_of(&scratch.db, fx.workspace_id).await;

        // Second move: B loses the entry, A gains it — two existing heads in one transaction.
        let second = run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_a })),
        )
        .await
        .expect("second move succeeds");

        assert_eq!(
            head_seq(&scratch.db, doc_a).await,
            before_a + 1,
            "A's navigator advanced"
        );
        assert_eq!(
            head_seq(&scratch.db, doc_b).await,
            before_b + 1,
            "B's navigator advanced"
        );
        assert_eq!(parent_of(&scratch.db, page).await, Some(nav_a));
        assert_eq!(project_of(&scratch.db, page).await, Some(fx.project_a));

        let result = command_result(&second);
        assert_eq!(result["existing_document_cardinality"], json!("bounded_many"));
        assert_eq!(result["existing_document_cardinality_max"], json!(2));

        let contended = uuid_list(&result["contended_existing_document_set"]);
        let observed = uuid_list(&result["document_lock_order"]);
        assert_eq!(
            contended.len(),
            2,
            "a cross-project move contends exactly two navigators"
        );
        assert_eq!(
            observed,
            ascending_document_lock_order(&[doc_a, doc_b]),
            "the database lock order must be the ascending document_id order, byte for byte"
        );
        assert_eq!(
            observed,
            ascending_document_lock_order(&contended),
            "coordinator order (acquire_many) and database order come from the same function, \
             so the order actually taken must equal that function's output"
        );

        // `ADR-0012` §3.1 point 2's gate artifact fields, and the epoch bump a `parent_id` change
        // owes (§3.1 point 1).
        assert_eq!(result["checked_epoch"], json!(epoch_before));
        assert_eq!(result["committed_epoch"], json!(epoch_before + 1));
        assert_eq!(epoch_of(&scratch.db, fx.workspace_id).await, epoch_before + 1);

        assert_ne!(first.event_id, second.event_id);
        assert_eq!(moved_event_count(&scratch.db, fx.workspace_id).await, 2);
        assert!(second.affected_object_ids.contains(&page));
        assert!(second.affected_object_ids.contains(&nav_a));
        assert!(second.affected_object_ids.contains(&nav_b));

        scratch.drop_self().await;
    }

    /// The layer-0/layer-3 agreement the previous test asserts on one invocation, stated as the
    /// property it comes from: whatever set is derived, both layers order it through
    /// `ascending_document_lock_order`, so a reversed *request* order produces an identical
    /// *lock* order.
    #[tokio::test]
    async fn document_lock_order_is_identical_whichever_direction_the_move_goes() {
        let scratch = scratch_or_skip!("lock_order_symmetry");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let doc_a = document_of(&scratch.db, nav_a).await;
        let doc_b = document_of(&scratch.db, nav_b).await;

        let one = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let two = create(&state, &fx, "page", Some(fx.project_b), Some(nav_b)).await;

        let a_to_b = run_move(
            &state,
            &collab,
            &fx,
            &move_input(one, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("A -> B move succeeds");
        let b_to_a = run_move(
            &state,
            &collab,
            &fx,
            &move_input(two, fx.owner_id, "owner", json!({ "target_object_id": nav_a })),
        )
        .await
        .expect("B -> A move succeeds");

        let expected = ascending_document_lock_order(&[doc_a, doc_b]);
        assert_eq!(uuid_list(&command_result(&a_to_b)["document_lock_order"]), expected);
        assert_eq!(
            uuid_list(&command_result(&b_to_a)["document_lock_order"]),
            expected,
            "the opposite-direction move must lock the same two documents in the same order — \
             this is the whole of ADR-0013 §2.1's deadlock argument"
        );
        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 2. Two concurrent moves whose contended sets are reversed.
    // -----------------------------------------------------------------------------------------

    /// `multi_document_lock_order_and_atomicity`: two moves in opposite directions between the
    /// same two projects, issued at the same time on **two separate `CollabRuntime`s** (the
    /// coordinator is instance-local, so one runtime each is what "two API instances" means).
    ///
    /// The required outcome is "both succeed or fail cleanly": no deadlock, no timeout, no
    /// half-applied move. Both do in fact succeed here — the exclusive `authz_epoch` lock
    /// serializes them at layer 1, and because this command *re-verifies* permission under that
    /// lock instead of comparing epochs, the second one is delayed rather than rejected.
    #[tokio::test]
    async fn two_concurrent_moves_with_reversed_contended_sets_both_complete_cleanly() {
        let scratch = scratch_or_skip!("concurrent_reversed");
        let state = Arc::new(state_for(scratch.db.clone()));
        let fx = Arc::new(seed_workspace(&scratch.db).await);

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let one = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let two = create(&state, &fx, "page", Some(fx.project_b), Some(nav_b)).await;

        // Two runtimes: two instances, two independent coordinators, one database.
        let left_runtime = Arc::new(CollabRuntime::default());
        let right_runtime = Arc::new(CollabRuntime::default());

        let left = {
            let (state, fx, runtime) = (state.clone(), fx.clone(), left_runtime.clone());
            tokio::spawn(async move {
                run_move(
                    &state,
                    &runtime,
                    &fx,
                    &move_input(one, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
                )
                .await
                .map(|_| ())
            })
        };
        let right = {
            let (state, fx, runtime) = (state.clone(), fx.clone(), right_runtime.clone());
            tokio::spawn(async move {
                run_move(
                    &state,
                    &runtime,
                    &fx,
                    &move_input(two, fx.owner_id, "owner", json!({ "target_object_id": nav_a })),
                )
                .await
                .map(|_| ())
            })
        };

        let left = tokio::time::timeout(Duration::from_secs(30), left)
            .await
            .expect("the first concurrent move must not hang")
            .expect("task joins");
        let right = tokio::time::timeout(Duration::from_secs(30), right)
            .await
            .expect("the second concurrent move must not hang")
            .expect("task joins");

        for (label, outcome) in [("left", &left), ("right", &right)] {
            match outcome {
                Ok(()) => {}
                Err(err) => panic!("{label} move failed instead of serializing cleanly: {err:?}"),
            }
        }
        assert_eq!(parent_of(&scratch.db, one).await, Some(nav_b));
        assert_eq!(parent_of(&scratch.db, two).await, Some(nav_a));
        assert_eq!(
            moved_event_count(&scratch.db, fx.workspace_id).await,
            2,
            "two logical moves, two events, no duplicate and no lost write"
        );
        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 3. Inheritance flips immediately, and a boundary denies rather than downgrading to view.
    // -----------------------------------------------------------------------------------------

    #[tokio::test]
    async fn moving_under_a_boundary_denies_a_baseline_member_immediately() {
        let scratch = scratch_or_skip!("inheritance_flip");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let open_parent = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;
        let closed_parent = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(open_parent)).await;

        // The authorization boundary, with no grant for the member beneath it.
        exec(
            &scratch.db,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![closed_parent.into()],
        )
        .await;

        assert_eq!(
            level_for(&scratch.db, &fx, page, fx.member_id, "member").await,
            PermissionLevel::Edit,
            "before the move the member reaches the page through the workspace baseline"
        );
        assert_eq!(
            level_for(&scratch.db, &fx, closed_parent, fx.member_id, "member").await,
            PermissionLevel::Denied
        );

        run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": closed_parent })),
        )
        .await
        .expect("the owner may move the page under the boundary");

        assert_eq!(
            level_for(&scratch.db, &fx, page, fx.member_id, "member").await,
            PermissionLevel::Denied,
            "ADR-0012 §4: inheritance flips immediately, and the boundary cuts the workspace \
             baseline entirely — the member is denied, not downgraded to view"
        );
        scratch.drop_self().await;
    }

    /// The other half of the double-sided rule (`ADR-0012` §4): the target parent needs `edit`,
    /// and a boundary the caller holds nothing beneath denies it.
    #[tokio::test]
    async fn a_target_the_caller_cannot_edit_rejects_the_move() {
        let scratch = scratch_or_skip!("target_edit");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;
        let closed_parent = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;
        exec(
            &scratch.db,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![closed_parent.into()],
        )
        .await;
        // The member keeps full_access on the object being moved, so only the *target* side of
        // the rule can be what refuses.
        exec(
            &scratch.db,
            "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level) \
             VALUES ($1, $2, 'user', $3, 'full_access')",
            vec![fx.workspace_id.into(), page.into(), fx.member_id.into()],
        )
        .await;

        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(
                page,
                fx.member_id,
                "member",
                json!({ "target_object_id": closed_parent }),
            ),
        )
        .await
        .expect_err("without edit on the target the move must be refused");
        assert_eq!(err.kind(), ApiErrorKind::PolicyRejected, "got {err:?}");
        assert_eq!(parent_of(&scratch.db, page).await, Some(nav), "nothing moved");
        assert_eq!(moved_event_count(&scratch.db, fx.workspace_id).await, 0);
        scratch.drop_self().await;
    }

    /// `ADR-0012` §4.1 applied to the move: a caller who would lose their own `full_access` must
    /// say so explicitly.
    #[tokio::test]
    async fn self_lockout_needs_confirmation_and_then_proceeds() {
        let scratch = scratch_or_skip!("self_lockout");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;
        let closed_parent = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;
        exec(
            &scratch.db,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![closed_parent.into()],
        )
        .await;
        // The member holds full_access on both sides *today*: on the page directly, and on the
        // restricted parent through a grant. Moving the page under the boundary drops the page's
        // own grant out of the picture... it does not: an object's explicit grants travel with it
        // (ADR-0012 §4), so the lockout case needs the grant to live on the *old parent*.
        let open_parent = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;
        exec(
            &scratch.db,
            "UPDATE flow_objects SET parent_id = $2 WHERE id = $1",
            vec![page.into(), open_parent.into()],
        )
        .await;
        for (object_id, level) in [(open_parent, "full_access"), (closed_parent, "edit")] {
            exec(
                &scratch.db,
                "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level) \
                 VALUES ($1, $2, 'user', $3, $4)",
                vec![
                    fx.workspace_id.into(),
                    object_id.into(),
                    fx.member_id.into(),
                    level.into(),
                ],
            )
            .await;
        }

        assert_eq!(
            level_for(&scratch.db, &fx, page, fx.member_id, "member").await,
            PermissionLevel::FullAccess,
            "the member's full_access on the page is inherited from its current parent"
        );

        // Materialise the navigator ordering entry so the refused attempt below really does stage
        // a document head advance before it is refused.
        let nav_doc = document_of(&scratch.db, nav).await;
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": open_parent })),
        )
        .await
        .expect("the owner re-seats the page under the same parent, creating its navigator entry");
        let before = (
            head_seq(&scratch.db, nav_doc).await,
            update_count(&scratch.db, nav_doc).await,
            epoch_of(&scratch.db, fx.workspace_id).await,
            moved_event_count(&scratch.db, fx.workspace_id).await,
        );
        assert!(
            before.0 > 0,
            "the navigator entry must exist before the refused attempt"
        );

        let refused = run_move(
            &state,
            &collab,
            &fx,
            &move_input(
                page,
                fx.member_id,
                "member",
                json!({ "target_object_id": closed_parent }),
            ),
        )
        .await
        .expect_err("an unconfirmed self-lockout must be refused");
        assert_eq!(refused.kind(), ApiErrorKind::PolicyRejected, "got {refused:?}");
        let ApiError::Typed { details, .. } = &refused else {
            panic!("expected a typed policy_rejected, got {refused:?}");
        };
        let details = details.as_ref().expect("the refusal carries a post-state summary");
        assert_eq!(details["action"], json!("move_self_lockout"));
        assert_eq!(details["caller"]["before_level"], json!("full_access"));
        assert_eq!(details["caller"]["after_level"], json!("edit"));
        assert_eq!(parent_of(&scratch.db, page).await, Some(open_parent), "nothing moved");
        // `ADR-0013` §2.3's atomicity claim, on the one fault class that does *not* poison the
        // `PostgreSQL` transaction: the §4.1 refusal is raised **after** the navigator head has
        // been advanced and `parent_id` rewritten inside this transaction, so if the error path
        // committed instead of rolling back, every number below would have moved. This is the
        // assertion that goes red when `tx.rollback()` on the error path becomes `tx.commit()`.
        assert_eq!(
            (
                head_seq(&scratch.db, nav_doc).await,
                update_count(&scratch.db, nav_doc).await,
                epoch_of(&scratch.db, fx.workspace_id).await,
                moved_event_count(&scratch.db, fx.workspace_id).await,
            ),
            before,
            "a refusal raised after the document head was already staged must roll back the head, \
             the collab_updates row, the epoch bump and the event together with the parent change"
        );

        run_move(
            &state,
            &collab,
            &fx,
            &move_input(
                page,
                fx.member_id,
                "member",
                json!({ "target_object_id": closed_parent, "confirm_self_lockout": true }),
            ),
        )
        .await
        .expect("with the confirmation the move proceeds");
        assert_eq!(parent_of(&scratch.db, page).await, Some(closed_parent));
        assert_eq!(
            level_for(&scratch.db, &fx, page, fx.member_id, "member").await,
            PermissionLevel::Edit,
            "the caller kept only what the boundary grants them"
        );
        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 4. Cycles and depth.
    // -----------------------------------------------------------------------------------------

    #[tokio::test]
    async fn moving_an_ancestor_under_its_own_descendant_is_rejected() {
        let scratch = scratch_or_skip!("cycle");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let a = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;
        let b = create(&state, &fx, "page", Some(fx.project_a), Some(a)).await;
        let c = create(&state, &fx, "page", Some(fx.project_a), Some(b)).await;

        for (label, target) in [("its own child", b), ("a deeper descendant", c)] {
            let err = run_move(
                &state,
                &collab,
                &fx,
                &move_input(a, fx.owner_id, "owner", json!({ "target_object_id": target })),
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("moving A under {label} must be refused, but it succeeded"));
            assert_eq!(
                err.kind(),
                ApiErrorKind::InvalidUpdate,
                "moving A under {label} must be an invalid_update, got {err:?}"
            );
            assert_eq!(parent_of(&scratch.db, a).await, Some(nav), "nothing moved");
        }
        assert_eq!(moved_event_count(&scratch.db, fx.workspace_id).await, 0);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_subtree_that_would_not_fit_under_the_target_is_refused_while_a_leaf_fits() {
        let scratch = scratch_or_skip!("depth");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        // Root at depth 0, then 31 more hops: the deepest node sits at depth 31, so one more hop
        // is legal (32) and two are not (33).
        let mut chain = vec![create(&state, &fx, "navigator", Some(fx.project_a), None).await];
        for _ in 0..31 {
            let parent = chain[chain.len() - 1];
            chain.push(create(&state, &fx, "page", Some(fx.project_a), Some(parent)).await);
        }
        let deepest = chain[chain.len() - 1];

        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let leaf = create(&state, &fx, "page", Some(fx.project_b), Some(nav_b)).await;
        let with_child = create(&state, &fx, "page", Some(fx.project_b), Some(nav_b)).await;
        let _child = create(&state, &fx, "page", Some(fx.project_b), Some(with_child)).await;

        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(with_child, fx.owner_id, "owner", json!({ "target_object_id": deepest })),
        )
        .await
        .expect_err("a height-1 subtree does not fit under a depth-31 parent");
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded, "got {err:?}");
        let ApiError::Typed { details, .. } = &err else {
            panic!("expected a typed limit_exceeded, got {err:?}");
        };
        assert_eq!(
            details.as_ref().and_then(|d| d.get("limit_kind")),
            Some(&json!("tree_depth"))
        );
        assert_eq!(parent_of(&scratch.db, with_child).await, Some(nav_b), "nothing moved");

        run_move(
            &state,
            &collab,
            &fx,
            &move_input(leaf, fx.owner_id, "owner", json!({ "target_object_id": deepest })),
        )
        .await
        .expect("a leaf does fit at depth 32 — the refusal above is about the subtree, not the target");
        assert_eq!(parent_of(&scratch.db, leaf).await, Some(deepest));
        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 5. Atomicity: any failure point rolls the whole thing back.
    // -----------------------------------------------------------------------------------------

    /// A real fault injected at the **last** write of the transaction (the `flow.object.moved`
    /// event), by a trigger in the scratch database — so every earlier write (both navigator head
    /// advances, both `collab_updates` rows, the two `flow.content.accepted` events, the
    /// `parent_id`/`project_id` rewrite) is already staged when it fires.
    ///
    /// `ADR-0013` §3's claim is that atomicity comes from the database and nothing needs
    /// compensating. This is that claim, tested.
    #[tokio::test]
    async fn a_failure_at_the_last_write_leaves_no_trace_of_the_move() {
        let scratch = scratch_or_skip!("atomicity");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let doc_a = document_of(&scratch.db, nav_a).await;
        let doc_b = document_of(&scratch.db, nav_b).await;

        // Materialise A's entry so the failing move is a genuine two-document one.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("setup move succeeds");
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_a })),
        )
        .await
        .expect("setup move back succeeds");

        let before = (
            head_seq(&scratch.db, doc_a).await,
            head_seq(&scratch.db, doc_b).await,
            update_count(&scratch.db, doc_a).await,
            update_count(&scratch.db, doc_b).await,
            epoch_of(&scratch.db, fx.workspace_id).await,
            moved_event_count(&scratch.db, fx.workspace_id).await,
            parent_of(&scratch.db, page).await,
            project_of(&scratch.db, page).await,
        );

        scratch
            .db
            .execute_unprepared(
                "CREATE FUNCTION move_fault() RETURNS trigger AS $$ \
                 BEGIN \
                   IF NEW.event_type = 'flow.object.moved' THEN \
                     RAISE EXCEPTION 'injected fault at the last write of the move transaction'; \
                   END IF; \
                   RETURN NEW; \
                 END $$ LANGUAGE plpgsql; \
                 CREATE TRIGGER move_fault_trigger BEFORE INSERT ON business_events \
                   FOR EACH ROW EXECUTE FUNCTION move_fault();",
            )
            .await
            .expect("fault injection trigger installs");

        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect_err("the injected fault must surface as an error, not a partial success");
        eprintln!("injected-fault error: {err:?}");
        // Worth being precise about what this half proves: a trigger exception aborts the
        // `PostgreSQL` transaction, so `PostgreSQL` itself guarantees the rollback and this
        // assertion cannot distinguish `tx.rollback()` from `tx.commit()`. The *application*-level
        // refusal that leaves the transaction perfectly healthy is the falsifiable case, and it is
        // asserted in `self_lockout_needs_confirmation_and_then_proceeds` below, which checks that
        // the already-staged navigator head advance and `parent_id` rewrite are gone after an
        // unconfirmed self-lockout.
        assert!(
            matches!(err, ApiError::Database(_)),
            "the fault must be the trigger firing inside the transaction, not a validation \
             rejection taken before one was ever opened — a test that never reaches the write it \
             claims to roll back would pass its before/after comparison for the wrong reason. \
             Got {err:?}"
        );

        scratch
            .db
            .execute_unprepared("DROP TRIGGER move_fault_trigger ON business_events; DROP FUNCTION move_fault();")
            .await
            .expect("fault injection trigger is removed");

        let after = (
            head_seq(&scratch.db, doc_a).await,
            head_seq(&scratch.db, doc_b).await,
            update_count(&scratch.db, doc_a).await,
            update_count(&scratch.db, doc_b).await,
            epoch_of(&scratch.db, fx.workspace_id).await,
            moved_event_count(&scratch.db, fx.workspace_id).await,
            parent_of(&scratch.db, page).await,
            project_of(&scratch.db, page).await,
        );
        assert_eq!(
            before, after,
            "a failure anywhere in the move transaction must leave heads, updates, epoch, events, \
             parent_id and project_id exactly as they were"
        );

        // And the command still works afterwards: the rollback left nothing wedged.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("the same move succeeds once the fault is removed");
        assert_eq!(parent_of(&scratch.db, page).await, Some(nav_b));
        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 6. Idempotency and shape rejections.
    // -----------------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_replayed_idempotency_key_returns_the_original_event_and_moves_nothing_twice() {
        let scratch = scratch_or_skip!("idempotency");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let doc_b = document_of(&scratch.db, nav_b).await;

        let input = move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_b }));
        let first = run_move(&state, &collab, &fx, &input).await.expect("first attempt");
        let head_after_first = head_seq(&scratch.db, doc_b).await;

        // Replayed through the same entry point the REST surface would use on a retry: the
        // `business_events` idempotency index is what decides, not a cached answer.
        let checked_epoch = authz::read_epoch(&scratch.db, fx.workspace_id).await.expect("epoch");
        let second = super::replay(&state, fx.workspace_id, &input)
            .await
            .expect("replay lookup runs")
            .expect("the key is already recorded");
        assert_eq!(second.event_id, first.event_id);
        assert_eq!(
            head_seq(&scratch.db, doc_b).await,
            head_after_first,
            "no second advance"
        );
        assert_eq!(moved_event_count(&scratch.db, fx.workspace_id).await, 1);
        let _ = checked_epoch;
        scratch.drop_self().await;
    }

    /// `rest-api-v1.md`: "只有真正推进 target 文档 canonical head 的命令才携带
    /// `expected_target_frontier?`". `move_object` is such a command, so the guard has to actually
    /// guard the *target navigator's* head — not the moved object's own document, which this
    /// command never touches.
    #[tokio::test]
    async fn expected_target_frontier_guards_the_target_navigator_head() {
        let scratch = scratch_or_skip!("expected_target_frontier");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let doc_b = document_of(&scratch.db, nav_b).await;

        let wrong_frontier = crate::flow::collab::frame::encode_bytes(b"not-this-documents-frontier");
        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(
                page,
                fx.owner_id,
                "owner",
                json!({ "target_object_id": nav_b, "expected_target_frontier": wrong_frontier }),
            ),
        )
        .await
        .expect_err("a frontier that is not the target navigator's head must be refused");
        assert_eq!(err.kind(), ApiErrorKind::StaleFrontier, "got {err:?}");
        assert_eq!(parent_of(&scratch.db, page).await, Some(nav_a), "nothing moved");
        assert_eq!(
            head_seq(&scratch.db, doc_b).await,
            0,
            "the target navigator did not advance"
        );

        let current = head_frontier(&scratch.db, doc_b).await;
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(
                page,
                fx.owner_id,
                "owner",
                json!({ "target_object_id": nav_b, "expected_target_frontier": current }),
            ),
        )
        .await
        .expect("the target navigator's actual head frontier is accepted");
        assert_eq!(parent_of(&scratch.db, page).await, Some(nav_b));
        assert_eq!(head_seq(&scratch.db, doc_b).await, 1);
        scratch.drop_self().await;
    }

    /// `cli-surface-v1.md`'s `--after ID` / `mcp-surface-v1.md`'s `after_id?`: the moved entry
    /// lands immediately after the named sibling in the target navigator's ordering, and appends
    /// when no sibling is named.
    #[tokio::test]
    async fn after_id_places_the_entry_where_the_caller_asked() {
        let scratch = scratch_or_skip!("after_id");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let first = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let second = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let third = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;

        // Append, append: B's navigator ends up [first, second].
        for page in [first, second] {
            run_move(
                &state,
                &collab,
                &fx,
                &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
            )
            .await
            .expect("append move succeeds");
        }
        assert_eq!(navigator_order(&scratch.db, nav_b).await, vec![first, second]);

        // `after_id = first` must land the third entry between them, not at the end.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(
                third,
                fx.owner_id,
                "owner",
                json!({ "target_object_id": nav_b, "after_id": first }),
            ),
        )
        .await
        .expect("positioned move succeeds");
        assert_eq!(
            navigator_order(&scratch.db, nav_b).await,
            vec![first, third, second],
            "after_id must place the entry immediately after the named sibling"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn shape_rules_are_refused_before_anything_is_locked() {
        let scratch = scratch_or_skip!("shape");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;

        // A missing `target_object_id`: "move to the workspace root" is not a v0.5 operation.
        let err = run_move(&state, &collab, &fx, &move_input(page, fx.owner_id, "owner", json!({})))
            .await
            .expect_err("target_object_id is required");
        assert_eq!(err.kind(), ApiErrorKind::InvalidUpdate, "got {err:?}");

        // The object as its own target.
        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": page })),
        )
        .await
        .expect_err("an object cannot be moved under itself");
        assert_eq!(err.kind(), ApiErrorKind::InvalidUpdate, "got {err:?}");

        // A navigator is its scope's root and has nowhere to be moved to.
        let other = create(&state, &fx, "page", Some(fx.project_a), Some(nav)).await;
        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(nav, fx.owner_id, "owner", json!({ "target_object_id": other })),
        )
        .await
        .expect_err("a navigator cannot be moved");
        assert_eq!(err.kind(), ApiErrorKind::InvalidUpdate, "got {err:?}");

        // `expected_frontier` belongs to content commands; the move's guard is
        // `expected_target_frontier`.
        let mut input = move_input(page, fx.owner_id, "owner", json!({ "target_object_id": other }));
        input.expected_frontier = Some("AAAA".to_string());
        let err = run_move(&state, &collab, &fx, &input)
            .await
            .expect_err("expected_frontier is not a move_object field");
        assert_eq!(err.kind(), ApiErrorKind::InvalidUpdate, "got {err:?}");

        assert_eq!(moved_event_count(&scratch.db, fx.workspace_id).await, 0);
        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 8. WP-07b: the parent/child project-scope invariant, and the transitional fail-closed rule
    //    that stands in for the cascade until `move_subtree_nodes_max` is frozen (ADR-0013 §2.2 R17).
    // -----------------------------------------------------------------------------------------

    /// Runs a raw statement and hands back the database's own error instead of panicking on it.
    /// The constraint tests are *about* that error text, so it has to survive to the assertion.
    async fn try_exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> Result<(), sea_orm::DbErr> {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .map(|_| ())
    }

    async fn try_create(
        state: &AppState,
        fx: &Fixture,
        object_type: &str,
        project_id: Option<Uuid>,
        parent: Option<Uuid>,
    ) -> Result<Uuid, ApiError> {
        create_object(
            state,
            CreateObjectInput {
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                object_type: object_type.to_string(),
                project_id,
                parent_object_id: parent,
                title: "Scope Fixture".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .map(|accepted| accepted.object.id)
    }

    /// Writes a `flow_objects` row straight through, bypassing `create_object`. Used to build the
    /// pre-`0056` shapes the constraint is supposed to make impossible.
    async fn try_insert_raw(
        db: &DatabaseConnection,
        fx: &Fixture,
        project_id: Option<Uuid>,
        parent_id: Option<Uuid>,
    ) -> Result<Uuid, sea_orm::DbErr> {
        let id = Uuid::new_v4();
        try_exec(
            db,
            "INSERT INTO flow_objects (id, workspace_id, project_id, object_type, parent_id) \
             VALUES ($1, $2, $3, 'page', $4)",
            vec![id.into(), fx.workspace_id.into(), project_id.into(), parent_id.into()],
        )
        .await
        .map(|()| id)
    }

    async fn scope_violation_count(db: &DatabaseConnection) -> i64 {
        crate::flow::repository::project_scope_violation_count(db)
            .await
            .expect("the invariant monitor view is queryable")
    }

    fn reason_of(err: &ApiError) -> Option<String> {
        let ApiError::Typed { details, .. } = err else {
            return None;
        };
        details
            .as_ref()
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// `ADR-0013` §2.2 R17 step one: the invariant is a database constraint, not an application
    /// convention, so a writer that never goes through `create_object` is bound by it too.
    ///
    /// Both directions of the NULL rule are asserted here rather than left to prose: `project_id`
    /// is nullable and NULL is a *scope* (the unprojected navigator is a real document), so the
    /// rule is strict equality with NULL participating — not "NULL means unspecified, allow it
    /// anywhere". The two rows that would exist under the looser reading (`P -> NULL` and
    /// `NULL -> P`) are exactly the rows asserted to be rejected.
    #[tokio::test]
    async fn the_database_refuses_a_child_in_a_different_project_scope_than_its_parent() {
        let scratch = scratch_or_skip!("scope_constraint");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;

        let root_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let root_unprojected = create(&state, &fx, "navigator", None, None).await;

        // Allowed: a root defines its own scope, in either direction.
        assert_eq!(project_of(&scratch.db, root_a).await, Some(fx.project_a));
        assert_eq!(project_of(&scratch.db, root_unprojected).await, None);

        // Allowed: child scope == parent scope, for both spellings of "a scope".
        let same_project = try_insert_raw(&scratch.db, &fx, Some(fx.project_a), Some(root_a))
            .await
            .expect("a child in its parent's project is legal");
        let both_unprojected = try_insert_raw(&scratch.db, &fx, None, Some(root_unprojected))
            .await
            .expect("an unprojected child of an unprojected parent is legal");

        // Rejected: a different project.
        let err = try_insert_raw(&scratch.db, &fx, Some(fx.project_b), Some(root_a))
            .await
            .expect_err("a child in another project must be refused by the database");
        let text = format!("{err}");
        assert!(
            text.contains("flow_objects_parent_project_fk"),
            "the refusal must come from the invariant's own constraint, got: {text}"
        );

        // Rejected: parent projected, child unprojected. This is the case a plain MATCH SIMPLE
        // composite foreign key on `(workspace_id, parent_id, project_id)` would let through.
        let err = try_insert_raw(&scratch.db, &fx, None, Some(root_a))
            .await
            .expect_err("an unprojected child of a projected parent must be refused");
        assert!(
            format!("{err}").contains("flow_objects_parent_project_fk"),
            "got: {err}"
        );

        // Rejected: parent unprojected, child projected — the mirror image.
        let err = try_insert_raw(&scratch.db, &fx, Some(fx.project_a), Some(root_unprojected))
            .await
            .expect_err("a projected child of an unprojected parent must be refused");
        assert!(
            format!("{err}").contains("flow_objects_parent_project_fk"),
            "got: {err}"
        );

        // Rejected on UPDATE too, not only on INSERT — in both roles of the edge.
        let err = try_exec(
            &scratch.db,
            "UPDATE flow_objects SET project_id = $2 WHERE id = $1",
            vec![same_project.into(), fx.project_b.into()],
        )
        .await
        .expect_err("moving a child out of its parent's scope must be refused");
        assert!(
            format!("{err}").contains("flow_objects_parent_project_fk"),
            "got: {err}"
        );
        let err = try_exec(
            &scratch.db,
            "UPDATE flow_objects SET project_id = $2 WHERE id = $1",
            vec![root_a.into(), fx.project_b.into()],
        )
        .await
        .expect_err("moving a parent out from under its children must be refused");
        assert!(
            format!("{err}").contains("flow_objects_parent_project_fk"),
            "got: {err}"
        );

        // And nothing above left a violation behind.
        assert_eq!(scope_violation_count(&scratch.db).await, 0);
        assert_eq!(project_of(&scratch.db, same_project).await, Some(fx.project_a));
        assert_eq!(project_of(&scratch.db, both_unprojected).await, None);
        scratch.drop_self().await;
    }

    /// The write path answers the same rule with a decidable error rather than letting the
    /// foreign key surface as a 500, and writes nothing when it refuses.
    #[tokio::test]
    async fn create_object_refuses_a_parent_in_a_different_project_scope() {
        let scratch = scratch_or_skip!("scope_create");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;

        let root_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let root_unprojected = create(&state, &fx, "navigator", None, None).await;
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

        for (project, parent, label) in [
            (Some(fx.project_b), root_a, "a child in a different project"),
            (None, root_a, "an unprojected child of a projected parent"),
            (
                Some(fx.project_a),
                root_unprojected,
                "a projected child of an unprojected parent",
            ),
        ] {
            let err = match try_create(&state, &fx, "page", project, Some(parent)).await {
                Ok(id) => panic!("{label} must be refused, but object {id} was created"),
                Err(err) => err,
            };
            assert_eq!(
                err.kind(),
                ApiErrorKind::InvalidUpdate,
                "{label} must be a decidable invalid_update, not a database 500: {err:?}"
            );
            assert_eq!(
                reason_of(&err).as_deref(),
                Some(crate::flow::command::CHILD_PROJECT_MUST_MATCH_PARENT),
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

        // The three legal shapes still work, so the check is a rule and not a blanket refusal.
        let same = try_create(&state, &fx, "page", Some(fx.project_a), Some(root_a))
            .await
            .expect("a child in its parent's project is legal");
        assert_eq!(project_of(&scratch.db, same).await, Some(fx.project_a));
        let unprojected = try_create(&state, &fx, "page", None, Some(root_unprojected))
            .await
            .expect("an unprojected child of an unprojected parent is legal");
        assert_eq!(project_of(&scratch.db, unprojected).await, None);
        let root = try_create(&state, &fx, "page", Some(fx.project_b), None)
            .await
            .expect("a root has no parent to agree with");
        assert_eq!(project_of(&scratch.db, root).await, Some(fx.project_b));

        scratch.drop_self().await;
    }

    /// The transitional fail-closed rule: until `limits-v1.md`'s `move_subtree_nodes_max` is
    /// frozen, `move_object` refuses any move that would leave the subtree spanning more than one
    /// project scope, with the reason code `ADR-0013` §2.2 R17 names.
    #[tokio::test]
    async fn move_object_fails_closed_on_a_subtree_that_would_span_project_scopes() {
        let scratch = scratch_or_skip!("scope_failclosed");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let parent = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let child = create(&state, &fx, "page", Some(fx.project_a), Some(parent)).await;
        let leaf = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;

        let doc_a = document_of(&scratch.db, nav_a).await;
        let doc_b = document_of(&scratch.db, nav_b).await;
        let before = (
            head_seq(&scratch.db, doc_a).await,
            head_seq(&scratch.db, doc_b).await,
            moved_event_count(&scratch.db, fx.workspace_id).await,
            epoch_of(&scratch.db, fx.workspace_id).await,
        );

        // A subtree with descendants cannot cross a project boundary today: cascading is step two
        // of the ruling and its node ceiling is not frozen.
        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(parent, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect_err("a subtree cannot cross a project boundary before the cascade exists");
        assert_eq!(err.kind(), ApiErrorKind::InvalidUpdate, "got {err:?}");
        assert_eq!(
            reason_of(&err).as_deref(),
            Some(super::SUBTREE_SPANS_MULTIPLE_PROJECTS),
            "the refusal must carry the reason code ADR-0013 s2.2 R17 names, got {err:?}"
        );

        // Zero change: no re-parent, no head advance, no event, no epoch bump.
        assert_eq!(parent_of(&scratch.db, parent).await, Some(nav_a), "nothing moved");
        assert_eq!(project_of(&scratch.db, parent).await, Some(fx.project_a));
        assert_eq!(parent_of(&scratch.db, child).await, Some(parent), "no partial cascade");
        assert_eq!(project_of(&scratch.db, child).await, Some(fx.project_a));
        assert_eq!(
            (
                head_seq(&scratch.db, doc_a).await,
                head_seq(&scratch.db, doc_b).await,
                moved_event_count(&scratch.db, fx.workspace_id).await,
                epoch_of(&scratch.db, fx.workspace_id).await,
            ),
            before,
            "the refusal must leave both navigators, the event log and the epoch untouched"
        );
        assert_eq!(update_count(&scratch.db, doc_a).await, 0);
        assert_eq!(update_count(&scratch.db, doc_b).await, 0);

        // The same subtree moved *within* its own scope is unaffected: the rule is about spanning
        // scopes, not about having descendants.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(parent, fx.owner_id, "owner", json!({ "target_object_id": leaf })),
        )
        .await
        .expect("a same-scope move of a subtree is still allowed");
        assert_eq!(parent_of(&scratch.db, parent).await, Some(leaf));

        // And a childless object still crosses freely — one node, one scope, two navigators.
        let single = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(single, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("a leaf crossing a project boundary is the case the cascade is not needed for");
        assert_eq!(project_of(&scratch.db, single).await, Some(fx.project_b));
        scratch.drop_self().await;
    }

    /// Defence in depth: the same refusal on data that already violates the invariant, i.e. rows
    /// that could only exist on a database predating migration `0056`. The constraint is dropped
    /// to build them, then restored — so this asserts the *application* check, independently of
    /// whether the constraint is in place.
    #[tokio::test]
    async fn move_object_fails_closed_on_a_pre_existing_cross_project_subtree() {
        let scratch = scratch_or_skip!("scope_legacy");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let parent = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;

        // The pre-`0056` world, reproduced exactly: with the constraint gone, a child in another
        // project is accepted, which is the defect the cross audit found.
        exec(
            &scratch.db,
            "ALTER TABLE flow_objects DROP CONSTRAINT flow_objects_parent_project_fk",
            vec![],
        )
        .await;
        let stranded = try_insert_raw(&scratch.db, &fx, Some(fx.project_b), Some(parent))
            .await
            .expect("without the constraint the illegal shape is accepted — that is the defect");
        assert_eq!(
            scope_violation_count(&scratch.db).await,
            1,
            "the invariant monitor must see the injected violation, or it is asserting nothing"
        );

        let before_events = moved_event_count(&scratch.db, fx.workspace_id).await;

        // Same scope on both sides of the move, so nothing about the *move* crosses a boundary —
        // the subtree itself is what spans, and that is enough to refuse.
        let sibling = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(parent, fx.owner_id, "owner", json!({ "target_object_id": sibling })),
        )
        .await
        .expect_err("a subtree that already spans scopes must be refused");
        assert_eq!(err.kind(), ApiErrorKind::InvalidUpdate, "got {err:?}");
        assert_eq!(
            reason_of(&err).as_deref(),
            Some(super::SUBTREE_SPANS_MULTIPLE_PROJECTS),
            "got {err:?}"
        );
        assert_eq!(parent_of(&scratch.db, parent).await, Some(nav_a), "nothing moved");
        assert_eq!(project_of(&scratch.db, stranded).await, Some(fx.project_b));
        assert_eq!(moved_event_count(&scratch.db, fx.workspace_id).await, before_events);

        // Repair the data, restore the constraint: it is accepted again, which proves the repaired
        // rows really do satisfy it.
        exec(
            &scratch.db,
            "UPDATE flow_objects SET project_id = $2 WHERE id = $1",
            vec![stranded.into(), fx.project_a.into()],
        )
        .await;
        exec(
            &scratch.db,
            "ALTER TABLE flow_objects ADD CONSTRAINT flow_objects_parent_project_fk \
             FOREIGN KEY (parent_id, project_scope_id) \
             REFERENCES flow_objects (id, project_scope_id) ON DELETE CASCADE",
            vec![],
        )
        .await;
        assert_eq!(scope_violation_count(&scratch.db).await, 0);
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(parent, fx.owner_id, "owner", json!({ "target_object_id": sibling })),
        )
        .await
        .expect("once the subtree is single-scoped again the move is ordinary");
        scratch.drop_self().await;
    }

    /// The existing-data proof as a repeatable check rather than a one-off query: a
    /// freshly-migrated database reports zero violations, and the same monitor reports them when
    /// they are injected (so "zero" is a measurement, not a vacuous truth).
    #[tokio::test]
    async fn the_invariant_monitor_reports_zero_on_a_migrated_database_and_counts_real_violations() {
        let scratch = scratch_or_skip!("scope_monitor");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;

        assert_eq!(
            scope_violation_count(&scratch.db).await,
            0,
            "a database built from migrations/ must start with no invariant violations"
        );

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_unprojected = create(&state, &fx, "navigator", None, None).await;
        for parent in [nav_a, nav_unprojected] {
            let project = project_of(&scratch.db, parent).await;
            let child = create(&state, &fx, "page", project, Some(parent)).await;
            let _ = create(&state, &fx, "page", project, Some(child)).await;
        }
        assert_eq!(
            scope_violation_count(&scratch.db).await,
            0,
            "objects created through the write path never violate the invariant"
        );

        exec(
            &scratch.db,
            "ALTER TABLE flow_objects DROP CONSTRAINT flow_objects_parent_project_fk",
            vec![],
        )
        .await;
        try_insert_raw(&scratch.db, &fx, Some(fx.project_b), Some(nav_a))
            .await
            .expect("the constraint is gone, so the illegal row lands");
        try_insert_raw(&scratch.db, &fx, Some(fx.project_a), Some(nav_unprojected))
            .await
            .expect("the constraint is gone, so the illegal row lands");
        assert_eq!(
            scope_violation_count(&scratch.db).await,
            2,
            "the monitor must count both spellings of a scope mismatch"
        );
        scratch.drop_self().await;
    }
}

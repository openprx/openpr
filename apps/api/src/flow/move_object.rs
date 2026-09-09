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
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event_with_id};

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
/// **Why a cascade does not make it three or more** (`ADR-0013` §2.2 R17, step two): a cross-project
/// move rewrites the `project_id` of the moved object *and every descendant*, so the ordering
/// entries of the whole subtree leave one navigator and arrive in another. The count that matters
/// to this ceiling is documents, not entries — and migration `0056`'s
/// `flow_objects_parent_project_fk` makes "a subtree belongs to exactly one project scope" a
/// database invariant, so the subtree's entries all sat in the *same* source navigator and all
/// land in the *same* target navigator. Two documents, whatever the subtree's size. The number of
/// entries is bounded separately by [`MOVE_SUBTREE_NODES_MAX`], and
/// [`ensure_subtree_is_single_scoped`] keeps defending the premise on databases whose constraint
/// was never applied.
///
/// The locked phase asserts the derived set never exceeds this ceiling, and refuses rather than
/// silently locking more.
pub const MOVE_OBJECT_CONTENDED_DOCUMENT_MAX: u8 = 2;

/// `limits-v1.md`'s `move_subtree_nodes_max`, **frozen at 100 on 2026-08-31** — the number of
/// nodes (the moved object plus its descendants) one cross-project `move_object` may cascade.
///
/// Not invented here and not derivable from anything in this file: the contract froze it from a
/// measured cost curve on a dedicated `PostgreSQL` 16, taking the largest ladder rung whose
/// in-lock hold p95 stayed inside `document_lock_hold_ms_p95_max` (N = 100 → 12.21 ms of 25 ms).
/// `container_count_max` is deliberately **not** reused: that is a single-document live-node
/// ceiling, this is a single-command fan-out ceiling, and the contract rejects substituting one
/// for the other.
///
/// **This command has to enforce it itself**, and the reason is *not* that nothing else would
/// catch an oversized cascade. An earlier version of this comment claimed that, and it was wrong:
/// `hydrate_and_apply` hands the update to `collab_core::isolation::isolated_apply`, whose worker
/// calls `LoroCollabEngine::import_update`, whose **first line** is
/// `InputLimits::default().validate_update(update)?` — an 80,369-byte navigator update really does
/// come back `InputTooLarge { input: "update", actual_bytes: 80369, max_bytes: 65536 }` from
/// there. What is missing downstream is not a ceiling, it is *this* ceiling: nothing counts
/// **nodes**, so a subtree that is under 64 KiB but far past the measured lock-hold budget would
/// sail straight through. `update_bytes_max` is a byte ceiling standing in for a node ceiling, and
/// it stops being a proxy at all once the cascade gets cheaper per node.
/// See [`enforce_move_subtree_nodes_max`].
pub const MOVE_SUBTREE_NODES_MAX: usize = 100;

/// `limits-v1.md`'s frozen `limit_kind` string for [`MOVE_SUBTREE_NODES_MAX`], verbatim.
pub const MOVE_SUBTREE_NODES_LIMIT_KIND: &str = "move_subtree_nodes";

/// `ADR-0012` §3's inheritance-chain depth ceiling (`limits-v1.md`'s frozen `tree_depth_max`),
/// counted in `parent_id` hops with a root at depth 0 — the same counting
/// `authz::ensure_parent_can_adopt_child` uses.
const TREE_DEPTH_MAX: usize = 32;

/// The v0.5 governance command family: server-owned structural/metadata writes rather than
/// caller-authored CRDT content. `MoveObject` is implemented in this module; `Link`/`Unlink` are
/// PostgreSQL-only relation writes implemented by `flow::relations`.
///
/// Deliberately declared here rather than in `flow::command`: the wire name lives with the
/// implementation, and `flow::command` stays the v0.4-frozen content/lifecycle registry it already
/// is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernanceCommandType {
    MoveObject,
    Link,
    Unlink,
}

impl GovernanceCommandType {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "move_object" => Some(Self::MoveObject),
            "link" => Some(Self::Link),
            "unlink" => Some(Self::Unlink),
            _ => None,
        }
    }

    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::MoveObject => "move_object",
            Self::Link => "link",
            Self::Unlink => "unlink",
        }
    }

    /// `events-v1.md`'s registry row for this command.
    #[must_use]
    pub const fn event_type(self) -> &'static str {
        match self {
            Self::MoveObject => "flow.object.moved",
            Self::Link => "flow.relation.linked",
            Self::Unlink => "flow.relation.unlinked",
        }
    }

    /// `ADR-0013` §1's machine-checkable declaration. `BoundedMany`, and the *only* one in the
    /// frozen v0.5 set — `command_contended_document_cardinality` scans every command variant and
    /// fails closed on anything undeclared.
    #[must_use]
    pub const fn existing_document_cardinality(self) -> ExistingDocumentCardinality {
        match self {
            Self::MoveObject => ExistingDocumentCardinality::BoundedMany(MOVE_OBJECT_CONTENDED_DOCUMENT_MAX),
            Self::Link | Self::Unlink => ExistingDocumentCardinality::Zero,
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
    /// The id the `flow.object.moved` row will be inserted under, minted **before** any of this
    /// command's writes run.
    ///
    /// `events-v1.md` requires a command's derived events to carry "直接父 event id" in
    /// `causation_id`, and this command's derived events — one `flow.content.accepted` per
    /// navigator document whose head advances — are written *before* the `flow.object.moved` row
    /// that causes them, because `ADR-0013` §2.2's lock order fixes that sequence and an audit
    /// field is not a reason to reorder a lock-ordered transaction. Pre-minting the parent id is
    /// how the children can name a parent that does not exist yet; `insert_flow_event_with_id`
    /// then inserts the parent under exactly this id.
    ///
    /// Stable across bounded-rebase retries of the same invocation (it is minted once, here), and
    /// discarded wholesale on the `AlreadyCommitted` path, where the whole transaction — derived
    /// rows included — rolls back.
    primary_event_id: Uuid,
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
    /// The moved object's **strict** descendants, ascending `id`, whose `project_id` and ordering
    /// entries travel with it (`ADR-0013` §2.2 R17 step two). Empty unless [`Self::cascades`].
    cascaded_ids: Vec<Uuid>,
    /// Whether this move changes the subtree's project scope at all. A same-scope re-parent
    /// rewrites one row and repositions one entry, exactly as it did before the cascade existed:
    /// no descendant's `project_id` changes, so no descendant's navigator entry moves either.
    cascades: bool,
}

impl MovePlan {
    /// The whole subtree the cascade rewrites, ascending `id` — the moved object plus
    /// [`Self::cascaded_ids`], which is what `move_subtree_nodes_max` counts and what the locked
    /// phase locks. Just the moved object when the move does not cascade.
    fn subtree_ids(&self) -> Vec<Uuid> {
        if !self.cascades {
            return vec![self.object_id];
        }
        let mut ids = self.cascaded_ids.clone();
        ids.push(self.object_id);
        // Load-bearing, and the only reason the locked phase may compare this against
        // `repository::subtree_nodes`' `ORDER BY s.id` result with `!=`: without it the moved
        // object would sit at the end instead of in id order, every cascading move would read as
        // drift, and three attempts later the caller would get `server_draining`. It is also what
        // makes the event envelope's `affected_object_ids[]` deterministic. Nothing about the
        // caller-visible ordering of navigator entries depends on it — that is
        // `observed_cascade_order`'s job, on a deliberately separate value.
        ids.sort_unstable();
        ids
    }
}

/// The ordering entries an object may occupy inside a navigator document all start with the
/// object's UUID: `"<uuid>"` for its first placement there, `"<uuid>#1"`, `"<uuid>#2"`, ... for
/// each later one. This is the parser for that spelling, and the *only* one — the batched index
/// below and every position query go through it, so "which object does this entry belong to" has
/// one answer rather than two that can drift.
///
/// **Why generations exist at all.** A CRDT delete is a tombstone: the engine keeps the id in its
/// `id_to_tree` map forever, so re-creating the same node id fails `DuplicateNode`, and the loro
/// adapter refuses to `mov_to` a deleted node (`"TreeID … is deleted or does not exist"`) — so a
/// tombstoned entry can be neither re-created nor resurrected. Without generations, moving an
/// object out of a scope and later back into it works exactly once and then fails permanently.
/// That is not hypothetical: it is the bug this package's atomicity test caught on its third
/// move.
///
/// The consequence, recorded honestly, and made worse by the cascade: a navigator document
/// accumulates one tombstone per removed entry, and a cross-project move now removes `N` of them
/// in one command instead of one (`limits-v1.md`'s `move_subtree_nodes_max`
/// `tombstone_interaction`). `collab_core::limits`' `container_count_max` counts *live* nodes, so
/// tombstones do not consume that ceiling, but they do grow the document. `limits-v1.md` has no
/// number for it and this package does not invent one; the growth is measured by
/// [`navigator_tombstone_growth_is_measured_on_a_cascading_command`] and carried to the delivery
/// report as an open contract item.
fn entry_object_of(id: &str) -> Option<Uuid> {
    let head = id.get(..36)?;
    let rest = id.get(36..)?;
    let object_id = Uuid::parse_str(head).ok()?;
    if rest.is_empty() {
        return Some(object_id);
    }
    let generation = rest.strip_prefix('#')?;
    if generation.is_empty() || !generation.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(object_id)
}

/// Everything a batched navigator rewrite needs about one snapshot, computed in a single
/// `O(nodes)` pass.
///
/// This exists because the per-node shape it replaces was `O(N^2)`: the previous
/// `build_navigator_update` took one `semantic_snapshot()` *and* one
/// `collab_core::limits::check_operation` per node, and both are `O(nodes)`. Measured on the
/// contract's own ladder (`apps/api/tests/flow_v05_subtree_budget.rs`): N = 100 → 8.11 ms,
/// N = 400 → 135.87 ms, N = 1000 → 863.10 ms per document, against 1.12 / 4.33 / 10.97 ms batched.
/// `limits-v1.md` froze `move_subtree_nodes_max` against the **batched** curve, so this shape is
/// part of what makes the frozen value true rather than an optimisation on top of it.
struct NavigatorIndex {
    /// The one *live* ordering entry each object currently occupies here, if any.
    live_entry: std::collections::HashMap<Uuid, NodeId>,
    /// Live root-level entries — the append index for a new entry, and the length `loro`'s
    /// `mov_to` validates a same-parent reposition against.
    live_roots: u32,
    /// Live `NavigatorNode`s, for `container_count_max`.
    live_navigator_nodes: usize,
    /// Node ids that some node (live or tombstoned) names as its parent. A `MoveNode` of an id
    /// that is in this set can change a descendant's depth and therefore still needs the full
    /// `check_operation`; one that is not cannot, and skipping it is what keeps the batch linear.
    parents: std::collections::HashSet<NodeId>,
}

impl NavigatorIndex {
    fn build(snapshot: &collab_core::SemanticSnapshot) -> Self {
        let mut live_entry = std::collections::HashMap::new();
        let mut parents = std::collections::HashSet::new();
        let mut live_roots = 0u32;
        let mut live_navigator_nodes = 0usize;
        for (id, node) in &snapshot.nodes {
            if let Some(parent) = &node.parent {
                parents.insert(parent.clone());
            }
            if node.deleted {
                continue;
            }
            if node.parent.is_none() {
                live_roots = live_roots.saturating_add(1);
            }
            if node.kind == NodeKind::NavigatorNode {
                live_navigator_nodes = live_navigator_nodes.saturating_add(1);
            }
            if let Some(object_id) = entry_object_of(id) {
                // First writer wins, deterministically: `snapshot.nodes` is a `BTreeMap`, so the
                // iteration order is the entry ids' own order and `"<uuid>"` precedes
                // `"<uuid>#1"`. An object with two live entries in one navigator is already a
                // defect; picking a stable one keeps this function from being the place it turns
                // into non-determinism.
                live_entry.entry(object_id).or_insert_with(|| id.clone());
            }
        }
        Self {
            live_entry,
            live_roots,
            live_navigator_nodes,
            parents,
        }
    }
}

/// A never-before-used entry id for a *new* placement: the bare UUID if this navigator has never
/// held one, else the first free `#n` generation.
///
/// The search is bounded by the snapshot's own node count: with `n` nodes present, at most `n + 1`
/// candidate ids can be taken, so the first free one is always found inside that many probes.
/// There is no arbitrary constant here and therefore no invented limit.
fn fresh_entry_id(snapshot: &collab_core::SemanticSnapshot, object_id: Uuid) -> Result<NodeId, ApiError> {
    let prefix = object_id.to_string();
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

/// One navigator document's whole share of one move: the moved object's entry, plus every
/// descendant's entry when the move crosses a project scope.
struct NavigatorBatch<'a> {
    /// The moved object — the one entry whose position `after_id` governs.
    primary: Uuid,
    /// Its descendants, in the order their entries are appended. Empty unless the move cascades.
    cascaded: &'a [Uuid],
    /// `true` for the navigator the subtree is leaving (entries are removed), `false` for the one
    /// it is joining or repositioning within.
    remove: bool,
    /// Place the moved object's entry immediately after this sibling's entry. Descendants always
    /// append after it; `rest-api-v1.md` gives `after_id` one subject, not `N`.
    after_id: Option<Uuid>,
}

/// What one navigator document's share of a move works out to.
struct NavigatorChange {
    /// The CRDT update bytes, or `None` when this document needs no change at all.
    bytes: Option<Vec<u8>>,
    /// `batch.cascaded`, reordered to the order those objects' ordering entries occupy **in this
    /// document**. Meaningful on the source navigator, where it is the order the target has to
    /// reproduce; ignored elsewhere. Always a permutation of the input.
    cascade_order: Vec<Uuid>,
}

/// Builds the CRDT update bytes that put a whole subtree's ordering entries where this move says
/// they belong, or `None` when this document needs no change at all.
///
/// One `semantic_snapshot()`, one [`NavigatorIndex`], one aggregate limit check, `N` engine
/// operations, one `export_from` — see [`NavigatorIndex`] for the measured reason that shape is
/// mandatory rather than tidy.
///
/// "No change at all" is decided by the *frontier*, not by guessing which operations are no-ops: a
/// `MoveNode` that lands an entry exactly where it already sat may produce no operation in the
/// engine, and shipping an empty update would advance a head with nothing in it.
fn build_navigator_update(
    engine: &mut LoroCollabEngine,
    batch: &NavigatorBatch<'_>,
) -> Result<NavigatorChange, ApiError> {
    let base_frontier = engine.frontier();
    let snapshot = engine.semantic_snapshot().map_err(|err| map_collab_error(&err))?;
    let index = NavigatorIndex::build(&snapshot);
    // Read off the *incoming* snapshot, before a single operation is applied, and returned
    // whichever way this function exits — including the two "no change" exits, because a document
    // that needs no update of its own can still be the one holding the order.
    let cascade_order = observed_cascade_order(&snapshot, &index, batch.cascaded);

    let mut operations: Vec<Operation> = Vec::with_capacity(batch.cascaded.len().saturating_add(1));
    if batch.remove {
        for object_id in std::iter::once(batch.primary).chain(batch.cascaded.iter().copied()) {
            if let Some(entry) = index.live_entry.get(&object_id) {
                operations.push(Operation::DeleteNode { id: entry.clone() });
            }
        }
    } else {
        // Root-level position: one past the requested predecessor, or the end of the list. The
        // engine clamps an out-of-range index itself (`LoroCollabEngine::clamp_index`), so a
        // stale `after_id` degrades to "append" rather than failing the command.
        let mut roots = index.live_roots;
        if let Some(entry) = index.live_entry.get(&batch.primary) {
            operations.push(Operation::MoveNode {
                // Computed over the siblings *excluding this entry*, which is what makes a
                // same-parent reposition legal: loro's `mov_to` takes the node out of the list
                // before re-inserting it, so the valid range is `0..=len-1`, while
                // `LoroCollabEngine::clamp_index` only clamps to `len` — passing `len` fails with
                // "The index(n) should be <= the length of children (n-1)". Excluding the entry
                // makes both the create and the move case use one formula.
                index: position_after(&snapshot, &index, batch.after_id, Some(entry)),
                id: entry.clone(),
                new_parent: None,
            });
        } else {
            operations.push(Operation::CreateNode {
                id: fresh_entry_id(&snapshot, batch.primary)?,
                parent: None,
                index: position_after(&snapshot, &index, batch.after_id, None),
                kind: NodeKind::NavigatorNode,
            });
            roots = roots.saturating_add(1);
        }
        // Descendants append after whatever the moved object's own placement produced. `roots`
        // tracks the live root count the engine will have when each operation is applied, because
        // the snapshot above is from *before* any of them ran and re-snapshotting per node is
        // exactly the quadratic shape this function exists to avoid.
        for object_id in batch.cascaded {
            if let Some(entry) = index.live_entry.get(object_id) {
                operations.push(Operation::MoveNode {
                    index: roots.saturating_sub(1),
                    id: entry.clone(),
                    new_parent: None,
                });
            } else {
                operations.push(Operation::CreateNode {
                    id: fresh_entry_id(&snapshot, *object_id)?,
                    parent: None,
                    index: roots,
                    kind: NodeKind::NavigatorNode,
                });
                roots = roots.saturating_add(1);
            }
        }
    }
    if operations.is_empty() {
        return Ok(NavigatorChange {
            bytes: None,
            cascade_order,
        });
    }

    check_navigator_batch(&snapshot, &index, &operations)?;
    for operation in &operations {
        engine
            .apply_operation(operation)
            .map_err(|err| map_collab_error(&err))?;
    }

    if engine.frontier().as_bytes() == base_frontier.as_bytes() {
        // The engine merged the operations into nothing — every entry was already exactly here.
        return Ok(NavigatorChange {
            bytes: None,
            cascade_order,
        });
    }
    let bytes = engine
        .export_from(&base_frontier)
        .map_err(|err| map_collab_error(&err))?;
    if bytes.is_empty() {
        return Ok(NavigatorChange {
            bytes: None,
            cascade_order,
        });
    }
    // `update_bytes_max`, executed here for **error localisation**, not because the hole it once
    // claimed to plug exists. It does not: this update goes on to
    // `write::hydrate_and_apply` -> `collab_core::isolation::isolated_apply` -> the worker's
    // `LoroCollabEngine::import_update`, and that function's first line is
    // `InputLimits::default().validate_update(update)?` (`crates/collab-core/src/engine.rs`), so an
    // oversized update is already refused one layer down. What differs is *what the caller gets*:
    // from here it is a decidable `limit_exceeded{limit_kind:"update_bytes"}` naming this command's
    // own input; from the worker it is a rejection about an opaque byte blob the caller never
    // supplied. Defence in depth with a better error, and the cheaper check first.
    //
    // At the frozen `move_subtree_nodes_max` neither can fire (the contract measured 8,122 B at
    // N = 100 against 65,536 — 8.07x of headroom, which is why the *binding* ceiling is the lock
    // budget and not this one).
    let observed = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if observed > collab_limits::UPDATE_BYTES_MAX {
        return Err(ApiError::limit_exceeded(
            "the navigator ordering update this move produces is larger than the frozen update ceiling",
            "update_bytes",
            Some(json!(collab_limits::UPDATE_BYTES_MAX)),
            Some(json!(observed)),
            None,
        ));
    }
    Ok(NavigatorChange {
        bytes: Some(bytes),
        cascade_order,
    })
}

/// `objects` reordered to match the order their ordering entries occupy in this navigator.
///
/// `rest-api-v1.md` (2026-08-31 裁定): "`after_id` 只定位被移动对象本身……被级联的后代之间，
/// 必须保持它们在源 navigator 里的相对顺序，不得按 id 或任何其它规则重新推导". That order is what
/// a user looking at the tree sees, so re-deriving it during a cross-project move would silently
/// reshuffle a subtree nobody asked to reshuffle.
///
/// Objects with no live root-level entry here have no order to preserve; they keep their incoming
/// relative order, after the ones that do. The incoming order is ascending `id`
/// (`repository::subtree_nodes`' `ORDER BY s.id`), so the result is total and deterministic in
/// every case — including a navigator that holds no entries for this subtree at all, which is what
/// the very first cross-project move of a freshly created subtree looks like.
fn observed_cascade_order(
    snapshot: &collab_core::SemanticSnapshot,
    index: &NavigatorIndex,
    objects: &[Uuid],
) -> Vec<Uuid> {
    let mut placed: Vec<(&str, Uuid)> = Vec::with_capacity(objects.len());
    let mut unplaced: Vec<Uuid> = Vec::new();
    for object_id in objects {
        match index
            .live_entry
            .get(object_id)
            .and_then(|entry| snapshot.nodes.get(entry))
            .filter(|node| node.parent.is_none())
        {
            Some(node) => placed.push((node.order_key.as_str(), *object_id)),
            None => unplaced.push(*object_id),
        }
    }
    // `sort_by` is stable, so two entries that somehow share an `order_key` keep the incoming
    // ascending-`id` order rather than swapping unpredictably between runs.
    placed.sort_by(|left, right| left.0.cmp(right.0));
    placed.into_iter().map(|(_, id)| id).chain(unplaced).collect()
}

/// The batched replacement for one `collab_core::limits::check_operation` per node.
///
/// Two ceilings can actually be reached by the operations [`build_navigator_update`] emits, and
/// each is decided once for the whole batch instead of once per node:
///
/// * `container_count_max` — every emitted `CreateNode` is a root-level `NavigatorNode`, so the
///   post-batch live count is simply the pre-batch count plus the number of creates. Checking it
///   per node would also be *wrong* in the same direction the contract cares about: each call
///   would compare `live + 1` against the ceiling and a 100-entry batch would sail past it.
/// * `tree_depth_max` — a `CreateNode` with `parent: None` lands at depth 0 and cannot breach it,
///   and a `MoveNode` to `new_parent: None` lands at depth 0 too, so the only way one can breach
///   it is by carrying a subtree down with it. Navigator ordering entries are flat, so
///   `index.parents` is normally empty of them and the loop below does nothing; when a document
///   does hold a nested node, that node's own move is handed to the real `check_operation`.
///   `DeleteNode` has no ceiling at all (`check_operation` returns `Ok` for it unconditionally).
///
/// **What the `index.parents` guard is and is not**, written down rather than left as a line no
/// test can see. It *runs* whenever a navigator holds a nested node — navigator documents are
/// ordinary collab documents and nothing forbids that — so it is not dead. But it cannot *fail*
/// on a document that already satisfies `tree_depth_max`: the operation moves the entry to the
/// root, so `deepest_after_move = 0 + subtree_height(entry)`, and in a valid document an entry at
/// depth `d` has `d + height <= 32`, hence `height <= 32`. A move to the root only ever reduces
/// depths. It is therefore defence in depth against a document that is **already** over the
/// ceiling, and `a_navigator_entry_carrying_an_over_deep_subtree_is_refused_by_the_batched_depth_check`
/// builds exactly that document so the branch is falsifiable rather than merely argued about.
/// The guard is kept rather than replaced by an unconditional call because `check_operation`'s
/// `MoveNode` arm is `O(nodes)` (it rebuilds a children index per call), and dropping the guard
/// would make a batch of `N` repositions `O(N * nodes)`.
fn check_navigator_batch(
    snapshot: &collab_core::SemanticSnapshot,
    index: &NavigatorIndex,
    operations: &[Operation],
) -> Result<(), ApiError> {
    let limits = collab_limits::document_limits();
    let created = operations
        .iter()
        .filter(|operation| {
            matches!(
                operation,
                Operation::CreateNode {
                    kind: NodeKind::NavigatorNode,
                    ..
                }
            )
        })
        .count();
    if created > 0 {
        let observed = index.live_navigator_nodes.saturating_add(created);
        if observed > limits.container_count_max {
            return Err(map_collab_error(&collab_core::CollabError::from(
                collab_core::limits::LimitViolation {
                    limit_kind: "container_count",
                    limit: u64::try_from(limits.container_count_max).unwrap_or(u64::MAX),
                    observed: u64::try_from(observed).unwrap_or(u64::MAX),
                },
            )));
        }
    }
    for operation in operations {
        if let Operation::MoveNode { id, .. } = operation
            && index.parents.contains(id)
        {
            collab_core::limits::check_operation(snapshot, operation, &limits)
                .map_err(|violation| map_collab_error(&collab_core::CollabError::from(violation)))?;
        }
    }
    Ok(())
}

/// Index for a new/moved root-level navigator entry: immediately after `after_id`'s entry, or at
/// the end of the root list.
///
/// `exclude` is the entry being repositioned, if it is already in this list — see
/// [`build_navigator_update`] for why leaving it in produces an out-of-range index for a
/// same-parent move.
fn position_after(
    snapshot: &collab_core::SemanticSnapshot,
    index: &NavigatorIndex,
    after_id: Option<Uuid>,
    exclude: Option<&NodeId>,
) -> u32 {
    let mut roots: Vec<(&str, &NodeId)> = snapshot
        .nodes
        .iter()
        .filter(|(id, node)| node.parent.is_none() && !node.deleted && Some(*id) != exclude)
        .map(|(id, node)| (node.order_key.as_str(), id))
        .collect();
    roots.sort_unstable();
    let end = u32::try_from(roots.len()).unwrap_or(u32::MAX);
    let Some(after_id) = after_id else { return end };
    let Some(wanted) = index.live_entry.get(&after_id) else {
        return end;
    };
    roots
        .iter()
        .position(|(_, id)| *id == wanted)
        .and_then(|index| u32::try_from(index.saturating_add(1)).ok())
        .unwrap_or(end)
}

/// Hydrates one navigator document outside any lock, computes its ordering change, and prepares
/// the write through the *same* [`write::hydrate_and_apply`] every content write uses — isolated
/// apply, resource ceilings, projection prepare and all.
async fn plan_document(
    ctx: &MoveContext<'_>,
    document_id: Uuid,
    batch: &NavigatorBatch<'_>,
    expected_frontier: Option<&[u8]>,
) -> Result<(DocumentPlan, Vec<Uuid>), ApiError> {
    let input = ctx.input;
    let boot = bootstrap::load(&ctx.state.db, document_id).await?;
    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|err| map_collab_error(&err))?;
    for tail in &boot.tail_updates {
        engine
            .import_update(&tail.bytes)
            .map_err(|err| map_collab_error(&err))?;
    }

    let NavigatorChange { bytes, cascade_order } = build_navigator_update(&mut engine, batch)?;
    let Some(bytes) = bytes else {
        // Locked but not advanced. `boot.head_seq` is the head the "no change" conclusion was
        // drawn against, so the locked phase can still detect a concurrent writer.
        return Ok((
            DocumentPlan::Locked {
                document_id,
                observed_head_seq: boot.head_seq,
            },
            cascade_order,
        ));
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
        actor_is_bot: input.actor_is_bot(),
        workspace_id: ctx.workspace_id,
        checked_epoch: ctx.checked_epoch,
        expected_frontier: expected_frontier.map(<[u8]>::to_vec),
        // `events-v1.md`: "由 command ... 导出的下一事件把直接父 event id 写 `causation_id` 并
        // 继承 correlation". This navigator head advance exists *because* of the move, so its
        // `flow.content.accepted` is a derived event: same surface (the caller's, not a literal),
        // same correlation as every other event this request writes, and a `causation_id` naming
        // the `flow.object.moved` event — `ctx.primary_event_id`, which this transaction inserts
        // that event under further down.
        origin: input.origin.derived_from(ctx.primary_event_id),
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
        write::HydrateOutcome::Prepared(prepared) => Ok((
            DocumentPlan::Advance {
                document_id,
                request: Box::new(request),
                prepared,
            },
            cascade_order,
        )),
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
    // [layer 2, cascade] the rest of the subtree, ascending `id`, in one statement.
    //
    // Taken *before* the re-derivation below and not after: `FOR UPDATE` on a row conflicts with
    // the `FOR KEY SHARE` a concurrent `create_object` takes on its parent through the foreign
    // key, so once these rows are held the subtree cannot gain a member under any of them. Locking
    // after re-deriving would leave exactly that window open, and the whole point of the
    // re-derivation is to close it.
    let subtree_ids = plan.subtree_ids();
    if plan.cascades {
        let locked_subtree = repository::lock_objects_for_update(tx, &subtree_ids).await?;
        if locked_subtree.len() != subtree_ids.len() {
            return Ok(LockedOutcome::Drift("part of the moved subtree disappeared"));
        }
        // No `lifecycle_status` filter, and that is a decision rather than an omission: archiving
        // is a flag flip that leaves `parent_id`/`project_id` alone, so an archived descendant is
        // still a subtree member the composite foreign key applies to. Skipping it would make
        // every cascade over a subtree containing one archived page fail the constraint.

        // Membership re-derived on this transaction's own snapshot, for the same reason
        // `derive_contended_set` is re-derived below: the unlocked prepare ran before this
        // transaction existed. A subtree that grew past the ceiling in that window is a *refusal*,
        // not a retry — retrying would re-prepare and re-refuse — so the limit is re-enforced here
        // rather than reported as drift.
        let probe = i64::try_from(TREE_DEPTH_MAX.saturating_add(1)).unwrap_or(i64::MAX);
        let relocked = repository::subtree_nodes(
            tx,
            plan.workspace_id,
            plan.object_id,
            probe,
            i64::try_from(MOVE_SUBTREE_NODES_MAX).unwrap_or(i64::MAX),
        )
        .await?;
        enforce_move_subtree_nodes_max(relocked.total)?;
        if relocked.ids != subtree_ids {
            return Ok(LockedOutcome::Drift("the moved subtree changed shape concurrently"));
        }

        // The cascade's premise, re-decided on the rows this transaction holds rather than on a
        // second read: every strict descendant must already sit in the moved object's scope, or
        // its ordering entries are not all in the one navigator this command is about to empty.
        // Deliberately over *every* locked row, the moved object included: its own scope was
        // already compared against the plan above, so including it is free, and excluding it would
        // be a filter no test could falsify.
        refuse_if_multi_scoped(locked_subtree.iter().map(|row| row.project_id), source.project_id)?;
    } else {
        // A same-scope re-parent rewrites one row and cascades nothing, so there is no subtree to
        // lock and no ceiling to re-enforce. The premise is still checked: a subtree that already
        // spans scopes is a known inconsistency whether or not this command widens it.
        ensure_subtree_is_single_scoped(tx, plan.workspace_id, plan.object_id, source.project_id).await?;
    }

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
        match write::stage_one_document(tx, request, prepared, dispatch_max_attempts).await? {
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

    // The governance write itself — the re-parent and the whole subtree's scope rewrite, in
    // **one** statement. `flow_objects_parent_project_fk` is `NOT DEFERRABLE`, so a two-statement
    // split fails at the end of the first one with the root already in the new scope and its
    // children still in the old; see `repository::cascade_move_subtree`.
    let rewritten = repository::cascade_move_subtree(
        tx,
        &subtree_ids,
        plan.object_id,
        plan.target_object_id,
        target.project_id,
        crate::flow::command::actor_user_id(input.actor_id, input.actor_is_bot()),
    )
    .await?;
    if rewritten != u64::try_from(subtree_ids.len()).unwrap_or(u64::MAX) {
        // The set was locked `FOR UPDATE` above, so this cannot happen; refused rather than
        // assumed away, because a cascade that silently rewrote fewer rows than it locked is
        // precisely the data inconsistency `ADR-0013` §2.2 R17 exists to remove.
        tracing::error!(
            expected = subtree_ids.len(),
            rewritten,
            "move_object: the cascade rewrote a different number of rows than it locked"
        );
        return Err(ApiError::Internal);
    }

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
    //
    // `insert_flow_event_with_id` rather than `insert_flow_event`: the derived
    // `flow.content.accepted` rows staged above already name this id as their `causation_id`, so the
    // parent has to land under the id they were told about. See `MoveContext::primary_event_id`.
    let outcome = insert_flow_event_with_id(
        tx,
        ctx.primary_event_id,
        BusinessEventInput {
            workspace_id: plan.workspace_id,
            project_id: target.project_id,
            event_type: GovernanceCommandType::MoveObject.event_type().to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: plan.object_id.to_string(),
            actor_id: crate::flow::command::actor_user_id(input.actor_id, input.actor_is_bot()),
            source: input.origin.source_json(),
            payload: json!({
                "object_id": plan.object_id,
                "old_parent_id": plan.source_parent_id,
                "new_parent_id": plan.target_object_id,
                "position_key": payload.after_id,
            }),
            // `events-v1.md`'s envelope carries `metadata.affected_object_ids[]`, and the
            // 2026-08-31 ruling puts the cascade there rather than in the payload: the frozen
            // `flow.object.moved` payload names only the object that was asked to move, so without
            // this **nothing in the audit stream records that N descendants changed project**.
            // `affected_object_ids` is an envelope field that exists for exactly this and
            // cascading archive is the same pattern (`ADR-0012` §2).
            //
            // The set is exactly the `flow_objects` rows this transaction rewrote — the moved
            // object plus every cascaded descendant, ascending `id`, bounded by
            // `move_subtree_nodes_max`. The two navigator objects are deliberately **not** in it:
            // their governance rows were not touched, only their document heads advanced, and each
            // of those advances already emits its own `flow.content.accepted` carrying that
            // document's `object_id`. One fact, one event, no overlap.
            //
            // No count field here on purpose (same ruling): a count is a consequence of the set,
            // and recording it twice only creates two truths that can disagree. `command_result`
            // carries `cascaded_node_count` for the caller.
            metadata: json!({
                "message": input.message,
                "affected_object_ids": subtree_ids,
            }),
            // The primary event of a first user request: it roots the correlation every derived
            // `flow.content.accepted` above inherits, and its own `causation_id` is whatever
            // caused the *command* (`None` for a first request, a parent event id when a job or
            // retry issued it).
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

/// `details.reason` on the refusal `ADR-0013` §2.2 R17 and `rest-api-v1.md`'s `move_object`
/// clause both spell out by name.
pub const SUBTREE_SPANS_MULTIPLE_PROJECTS: &str = "subtree_spans_multiple_projects";

/// The cascade's premise, checked rather than assumed (`ADR-0013` §2.2 R17).
///
/// The ruling has two steps. Step one — migration `0056`'s `flow_objects_parent_project_fk` —
/// makes "a subtree belongs to one project scope" a database invariant. Step two, implemented
/// here, cascades the subtree's `project_id` and ordering entries with the moved object, and it is
/// **only** correct because of step one: the whole subtree's entries sit in one navigator before
/// the move and one navigator after, which is what keeps
/// [`MOVE_OBJECT_CONTENDED_DOCUMENT_MAX`] true.
///
/// So this is no longer a transitional refusal, it is defence in depth on the premise. The
/// predicate is one statement about the subtree **as it stands, before the move**: every strict
/// descendant's scope must equal the moved object's own. `ADR-0013` §2.2's decisive counterexample
/// (`P1 root → P2 child → P3 grandchild`) was a legal shape before `0056`, and this check must not
/// depend on the constraint having been applied to the database it is running against — cascading
/// such a subtree would touch three or four navigators and silently break the declared
/// `bounded_many` ceiling.
///
/// It is checked on every move, cascading or not: a subtree that already spans scopes is a known
/// data inconsistency whether or not this particular command would widen it, and the contract
/// rejects proceeding past it either way.
///
/// `None` participates as a scope value, matching the invariant's own NULL semantics.
async fn ensure_subtree_is_single_scoped<C: sea_orm::ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
    source_project_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let probe = i64::try_from(TREE_DEPTH_MAX.saturating_add(1)).unwrap_or(i64::MAX);
    let scopes = repository::descendant_project_scopes(conn, workspace_id, object_id, probe).await?;
    refuse_if_multi_scoped(scopes.into_iter(), source_project_id)
}

/// The predicate itself, over any iterator of descendant scopes, so the unlocked path (a recursive
/// query) and the locked path (the rows it already holds `FOR UPDATE`) decide it with the *same*
/// code instead of two copies that can disagree.
fn refuse_if_multi_scoped(
    descendant_scopes: impl Iterator<Item = Option<Uuid>>,
    source_project_id: Option<Uuid>,
) -> Result<(), ApiError> {
    for scope in descendant_scopes {
        if scope != source_project_id {
            return Err(ApiError::invalid_update_with_details(
                "this subtree already spans more than one project scope, so its ordering entries \
                 are not all in one navigator and it cannot be cascaded",
                json!({ "reason": SUBTREE_SPANS_MULTIPLE_PROJECTS }),
            ));
        }
    }
    Ok(())
}

/// `limits-v1.md`'s `move_subtree_nodes_max`, executed by this command because nothing else will
/// (see [`MOVE_SUBTREE_NODES_MAX`]).
///
/// Called twice per attempt and both times *before* anything is written: once outside the
/// transaction, so the refusal costs zero rows, zero head advances, zero events and zero dispatch;
/// once inside it on the transaction's own snapshot, because a concurrent `create_object` can add
/// a node to the subtree between the two.
fn enforce_move_subtree_nodes_max(total: i64) -> Result<(), ApiError> {
    if total > i64::try_from(MOVE_SUBTREE_NODES_MAX).unwrap_or(i64::MAX) {
        return Err(ApiError::limit_exceeded(
            "this move would cascade more subtree nodes than the frozen ceiling allows",
            MOVE_SUBTREE_NODES_LIMIT_KIND,
            Some(json!(MOVE_SUBTREE_NODES_MAX)),
            Some(json!(total)),
            None,
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
        primary_event_id: Uuid::new_v4(),
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

    // `ADR-0013` §2.2 R17 step two: a scope change takes the whole subtree with it, a re-parent
    // inside one scope takes nothing. Everything the cascade costs — the extra rows, the extra
    // ordering entries, the extra tombstones, the frozen node ceiling — hangs off this one
    // comparison, so it is made once and carried in the plan.
    let cascades = source.project_id != target.project_id;
    let mut plan = MovePlan {
        workspace_id,
        object_id: input.object_id,
        target_object_id: payload.target_object_id,
        source_parent_id: source.parent_id,
        source_project_id: source.project_id,
        target_project_id: target.project_id,
        target_chain,
        document_lock_order,
        navigator_object_ids,
        cascaded_ids: Vec::new(),
        cascades,
    };
    check_cycle_and_depth(&state.db, &plan, &plan.target_chain).await?;
    ensure_subtree_is_single_scoped(&state.db, workspace_id, plan.object_id, source.project_id).await?;

    if cascades {
        // Outside the transaction, before the coordinator: a refusal here has touched nothing at
        // all. `subtree_nodes` truncates the id list at the ceiling but reports the real total, so
        // `details.observed` is the subtree's actual size rather than the cap read back.
        let probe = i64::try_from(TREE_DEPTH_MAX.saturating_add(1)).unwrap_or(i64::MAX);
        let subtree = repository::subtree_nodes(
            &state.db,
            workspace_id,
            plan.object_id,
            probe,
            i64::try_from(MOVE_SUBTREE_NODES_MAX).unwrap_or(i64::MAX),
        )
        .await?;
        enforce_move_subtree_nodes_max(subtree.total)?;
        plan.cascaded_ids = subtree.ids.into_iter().filter(|id| *id != plan.object_id).collect();
    }
    let plan = plan;

    // [layer 0] every contended document's admission slot, ascending, before anything else.
    let Ok(_permits) = collab.coordinator.acquire_many(&plan.document_lock_order).await else {
        return Err(ApiError::server_draining(
            crate::error::ServerDrainingReason::Contention,
            200,
            "server_draining",
        ));
    };

    // Hydration order, which is **not** lock order: locking still walks
    // `plan.document_lock_order` (ascending `document_id`, `ADR-0013` §2.1) inside the
    // transaction, while this only decides which document is read first, outside every lock. The
    // source navigator goes first because it is where the subtree's current ordering lives and the
    // target has to reproduce it. `sort_by_key` is stable, so everything else keeps lock order.
    let source_only_document = match (source_navigator, target_navigator) {
        (Some(source), target) if Some(source) != target => Some(source),
        _ => None,
    };
    let mut prepare_order = plan.document_lock_order.clone();
    prepare_order.sort_by_key(|document_id| u8::from(Some(*document_id) != source_only_document));

    let mut attempts = 0u32;
    loop {
        attempts += 1;

        // The order the cascaded entries are appended to the target navigator in. It starts as
        // ascending `id` — `plan.cascaded_ids`' own order, and the only order available when the
        // source scope has no navigator or holds no entries for this subtree — and is replaced by
        // the source navigator's real order as soon as that document has been hydrated below.
        //
        // `plan.cascaded_ids` itself is never reordered, but **not** because the drift check
        // depends on its order — it does not: `MovePlan::subtree_ids` sorts before comparing, and
        // that `sort_unstable` is the line actually holding the comparison together. The reason is
        // narrower and worth stating plainly: `plan` is computed once and reused across every
        // rebase attempt, while this display order is re-derived *per attempt* from whatever the
        // source navigator looks like now. Writing an attempt's finding back into the plan would
        // make the plan attempt-dependent, which is exactly what a rebase must not carry over.
        let mut cascade_order: Vec<Uuid> = plan.cascaded_ids.clone();
        let mut plans: Vec<DocumentPlan> = Vec::with_capacity(plan.document_lock_order.len());
        for document_id in &prepare_order {
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
            let (document_plan, observed) = {
                let batch = NavigatorBatch {
                    primary: plan.object_id,
                    cascaded: &cascade_order,
                    remove: is_source_only,
                    after_id: payload.after_id,
                };
                plan_document(&ctx, *document_id, &batch, expected).await?
            };
            plans.push(document_plan);
            if is_source_only {
                // `observed` is a permutation of what was passed in, so this cannot add, drop or
                // invent a member — only reorder.
                cascade_order = observed;
            }
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
                match super::collab::permission_cache::PermissionCache::for_state(state) {
                    Ok(cache) => {
                        cache.invalidate_workspace(workspace_id);
                    }
                    Err(err) => {
                        tracing::warn!(object_id = %plan.object_id, %workspace_id, %err, "permission cache unavailable after move commit");
                    }
                }
                let revocation_stats = super::collab::revocation::revalidate_authorization_change_with_registry(
                    state,
                    &collab.registry,
                    workspace_id,
                    committed_epoch,
                )
                .await;
                tracing::debug!(
                    object_id = %plan.object_id,
                    %workspace_id,
                    ?revocation_stats,
                    "authorization sessions re-evaluated after move commit"
                );
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
    // `rest-api-v1.md`: a cross-object command "返回全部 `affected_object_ids`". The response is
    // wider than the event's envelope set on purpose — it also names the two navigator objects,
    // because the caller needs to know which documents' heads it should expect to have moved,
    // which is a client concern rather than an audit fact.
    //
    // De-duplicated by identity rather than with `Vec::dedup`, which only collapses *consecutive*
    // repeats: "move to the navigator root" makes `target_object_id` equal to one of the navigator
    // objects, and those two land in non-adjacent positions.
    let mut seen = std::collections::HashSet::new();
    let affected: Vec<Uuid> = [plan.object_id, plan.target_object_id]
        .into_iter()
        .chain(plan.navigator_object_ids.iter().copied())
        .chain(plan.cascaded_ids.iter().copied())
        .filter(|object_id| seen.insert(*object_id))
        .collect();
    change.affected_object_ids = affected;
    change.command_result = Some(json!({
        "command": GovernanceCommandType::MoveObject.wire_name(),
        "existing_document_cardinality": "bounded_many",
        "existing_document_cardinality_max": MOVE_OBJECT_CONTENDED_DOCUMENT_MAX,
        "contended_existing_document_set": plan.document_lock_order,
        "document_lock_order": observed_lock_order,
        "old_parent_id": plan.source_parent_id,
        "new_parent_id": plan.target_object_id,
        // `ADR-0013` §2.2 R17 step two, made observable to the caller. The *audit* record of the
        // cascade is not here — it is the event envelope's `affected_object_ids[]`, written where
        // the `flow.object.moved` event is inserted. This count is deliberately **not** duplicated
        // into the event (2026-08-31 ruling): a count is a consequence of that set, and recording
        // it in two places only creates two truths that can disagree. The frozen
        // `flow.object.moved` payload (`object_id,old_parent_id?,new_parent_id?,position_key?`) is
        // likewise untouched — `flow::event_policy`'s allow-list is what a delivery is filtered
        // through, so adding a field there would be a contract change rather than extra evidence.
        "cascaded": plan.cascades,
        "cascaded_node_count": plan.cascaded_ids.len().saturating_add(1),
        "move_subtree_nodes_max": MOVE_SUBTREE_NODES_MAX,
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
    use super::{
        GovernanceCommandType, MOVE_OBJECT_CONTENDED_DOCUMENT_MAX, MOVE_SUBTREE_NODES_MAX, NavigatorIndex,
        entry_object_of, position_after,
    };
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

        let index = NavigatorIndex::build(&snapshot);
        assert_eq!(
            position_after(&snapshot, &index, None, None),
            2,
            "no predecessor means append"
        );
        assert_eq!(position_after(&snapshot, &index, Some(first), None), 1);
        assert_eq!(position_after(&snapshot, &index, Some(second), None), 2);
        assert_eq!(
            position_after(&snapshot, &index, Some(Uuid::from_u128(99)), None),
            2,
            "a stale after_id degrades to append rather than failing the command"
        );

        // Repositioning an existing entry: the entry itself must not be counted, or the index
        // lands one past what a same-parent `MoveNode` accepts.
        let moving: super::NodeId = Arc::from(second.to_string());
        assert_eq!(
            position_after(&snapshot, &index, None, Some(&moving)),
            1,
            "appending a repositioned entry must index into the list without it"
        );
        assert_eq!(position_after(&snapshot, &index, Some(first), Some(&moving)), 1);
    }

    /// The entry-id parser, which is the seam the batched index hangs off: every lookup of "which
    /// object does this navigator entry belong to" goes through it, so a wrong answer here is a
    /// cascade that silently skips or steals an entry rather than a compile error.
    #[test]
    fn entry_ids_are_parsed_back_to_their_object_and_nothing_else_is() {
        let object_id = Uuid::from_u128(7);
        let text = object_id.to_string();
        assert_eq!(entry_object_of(&text), Some(object_id), "the bare uuid is generation 0");
        assert_eq!(entry_object_of(&format!("{text}#1")), Some(object_id));
        assert_eq!(entry_object_of(&format!("{text}#42")), Some(object_id));

        assert_eq!(entry_object_of(&format!("{text}#")), None, "an empty generation");
        assert_eq!(entry_object_of(&format!("{text}#a")), None, "a non-numeric generation");
        assert_eq!(entry_object_of(&format!("{text}x")), None, "a missing separator");
        assert_eq!(
            entry_object_of(&format!("{text}1")),
            None,
            "a digit without a separator"
        );
        assert_eq!(entry_object_of("block-1"), None, "a content block id is not an entry");
        assert_eq!(entry_object_of(""), None);
        // Not a panic: `get(..36)` on a shorter or non-char-boundary string yields `None`.
        assert_eq!(entry_object_of("短"), None, "a multi-byte prefix must not index-slice");
    }

    /// One pass, four answers. The index is what makes the cascade linear, so each field it
    /// reports is asserted against a snapshot whose shape makes a wrong answer visible.
    #[test]
    fn the_navigator_index_reports_live_entries_roots_and_parents_from_one_pass() {
        let mut snapshot = SemanticSnapshot::default();
        let live = Uuid::from_u128(1);
        let tombstoned = Uuid::from_u128(2);
        let regenerated = Uuid::from_u128(3);

        snapshot.nodes.insert(Arc::from(live.to_string()), node("0000000000"));
        let mut dead = node("0000000001");
        dead.deleted = true;
        snapshot.nodes.insert(Arc::from(tombstoned.to_string()), dead);
        let mut old_generation = node("0000000002");
        old_generation.deleted = true;
        snapshot
            .nodes
            .insert(Arc::from(regenerated.to_string()), old_generation);
        snapshot
            .nodes
            .insert(Arc::from(format!("{regenerated}#1")), node("0000000003"));
        let mut child = node("0000000004");
        child.parent = Some(Arc::from(live.to_string()));
        snapshot.nodes.insert(Arc::from("nested-child"), child);

        let index = NavigatorIndex::build(&snapshot);
        assert_eq!(
            index.live_entry.get(&live).map(ToString::to_string),
            Some(live.to_string())
        );
        assert_eq!(
            index.live_entry.get(&tombstoned),
            None,
            "a tombstone is not a live entry"
        );
        assert_eq!(
            index.live_entry.get(&regenerated).map(ToString::to_string),
            Some(format!("{regenerated}#1")),
            "the live generation wins over the tombstoned one"
        );
        assert_eq!(index.live_roots, 2, "one live entry plus one live regenerated entry");
        assert_eq!(index.live_navigator_nodes, 3, "roots plus the live nested child");
        assert!(
            index.parents.contains(&Arc::from(live.to_string()) as &super::NodeId),
            "an entry with a child must be recognised as a parent, or its MoveNode skips the depth check"
        );
        assert!(
            !index
                .parents
                .contains(&Arc::from(tombstoned.to_string()) as &super::NodeId)
        );
    }

    /// The byte ceiling this command executes for itself, shown firing.
    ///
    /// It is **not** the only one behind a server-generated navigator update, and an earlier
    /// version of this comment wrongly said it was: the same update goes on to
    /// `isolated_apply` -> `LoroCollabEngine::import_update`, which validates
    /// `update_bytes_max` on its first line. This one exists so the caller gets a decidable
    /// `limit_exceeded{update_bytes}` about its own command instead of a worker rejection about an
    /// opaque blob, and so the cheap check runs before a process is spawned.
    ///
    /// It cannot be reached through the command itself — `move_subtree_nodes_max` (100) refuses
    /// first, and 100 entries is about 8 KB — which is precisely why it is exercised here at the
    /// function boundary instead of being asserted to be unreachable.
    ///
    /// The **target** side is the expensive one and the one this uses: `limits-v1.md` measured
    /// 82.6 bytes per created entry (`bytes ~ 82.6 * N - 170`, over 65,536 near N = 795), while a
    /// removal is roughly 11 bytes per entry and would not cross the ceiling until far past any
    /// size this command can produce. 1,000 created entries produce 80,369 bytes on this engine,
    /// within 2.5% of the contract's model.
    #[test]
    fn an_oversized_navigator_update_is_refused_by_the_byte_ceiling_this_command_owns() {
        use collab_core::{CollabEngine, LoroCollabEngine};

        let objects: Vec<Uuid> = (0..1_000).map(|_| Uuid::new_v4()).collect();
        let empty = LoroCollabEngine::new_empty(9)
            .export_snapshot()
            .expect("an empty navigator exports");
        let mut engine = LoroCollabEngine::load(&empty).expect("it loads back");

        let err = super::build_navigator_update(
            &mut engine,
            &super::NavigatorBatch {
                primary: objects[0],
                cascaded: &objects[1..],
                remove: false,
                after_id: None,
            },
        )
        .map(|change| change.bytes.map(|bytes| bytes.len()))
        .expect_err("an update past update_bytes_max must be refused, not shipped");
        let crate::error::ApiError::Typed { kind, details, .. } = &err else {
            panic!("expected a typed limit_exceeded, got {err:?}")
        };
        assert_eq!(*kind, crate::error::ApiErrorKind::LimitExceeded, "got {err:?}");
        let details = details.clone().unwrap_or(serde_json::Value::Null);
        assert_eq!(
            details["limit_kind"],
            serde_json::json!("update_bytes"),
            "got {details}"
        );
        assert_eq!(details["limit"], serde_json::json!(65_536), "got {details}");
        assert!(
            details["observed"].as_u64().unwrap_or(0) > 65_536,
            "the reported size must be the real one, got {details}"
        );
    }

    /// The one branch of [`check_navigator_batch`] that is not exercised by any move a caller can
    /// make, shown firing at the function boundary.
    ///
    /// `check_navigator_batch` hands a `MoveNode` to the real `collab_core::limits::check_operation`
    /// only when the entry being moved is somebody's parent (`index.parents`). Two facts about that
    /// branch, both worth stating because "unreachable" was not an acceptable answer for it:
    ///
    /// * **It runs.** Navigator documents are ordinary collab documents; nothing stops a writer
    ///   from nesting a node under an ordering entry, and then `index.parents` is non-empty and
    ///   the guarded call happens on every reposition of that entry.
    /// * **It cannot *fail* on a document that satisfies `tree_depth_max`.** The operation moves
    ///   the entry to `new_parent: None`, so `check_operation` computes
    ///   `deepest_after_move = 0 + subtree_height(entry)`. In a valid document an entry at depth
    ///   `d` has `d + height <= 32`, hence `height <= 32`, hence the check passes. A move to the
    ///   root can only ever *reduce* depths.
    ///
    /// So it is defence in depth against a document that is **already** over the ceiling — which
    /// is what this test builds directly, since no accepted write could produce one
    /// (`check_snapshot` in the isolated worker enforces `tree_depth_max` on the whole document).
    /// Building it here is the only way to make the branch falsifiable instead of leaving a line
    /// no test can see.
    #[test]
    fn a_navigator_entry_carrying_an_over_deep_subtree_is_refused_by_the_batched_depth_check() {
        use collab_core::{CollabEngine, LoroCollabEngine, Operation};

        let object_id = Uuid::from_u128(11);
        let entry: super::NodeId = Arc::from(object_id.to_string().as_str());
        let mut engine = LoroCollabEngine::new_empty(3);
        engine
            .apply_operation(&Operation::CreateNode {
                id: entry.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::NavigatorNode,
            })
            .expect("the ordering entry is created");
        // A chain one hop deeper than `tree_depth_max`, hanging off the entry. `apply_operation`
        // does not itself enforce `DocumentLimits` — that is `check_operation`/`check_snapshot`'s
        // job — so this builds the invalid shape the guard exists for.
        let mut parent = entry;
        for depth in 1..=33u32 {
            let child: super::NodeId = Arc::from(format!("nested-{depth}").as_str());
            engine
                .apply_operation(&Operation::CreateNode {
                    id: child.clone(),
                    parent: Some(parent.clone()),
                    index: 0,
                    kind: NodeKind::NavigatorNode,
                })
                .expect("a nested node is created");
            parent = child;
        }
        let snapshot_bytes = engine.export_snapshot().expect("the document exports");
        let mut engine = LoroCollabEngine::load(&snapshot_bytes).expect("it loads back");

        let err = super::build_navigator_update(
            &mut engine,
            &super::NavigatorBatch {
                primary: object_id,
                cascaded: &[],
                remove: false,
                after_id: None,
            },
        )
        .map(|change| change.bytes.map(|bytes| bytes.len()))
        .expect_err("repositioning an entry that carries an over-deep subtree must be refused");
        let crate::error::ApiError::Typed { kind, details, .. } = &err else {
            panic!("expected a typed limit_exceeded, got {err:?}")
        };
        assert_eq!(*kind, crate::error::ApiErrorKind::LimitExceeded, "got {err:?}");
        let details = details.clone().unwrap_or(serde_json::Value::Null);
        assert_eq!(details["limit_kind"], serde_json::json!("tree_depth"), "got {details}");
        assert_eq!(details["limit"], serde_json::json!(32), "got {details}");
        assert_eq!(details["observed"], serde_json::json!(33), "got {details}");
    }

    /// The frozen number itself. It is a contract value, not a tuning knob: a change here without
    /// a matching change in `limits-v1.md` is the failure this asserts against.
    #[test]
    fn the_subtree_ceiling_is_the_value_limits_v1_froze() {
        assert_eq!(
            MOVE_SUBTREE_NODES_MAX, 100,
            "limits-v1.md froze move_subtree_nodes_max at 100 on 2026-08-31"
        );
        assert_eq!(super::MOVE_SUBTREE_NODES_LIMIT_KIND, "move_subtree_nodes");
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
    use crate::error::{ApiError, ApiErrorKind, ServerDrainingReason};
    use crate::flow::collab::authz::{self, PermissionLevel};
    use crate::flow::collab::coordinator::ascending_document_lock_order;
    use crate::flow::collab::registry::OutboundEvent;
    use crate::flow::collab::runtime::CollabRuntime;
    use crate::flow::command::{CreateObjectInput, ExecuteCommandInput, create_object};
    use crate::flow::event_origin::{CommandOrigin, EventSource, EventSurface};
    use crate::flow::model::AcceptedChange;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
    /// One request-level retry for each bounded internal rebase attempt. The race test accepts
    /// only the protocol's explicit, safe contention result and never retries indefinitely.
    const RACING_MOVE_CONTENTION_RETRIES: u32 = super::MAX_REBASE_ATTEMPTS;

    fn move_contention_retry_after_ms(error: &ApiError) -> Option<u64> {
        let ApiError::Typed {
            kind: ApiErrorKind::ServerDraining(ServerDrainingReason::Contention),
            details: Some(details),
            ..
        } = error
        else {
            return None;
        };
        details
            .get("retry_after_ms")
            .and_then(Value::as_u64)
            .filter(|retry_after_ms| *retry_after_ms > 0)
    }

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
                origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                    crate::flow::event_origin::EventSurface::Rest,
                ),
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                actor_is_bot: false,
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
            origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                crate::flow::event_origin::EventSurface::Rest,
            ),
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
    async fn run_move_once(
        state: &AppState,
        collab: &CollabRuntime,
        fx: &Fixture,
        input: &ExecuteCommandInput,
    ) -> Result<AcceptedChange, ApiError> {
        let checked_epoch = authz::read_epoch(&state.db, fx.workspace_id).await?;
        execute_on(state, collab, input, fx.workspace_id, checked_epoch).await
    }

    async fn run_move_with_contention_retries(
        state: &AppState,
        collab: &CollabRuntime,
        fx: &Fixture,
        input: &ExecuteCommandInput,
    ) -> Result<(AcceptedChange, u32), ApiError> {
        retry_safe_contention(|| run_move_once(state, collab, fx, input)).await
    }

    async fn retry_safe_contention<T, Attempt, AttemptFuture>(mut attempt: Attempt) -> Result<(T, u32), ApiError>
    where
        Attempt: FnMut() -> AttemptFuture,
        AttemptFuture: std::future::Future<Output = Result<T, ApiError>>,
    {
        let mut contention_retries = 0_u32;
        loop {
            match attempt().await {
                Ok(value) => return Ok((value, contention_retries)),
                Err(error) => {
                    let Some(retry_after_ms) = move_contention_retry_after_ms(&error) else {
                        return Err(error);
                    };
                    if contention_retries >= RACING_MOVE_CONTENTION_RETRIES {
                        return Err(error);
                    }
                    contention_retries += 1;
                    tokio::time::sleep(Duration::from_millis(retry_after_ms)).await;
                }
            }
        }
    }

    /// Every database test goes through the same bounded safe-contention handling. This matters
    /// even for logically sequential scenarios: the test runner exercises many isolated scratch
    /// databases concurrently, so scheduler pressure can spend the command's short lock/statement
    /// budget without creating a product correctness failure.
    async fn run_move(
        state: &AppState,
        collab: &CollabRuntime,
        fx: &Fixture,
        input: &ExecuteCommandInput,
    ) -> Result<AcceptedChange, ApiError> {
        run_move_with_contention_retries(state, collab, fx, input)
            .await
            .map(|(change, _)| change)
    }

    #[test]
    fn racing_move_retries_only_safe_contention_errors() {
        let contention = ApiError::server_draining(ServerDrainingReason::Contention, 200, "server_draining");
        assert_eq!(move_contention_retry_after_ms(&contention), Some(200));

        let draining = ApiError::server_draining(ServerDrainingReason::Drain, 200, "server_draining");
        assert_eq!(move_contention_retry_after_ms(&draining), None);
        let missing_hint = ApiError::Typed {
            kind: ApiErrorKind::ServerDraining(ServerDrainingReason::Contention),
            message: "server_draining".to_string(),
            details: Some(json!({})),
        };
        assert_eq!(move_contention_retry_after_ms(&missing_hint), None);
        let immediate_retry = ApiError::server_draining(ServerDrainingReason::Contention, 0, "server_draining");
        assert_eq!(move_contention_retry_after_ms(&immediate_retry), None);
        assert_eq!(move_contention_retry_after_ms(&ApiError::Internal), None);
    }

    #[tokio::test]
    async fn racing_move_retry_count_matches_injected_contention_and_exhausts_the_bound() {
        let mut attempts = 0_u32;
        let (value, contention_retries) = retry_safe_contention(|| {
            attempts += 1;
            std::future::ready(if attempts <= 2 {
                Err(ApiError::server_draining(
                    ServerDrainingReason::Contention,
                    1,
                    "server_draining",
                ))
            } else {
                Ok("accepted")
            })
        })
        .await
        .expect("two injected contention results are retried");
        assert_eq!(value, "accepted");
        assert_eq!(contention_retries, 2, "the observed count must equal the injections");
        assert_eq!(attempts, 3, "two retries require exactly three attempts");

        let mut exhausted_attempts = 0_u32;
        let exhausted: Result<((), u32), ApiError> = retry_safe_contention(|| {
            exhausted_attempts += 1;
            std::future::ready(Err(ApiError::server_draining(
                ServerDrainingReason::Contention,
                1,
                "server_draining",
            )))
        })
        .await;
        let error = exhausted.expect_err("contention beyond the retry bound must fail");
        assert_eq!(
            error.kind(),
            ApiErrorKind::ServerDraining(ServerDrainingReason::Contention)
        );
        assert_eq!(
            exhausted_attempts,
            RACING_MOVE_CONTENTION_RETRIES + 1,
            "the initial attempt plus the exact retry budget must run, then stop"
        );
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

    /// One committed `business_events` row, read back **from the database** rather than from any
    /// value this command returned about itself — the whole point of these assertions is that the
    /// audit row is right, and a response body is not an audit row.
    #[derive(Debug, FromQueryResult)]
    struct EventRow {
        id: Uuid,
        event_type: String,
        source: Value,
        correlation_id: Option<Uuid>,
        causation_id: Option<Uuid>,
    }

    /// Every committed event of a workspace, oldest first.
    async fn events_of(db: &DatabaseConnection, workspace_id: Uuid) -> Vec<EventRow> {
        EventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, event_type, source, correlation_id, causation_id FROM business_events \
             WHERE workspace_id = $1 ORDER BY created_at, id",
            vec![workspace_id.into()],
        ))
        .all(db)
        .await
        .expect("business_events query runs")
    }

    /// The events one command wrote, identified by the correlation it rooted. Deliberately *not*
    /// "every event in the workspace": the fixture's `create_object` calls write their own
    /// `flow.object.created` rows, and a test that could not tell them apart from the command's
    /// own rows would be asserting on the wrong set.
    fn with_correlation(rows: &[EventRow], correlation_id: Uuid) -> Vec<&EventRow> {
        rows.iter()
            .filter(|row| row.correlation_id == Some(correlation_id))
            .collect()
    }

    /// A move whose origin is an explicit, non-REST surface — the only way to tell "the producer
    /// copied what the caller declared" apart from "the producer hardcoded the value REST happens
    /// to use".
    fn move_input_with_origin(
        object_id: Uuid,
        actor_id: Uuid,
        role: &str,
        payload: Value,
        origin: crate::flow::event_origin::CommandOrigin,
    ) -> ExecuteCommandInput {
        let mut input = move_input(object_id, actor_id, role, payload);
        input.origin = origin;
        input
    }

    /// Every `collab_updates.origin_surface` value written for one document.
    async fn origin_surfaces_of(db: &DatabaseConnection, document_id: Uuid) -> Vec<String> {
        #[derive(FromQueryResult)]
        struct Row {
            origin_surface: String,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT origin_surface FROM collab_updates WHERE document_id = $1 ORDER BY seq",
            vec![document_id.into()],
        ))
        .all(db)
        .await
        .expect("collab_updates query runs")
        .into_iter()
        .map(|row| row.origin_surface)
        .collect()
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

        // This deliberately bypasses the race-only retry helper. A logically sequential lock
        // order scenario producing contention is a regression signal, not an event to mask.
        let a_to_b = run_move_once(
            &state,
            &collab,
            &fx,
            &move_input(one, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("A -> B move succeeds");
        let b_to_a = run_move_once(
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
    ///
    /// `ServerDraining{reason=contention}` is the command's explicit not-applied, retryable result
    /// after its internal rebase budget is exhausted. The test waits the returned hint and retries
    /// at most [`RACING_MOVE_CONTENTION_RETRIES`] times. Every other error, and contention beyond
    /// that bound, remains a hard failure. A deterministic injection test above asserts the exact
    /// retry count and exhaustion point; this scheduler-dependent race only requires both logical
    /// moves to complete.
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
            let input = move_input(one, fx.owner_id, "owner", json!({ "target_object_id": nav_b }));
            tokio::spawn(async move { run_move_with_contention_retries(&state, &runtime, &fx, &input).await })
        };
        let right = {
            let (state, fx, runtime) = (state.clone(), fx.clone(), right_runtime.clone());
            let input = move_input(two, fx.owner_id, "owner", json!({ "target_object_id": nav_a }));
            tokio::spawn(async move { run_move_with_contention_retries(&state, &runtime, &fx, &input).await })
        };

        let left = tokio::time::timeout(Duration::from_secs(30), left)
            .await
            .expect("the first concurrent move must not hang")
            .expect("task joins");
        let right = tokio::time::timeout(Duration::from_secs(30), right)
            .await
            .expect("the second concurrent move must not hang")
            .expect("task joins");

        let (_left_change, _left_retries) = left.unwrap_or_else(|err| {
            panic!("left move failed instead of serializing cleanly after bounded safe retries: {err:?}")
        });
        let (_right_change, _right_retries) = right.unwrap_or_else(|err| {
            panic!("right move failed instead of serializing cleanly after bounded safe retries: {err:?}")
        });
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

        let document_id = document_of(&scratch.db, page).await;
        let session_id = Uuid::new_v4();
        let mut registered = collab
            .registry
            .try_register_authorized(document_id, page, fx.member_id, fx.workspace_id, session_id, 0)
            .expect("the baseline member session registers before the move");
        collab
            .registry
            .upsert_presence(
                document_id,
                session_id,
                json!({"cursor": "before-move"}),
                Duration::from_secs(30),
            )
            .expect("member presence registers before the move");

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
        let OutboundEvent::Close { code, reason } =
            registered.receiver.try_recv().expect("move closes revoked session")
        else {
            panic!("expected an authorization close")
        };
        assert_eq!(code, 4403);
        assert_eq!(reason, "authorization revoked");
        assert_eq!(collab.registry.presence_count(document_id), 0);
        assert_eq!(collab.registry.session_count(document_id), 0);
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

    // -----------------------------------------------------------------------------------------
    // 8. The cascade (`ADR-0013` §2.2 R17 step two).
    // -----------------------------------------------------------------------------------------

    /// A three-level subtree crossing a project boundary: **every** descendant's `project_id`
    /// becomes the target's, and **every** descendant's ordering entry ends up in the target
    /// navigator with nothing left behind in the source one.
    ///
    /// The move is made in both directions on purpose. `create_object` does not write navigator
    /// ordering entries — only a move does — so the first leg is what materialises the subtree's
    /// entries at all, and the second leg is the only way to assert the *removal* half against
    /// entries that really exist. A one-way test would assert "the old navigator is empty" against
    /// a navigator that was empty to begin with, which is the vacuous-truth shape this suite is
    /// supposed to avoid.
    #[tokio::test]
    async fn a_cross_project_move_cascades_the_whole_subtree_into_the_target_navigator() {
        let scratch = scratch_or_skip!("cascade_subtree");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let root = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let child = create(&state, &fx, "page", Some(fx.project_a), Some(root)).await;
        let grandchild = create(&state, &fx, "page", Some(fx.project_a), Some(child)).await;
        // Never part of the subtree: it shares the source scope and must not be dragged along.
        let bystander = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;

        let doc_a = document_of(&scratch.db, nav_a).await;
        let doc_b = document_of(&scratch.db, nav_b).await;

        let change = run_move(
            &state,
            &collab,
            &fx,
            &move_input(root, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("a subtree may cross a project boundary now that the cascade exists");

        let result = command_result(&change);
        assert_eq!(result["cascaded"], json!(true));
        assert_eq!(
            result["cascaded_node_count"],
            json!(3),
            "the root and both descendants, not just the root"
        );
        assert_eq!(
            uuid_list(&result["contended_existing_document_set"]),
            ascending_document_lock_order(&[doc_a, doc_b]),
            "a cascade still contends exactly two navigator documents, whatever the subtree's size"
        );
        for object_id in [root, child, grandchild] {
            assert!(
                change.affected_object_ids.contains(&object_id),
                "every cascaded object is an affected object; {object_id} was not reported"
            );
        }

        // `events-v1.md` (2026-08-31 裁定): the cascade is recorded in the **envelope**, because
        // the frozen payload names only the object that was asked to move. Asserted as an exact
        // set, not with `contains` and not by length: a `contains` loop cannot see a stray extra
        // member, and a length check cannot see a swap.
        let mut rewritten_rows = vec![root, child, grandchild];
        rewritten_rows.sort();
        assert_eq!(
            affected_object_ids_of_event(&scratch.db, change.event_id).await,
            rewritten_rows,
            "the moved event's envelope must name exactly the flow_objects rows this command \
             rewrote — the moved object and every cascaded descendant. The two navigator objects \
             are not among them: their governance rows were untouched and their head advances are \
             already recorded by their own flow.content.accepted events"
        );

        // The governance columns: every descendant now sits in the target scope, and the shape of
        // the subtree itself is untouched.
        for object_id in [root, child, grandchild] {
            assert_eq!(
                project_of(&scratch.db, object_id).await,
                Some(fx.project_b),
                "the cascade must rewrite every descendant's project_id, not only the moved object's"
            );
        }
        assert_eq!(parent_of(&scratch.db, root).await, Some(nav_b));
        assert_eq!(
            parent_of(&scratch.db, child).await,
            Some(root),
            "the subtree keeps its shape"
        );
        assert_eq!(parent_of(&scratch.db, grandchild).await, Some(child));
        assert_eq!(
            project_of(&scratch.db, bystander).await,
            Some(fx.project_a),
            "an object outside the subtree is not cascaded"
        );
        assert_eq!(
            scope_violation_count(&scratch.db).await,
            0,
            "the cascade must land on the invariant, not merely avoid the constraint"
        );

        // The ordering entries: all three in the target navigator, none in the source one, and in
        // a *specified* order rather than merely present as a set. The moved object's entry goes
        // where `after_id` says (here: appended, since nothing was asked for), and the cascaded
        // entries follow it in the order the cascade appends them, which is ascending `id` because
        // that is the order `repository::subtree_nodes` returns. Asserting the set alone would
        // leave the append-index bookkeeping in `build_navigator_update` unfalsifiable — every
        // entry would still exist if all N creates landed on the same index.
        let mut cascaded_in_order = vec![child, grandchild];
        cascaded_in_order.sort();
        let expected_order: Vec<Uuid> = std::iter::once(root).chain(cascaded_in_order).collect();
        assert_eq!(
            navigator_order(&scratch.db, nav_b).await,
            expected_order,
            "every cascaded object's ordering entry must be in the target navigator, in the order              the cascade appends them"
        );
        let mut expected = expected_order.clone();
        expected.sort();
        assert_eq!(
            navigator_order(&scratch.db, nav_a).await,
            Vec::<Uuid>::new(),
            "and the source navigator must hold none of them"
        );

        // ---- back again: this leg is the one that exercises the removal half ----
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(root, fx.owner_id, "owner", json!({ "target_object_id": nav_a })),
        )
        .await
        .expect("the same subtree moves back");

        for object_id in [root, child, grandchild] {
            assert_eq!(project_of(&scratch.db, object_id).await, Some(fx.project_a));
        }
        assert_eq!(
            navigator_order(&scratch.db, nav_a).await,
            expected_order,
            "the whole subtree's entries came back, in the same specified order"
        );
        let mut back_home = navigator_order(&scratch.db, nav_a).await;
        back_home.sort();
        assert_eq!(back_home, expected, "and as a set, none lost and none invented");
        assert_eq!(
            navigator_order(&scratch.db, nav_b).await,
            Vec::<Uuid>::new(),
            "and every entry left the navigator the subtree left — one entry per cascaded node, \
             not just the moved object's"
        );
        scratch.drop_self().await;
    }

    /// Cascaded entries arrive in the target navigator in the order they had in the **source**
    /// navigator, not in any order this code re-derives.
    ///
    /// `rest-api-v1.md` (2026-08-31 裁定): "`after_id` 只定位被移动对象本身……被级联的后代之间，
    /// 必须保持它们在源 navigator 里的相对顺序". That order is visible to whoever is looking at
    /// the tree, so re-deriving it would silently reshuffle a subtree nobody asked to reshuffle.
    ///
    /// **The fixture deliberately makes the source order disagree with ascending `id`.** If it did
    /// not, this assertion would be vacuously true against the ascending-`id` order the cascade
    /// starts from, and the whole ordering rule could be deleted with every test still green —
    /// which is exactly the trap the set-only assertion in
    /// `a_cross_project_move_cascades_the_whole_subtree_into_the_target_navigator` fell into
    /// before it was tightened.
    #[tokio::test]
    async fn cascaded_entries_keep_the_relative_order_they_had_in_the_source_navigator() {
        let scratch = scratch_or_skip!("cascade_order");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let root = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let mut children = Vec::new();
        for _ in 0..3 {
            children.push(create(&state, &fx, "page", Some(fx.project_a), Some(root)).await);
        }
        children.sort();
        let (low, middle, high) = (children[0], children[1], children[2]);

        // Leg one materialises the entries. Nothing was ordered before, so they land in ascending
        // `id` — the cascade's fallback order, and the order this test must go on to disturb.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(root, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("the subtree moves into B");
        assert_eq!(
            navigator_order(&scratch.db, nav_b).await,
            vec![root, low, middle, high],
            "setup precondition: the first cascade appends in ascending id"
        );

        // Now disturb it, through the real command: a **same-scope** re-parent of `high` under the
        // parent it already has, positioned right after `root`. Same scope means no cascade, so
        // this repositions exactly one entry and touches nothing else.
        let reorder = run_move(
            &state,
            &collab,
            &fx,
            &move_input(
                high,
                fx.owner_id,
                "owner",
                json!({ "target_object_id": root, "after_id": root }),
            ),
        )
        .await
        .expect("a same-scope reposition is an ordinary move");
        assert_eq!(
            command_result(&reorder)["cascaded"],
            json!(false),
            "a same-scope move must not cascade, or this fixture is measuring the wrong thing"
        );
        assert_eq!(
            affected_object_ids_of_event(&scratch.db, reorder.event_id).await,
            vec![high],
            "a non-cascading move rewrote exactly one row, so its envelope names exactly one object"
        );

        let source_order = navigator_order(&scratch.db, nav_b).await;
        assert_eq!(
            source_order,
            vec![root, high, low, middle],
            "the fixture's chosen order"
        );
        let source_descendants: Vec<Uuid> = source_order.iter().copied().filter(|id| *id != root).collect();
        assert_ne!(
            source_descendants,
            vec![low, middle, high],
            "the fixture must disagree with ascending id, or the assertion below is vacuous"
        );

        // Leg two: the cascade has to reproduce that order, not re-derive one.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(root, fx.owner_id, "owner", json!({ "target_object_id": nav_a })),
        )
        .await
        .expect("the subtree moves back to A");
        assert_eq!(
            navigator_order(&scratch.db, nav_a).await,
            source_order,
            "the cascaded entries must arrive in the order they had in the source navigator; \
             ascending id would have produced [root, low, middle, high]"
        );
        assert_eq!(navigator_order(&scratch.db, nav_b).await, Vec::<Uuid>::new());
        scratch.drop_self().await;
    }

    /// An archived descendant is still a subtree member, and the cascade has to take it along.
    ///
    /// `archive` is a pure `lifecycle_status` flip — it leaves `parent_id` and `project_id` exactly
    /// where they were — so an archived page still sits under its parent and
    /// `flow_objects_parent_project_fk` still holds it to its parent's scope. An implementation
    /// that cascaded only the active rows would fail the constraint on its own `UPDATE`, which is
    /// what this asserts: not "archived rows are handled gracefully" but "the whole subtree really
    /// is the whole subtree".
    #[tokio::test]
    async fn the_cascade_carries_archived_descendants_because_the_constraint_still_holds_them() {
        let scratch = scratch_or_skip!("cascade_archived");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let root = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let child = create(&state, &fx, "page", Some(fx.project_a), Some(root)).await;
        let archived = create(&state, &fx, "page", Some(fx.project_a), Some(child)).await;

        // The same write `flow::command`'s `archive` command makes: status and timestamp only.
        exec(
            &scratch.db,
            "UPDATE flow_objects SET lifecycle_status = 'archived', archived_at = now() WHERE id = $1",
            vec![archived.into()],
        )
        .await;

        let change = run_move(
            &state,
            &collab,
            &fx,
            &move_input(root, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("a subtree containing an archived page still moves");
        assert_eq!(
            command_result(&change)["cascaded_node_count"],
            json!(3),
            "the archived page is counted, because the constraint counts it"
        );
        assert_eq!(
            project_of(&scratch.db, archived).await,
            Some(fx.project_b),
            "an archived descendant's project_id must travel with its parent, or the composite \
             foreign key would have refused the whole statement"
        );
        assert_eq!(
            scalar_i64(
                &scratch.db,
                "SELECT count(*)::bigint AS value FROM flow_objects \
                  WHERE id = $1 AND lifecycle_status = 'archived'",
                vec![archived.into()],
            )
            .await,
            1,
            "and the cascade must not resurrect it"
        );
        assert_eq!(scope_violation_count(&scratch.db).await, 0);
        scratch.drop_self().await;
    }

    /// `affected_object_ids` must not repeat an object, and the case that repeats one is a real
    /// request shape rather than a hypothetical: **moving to a navigator root**.
    ///
    /// The response list is assembled as `[object_id, target_object_id]` ++ the contended
    /// navigators (ascending `id`, from `derive_contended_set`) ++ the cascaded descendants. When
    /// `target_object_id` *is* one of those navigators — which is exactly what "move this page to
    /// the top level of that project" means (`rest-api-v1.md`: "顶层移动即
    /// `target_object_id = root_object_id`") — it occupies two slots. They are adjacent only when
    /// the target happens to be the **smaller** of the two navigator uuids; when it is the larger
    /// one the other navigator sits between the two copies, and `Vec::dedup`, which only collapses
    /// *consecutive* equal elements, leaves the duplicate in.
    ///
    /// So the fixture picks the larger uuid on purpose and asserts the precondition, because a
    /// coin-flip fixture would pass against the broken implementation half the time.
    #[tokio::test]
    async fn moving_to_a_navigator_root_does_not_report_that_navigator_twice() {
        let scratch = scratch_or_skip!("affected_dedup");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        // The move's target is whichever navigator has the *larger* uuid, so that after
        // `derive_contended_set` sorts the pair ascending, the target's two occurrences in the
        // assembled list are separated by the other navigator.
        let (target_nav, other_nav, other_project) = if nav_a > nav_b {
            (nav_a, nav_b, fx.project_b)
        } else {
            (nav_b, nav_a, fx.project_a)
        };
        assert!(
            target_nav > other_nav,
            "fixture precondition: the target must be the larger uuid, or the two copies are \
             adjacent and Vec::dedup would collapse them by luck"
        );
        let page = create(&state, &fx, "page", Some(other_project), Some(other_nav)).await;

        let change = run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": target_nav })),
        )
        .await
        .expect("moving a page to a navigator root is an ordinary cross-project move");

        assert_eq!(
            change.affected_object_ids,
            vec![page, target_nav, other_nav],
            "the moved object, its target, and the other contended navigator — each exactly once, \
             in first-occurrence order"
        );
        let mut unique = change.affected_object_ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            change.affected_object_ids.len(),
            "affected_object_ids must not repeat an object; got {:?}",
            change.affected_object_ids
        );
        scratch.drop_self().await;
    }

    /// `move_subtree_nodes_max`, on both sides of its frozen value: 100 nodes commit, 101 are
    /// refused with the contract's exact `limit_exceeded` shape and **zero** observable effect.
    ///
    /// The refusal half is the one that has to be airtight, so it is asserted against a full
    /// before/after census: both navigator heads, both `collab_updates` counts, the epoch, the
    /// `flow.object.moved` count, the `event_dispatch` count, and every subtree row's
    /// `parent_id`/`project_id`.
    #[tokio::test]
    async fn the_subtree_ceiling_admits_one_hundred_nodes_and_refuses_one_hundred_and_one() {
        let scratch = scratch_or_skip!("cascade_ceiling");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let doc_a = document_of(&scratch.db, nav_a).await;
        let doc_b = document_of(&scratch.db, nav_b).await;

        // Exactly `MOVE_SUBTREE_NODES_MAX` nodes: the root plus 99 children, kept wide rather than
        // deep so `tree_depth_max` (32) is not what refuses.
        let root = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let mut subtree = vec![root];
        for _ in 1..super::MOVE_SUBTREE_NODES_MAX {
            subtree.push(create(&state, &fx, "page", Some(fx.project_a), Some(root)).await);
        }
        assert_eq!(subtree.len(), super::MOVE_SUBTREE_NODES_MAX);

        let started = std::time::Instant::now();
        let change = run_move(
            &state,
            &collab,
            &fx,
            &move_input(root, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect(
            "the frozen ceiling is an inclusive maximum: a subtree of exactly move_subtree_nodes_max \
             nodes must commit, and must not be aborted by the locked phase's statement or lock \
             timeout budgets",
        );
        // `limits-v1.md`'s `per_host_authority` ruling: the frozen 12.21 ms in-lock hold is a
        // design figure, not a per-host pass criterion — this host's own run-to-run spread (2.8x)
        // is wider than the value's headroom (2.05x), so a hard `p95 < 25 ms` assertion here would
        // be flaky by construction. What *is* asserted is the thing the ruling says a slow host
        // must show: the command either completes or fails closed, never half-commits. The number
        // is recorded rather than judged.
        eprintln!(
            "measured: N={} cascade, end-to-end request span {:.2} ms (in-lock hold is a strict \
             subset of this; not a pass criterion, see limits-v1.md per_host_authority)",
            super::MOVE_SUBTREE_NODES_MAX,
            started.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(command_result(&change)["cascaded_node_count"], json!(100));
        for object_id in &subtree {
            assert_eq!(project_of(&scratch.db, *object_id).await, Some(fx.project_b));
        }

        // One more node, and the same move back is over the ceiling.
        let one_too_many = create(&state, &fx, "page", Some(fx.project_b), Some(root)).await;
        subtree.push(one_too_many);
        assert_eq!(subtree.len(), super::MOVE_SUBTREE_NODES_MAX + 1);

        let before = census(&scratch.db, &fx, doc_a, doc_b).await;
        let mut before_rows = Vec::with_capacity(subtree.len());
        for object_id in &subtree {
            before_rows.push((
                parent_of(&scratch.db, *object_id).await,
                project_of(&scratch.db, *object_id).await,
            ));
        }

        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(root, fx.owner_id, "owner", json!({ "target_object_id": nav_a })),
        )
        .await
        .expect_err("one node past the frozen ceiling must be refused");
        eprintln!("over-ceiling refusal: {err:?}");
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded, "got {err:?}");
        let ApiError::Typed { details, .. } = &err else {
            panic!("a limit_exceeded refusal must be a typed error carrying details; got {err:?}")
        };
        let details = details.clone().unwrap_or(Value::Null);
        assert_eq!(details["limit_kind"], json!("move_subtree_nodes"), "got {details}");
        assert_eq!(details["limit"], json!(100), "got {details}");
        assert_eq!(
            details["observed"],
            json!(101),
            "`observed` must be the subtree's real size, not the ceiling read back; got {details}"
        );

        assert_eq!(
            census(&scratch.db, &fx, doc_a, doc_b).await,
            before,
            "a limit_exceeded refusal must leave both navigator heads, both update logs, the epoch, \
             the move events and the dispatch rows exactly as they were"
        );
        for (object_id, was) in subtree.iter().zip(before_rows) {
            assert_eq!(
                (
                    parent_of(&scratch.db, *object_id).await,
                    project_of(&scratch.db, *object_id).await
                ),
                was,
                "and not one PG row of the subtree may have changed"
            );
        }
        scratch.drop_self().await;
    }

    /// Atomicity of the cascade, on the one path that is actually falsifiable.
    ///
    /// A trigger fault aborts the `PostgreSQL` transaction outright, so `COMMIT` and `ROLLBACK`
    /// become the same thing and such a test cannot tell a correct implementation from a broken
    /// one (that limitation is already recorded on
    /// `a_failure_at_the_last_write_leaves_no_trace_of_the_move`). This one uses an
    /// **application-level** refusal instead — `ADR-0012` §4.1's unconfirmed self-lockout — which
    /// is raised *after* the two navigator writes and after the whole cascade `UPDATE` have
    /// already been staged, on a transaction `PostgreSQL` considers perfectly healthy. Only
    /// `move_object`'s own rollback can undo it.
    #[tokio::test]
    async fn a_cascade_refused_after_it_has_written_rolls_every_descendant_back() {
        let scratch = scratch_or_skip!("cascade_atomicity");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let open_parent = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let closed_parent = create(&state, &fx, "page", Some(fx.project_b), Some(nav_b)).await;
        let root = create(&state, &fx, "page", Some(fx.project_a), Some(open_parent)).await;
        let child = create(&state, &fx, "page", Some(fx.project_a), Some(root)).await;
        let grandchild = create(&state, &fx, "page", Some(fx.project_a), Some(child)).await;

        // The member reaches the subtree with `full_access` only through the *old* parent, and the
        // target sits behind a boundary where they hold `edit`. Moving is therefore allowed on
        // both sides of `ADR-0012` §4 and self-locking on §4.1.
        exec(
            &scratch.db,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![closed_parent.into()],
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

        let doc_a = document_of(&scratch.db, nav_a).await;
        let doc_b = document_of(&scratch.db, nav_b).await;
        let before = census(&scratch.db, &fx, doc_a, doc_b).await;
        let mut before_rows = Vec::new();
        for object_id in [root, child, grandchild] {
            before_rows.push((
                parent_of(&scratch.db, object_id).await,
                project_of(&scratch.db, object_id).await,
            ));
        }

        let err = run_move(
            &state,
            &collab,
            &fx,
            &move_input(
                root,
                fx.member_id,
                "member",
                json!({ "target_object_id": closed_parent }),
            ),
        )
        .await
        .expect_err("an unconfirmed self-lockout must refuse the whole cascade");
        eprintln!("post-write application refusal: {err:?}");
        assert_eq!(err.kind(), ApiErrorKind::PolicyRejected, "got {err:?}");

        assert_eq!(
            census(&scratch.db, &fx, doc_a, doc_b).await,
            before,
            "the refusal happens after both navigator writes and after the cascade UPDATE, on a \
             healthy transaction — heads, update logs, epoch, events and dispatch must all be back"
        );
        for (object_id, was) in [root, child, grandchild].into_iter().zip(before_rows) {
            assert_eq!(
                (
                    parent_of(&scratch.db, object_id).await,
                    project_of(&scratch.db, object_id).await
                ),
                was,
                "every descendant's parent_id and project_id must be back where they were"
            );
        }
        assert_eq!(navigator_order(&scratch.db, nav_b).await, Vec::<Uuid>::new());

        // Confirmed, the identical command commits — the rollback left nothing wedged, and the
        // cascade really was the thing that got rolled back.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(
                root,
                fx.member_id,
                "member",
                json!({ "target_object_id": closed_parent, "confirm_self_lockout": true }),
            ),
        )
        .await
        .expect("with the lockout confirmed the same cascade commits");
        for object_id in [root, child, grandchild] {
            assert_eq!(project_of(&scratch.db, object_id).await, Some(fx.project_b));
        }
        scratch.drop_self().await;
    }

    /// `limits-v1.md`'s `move_subtree_nodes_max` `tombstone_interaction`: "级联把
    /// `navigator_tombstones` 的增长从「每命令 1 个」变成「每命令 N 个」... v0.5 强制的
    /// `navigator_tombstone_growth_measured` 取样口径应覆盖级联命令".
    ///
    /// So this samples the three fields that gate names — `live_entry_count`, `tombstone_count`,
    /// `snapshot_bytes` — on a command that really does cascade, and asserts the growth is `N` and
    /// not 1. A sample taken only on single-node moves would report 1 and be wrong by a factor of
    /// the subtree's size.
    #[tokio::test]
    async fn navigator_tombstone_growth_is_measured_on_a_cascading_command() {
        let scratch = scratch_or_skip!("cascade_tombstones");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let root = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;
        let child = create(&state, &fx, "page", Some(fx.project_a), Some(root)).await;
        let grandchild = create(&state, &fx, "page", Some(fx.project_a), Some(child)).await;
        let cascaded = 3usize;
        let _ = grandchild;

        // Leg one materialises three entries in B. Nothing is removed yet, so no tombstone exists
        // anywhere and the "growth" measured on leg two cannot be inherited from setup.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(root, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("the subtree moves into B");
        let b_before = navigator_census(&scratch.db, nav_b).await;
        let a_before = navigator_census(&scratch.db, nav_a).await;
        assert_eq!(b_before.live_entry_count, cascaded);
        assert_eq!(b_before.tombstone_count, 0, "no removal has happened yet");

        // Leg two removes all three from B in **one command**.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(root, fx.owner_id, "owner", json!({ "target_object_id": nav_a })),
        )
        .await
        .expect("the subtree moves back to A");
        let b_after = navigator_census(&scratch.db, nav_b).await;
        let a_after = navigator_census(&scratch.db, nav_a).await;
        eprintln!(
            "measured navigator_tombstone_growth (cascading command, N={cascaded}): \
             source live_entry_count {} -> {}, tombstone_count {} -> {}, snapshot_bytes {} -> {}; \
             target live_entry_count {} -> {}, tombstone_count {} -> {}, snapshot_bytes {} -> {}",
            b_before.live_entry_count,
            b_after.live_entry_count,
            b_before.tombstone_count,
            b_after.tombstone_count,
            b_before.snapshot_bytes,
            b_after.snapshot_bytes,
            a_before.live_entry_count,
            a_after.live_entry_count,
            a_before.tombstone_count,
            a_after.tombstone_count,
            a_before.snapshot_bytes,
            a_after.snapshot_bytes,
        );

        assert_eq!(
            b_after.tombstone_count - b_before.tombstone_count,
            cascaded,
            "one command removed N entries, so it left N tombstones — the per-command growth the \
             contract says the sampling must cover is N, not 1"
        );
        assert_eq!(b_after.live_entry_count, 0);
        assert_eq!(a_after.live_entry_count, cascaded);
        assert_eq!(
            a_after.tombstone_count, 0,
            "the receiving navigator gains entries, not tombstones"
        );
        for (label, bytes) in [("source", b_after.snapshot_bytes), ("target", a_after.snapshot_bytes)] {
            assert!(
                bytes > 0,
                "snapshot_bytes must be a real measurement on the {label} navigator, not a default \
                 zero the gate would read as a missing field"
            );
        }
        scratch.drop_self().await;
    }

    /// The envelope's `metadata.affected_object_ids[]`, read off the committed `business_events`
    /// row rather than off anything the command reported about itself.
    async fn affected_object_ids_of_event(db: &DatabaseConnection, event_id: Uuid) -> Vec<Uuid> {
        #[derive(FromQueryResult)]
        struct Row {
            metadata: Value,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT metadata FROM business_events WHERE id = $1",
            vec![event_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("the event exists");
        row.metadata["affected_object_ids"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "the flow.object.moved envelope must carry affected_object_ids; got {}",
                    row.metadata
                )
            })
            .iter()
            .map(|value| Uuid::parse_str(value.as_str().expect("uuid string")).expect("uuid"))
            .collect()
    }

    /// Every observable a refused move must leave untouched, in one comparable tuple.
    async fn census(
        db: &DatabaseConnection,
        fx: &Fixture,
        doc_a: Uuid,
        doc_b: Uuid,
    ) -> (i64, i64, i64, i64, i64, i64, i64) {
        (
            head_seq(db, doc_a).await,
            head_seq(db, doc_b).await,
            update_count(db, doc_a).await,
            update_count(db, doc_b).await,
            epoch_of(db, fx.workspace_id).await,
            moved_event_count(db, fx.workspace_id).await,
            scalar_i64(db, "SELECT count(*)::bigint AS value FROM event_dispatch", vec![]).await,
        )
    }

    struct NavigatorCensus {
        live_entry_count: usize,
        tombstone_count: usize,
        snapshot_bytes: i64,
    }

    /// The three fields `navigator_tombstone_growth_measured` names, read out of the committed
    /// projection and the document row rather than out of anything this command reported about
    /// itself.
    async fn navigator_census(db: &DatabaseConnection, navigator_object_id: Uuid) -> NavigatorCensus {
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
        let live_entry_count = nodes.values().filter(|node| node["deleted"] == json!(false)).count();
        let tombstone_count = nodes.values().filter(|node| node["deleted"] == json!(true)).count();
        let snapshot_bytes = scalar_i64(
            db,
            "SELECT octet_length(cd.snapshot)::bigint AS value FROM collab_documents cd WHERE cd.object_id = $1",
            vec![navigator_object_id.into()],
        )
        .await;
        NavigatorCensus {
            live_entry_count,
            tombstone_count,
            snapshot_bytes,
        }
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

    // -----------------------------------------------------------------------------------------
    // `events-v1.md` origin / correlation / causation
    // -----------------------------------------------------------------------------------------

    /// `events-v1.md`: "`source` 由服务端按 Web/REST/MCP/CLI/worker 覆盖，沿用 MCP origin
    /// contract", and `mcp-surface-v1.md`'s persisted origin shape
    /// `{"surface":..., "session":..., "tool":..., "request":...}`.
    ///
    /// The move is run with an `mcp_stdio` origin — a surface REST could never produce — so the
    /// assertion can only pass if the producer copied what its caller declared. Every event the
    /// command wrote is checked, not just the primary one: the derived `flow.content.accepted`
    /// rows go through `write::stage_one_document`, which had its own hardcoded surface literal.
    #[tokio::test]
    async fn a_move_stamps_the_surface_its_caller_declared_on_every_event_it_writes() {
        let scratch = scratch_or_skip!("origin_surface");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;

        // First move materialises the entry in B, so the second move advances *two* heads and
        // therefore writes two derived `flow.content.accepted` rows.
        run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("first move succeeds");

        let origin = CommandOrigin::first_request(
            EventSource::new(EventSurface::McpStdio)
                .with_session("stdio-session-1")
                .with_tool("objects.move")
                .with_request("json-rpc-42"),
        );
        let correlation_id = origin.correlation_id;
        run_move(
            &state,
            &collab,
            &fx,
            &move_input_with_origin(page, fx.owner_id, "owner", json!({ "target_object_id": nav_a }), origin),
        )
        .await
        .expect("the move succeeds");

        let all = events_of(&scratch.db, fx.workspace_id).await;
        let written = with_correlation(&all, correlation_id);
        assert_eq!(
            written.len(),
            3,
            "one cross-project move writes flow.object.moved plus one flow.content.accepted per \
             advancing navigator; got {:?}",
            written.iter().map(|row| row.event_type.as_str()).collect::<Vec<_>>()
        );

        let expected_source = json!({
            "surface": "mcp_stdio",
            "session": "stdio-session-1",
            "tool": "objects.move",
            "request": "json-rpc-42",
        });
        for row in &written {
            assert_eq!(
                row.source, expected_source,
                "'{}' was written with source {:?}, but the caller declared {:?} -- a producer \
                 that hardcodes its own surface silently mislabels every non-REST caller",
                row.event_type, row.source, expected_source
            );
        }

        // The same value reaches the `collab_updates` column, not only the envelope: the two are
        // filled from one `EventSurface`, so they cannot disagree.
        let surfaces = origin_surfaces_of(&scratch.db, document_of(&scratch.db, nav_a).await).await;
        assert!(
            surfaces.contains(&"mcp_stdio".to_string()),
            "collab_updates.origin_surface must carry the caller's surface too, got {surfaces:?}"
        );

        scratch.drop_self().await;
    }

    /// `events-v1.md`: "首个用户请求生成 `correlation_id`；由 command、job、worker 或 retry 导出的
    /// 下一事件把直接父 event id 写 `causation_id` 并继承 correlation".
    ///
    /// A cross-project move is the command that makes this checkable at all: it writes one
    /// primary `flow.object.moved` and one derived `flow.content.accepted` per navigator whose
    /// head advanced, so the request contains a real parent/child pair rather than a single row
    /// that would satisfy any implementation.
    #[tokio::test]
    async fn a_moves_derived_content_events_share_its_correlation_and_name_it_as_their_causation() {
        let scratch = scratch_or_skip!("origin_causation");
        let state = state_for(scratch.db.clone());
        let collab = CollabRuntime::default();
        let fx = seed_workspace(&scratch.db).await;

        let nav_a = create(&state, &fx, "navigator", Some(fx.project_a), None).await;
        let nav_b = create(&state, &fx, "navigator", Some(fx.project_b), None).await;
        let page = create(&state, &fx, "page", Some(fx.project_a), Some(nav_a)).await;

        run_move(
            &state,
            &collab,
            &fx,
            &move_input(page, fx.owner_id, "owner", json!({ "target_object_id": nav_b })),
        )
        .await
        .expect("first move succeeds");

        let origin = CommandOrigin::first_request(EventSource::new(EventSurface::Rest).with_request("req-7"));
        let correlation_id = origin.correlation_id;
        let accepted = run_move(
            &state,
            &collab,
            &fx,
            &move_input_with_origin(page, fx.owner_id, "owner", json!({ "target_object_id": nav_a }), origin),
        )
        .await
        .expect("the move succeeds");

        let all = events_of(&scratch.db, fx.workspace_id).await;
        let written = with_correlation(&all, correlation_id);
        assert_eq!(
            written.len(),
            3,
            "the request wrote its primary event plus two derived ones"
        );

        // (a) the correlation is real and is the one the request generated -- not `NULL`, which is
        // what every Flow producer wrote before this.
        assert_ne!(
            correlation_id,
            Uuid::nil(),
            "a generated correlation is not the nil UUID"
        );

        let moved: Vec<&&EventRow> = written
            .iter()
            .filter(|row| row.event_type == "flow.object.moved")
            .collect();
        assert_eq!(moved.len(), 1, "exactly one primary event");
        let moved = moved[0];
        assert_eq!(
            moved.id, accepted.event_id,
            "the primary row is the event the command reported"
        );
        assert_eq!(
            moved.causation_id, None,
            "a first user request roots its own causal chain and has no parent event"
        );

        // (b) every derived event names the primary as its direct parent.
        let derived: Vec<&&EventRow> = written
            .iter()
            .filter(|row| row.event_type == "flow.content.accepted")
            .collect();
        assert_eq!(
            derived.len(),
            2,
            "both navigator heads advanced, so both wrote an event"
        );
        for row in &derived {
            assert_eq!(
                row.causation_id,
                Some(moved.id),
                "a derived flow.content.accepted must name flow.object.moved as its causation, got {:?}",
                row.causation_id
            );
        }

        // (c) one request, one correlation -- asserted against the *distinct* set so an
        // implementation that minted a fresh correlation per event cannot pass by writing three
        // rows that merely each have some correlation.
        let distinct: std::collections::BTreeSet<Option<Uuid>> = written.iter().map(|row| row.correlation_id).collect();
        assert_eq!(
            distinct,
            std::iter::once(Some(correlation_id)).collect(),
            "every event of one request shares exactly one correlation"
        );

        // A second, independent request must *not* land in the same chain -- otherwise "shares one
        // correlation" would be satisfiable by a constant.
        let other = CommandOrigin::first_request(EventSource::new(EventSurface::Rest));
        assert_ne!(
            other.correlation_id, correlation_id,
            "two first requests do not share a correlation"
        );

        scratch.drop_self().await;
    }
}

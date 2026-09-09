//! Policy-filtered, accepted-only Sylvode Flow full-text search.
//!
//! Candidate snippets are read only from `flow_search_index`, which the worker copies from
//! accepted `flow_object_projections`. Every candidate is reauthorized at the request epoch and
//! the caller cursor is derived only from returned rows. Request-span logging is sanitized by the
//! global trace layer; this module adds no content, candidate-id, or pre-filter cardinality logs.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use platform::app::AppState;
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;

use super::collab::{authz::PermissionLevel, limits, runtime};
use super::model::{FlowObjectSummary, FlowSearchHit, FlowSearchResponse, SearchIndexFrontier};
use super::policy::{self, FlowReadContext};
use super::query;

const SEARCH_SCAN_BATCH_SIZE: u64 = 100;
const CURSOR_VERSION: u8 = 1;
const CURSOR_AAD: &[u8] = b"openpr.flow.search.cursor.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchScope {
    Project(Uuid),
    Unprojected,
    AllVisible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Freshness {
    AllowStale,
    RequireCurrent,
}

pub struct SearchParams {
    pub workspace_id: Uuid,
    pub q: String,
    pub project_id: Option<Uuid>,
    pub unprojected: bool,
    pub all_visible: bool,
    pub object_type: Option<String>,
    pub freshness: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

struct ValidatedSearch {
    workspace_id: Uuid,
    q: String,
    scope: SearchScope,
    object_type: Option<String>,
    freshness: Freshness,
    cursor: Option<String>,
    limit: u64,
}

fn validate(params: SearchParams, is_bot: bool) -> Result<ValidatedSearch, ApiError> {
    let query_chars = params.q.chars().count();
    if !(1..=256).contains(&query_chars) {
        return Err(ApiError::invalid_update("q must contain between 1 and 256 characters"));
    }

    let selected_scopes =
        usize::from(params.project_id.is_some()) + usize::from(params.unprojected) + usize::from(params.all_visible);
    if selected_scopes != 1 {
        return Err(ApiError::invalid_update(
            "select exactly one of project_id, unprojected=true, or all_visible=true",
        ));
    }
    if is_bot && params.all_visible {
        return Err(ApiError::Forbidden(
            "bot callers cannot request all_visible Flow search".to_string(),
        ));
    }
    let scope = if let Some(project_id) = params.project_id {
        SearchScope::Project(project_id)
    } else if params.unprojected {
        SearchScope::Unprojected
    } else {
        SearchScope::AllVisible
    };

    if params
        .object_type
        .as_deref()
        .is_some_and(|object_type| !matches!(object_type, "page" | "navigator"))
    {
        return Err(ApiError::invalid_update(
            "object_type is not a supported Flow object type",
        ));
    }
    let freshness = match params.freshness.as_deref().unwrap_or("allow_stale") {
        "allow_stale" => Freshness::AllowStale,
        "require_current" => Freshness::RequireCurrent,
        _ => return Err(ApiError::invalid_update("freshness is not valid")),
    };

    Ok(ValidatedSearch {
        workspace_id: params.workspace_id,
        q: params.q,
        scope,
        object_type: params.object_type,
        freshness,
        cursor: params.cursor,
        limit: query::validate_limit(params.limit)?,
    })
}

fn check_scan_budget(examined: u64) -> Result<(), ApiError> {
    if examined > limits::AUTHORIZED_SCAN_ROWS_MAX {
        return Err(ApiError::limit_exceeded(
            "Flow search authorization scan budget exceeded",
            "scan_budget",
            Some(json!(limits::AUTHORIZED_SCAN_ROWS_MAX)),
            Some(json!(limits::AUTHORIZED_SCAN_ROWS_MAX)),
            None,
        ));
    }
    Ok(())
}

fn candidate_is_policy_visible(is_visible: bool) -> bool {
    #[cfg(test)]
    if std::env::var_os("OPENPR_FLOW_TEST_MUTATION_SEARCH_AUTHORIZE_ALL").is_some() {
        eprintln!("WP28_MUTATION_SEARCH_AUTHORIZE_ALL_ACTIVE");
        return true;
    }
    is_visible
}

fn add_scope_predicate(sql: &mut String, values: &mut Vec<sea_orm::Value>, scope: SearchScope, object_alias: &str) {
    match scope {
        SearchScope::Project(project_id) => {
            values.push(project_id.into());
            let _ = write!(sql, " AND {object_alias}.project_id = ${}", values.len());
        }
        SearchScope::Unprojected => {
            let _ = write!(sql, " AND {object_alias}.project_id IS NULL");
        }
        SearchScope::AllVisible => {}
    }
}

fn add_object_type_predicate(
    sql: &mut String,
    values: &mut Vec<sea_orm::Value>,
    object_type: Option<&str>,
    object_alias: &str,
) {
    if let Some(object_type) = object_type {
        values.push(object_type.into());
        let _ = write!(sql, " AND {object_alias}.object_type = ${}", values.len());
    }
}

#[derive(Debug, FromQueryResult)]
struct FrontierAggregateRow {
    indexed_seq: i64,
    head_seq: i64,
    stale: bool,
}

async fn policy_filtered_frontier(
    state: &AppState,
    access: &FlowReadContext,
    search: &ValidatedSearch,
) -> Result<SearchIndexFrontier, ApiError> {
    let mut values: Vec<sea_orm::Value> = vec![search.workspace_id.into()];
    let mut scope_predicate = String::from("fo.workspace_id = $1 AND fo.lifecycle_status = 'active'");
    add_scope_predicate(&mut scope_predicate, &mut values, search.scope, "fo");
    add_object_type_predicate(&mut scope_predicate, &mut values, search.object_type.as_deref(), "fo");

    values.push(access.actor_id().into());
    let actor_index = values.len();
    values.push(access.principal_kind().as_str().into());
    let principal_kind_index = values.len();
    values.push(access.is_human_admin().into());
    let human_admin_index = values.len();
    values.push(
        i64::try_from(super::collab::authz::MAX_CHAIN_NODES)
            .unwrap_or(i64::MAX)
            .into(),
    );
    let probe_depth_index = values.len();
    values.push(
        i64::try_from(super::collab::authz::TREE_DEPTH_MAX)
            .unwrap_or(i64::MAX)
            .into(),
    );
    let tree_depth_index = values.len();

    // Authorization is evaluated inside the aggregate statement. This preserves the boundary
    // semantics of `authz::effective_permissions` without shipping every scope row to Rust. The
    // fixed scan budget remains reserved for query candidates that are actually overfetched.
    let sql = format!(
        r"
        WITH RECURSIVE scoped AS (
            SELECT fo.id
              FROM flow_objects fo
             WHERE {scope_predicate}
        ), chain AS (
            SELECT s.id AS seed_id, o.id, o.parent_id, o.inherit_from_parent, 0 AS depth,
                   ARRAY[o.id]::uuid[] AS path, false AS cycle
              FROM scoped s
              JOIN flow_objects o ON o.id = s.id
            UNION ALL
            SELECT c.seed_id, p.id, p.parent_id, p.inherit_from_parent, c.depth + 1,
                   c.path || p.id, p.id = ANY(c.path)
              FROM chain c
              JOIN flow_objects p ON p.id = c.parent_id AND p.workspace_id = $1
             WHERE c.parent_id IS NOT NULL
               AND c.depth < ${probe_depth_index}::int
               AND NOT c.cycle
        ), chain_state AS (
            SELECT seed_id,
                   bool_or(cycle OR depth > ${tree_depth_index}::int) AS invalid,
                   (array_agg(parent_id ORDER BY depth DESC))[1] IS NOT NULL AS incomplete,
                   min(depth) FILTER (WHERE NOT inherit_from_parent) AS boundary_depth
              FROM chain
             GROUP BY seed_id
        ), visible AS (
            SELECT s.id
              FROM scoped s
              JOIN chain_state cs ON cs.seed_id = s.id
             WHERE ${human_admin_index}
                OR (
                    NOT cs.invalid
                    AND NOT cs.incomplete
                    AND (
                        cs.boundary_depth IS NULL
                        OR EXISTS (
                            SELECT 1
                              FROM chain c
                              JOIN flow_object_grants g ON g.object_id = c.id
                             WHERE c.seed_id = s.id
                               AND c.depth <= cs.boundary_depth
                               AND g.principal_id = ${actor_index}
                               AND g.principal_kind = ${principal_kind_index}
                        )
                    )
                )
        )
        SELECT COALESCE(MAX(si.indexed_seq), 0) AS indexed_seq,
               COALESCE(MAX(cd.head_seq), 0) AS head_seq,
               COALESCE(bool_or(
                   si.object_id IS NULL
                   OR si.indexed_seq IS DISTINCT FROM p.document_seq
                   OR si.indexed_frontier IS DISTINCT FROM p.document_frontier
                   OR si.title IS DISTINCT FROM p.title
                   OR si.plain_text IS DISTINCT FROM p.plain_text
                   OR si.indexed_seq IS DISTINCT FROM cd.head_seq
               ), false) AS stale
          FROM visible v
          JOIN collab_documents cd ON cd.object_id = v.id
          JOIN flow_object_projections p ON p.object_id = v.id
     LEFT JOIN flow_search_index si ON si.object_id = v.id
        "
    );
    let row = FrontierAggregateRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(&state.db)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok(SearchIndexFrontier {
        indexed_seq: row.indexed_seq,
        head_seq: row.head_seq,
        lag: query::projection_lag(row.head_seq, row.indexed_seq),
        stale: row.stale,
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct CursorPayload {
    fingerprint: String,
    rank_bits: u32,
    object_id: Uuid,
}

fn cursor_fingerprint(search: &ValidatedSearch) -> String {
    let mut digest = Sha256::new();
    digest.update(search.workspace_id.as_bytes());
    digest.update([0]);
    digest.update(search.q.as_bytes());
    digest.update([0]);
    match search.scope {
        SearchScope::Project(project_id) => {
            digest.update(b"project");
            digest.update(project_id.as_bytes());
        }
        SearchScope::Unprojected => digest.update(b"unprojected"),
        SearchScope::AllVisible => digest.update(b"all_visible"),
    }
    digest.update([0]);
    digest.update(search.object_type.as_deref().unwrap_or("*").as_bytes());
    digest.update([0]);
    digest.update(match search.freshness {
        Freshness::AllowStale => b"allow_stale".as_slice(),
        Freshness::RequireCurrent => b"require_current".as_slice(),
    });
    hex::encode(digest.finalize())
}

fn cursor_key(secret: &str) -> Result<LessSafeKey, ApiError> {
    let mut digest = Sha256::new();
    digest.update(CURSOR_AAD);
    digest.update([0]);
    digest.update(secret.as_bytes());
    UnboundKey::new(&aead::CHACHA20_POLY1305, &digest.finalize())
        .map(LessSafeKey::new)
        .map_err(|_| ApiError::Internal)
}

fn encode_cursor(secret: &str, search: &ValidatedSearch, rank: f32, object_id: Uuid) -> Result<String, ApiError> {
    let payload = CursorPayload {
        fingerprint: cursor_fingerprint(search),
        rank_bits: rank.to_bits(),
        object_id,
    };
    let mut encrypted = serde_json::to_vec(&payload).map_err(|_| ApiError::Internal)?;
    let mut nonce_bytes = [0_u8; aead::NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| ApiError::Internal)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    cursor_key(secret)?
        .seal_in_place_append_tag(nonce, Aad::from(CURSOR_AAD), &mut encrypted)
        .map_err(|_| ApiError::Internal)?;
    let mut token = Vec::with_capacity(1 + aead::NONCE_LEN + encrypted.len());
    token.push(CURSOR_VERSION);
    token.extend_from_slice(&nonce_bytes);
    token.extend_from_slice(&encrypted);
    Ok(BASE64_URL.encode(token))
}

fn decode_cursor(secret: &str, search: &ValidatedSearch, raw: &str) -> Result<(f32, Uuid), ApiError> {
    let token = BASE64_URL
        .decode(raw)
        .map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let (version, payload) = token
        .split_first()
        .ok_or_else(|| ApiError::invalid_update("cursor is not valid"))?;
    if *version != CURSOR_VERSION {
        return Err(ApiError::invalid_update("cursor is not valid"));
    }
    let (nonce_bytes, ciphertext) = payload
        .split_at_checked(aead::NONCE_LEN)
        .ok_or_else(|| ApiError::invalid_update("cursor is not valid"))?;
    let nonce =
        Nonce::try_assume_unique_for_key(nonce_bytes).map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let mut in_out = ciphertext.to_vec();
    let plaintext = cursor_key(secret)?
        .open_in_place(nonce, Aad::from(CURSOR_AAD), &mut in_out)
        .map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let payload: CursorPayload =
        serde_json::from_slice(plaintext).map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    if payload.fingerprint != cursor_fingerprint(search) {
        return Err(ApiError::invalid_update("cursor does not belong to this search"));
    }
    let rank = f32::from_bits(payload.rank_bits);
    if !rank.is_finite() || rank < 0.0 {
        return Err(ApiError::invalid_update("cursor is not valid"));
    }
    Ok((rank, payload.object_id))
}

#[derive(Debug, FromQueryResult)]
struct SearchCandidate {
    object_id: Uuid,
    project_id: Option<Uuid>,
    object_type: String,
    lifecycle_status: String,
    title: String,
    indexed_seq: i64,
    head_seq: i64,
    projection_seq: i64,
    projection_frontier: Vec<u8>,
    indexed_frontier: Vec<u8>,
    projection_title: String,
    projection_plain_text: String,
    indexed_plain_text: String,
    title_matches: bool,
    body_matches: bool,
    title_snippet: String,
    body_snippet: String,
    rank: f32,
}

fn candidate_is_stale(row: &SearchCandidate) -> bool {
    row.indexed_seq != row.projection_seq
        || row.indexed_frontier != row.projection_frontier
        || row.title != row.projection_title
        || row.indexed_plain_text != row.projection_plain_text
        || row.indexed_seq != row.head_seq
}

async fn fetch_search_batch(
    state: &AppState,
    search: &ValidatedSearch,
    after: Option<(f32, Uuid)>,
) -> Result<Vec<SearchCandidate>, ApiError> {
    let mut values: Vec<sea_orm::Value> = vec![search.q.clone().into(), search.workspace_id.into()];
    let mut where_sql = String::from("fo.workspace_id = $2 AND fo.lifecycle_status = 'active'");
    add_scope_predicate(&mut where_sql, &mut values, search.scope, "fo");
    add_object_type_predicate(&mut where_sql, &mut values, search.object_type.as_deref(), "fo");
    let mut outer_predicate = String::new();
    if let Some((rank, object_id)) = after {
        values.push(rank.into());
        let rank_index = values.len();
        values.push(object_id.into());
        let id_index = values.len();
        let _ = write!(
            outer_predicate,
            "WHERE (rank < ${rank_index} OR (rank = ${rank_index} AND object_id > ${id_index}))"
        );
    }
    values.push(i64::try_from(SEARCH_SCAN_BATCH_SIZE).unwrap_or(i64::MAX).into());
    let limit_index = values.len();
    let sql = format!(
        r"
            WITH search_query AS (
                SELECT websearch_to_tsquery('simple', $1) AS terms
            ), ranked AS (
                SELECT fo.id AS object_id, fo.project_id, fo.object_type, fo.lifecycle_status,
                       si.title, si.indexed_seq, cd.head_seq,
                       p.document_seq AS projection_seq,
                       p.document_frontier AS projection_frontier,
                       si.indexed_frontier,
                       p.title AS projection_title,
                       p.plain_text AS projection_plain_text,
                       si.plain_text AS indexed_plain_text,
                       to_tsvector('simple', si.title) @@ sq.terms AS title_matches,
                       to_tsvector('simple', si.plain_text) @@ sq.terms AS body_matches,
                       ts_headline(
                           'simple', replace(replace(replace(si.title, '&', '&amp;'), '<', '&lt;'), '>', '&gt;'), sq.terms,
                           'StartSel=<mark>, StopSel=</mark>, MaxFragments=1, MinWords=1, MaxWords=12'
                       ) AS title_snippet,
                       ts_headline(
                           'simple', replace(replace(replace(si.plain_text, '&', '&amp;'), '<', '&lt;'), '>', '&gt;'), sq.terms,
                           'StartSel=<mark>, StopSel=</mark>, MaxFragments=2, MinWords=3, MaxWords=24'
                       ) AS body_snippet,
                       ts_rank_cd(si.search_vector, sq.terms) AS rank
                  FROM flow_search_index si
                  JOIN flow_objects fo ON fo.id = si.object_id
                  JOIN collab_documents cd ON cd.object_id = fo.id
                  JOIN flow_object_projections p ON p.object_id = fo.id
                 CROSS JOIN search_query sq
                 WHERE {where_sql} AND si.search_vector @@ sq.terms
            )
            SELECT * FROM ranked
            {outer_predicate}
            ORDER BY rank DESC, object_id ASC
            LIMIT ${limit_index}
        "
    );
    Ok(
        SearchCandidate::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .all(&state.db)
            .await?,
    )
}

fn hit_from_candidate(row: SearchCandidate) -> FlowSearchHit {
    let stale = candidate_is_stale(&row);
    let mut matched_fields = Vec::with_capacity(2);
    let mut snippets = BTreeMap::new();
    if row.title_matches {
        matched_fields.push("title".to_string());
        snippets.insert("title".to_string(), row.title_snippet);
    }
    if row.body_matches {
        matched_fields.push("plain_text".to_string());
        snippets.insert("plain_text".to_string(), row.body_snippet);
    }
    FlowSearchHit {
        object: FlowObjectSummary {
            id: row.object_id,
            object_type: row.object_type,
            title: row.title,
            lifecycle_status: row.lifecycle_status,
            project_id: row.project_id,
        },
        matched_fields,
        snippets,
        indexed_seq: row.indexed_seq,
        head_seq: row.head_seq,
        projection_lag: query::projection_lag(row.head_seq, row.indexed_seq),
        stale,
    }
}

/// Executes a Flow search at one authorization epoch. `None` asks the handler to restart after an
/// epoch change; no partial page or pre-filter count leaves this function.
pub async fn search(
    state: &AppState,
    access: &FlowReadContext,
    params: SearchParams,
) -> Result<Option<FlowSearchResponse>, ApiError> {
    if params.workspace_id != access.workspace_id() {
        return Err(ApiError::Internal);
    }
    runtime::runtime().ensure_workspace_accepting(params.workspace_id)?;
    let search = validate(params, access.is_bot())?;
    let frontier = policy_filtered_frontier(state, access, &search).await?;
    if search.freshness == Freshness::RequireCurrent && frontier.stale {
        return Err(ApiError::stale_frontier(
            "Flow search index is behind the accepted projection frontier",
            Some(frontier.head_seq),
            None,
        ));
    }

    let mut after = search
        .cursor
        .as_deref()
        .map(|raw| decode_cursor(state.cfg.jwt_secret.expose(), &search, raw))
        .transpose()?;
    let limit = usize::try_from(search.limit).unwrap_or(usize::MAX);
    let needed = limit.saturating_add(1);
    let mut accepted: Vec<SearchCandidate> = Vec::with_capacity(needed);
    let mut examined = 0_u64;
    while accepted.len() < needed {
        let batch = fetch_search_batch(state, &search, after).await?;
        let batch_len = batch.len();
        if batch_len == 0 {
            break;
        }
        let ids: Vec<Uuid> = batch.iter().map(|row| row.object_id).collect();
        let Some(visible) = policy::authorize_flow_objects(state, access, &ids, PermissionLevel::View).await? else {
            return Ok(None);
        };
        if visible.len() != batch_len {
            return Err(ApiError::Internal);
        }
        for (row, is_visible) in batch.into_iter().zip(visible) {
            examined = examined.saturating_add(1);
            check_scan_budget(examined)?;
            after = Some((row.rank, row.object_id));
            if candidate_is_policy_visible(is_visible) {
                accepted.push(row);
                if accepted.len() >= needed {
                    break;
                }
            }
        }
        if (batch_len as u64) < SEARCH_SCAN_BATCH_SIZE {
            break;
        }
    }
    if !policy::ensure_epoch_current(state, access).await? {
        return Ok(None);
    }

    let next_cursor = if accepted.len() > limit {
        accepted.truncate(limit);
        accepted
            .last()
            .map(|row| encode_cursor(state.cfg.jwt_secret.expose(), &search, row.rank, row.object_id))
            .transpose()?
    } else {
        None
    };
    Ok(Some(FlowSearchResponse {
        items: accepted.into_iter().map(hit_from_candidate).collect(),
        next_cursor,
        index_frontier: frontier,
    }))
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
    use uuid::Uuid;

    use super::{
        Freshness, SearchParams, SearchScope, check_scan_budget, cursor_fingerprint, decode_cursor, encode_cursor,
        validate,
    };

    fn params() -> SearchParams {
        SearchParams {
            workspace_id: Uuid::new_v4(),
            q: "accepted words".to_string(),
            project_id: None,
            unprojected: true,
            all_visible: false,
            object_type: None,
            freshness: None,
            cursor: None,
            limit: None,
        }
    }

    #[test]
    fn flow_search_rejects_scope_when_neither_project_nor_unprojected_is_selected() {
        let mut none = params();
        none.unprojected = false;
        assert!(validate(none, false).is_err());
    }

    #[test]
    fn flow_search_rejects_scope_when_project_and_unprojected_are_both_selected() {
        let mut both = params();
        both.project_id = Some(Uuid::new_v4());
        assert!(validate(both, false).is_err());
    }

    #[test]
    fn flow_search_rejects_bot_all_visible() {
        let mut all_visible = params();
        all_visible.unprojected = false;
        all_visible.all_visible = true;
        assert!(validate(all_visible, true).is_err());
    }

    #[test]
    fn flow_search_allows_bot_single_scope() {
        let validated = validate(params(), true).expect("a bot may use unprojected scope");
        assert_eq!(validated.scope, SearchScope::Unprojected);
        assert_eq!(validated.freshness, Freshness::AllowStale);
    }

    #[test]
    fn flow_search_query_character_and_page_limits_are_enforced() {
        let mut empty = params();
        empty.q.clear();
        assert!(validate(empty, false).is_err());

        let mut exact = params();
        exact.q = "界".repeat(256);
        exact.limit = Some(100);
        assert!(validate(exact, false).is_ok());

        let mut too_long = params();
        too_long.q = "界".repeat(257);
        assert!(validate(too_long, false).is_err());

        let mut too_many = params();
        too_many.limit = Some(101);
        assert!(validate(too_many, false).is_err());
    }

    #[test]
    fn flow_search_scan_budget_accepts_the_exact_boundary_and_rejects_plus_one() {
        assert!(check_scan_budget(super::limits::AUTHORIZED_SCAN_ROWS_MAX).is_ok());
        let error = check_scan_budget(super::limits::AUTHORIZED_SCAN_ROWS_MAX + 1)
            .expect_err("the first row beyond the fixed scan budget is rejected");
        assert_eq!(error.kind(), crate::error::ApiErrorKind::LimitExceeded);
    }

    #[test]
    fn flow_search_cursor_is_encrypted_bound_and_authenticated() {
        let search = validate(params(), false).expect("fixture validates");
        let object_id = Uuid::new_v4();
        let cursor = encode_cursor("search-secret-a", &search, 0.75, object_id).expect("cursor encrypts");
        assert_eq!(
            decode_cursor("search-secret-a", &search, &cursor).expect("cursor decrypts"),
            (0.75, object_id)
        );
        let decoded = BASE64_URL.decode(&cursor).expect("cursor is base64url");
        assert!(!String::from_utf8_lossy(&decoded).contains(&object_id.to_string()));
        assert!(!String::from_utf8_lossy(&decoded).contains("accepted words"));
        assert!(decode_cursor("search-secret-b", &search, &cursor).is_err());

        let mut other_params = params();
        other_params.q = "another query".to_string();
        let other = validate(other_params, false).expect("other fixture validates");
        assert_ne!(cursor_fingerprint(&search), cursor_fingerprint(&other));
        assert!(decode_cursor("search-secret-a", &other, &cursor).is_err());

        let mut tampered = decoded;
        let last = tampered.last_mut().expect("cursor has authenticated ciphertext");
        *last ^= 1;
        assert!(decode_cursor("search-secret-a", &search, &BASE64_URL.encode(tampered)).is_err());
    }
}

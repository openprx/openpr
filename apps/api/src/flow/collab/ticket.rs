//! `ADR-0007`: one-time, 60s TTL WebSocket tickets. Raw ticket bytes are never stored, logged, or
//! echoed back after issuance — only their SHA-256 hex digest lives in `collab_tickets`.

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use rand::RngCore;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;

use super::limits::TICKET_TTL_SECONDS;
use super::origin;

fn sha256_hex(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

pub struct IssueTicketInput {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Uuid,
    pub client_id: String,
    pub origin: String,
}

pub struct IssuedTicket {
    /// The raw secret. Returned to the caller exactly once; never persisted.
    pub ticket: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

fn validate_client_id(client_id: &str) -> Result<(), ApiError> {
    let trimmed = client_id.trim();
    if trimmed.is_empty() || trimmed.len() > 256 {
        return Err(ApiError::BadRequest(
            "client_id must be 1-256 non-whitespace-only characters".to_string(),
        ));
    }
    Ok(())
}

/// Issues a one-time ticket. Callers must already have rejected `BotAuthContext` (`ADR-0007`:
/// "handler 在 `BotAuthContext` 存在时返回 forbidden") and validated `flow_enabled` before calling
/// this — this function does the checks specific to ticket issuance: the strict Origin allowlist,
/// workspace membership, the document's membership *of that same workspace*, and its effective
/// permission.
///
/// The checks run in that order on purpose. Membership in `workspace_id` is settled before the
/// document is looked up at all, so a caller who is not a member learns nothing about which
/// documents live there; and the document lookup itself is scoped to `workspace_id`, so a member
/// asking about a document in some *other* workspace gets the same `NotFound` as one asking about
/// a document that does not exist. Neither error can be used to enumerate another tenant's
/// documents.
///
/// # Errors
/// `BadRequest` for a malformed `client_id`/`origin`; `Forbidden` when the origin is not
/// allowlisted, the caller is not a member of `workspace_id`, or the effective permission is
/// below `edit` (`ADR-0007`: "document read+write ACL"); `NotFound` when `document_id` does not
/// resolve to a `collab_documents` row whose object belongs to `workspace_id`. Propagates a
/// database failure otherwise.
pub async fn issue<C: ConnectionTrait>(
    conn: &C,
    input: IssueTicketInput,
    allowed_origins: &[String],
) -> Result<IssuedTicket, ApiError> {
    validate_client_id(&input.client_id)?;
    let normalized_origin = origin::normalize(&input.origin)
        .ok_or_else(|| ApiError::BadRequest("origin is not a well-formed scheme://host[:port]".to_string()))?;
    if !origin::is_allowed(&normalized_origin, allowed_origins) {
        return Err(ApiError::Forbidden(
            "origin is not allowlisted for collab tickets".to_string(),
        ));
    }

    #[derive(FromQueryResult)]
    struct RoleRow {
        role: String,
    }
    let role = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![input.workspace_id.into(), input.user_id.into()],
    ))
    .one(conn)
    .await?
    .map(|r| r.role)
    .ok_or_else(|| ApiError::Forbidden("not a member of this workspace".to_string()))?;

    // `collab_documents` has no `workspace_id` column of its own (migration
    // `0054_flow_data_layer.sql`); a document's tenant is reachable only through
    // `object_id -> flow_objects.workspace_id`. So the join below is the *only* place this
    // request's `document_id` is ever tied back to a workspace, and without it the
    // caller-supplied `workspace_id` was the only thing the remaining checks looked at —
    // `effective_permission` returns `full_access` for a workspace owner/admin before it ever
    // reads the object, so an owner of workspace A could obtain a ticket for a document in
    // workspace B and then both read its snapshot and commit updates to it over the resulting
    // WebSocket session.
    //
    // A document belonging to another workspace collapses into exactly the same
    // `NotFound("document not found")` as a document that does not exist, so the response cannot
    // be used to tell the two apart.
    #[derive(FromQueryResult)]
    struct DocRow {
        object_id: Uuid,
    }
    let doc = DocRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT d.object_id FROM collab_documents d \
         JOIN flow_objects o ON o.id = d.object_id \
         WHERE d.id = $1 AND o.workspace_id = $2",
        vec![input.document_id.into(), input.workspace_id.into()],
    ))
    .one(conn)
    .await?
    .ok_or_else(|| ApiError::NotFound("document not found".to_string()))?;

    let level =
        super::authz::effective_permission(conn, input.workspace_id, doc.object_id, "user", input.user_id, &role)
            .await?;
    if level < super::authz::PermissionLevel::Edit {
        return Err(ApiError::Forbidden(
            "document read+write access is required to open a collab session".to_string(),
        ));
    }

    let mut secret = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let raw = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, secret);
    let ticket_hash = sha256_hex(&raw);
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(TICKET_TTL_SECONDS);

    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO collab_tickets
                (id, ticket_hash, user_id, workspace_id, document_id, client_id, origin, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ",
        vec![
            Uuid::new_v4().into(),
            ticket_hash.into(),
            input.user_id.into(),
            input.workspace_id.into(),
            input.document_id.into(),
            input.client_id.trim().to_string().into(),
            normalized_origin.into(),
            expires_at.into(),
        ],
    ))
    .await?;

    Ok(IssuedTicket {
        ticket: raw,
        expires_at,
    })
}

pub struct ConsumedTicket {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Uuid,
    /// The `client_id` this ticket was bound to at issuance (`ADR-0007`) and just matched
    /// exactly by the `UPDATE ... WHERE client_id = $2` above — carried through so the session
    /// loop can stamp it onto every `collab_updates.origin_client_id` this connection writes,
    /// instead of that column silently staying `NULL` for every direct WebSocket write.
    pub client_id: String,
}

/// Atomically consumes a ticket (`ADR-0007` point 2): a single conditional `UPDATE` requires an
/// exact match on hash, unexpired/unconsumed state, bound `client_id`, and bound `origin` all at
/// once, so a wrong-origin or wrong-client replay attempt fails exactly like an unknown ticket
/// (`ADR-0007`: "失败、过期和重放都返回不泄密的 401/403 upgrade failure") without a separate
/// check-then-consume race window.
///
/// `GET /api/v1/collab/ws` carries no `document_id` of its own (`rest-api-v1.md`: "WebSocket
/// upgrade;无 JSON body") — the ticket is the sole source of which document this connection binds
/// to; the later `open` frame's own `document_id` is checked against
/// [`ConsumedTicket::document_id`] by the session loop, not here.
///
/// # Errors
/// `Unauthorized` on any mismatch (unknown/expired/consumed/wrong-origin/wrong-client ticket) —
/// deliberately one variant for all of them, not distinguishable on the wire. Propagates a
/// database failure otherwise.
pub async fn consume<C: ConnectionTrait>(
    conn: &C,
    raw_ticket: &str,
    client_id: &str,
    raw_origin: &str,
) -> Result<ConsumedTicket, ApiError> {
    let normalized_origin =
        origin::normalize(raw_origin).ok_or_else(|| ApiError::Unauthorized("invalid ticket".to_string()))?;
    let ticket_hash = sha256_hex(raw_ticket);

    #[derive(FromQueryResult)]
    #[allow(clippy::struct_field_names)] // matches the ticket's own bound-identity columns
    struct Row {
        user_id: Uuid,
        workspace_id: Uuid,
        document_id: Uuid,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE collab_tickets
            SET consumed_at = now()
            WHERE ticket_hash = $1
              AND consumed_at IS NULL
              AND expires_at > now()
              AND client_id = $2
              AND origin = $3
            RETURNING user_id, workspace_id, document_id
        ",
        vec![ticket_hash.into(), client_id.into(), normalized_origin.into()],
    ))
    .one(conn)
    .await?
    .ok_or_else(|| ApiError::Unauthorized("invalid ticket".to_string()))?;

    Ok(ConsumedTicket {
        user_id: row.user_id,
        workspace_id: row.workspace_id,
        document_id: row.document_id,
        client_id: client_id.to_string(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::validate_client_id;

    #[test]
    fn client_id_rejects_empty_and_overlong() {
        assert!(validate_client_id("").is_err());
        assert!(validate_client_id(&"x".repeat(257)).is_err());
        assert!(validate_client_id("client-1").is_ok());
    }
}

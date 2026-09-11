# Sylvode Flow v0.6 Collections boundaries

## Universal Forms SQL ownership scan

The `flow_collection_forms_tables_untouched_scans_executable_sql_paths` regression is a deliberately bounded static check. It recursively discovers Rust production sources under `apps/api/src`, `apps/worker/src`, `apps/mcp-server/src`, and `crates`, asserts that discovery is non-empty, normalizes whitespace and quoted identifiers, and rejects literal mutations of the four Universal Forms tables outside the reviewed Forms owners:

- `apps/api/src/forms/projections.rs`
- `apps/api/src/routes/form.rs`
- `apps/api/src/routes/project.rs`

The same check scans those Forms owners for literal reads of the Flow canonical/projection tables. A new SQL execution location therefore enters the scan automatically; a new Forms writer requires an explicit allowlist review.

The tokenizer recognizes bare and schema-qualified table identifiers, including `public.form_records`, and the literal DML shapes `INSERT INTO`, `UPDATE`, `UPDATE ONLY`, `DELETE FROM`, `DELETE FROM ONLY`, `MERGE INTO`, `TRUNCATE [TABLE] [ONLY]`, and `COPY`. Those forms are regression fixtures, not entries in the bypass list.

This is not a SQL parser and must not be represented as complete protection. It can still be bypassed by a table name assembled dynamically (for example, `format!("INSERT INTO {table}")`), SQL loaded from a non-Rust resource, a stored procedure, a macro whose expanded SQL is absent from the scanned source text, ORM-generated writes with no literal DML in Rust source, database indirection through writable views or triggers, or a new non-Rust execution runtime. Runtime database roles and grants remain the authoritative enforcement layer. Any introduction of dynamic identifiers must use the repository's identifier validator and needs a dedicated database-level boundary test.

## Client CRDT and field secrecy

Collection and Record documents are server-only typed-command aggregates in v0.6. Generic content commands and direct collaboration tickets reject both object types. Consequently a Collection with field secrecy policy never exposes a complete Record CRDT snapshot or advertises realtime field collaboration; its supported surface is the server-side semantic query and mutation API. v0.6 does not claim field-level secrecy inside a client-readable Record document.

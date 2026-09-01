-- Sylvode Flow v0.5 — the index the cross-project `move_object` cascade needs to stay linear.
--
-- Migration `0056` added `flow_objects_parent_project_fk`:
--
--     FOREIGN KEY (parent_id, project_scope_id) REFERENCES flow_objects (id, project_scope_id)
--
-- `project_scope_id` is `GENERATED ALWAYS AS (COALESCE(project_id, nil)) STORED`, so the cascade's
-- single `UPDATE ... SET project_id = ...` rewrites a **referenced** key column on every row it
-- touches. That fires the constraint's `ON UPDATE NO ACTION` trigger once per updated row, whose
-- query is:
--
--     SELECT 1 FROM ONLY flow_objects x
--      WHERE $1 OPERATOR(pg_catalog.=) x.parent_id
--        AND $2 OPERATOR(pg_catalog.=) x.project_scope_id
--      FOR KEY SHARE
--
-- No index on `flow_objects` leads with `parent_id, project_scope_id`: `idx_flow_objects_parent`
-- is `(workspace_id, parent_id)` (wrong leading column) and `flow_objects_project_scope_key` is
-- `(id, project_scope_id)` (`parent_id` is not in it at all). The planner therefore falls back to
-- scanning `flow_objects_project_scope_key` on its **non-leading** column and filtering on
-- `parent_id` — measured on the `N = 5000` fixture of `apps/api/tests/flow_v05_subtree_budget.rs`:
-- `Index Cond: (project_scope_id = ...)`, `Filter: (... = parent_id)`, 131 buffers, 0.531 ms per
-- probe. `N` probes over an index that itself grows with `N` makes one cascade `O(N^2)`.
--
-- Counterfactual, measured rather than argued (same harness, `OPENPR_FLOW_SUBTREE_ADD_FK_INDEX=1`):
-- with this index present, `N = 800` and `N = 1000` went from 11 and 15 attempts killed by the
-- 100 ms statement budget to **0**, and `N = 5000` with the statement budget lifted went from
-- `hold p95 9.30 s` to `0.46 s`.
--
-- What this index does **not** do, stated so it is not over-claimed: it does not move
-- `move_subtree_nodes_max`. `limits-v1.md` freezes that at 100 on the lock-hold budget, and with
-- the index the cascade `UPDATE` is still ~0.052 ms/node (linear) — the 25 ms crossing stays in
-- the same order of magnitude. The value is not an artefact of the missing index; this index is
-- what stops the cost curve from being quadratic *below* it.
--
-- `workspace_id` is deliberately absent: the RI query above does not mention it, so including it
-- as a leading column would produce an index the trigger cannot use — which is precisely how
-- `idx_flow_objects_parent` came to be useless here.
--
-- `IF NOT EXISTS` because this file re-runs on every database whose migration ledger has no row
-- for it, and because `apps/api/tests/flow_v05_subtree_budget.rs` creates the same index by the
-- same name when measuring the counterfactual curve.
CREATE INDEX IF NOT EXISTS idx_flow_objects_parent_project_scope
  ON flow_objects (parent_id, project_scope_id);

COMMENT ON INDEX idx_flow_objects_parent_project_scope IS
  'ADR-0013 s2.2 R17: serves the referential-integrity probe of flow_objects_parent_project_fk, which fires once per row rewritten by a cross-project move_object cascade. Without it the cascade is O(N^2).';

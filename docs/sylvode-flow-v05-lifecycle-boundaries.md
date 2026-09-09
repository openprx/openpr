# Sylvode Flow v0.5 lifecycle boundaries

## Archive and restore request semantics

The registered v0.4/v0.5 `archive|restore` command payload does not contain a cascade operation.
It means a reversible, non-cascading transition of the addressed object, including when that Page
currently has children. This is the first row of `ADR-0012` section 2 and preserves the v0.4
`default_member_level=edit` baseline. Unknown payload members stay tolerated for v0.4 wire
compatibility, but do not change the impact set.

The matrix's separate "cascade the whole subtree" tier therefore cannot be inferred from tree
shape. It requires a future, explicitly registered request semantic; that operation will need
`full_access`, a complete locked impact set, and complete `affected_object_ids`. Until such a
surface exists, `archive|restore` changes and reports only the addressed object.

The v0.5 lifecycle permission matrix names two cases that this source version cannot construct:

- A Collection container does not exist in the v0.5 object registry. Migration
  `0054_flow_data_layer.sql` restricts `flow_objects.object_type` to `navigator|page`, and the
  command registry assigns Collection/Record creation to v0.6.
- Archive/restore are reversible lifecycle transitions only. There is no retention-triggering,
  purge, hard-delete, or permanent-cleanup Flow action in this version; compaction/retention is a
  v0.8 concern.

Those two matrix rows therefore cannot honestly pass an executable v0.5 fixture. They require a
version-boundary disposition (an explicit reason-code exclusion for v0.5, or transfer to the
version that introduces the object/action), not fabricated object types or unreachable test-only
actions.

The separate root-Page/navigator contract conflict is intentionally not changed here. Root object
policy continues to follow the current `parent_id IS NULL` contract while its verifier remains
red pending a contract-owner exception and the planned navigator-root implementation.

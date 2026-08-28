import type { Artifact } from "./artifact";
import { writeJsonArtifact } from "./artifact";
import type { CaseRunResult } from "./corpus-runner";

/**
 * Hand-typed mirror of `docs/schemas/sylvode-flow-convergence-result-v1.schema.json`
 * `$defs.case_category` -- the full 14-value enum, even though this package's
 * corpus runner currently only produces 4 of them. Kept as the schema's own
 * enum (not narrowed to our 4) so a `SchemaCaseResult` built here always
 * type-checks against a hand-written literal case_category from the schema.
 */
export type SchemaCaseCategory =
  | "same_text_range_edit"
  | "concurrent_block_move"
  | "ancestor_delete_descendant_move"
  | "concurrent_reorder"
  | "same_key_nested_container_creation"
  | "duplicate_out_of_order_partial_batch"
  | "offline_reconnect"
  | "snapshot_tail_restore"
  | "snapshot_boundary_update"
  | "adversarial_update_limits"
  | "unicode_ime"
  | "local_undo_remote_update"
  | "cursor_target_move_delete"
  | "policy_rejected_intent_recovery";

export type SchemaStatus = "passed" | "failed";

/** Mirrors `$defs.case_result`. `additionalProperties:false` in the schema -- this type has no extra fields either. */
export type SchemaCaseResult = Readonly<{
  id: string;
  category: SchemaCaseCategory;
  seed: string;
  status: SchemaStatus;
  operation_log: Artifact;
  final_semantic_json: Artifact;
  semantic_hash: string;
  binary_sizes: Readonly<{ update_bytes: number; snapshot_bytes: number }>;
  failure_minimal_reproduction?: Artifact;
}>;

const SHA256_PATTERN = /^[0-9a-f]{64}$/;

export function validateCaseResult(value: SchemaCaseResult): readonly string[] {
  const errors: string[] = [];
  if (value.id.length < 1) errors.push("id must be non-empty");
  if (value.seed.length < 1) errors.push("seed must be non-empty");
  if (!SHA256_PATTERN.test(value.semantic_hash)) errors.push(`semantic_hash "${value.semantic_hash}" is not 64 lowercase hex chars`);
  for (const [label, artifact] of [
    ["operation_log", value.operation_log],
    ["final_semantic_json", value.final_semantic_json],
    ...(value.failure_minimal_reproduction ? [["failure_minimal_reproduction", value.failure_minimal_reproduction] as const] : []),
  ] as const) {
    if (artifact.path.startsWith("/")) errors.push(`${label}.path must be relative, got "${artifact.path}"`);
    if (artifact.path.length < 1) errors.push(`${label}.path must be non-empty`);
    if (!SHA256_PATTERN.test(artifact.sha256)) errors.push(`${label}.sha256 is not 64 lowercase hex chars`);
  }
  if (!Number.isInteger(value.binary_sizes.update_bytes) || value.binary_sizes.update_bytes < 0) {
    errors.push("binary_sizes.update_bytes must be a non-negative integer");
  }
  if (!Number.isInteger(value.binary_sizes.snapshot_bytes) || value.binary_sizes.snapshot_bytes < 0) {
    errors.push("binary_sizes.snapshot_bytes must be a non-negative integer");
  }
  if (value.status === "failed" && !value.failure_minimal_reproduction) {
    errors.push('status "failed" requires failure_minimal_reproduction (schema allOf/if/then)');
  }
  return errors;
}

/**
 * Writes a `CaseRunResult` (this package's internal shape) out as real files
 * under `baseDir` and returns the schema-shaped `case_result` object with
 * genuine `{path, sha256}` artifact references -- not a hand-typed stand-in.
 */
export async function materializeCaseResult(baseDir: string, candidate: string, result: CaseRunResult): Promise<SchemaCaseResult> {
  const caseDir = `${candidate}/${result.category}`;
  const operationLog = await writeJsonArtifact(baseDir, `${caseDir}/operation-log.json`, result.operationLog);
  const finalSemanticJson = await writeJsonArtifact(baseDir, `${caseDir}/final-semantic.json`, result.finalSemanticJson);
  const failureMinimalReproduction =
    result.status === "failed"
      ? await writeJsonArtifact(baseDir, `${caseDir}/failure-reproduction.json`, {
          detail: result.detail,
          operation_log: result.operationLog,
        })
      : undefined;

  const base = {
    id: result.id,
    category: result.category,
    seed: result.seed,
    status: result.status,
    operation_log: operationLog,
    final_semantic_json: finalSemanticJson,
    semantic_hash: result.semanticHash,
    binary_sizes: { update_bytes: result.binarySizes.updateBytes, snapshot_bytes: result.binarySizes.snapshotBytes },
  };
  return failureMinimalReproduction ? { ...base, failure_minimal_reproduction: failureMinimalReproduction } : base;
}

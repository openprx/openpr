import { describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { runSameTextRangeEditCase } from "../src/corpus-runner";
import { materializeCaseResult, validateCaseResult } from "../src/result-types";
import { sha256Hex } from "../src/hash";
import { FakeScenarioAdapter } from "./support/fake-scenario-adapter";

describe("materializeCaseResult / validateCaseResult (schema-shape alignment with docs/schemas/sylvode-flow-convergence-result-v1.schema.json $defs.case_result)", () => {
  test("a real passing case materializes to a schema-valid case_result with real on-disk artifacts", async () => {
    const runResult = await runSameTextRangeEditCase(() => new FakeScenarioAdapter(), "result-writer-seed-1");
    expect(runResult.status).toBe("passed");

    const dir = await mkdtemp(join(tmpdir(), "collab-shared-result-writer-"));
    try {
      const schemaResult = await materializeCaseResult(dir, "loro", runResult);
      expect(validateCaseResult(schemaResult)).toEqual([]);
      expect(schemaResult.status).toBe("passed");
      expect(schemaResult.failure_minimal_reproduction).toBeUndefined();

      // artifact.path is relative and the sha256 matches the real bytes written to disk.
      expect(schemaResult.operation_log.path.startsWith("/")).toBe(false);
      const onDisk = await readFile(join(dir, schemaResult.operation_log.path));
      expect(await sha256Hex(new Uint8Array(onDisk))).toBe(schemaResult.operation_log.sha256);
      const onDiskFinal = await readFile(join(dir, schemaResult.final_semantic_json.path));
      expect(await sha256Hex(new Uint8Array(onDiskFinal))).toBe(schemaResult.final_semantic_json.sha256);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  test("a failing case materializes with a required failure_minimal_reproduction artifact", async () => {
    class AlwaysDivergesAdapter extends FakeScenarioAdapter {
      override toDebugJson(): unknown {
        return { random: Math.random() };
      }
    }
    const runResult = await runSameTextRangeEditCase(() => new AlwaysDivergesAdapter(), "result-writer-seed-2");
    expect(runResult.status).toBe("failed");

    const dir = await mkdtemp(join(tmpdir(), "collab-shared-result-writer-"));
    try {
      const schemaResult = await materializeCaseResult(dir, "yrs-yjs", runResult);
      expect(validateCaseResult(schemaResult)).toEqual([]);
      expect(schemaResult.failure_minimal_reproduction).toBeDefined();
      const reproPath = schemaResult.failure_minimal_reproduction?.path;
      expect(typeof reproPath).toBe("string");
      const onDisk = await readFile(join(dir, reproPath as string), "utf8");
      expect(JSON.parse(onDisk).detail).toContain("diverged");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  test("validateCaseResult rejects a status:\"failed\" result with no failure_minimal_reproduction (schema's if/then requirement)", () => {
    const errors = validateCaseResult({
      id: "x",
      category: "same_text_range_edit",
      seed: "s",
      status: "failed",
      operation_log: { path: "a.json", sha256: "0".repeat(64) },
      final_semantic_json: { path: "b.json", sha256: "0".repeat(64) },
      semantic_hash: "0".repeat(64),
      binary_sizes: { update_bytes: 1, snapshot_bytes: 1 },
    });
    expect(errors).toContain('status "failed" requires failure_minimal_reproduction (schema allOf/if/then)');
  });

  test("validateCaseResult rejects a non-hex, non-64-char semantic_hash and an absolute artifact path", () => {
    const errors = validateCaseResult({
      id: "x",
      category: "same_text_range_edit",
      seed: "s",
      status: "passed",
      operation_log: { path: "/absolute.json", sha256: "not-hex" },
      final_semantic_json: { path: "b.json", sha256: "0".repeat(64) },
      semantic_hash: "too-short",
      binary_sizes: { update_bytes: 1, snapshot_bytes: 1 },
    });
    expect(errors.some((e) => e.includes("semantic_hash"))).toBe(true);
    expect(errors.some((e) => e.includes("operation_log.path must be relative"))).toBe(true);
    expect(errors.some((e) => e.includes("operation_log.sha256"))).toBe(true);
  });
});

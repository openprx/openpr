export type { CandidateName, EngineAdapter, EngineDiff, Frontier } from "./contracts";
export type { EditorAdapter, EditorOrigin, RelativeSelection, RichTextCommand } from "./editor";
export type { EngineStore, IndexedDbEngineStore, StoredDocument } from "./storage";
export { SplitMix64 } from "./prng";
export type { AbstractOp, AbstractTextOp, AbstractTreeOp, ScenarioAdapter, ScenarioAdapterFactory } from "./testing";
export {
  generateCorruptUpdateFixture,
  generateDuplicateApplyFixture,
  generateTextCoEditFixture,
  generateTreeMoveFixture,
} from "./fixture";
export { canonicalStringify, semanticHash, sha256Hex } from "./hash";
export type { CaseCategory, CaseRunResult } from "./corpus-runner";
export {
  runConcurrentBlockMoveCase,
  runCorruptUpdateRejectedCase,
  runDuplicateApplyIdempotentCase,
  runFullCorpus,
  runSameTextRangeEditCase,
} from "./corpus-runner";
export type { DistributionMetric, SampleOptions } from "./benchmark-runner";
export { sampleAsync, summarizeDistribution } from "./benchmark-runner";
export type { Artifact } from "./artifact";
export { writeArtifact, writeJsonArtifact } from "./artifact";
export type { SchemaCaseCategory, SchemaCaseResult, SchemaStatus } from "./result-types";
export { materializeCaseResult, validateCaseResult } from "./result-types";

export { webIsolationKind } from "./isolation/worker-protocol";
export type {
  WorkerApplyLimits,
  WorkerApplyRequest,
  WorkerApplyResponse,
  WorkerCancelRequest,
  WorkerFailureResponse,
  WorkerRequest,
  WorkerResponse,
  WorkerSandboxHost,
} from "./isolation/worker-protocol";

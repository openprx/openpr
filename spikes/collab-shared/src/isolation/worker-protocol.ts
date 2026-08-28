export const webIsolationKind = "worker_sandbox" as const;

export type WorkerApplyLimits = Readonly<{
  cpuBudgetMs: number;
  wallBudgetMs: number;
  memoryBudgetBytes: number;
}>;

export type WorkerApplyRequest = Readonly<{
  kind: "apply";
  requestId: string;
  update: Uint8Array;
  limits: WorkerApplyLimits;
}>;

export type WorkerCancelRequest = Readonly<{
  kind: "cancel";
  requestId: string;
}>;

export type WorkerRequest = WorkerApplyRequest | WorkerCancelRequest;

export type WorkerApplyResponse = Readonly<{
  kind: "applied";
  requestId: string;
  consumedBytes: number;
}>;

export type WorkerFailureResponse = Readonly<{
  kind: "failed";
  requestId: string;
  code: "cancelled" | "cpu_limit" | "wall_limit" | "memory_limit" | "engine_error";
}>;

export type WorkerResponse = WorkerApplyResponse | WorkerFailureResponse;

export interface WorkerSandboxHost {
  readonly isolationKind: typeof webIsolationKind;
  apply(request: WorkerApplyRequest, signal: AbortSignal): Promise<WorkerResponse>;
  cancel(requestId: string): void;
  dispose(): void;
}

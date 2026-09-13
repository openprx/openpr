// State boundary for the v0.8 package-import wizard. Package bytes are handed directly to the
// HTTP adapter and are never copied into ObjectRepository, IndexedDB, or this session object.

import {
	flowApi,
	type FlowPackageArtifactReceipt,
	type FlowPackageConflictPolicy,
	type FlowPackageExternalReferencePolicy,
	type FlowPackageImportJobReceipt,
	type FlowPackageImportPreview
} from '$lib/api/flow';

function requireData<T>(result: { code: number; message: string; data: T | null }): T {
	if (result.code !== 0 || result.data === null)
		throw new Error(result.message || 'Package import failed');
	return result.data;
}

export class FlowPackageImportSession {
	private artifact: FlowPackageArtifactReceipt | null = null;
	private previewReceipt: FlowPackageImportPreview | null = null;
	private previewConflictPolicy: FlowPackageConflictPolicy | null = null;

	constructor(private readonly workspaceId: string) {}

	async upload(
		file: Blob,
		filename: string,
		idempotencyKey: string,
		signal?: AbortSignal
	): Promise<FlowPackageArtifactReceipt> {
		this.artifact = null;
		this.previewReceipt = null;
		this.previewConflictPolicy = null;
		this.artifact = requireData(
			await flowApi.uploadPackageArtifact(this.workspaceId, file, filename, idempotencyKey, signal)
		);
		return this.artifact;
	}

	async preview(input: {
		projectMapping?: Record<string, string | null>;
		externalReferencePolicy: FlowPackageExternalReferencePolicy;
		conflictPolicy: FlowPackageConflictPolicy;
		includeHistory: boolean;
		idempotencyKey: string;
	}): Promise<FlowPackageImportPreview> {
		if (!this.artifact) throw new Error('A verified package artifact is required before preview');
		this.previewReceipt = null;
		this.previewConflictPolicy = null;
		this.previewReceipt = requireData(
			await flowApi.previewPackageImport(this.workspaceId, {
				artifact_id: this.artifact.artifact_id,
				project_mapping: input.projectMapping ?? {},
				external_reference_policy: input.externalReferencePolicy,
				conflict_policy: input.conflictPolicy,
				include_history: input.includeHistory,
				idempotency_key: input.idempotencyKey
			})
		);
		this.previewConflictPolicy = input.conflictPolicy;
		return this.previewReceipt;
	}

	async commit(input: {
		exactPackageSha256: string;
		idempotencyKey: string;
	}): Promise<FlowPackageImportJobReceipt> {
		const preview = this.previewReceipt;
		const conflictPolicy = this.previewConflictPolicy;
		if (!preview || !conflictPolicy)
			throw new Error('A successful server preview is required before commit');
		if (input.exactPackageSha256 !== preview.package_sha256) {
			throw new Error('The confirmed package hash does not match the server preview');
		}
		return requireData(
			await flowApi.commitPackageImport(this.workspaceId, preview.preview_id, {
				package_sha256: preview.package_sha256,
				mapping_hash: preview.mapping_hash,
				conflict_policy: conflictPolicy,
				confirm: true,
				idempotency_key: input.idempotencyKey
			})
		);
	}
}

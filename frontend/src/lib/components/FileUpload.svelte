<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { apiClient } from '$lib/api/client';
	import { toast } from '$lib/stores/toast';
	import type { UploadedAttachment } from '$lib/utils/attachments';
	import { ISSUE_ATTACHMENT_ACCEPT, ISSUE_ATTACHMENT_MAX_SIZE_BYTES } from '$lib/utils/attachments';
	import { t } from 'svelte-i18n';

	interface UploadEventDetail {
		url: string;
		filename: string;
	}

	interface Props {
		accept?: string;
		maxSize?: number;
		multiple?: boolean;
		value?: UploadedAttachment[];
		compact?: boolean;
	}

	type UploadApiResult = {
		code?: number;
		message?: string;
		data?: { url?: string; filename?: string };
		url?: string;
		filename?: string;
	};

	type PendingUpload = {
		id: string;
		filename: string;
		size: number;
		progress: number;
	};

	let {
		accept = ISSUE_ATTACHMENT_ACCEPT,
		maxSize = ISSUE_ATTACHMENT_MAX_SIZE_BYTES,
		multiple = true,
		value = $bindable<UploadedAttachment[]>([]),
		compact = false
	}: Props = $props();

	const dispatch = createEventDispatcher<{
		upload: UploadEventDetail;
	}>();

	let fileInput = $state<HTMLInputElement | null>(null);
	let dragActive = $state(false);
	let pendingUploads = $state<PendingUpload[]>([]);

	function formatBytes(size: number): string {
		if (size < 1024) {
			return `${size} B`;
		}
		if (size < 1024 * 1024) {
			return `${(size / 1024).toFixed(1)} KB`;
		}
		return `${(size / (1024 * 1024)).toFixed(1)} MB`;
	}

	function parseAcceptTokens(acceptValue: string): string[] {
		return acceptValue
			.split(',')
			.map((item) => item.trim().toLowerCase())
			.filter(Boolean);
	}

	function isAccepted(file: File): boolean {
		const tokens = parseAcceptTokens(accept);
		if (tokens.length === 0) {
			return true;
		}

		const fileName = file.name.toLowerCase();
		const mimeType = file.type.toLowerCase();

		return tokens.some((token) => {
			if (token.startsWith('.')) {
				return fileName.endsWith(token);
			}
			if (token.endsWith('/*')) {
				return mimeType.startsWith(token.slice(0, -1));
			}
			return mimeType === token;
		});
	}

	function updatePendingProgress(uploadId: string, progress: number): void {
		pendingUploads = pendingUploads.map((item) =>
			item.id === uploadId ? { ...item, progress } : item
		);
	}

	function removePending(uploadId: string): void {
		pendingUploads = pendingUploads.filter((item) => item.id !== uploadId);
	}

	function normalizeUploadResult(result: UploadApiResult, fallbackFileName: string): UploadEventDetail | null {
		if (result.code === 0 && result.data?.url) {
			return {
				url: result.data.url,
				filename: result.data.filename || fallbackFileName
			};
		}
		if (result.url) {
			return {
				url: result.url,
				filename: result.filename || fallbackFileName
			};
		}
		return null;
	}

	async function uploadFile(file: File): Promise<void> {
		const uploadId = `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
		pendingUploads = [...pendingUploads, { id: uploadId, filename: file.name, size: file.size, progress: 0 }];

		const formData = new FormData();
		formData.append('file', file);

		await new Promise<void>((resolve) => {
			const xhr = new XMLHttpRequest();
			xhr.open('POST', '/api/v1/upload');

			const token = apiClient.getToken();
			if (token) {
				xhr.setRequestHeader('Authorization', `Bearer ${token}`);
			}

			xhr.upload.onprogress = (event) => {
				if (!event.lengthComputable) {
					return;
				}
				const progress = Math.round((event.loaded / event.total) * 100);
				updatePendingProgress(uploadId, progress);
			};

			xhr.onload = () => {
				removePending(uploadId);
				if (xhr.status < 200 || xhr.status >= 300) {
					toast.error($t('toast.uploadFail'));
					resolve();
					return;
				}

				try {
					const raw = JSON.parse(xhr.responseText) as UploadApiResult;
					const normalized = normalizeUploadResult(raw, file.name);
					if (!normalized) {
						toast.error(raw.message || $t('toast.uploadFail'));
						resolve();
						return;
					}

					value = [
						...value,
						{
							url: normalized.url,
							filename: normalized.filename,
							size: file.size,
							mimeType: file.type
						}
					];
					dispatch('upload', normalized);
					toast.success($t('toast.uploadSuccess'));
				} catch {
					toast.error($t('toast.uploadParseFail'));
				}
				resolve();
			};

			xhr.onerror = () => {
				removePending(uploadId);
				toast.error($t('toast.uploadNetworkFail'));
				resolve();
			};

			xhr.send(formData);
		});
	}

	async function processFiles(inputFiles: File[]): Promise<void> {
		const candidates = multiple ? inputFiles : inputFiles.slice(0, 1);

		for (const file of candidates) {
			if (!isAccepted(file)) {
				toast.error($t('issue.uploadTypeUnsupported'));
				continue;
			}
			if (file.size > maxSize) {
				toast.error($t('issue.uploadSizeLimit', { values: { size: Math.floor(maxSize / (1024 * 1024)) } }));
				continue;
			}
			await uploadFile(file);
		}
	}

	async function handleInputChange(event: Event): Promise<void> {
		const input = event.target as HTMLInputElement;
		const selectedFiles = Array.from(input.files ?? []);
		if (selectedFiles.length > 0) {
			await processFiles(selectedFiles);
		}
		input.value = '';
	}

	function openFilePicker(): void {
		fileInput?.click();
	}

	function handleDragOver(event: DragEvent): void {
		event.preventDefault();
		dragActive = true;
	}

	function handleDragLeave(event: DragEvent): void {
		event.preventDefault();
		dragActive = false;
	}

	async function handleDrop(event: DragEvent): Promise<void> {
		event.preventDefault();
		dragActive = false;
		const droppedFiles = Array.from(event.dataTransfer?.files ?? []);
		if (droppedFiles.length > 0) {
			await processFiles(droppedFiles);
		}
	}

	function removeAttachment(target: UploadedAttachment): void {
		value = value.filter((item) => item !== target);
	}
</script>

<div class="space-y-2">
	<input
		bind:this={fileInput}
		type="file"
		class="hidden"
		accept={accept}
		{multiple}
		onchange={handleInputChange}
	/>

	{#if compact}
		<button
			type="button"
			class="inline-flex items-center gap-1 rounded-md border border-slate-300 px-2.5 py-1.5 text-sm text-slate-700 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-700"
			onclick={openFilePicker}
		>
			<span aria-hidden="true">📎</span>
			<span>{$t('issue.addAttachments')}</span>
		</button>
	{:else}
		<div
			role="button"
			tabindex="0"
			class={`rounded-md border-2 border-dashed px-4 py-6 text-center transition ${
				dragActive
					? 'border-blue-400 bg-blue-50 dark:bg-blue-950/30'
					: 'border-slate-300 dark:border-slate-600 bg-slate-50 dark:bg-slate-900'
			}`}
			onclick={openFilePicker}
			ondragover={handleDragOver}
			ondragleave={handleDragLeave}
			ondrop={handleDrop}
			onkeydown={(event) => {
				if (event.key === 'Enter' || event.key === ' ') {
					event.preventDefault();
					openFilePicker();
				}
			}}
		>
			<p class="text-sm font-medium text-slate-700 dark:text-slate-200">📎 {$t('issue.addAttachments')}</p>
			<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">{$t('issue.dragFilesHere')}</p>
			<p class="mt-1 text-xs text-slate-400 dark:text-slate-500">{$t('issue.maxFileSizeHint', { values: { size: Math.floor(maxSize / (1024 * 1024)) } })}</p>
		</div>
	{/if}

	{#if pendingUploads.length > 0}
		<div class="space-y-2">
			{#each pendingUploads as pending (pending.id)}
				<div class="rounded-md border border-slate-200 bg-white px-3 py-2 text-sm dark:border-slate-700 dark:bg-slate-900">
					<div class="mb-1 flex items-center justify-between gap-3">
						<span class="truncate text-slate-700 dark:text-slate-200">{pending.filename}</span>
						<span class="text-xs text-slate-500 dark:text-slate-400">{$t('issue.uploading')} {pending.progress}%</span>
					</div>
					<div class="h-2 w-full overflow-hidden rounded bg-slate-100 dark:bg-slate-700">
						<div class="h-full rounded bg-blue-500 transition-all duration-200" style={`width: ${pending.progress}%`}></div>
					</div>
				</div>
			{/each}
		</div>
	{/if}

	{#if value.length > 0}
		<div class="grid gap-2 sm:grid-cols-2">
			{#each value as file (file.url)}
				<div class="flex items-start gap-2 rounded-md border border-slate-200 bg-white px-3 py-2 text-sm dark:border-slate-700 dark:bg-slate-900">
					<div class="pt-0.5" aria-hidden="true">{file.mimeType.startsWith('image/') ? '🖼️' : '📄'}</div>
					<div class="min-w-0 flex-1">
						<a
							href={file.url}
							target="_blank"
							rel="noreferrer"
							class="block truncate text-blue-600 hover:text-blue-800 dark:text-blue-400 dark:hover:text-blue-300"
						>
							{file.filename}
						</a>
						<p class="text-xs text-slate-500 dark:text-slate-400">{formatBytes(file.size)}</p>
					</div>
					<button
						type="button"
						class="rounded p-1 text-slate-500 hover:bg-slate-100 hover:text-slate-700 dark:text-slate-300 dark:hover:bg-slate-700"
						aria-label={$t('issue.removeAttachment')}
						onclick={() => removeAttachment(file)}
					>
						x
					</button>
				</div>
			{/each}
		</div>
	{/if}
</div>

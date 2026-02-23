<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { toast } from '$lib/stores/toast';
	import { t } from 'svelte-i18n';

	interface Props {
		onUploaded: (url: string) => void;
	}

	let { onUploaded }: Props = $props();

	let uploading = $state(false);
	let progress = $state(0);
	let previewUrl = $state('');

	function isAllowedImage(type: string): boolean {
		return ['image/png', 'image/jpeg', 'image/jpg', 'image/gif', 'image/webp'].includes(
			type.toLowerCase()
		);
	}

	async function uploadFile(file: File): Promise<void> {
		uploading = true;
		progress = 0;
		previewUrl = URL.createObjectURL(file);

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
				if (event.lengthComputable) {
					progress = Math.round((event.loaded / event.total) * 100);
				}
			};

			xhr.onload = () => {
				uploading = false;
				if (xhr.status < 200 || xhr.status >= 300) {
					toast.error($t('toast.uploadFail'));
					resolve();
					return;
				}

				try {
					const result = JSON.parse(xhr.responseText) as {
						code?: number;
						message?: string;
						data?: { url?: string };
					};
					if (result.code === 0 && result.data?.url) {
						progress = 100;
						onUploaded(result.data.url);
						toast.success($t('toast.uploadSuccess'));
					} else {
						toast.error(result.message || $t('toast.uploadFail'));
					}
				} catch {
					toast.error($t('toast.uploadParseFail'));
				}

				resolve();
			};

			xhr.onerror = () => {
				uploading = false;
				toast.error($t('toast.uploadNetworkFail'));
				resolve();
			};

			xhr.send(formData);
		});
	}

	async function handleUpload(event: Event): Promise<void> {
		const input = event.target as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) {
			return;
		}

		if (!isAllowedImage(file.type)) {
			toast.error($t('toast.uploadTypeFail'));
			input.value = '';
			return;
		}

		if (file.size > 10 * 1024 * 1024) {
			toast.error($t('toast.uploadSizeFail'));
			input.value = '';
			return;
		}

		await uploadFile(file);
		input.value = '';
	}
</script>

<div class="space-y-2">
	<label class="inline-flex cursor-pointer items-center gap-1 text-sm text-blue-600 hover:text-blue-800">
		<span class="inline-flex items-center gap-1">
			<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21.44 11.05l-8.49 8.49a5 5 0 01-7.07-7.07l8.49-8.49a3 3 0 114.24 4.24l-8.49 8.49a1 1 0 01-1.41-1.41l7.78-7.78"></path>
			</svg>
			{$t('issue.uploadImage')}
		</span>
		<input type="file" accept="image/png,image/jpeg,image/gif,image/webp" class="hidden" onchange={handleUpload} />
	</label>

	{#if uploading}
		<div class="space-y-1">
			<div class="h-2 w-full overflow-hidden rounded bg-slate-100 dark:bg-slate-800">
				<div
					class="h-full rounded bg-blue-500 transition-all duration-200"
					style={`width: ${progress}%;`}
				></div>
			</div>
			<p class="text-xs text-slate-500 dark:text-slate-400">{$t('search.searching')} {progress}%</p>
		</div>
	{/if}

	{#if previewUrl}
		<div class="overflow-hidden rounded-md border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-950 p-2">
			<img src={previewUrl} alt={$t('issue.previewMode')} class="max-h-32 rounded object-contain" />
		</div>
	{/if}
</div>

<script lang="ts">
	import { toast, type Toast } from '$lib/stores/toast';
	import { t } from 'svelte-i18n';

	let toasts = $state<Toast[]>([]);

	toast.subscribe((value) => {
		toasts = value;
	});

	function getToastColor(type: string) {
		const colors: Record<string, string> = {
			success: 'bg-green-500',
			error: 'bg-red-500',
			info: 'bg-blue-500',
			warning: 'bg-yellow-500'
		};
		return colors[type] || 'bg-slate-500';
	}
</script>

<div class="fixed bottom-4 right-4 z-50 space-y-2">
	{#each toasts as item (item.id)}
		<div
			class="flex items-center gap-3 min-w-[300px] max-w-md px-4 py-3 rounded-lg shadow-lg dark:shadow-slate-900/50 text-white {getToastColor(
				item.type
			)} animate-slide-in"
		>
			<div class="flex-1">
				<p class="text-sm font-medium">{item.message}</p>
			</div>
			<button
				onclick={() => toast.remove(item.id)}
				class="text-white hover:bg-white dark:bg-slate-900/20 rounded p-1"
				aria-label={$t('common.close')}
			>
				<svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="2"
						d="M6 18L18 6M6 6l12 12"
					></path>
				</svg>
			</button>
		</div>
	{/each}
</div>

<style>
	@keyframes slide-in {
		from {
			transform: translateX(100%);
			opacity: 0;
		}
		to {
			transform: translateX(0);
			opacity: 1;
		}
	}

	.animate-slide-in {
		animation: slide-in 0.3s ease-out;
	}
</style>

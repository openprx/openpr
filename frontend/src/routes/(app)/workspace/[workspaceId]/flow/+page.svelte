<script lang="ts">
	import { t } from 'svelte-i18n';
</script>

<div class="flex flex-1 items-center justify-center p-8 text-center">
	<div>
		<p class="mb-1 text-sm font-medium text-slate-700 dark:text-slate-200">{$t('flow.nav.empty')}</p>
		<p class="text-sm text-slate-500 dark:text-slate-400">{$t('flow.nav.createFirst')}</p>
	</div>
</div>

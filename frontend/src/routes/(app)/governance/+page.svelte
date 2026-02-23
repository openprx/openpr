<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { proposalsApi } from '$lib/api/proposals';
	import { governanceExtApi } from '$lib/api/governance-ext';
	import { governanceApi } from '$lib/api/governance';
	import { impactReviewsApi } from '$lib/api/impact-reviews';
	import { proposalTemplatesApi } from '$lib/api/proposal-templates';
	import { trustApi } from '$lib/api/trust';
	import { toast } from '$lib/stores/toast';
	import { projectOptionsStore } from '$lib/stores/project-options';
	import { groupProjectOptionsByWorkspace } from '$lib/utils/project-options';

	type MetricValue = number;

	interface GovernanceMetric {
		labelKey: string;
		value: MetricValue;
		hintKey?: string;
		format?: 'count' | 'decimal';
	}

	type GovernanceModuleId =
		| 'proposals'
		| 'decisions'
		| 'trustBoard'
		| 'impactReviews'
		| 'proposalTemplates'
		| 'auditReports'
		| 'trustAppeals'
		| 'config'
		| 'auditLogs';

	type GovernanceModuleGroup = 'core' | 'review' | 'system';

	interface GovernanceModuleCard {
		id: GovernanceModuleId;
		titleKey: string;
		descriptionKey: string;
		href: string;
		group: GovernanceModuleGroup;
		disabled?: boolean;
	}

	interface DecisionAggregate {
		total: number;
		approved: number;
		passRate: number;
		code: number;
	}

	let loading = $state(true);
	let activeProposals = $state(0);
	let pendingVotes = $state(0);
	let monthlyDecisions = $state(0);
	let averageTrustScore = $state(0);
	let selectedProjectId = $state('');
	let moduleStatsLoading = $state(true);
	let moduleStats = $state<Record<GovernanceModuleId, string>>({
		proposals: '--',
		decisions: '--',
		trustBoard: '--',
		impactReviews: '--',
		proposalTemplates: '--',
		auditReports: '--',
		trustAppeals: '--',
		config: '--',
		auditLogs: '--'
	});

	const projectOptions = $derived($projectOptionsStore.items);
	const groupedProjectOptions = $derived.by(() => groupProjectOptionsByWorkspace(projectOptions));
	const auditReportsHref = $derived.by(() => {
		if (selectedProjectId) {
			return `/projects/${selectedProjectId}/audit-reports`;
		}
		return '/workspace';
	});

	const overviewMetrics = $derived.by(
		(): GovernanceMetric[] => [
			{
				labelKey: 'governanceCenter.overview.activeProposals',
				value: activeProposals,
				format: 'count'
			},
			{
				labelKey: 'governanceCenter.overview.pendingVotes',
				value: pendingVotes,
				format: 'count'
			},
			{
				labelKey: 'governanceCenter.overview.monthlyDecisions',
				value: monthlyDecisions,
				hintKey: 'governanceCenter.overview.monthHint',
				format: 'count'
			},
			{
				labelKey: 'governanceCenter.overview.averageTrustScore',
				value: averageTrustScore,
				format: 'decimal'
			}
		]
	);

	const coreModules = $derived.by(
		(): GovernanceModuleCard[] => [
			{
				id: 'proposals',
				titleKey: 'governanceCenter.modules.proposals.title',
				descriptionKey: 'governanceCenter.modules.proposals.description',
				href: '/proposals',
				group: 'core'
			},
			{
				id: 'decisions',
				titleKey: 'governanceCenter.modules.decisions.title',
				descriptionKey: 'governanceCenter.modules.decisions.description',
				href: '/decisions/analytics',
				group: 'core'
			},
			{
				id: 'trustBoard',
				titleKey: 'governanceCenter.modules.trustBoard.title',
				descriptionKey: 'governanceCenter.modules.trustBoard.description',
				href: '/trust-board',
				group: 'core'
			}
		]
	);

	const reviewModules = $derived.by(
		(): GovernanceModuleCard[] => [
			{
				id: 'impactReviews',
				titleKey: 'governanceCenter.modules.impactReviews.title',
				descriptionKey: 'governanceCenter.modules.impactReviews.description',
				href: '/impact-reviews',
				group: 'review'
			},
			{
				id: 'proposalTemplates',
				titleKey: 'governanceCenter.modules.proposalTemplates.title',
				descriptionKey: 'governanceCenter.modules.proposalTemplates.description',
				href: '/proposal-templates',
				group: 'review'
			},
			{
				id: 'auditReports',
				titleKey: 'governanceCenter.modules.auditReports.title',
				descriptionKey: 'governanceCenter.modules.auditReports.description',
				href: auditReportsHref,
				group: 'review',
				disabled: !selectedProjectId
			}
		]
	);

	const systemModules = $derived.by(
		(): GovernanceModuleCard[] => [
			{
				id: 'trustAppeals',
				titleKey: 'governanceCenter.modules.trustAppeals.title',
				descriptionKey: 'governanceCenter.modules.trustAppeals.description',
				href: '/trust-scores/appeals',
				group: 'system'
			},
			{
				id: 'config',
				titleKey: 'governanceCenter.modules.config.title',
				descriptionKey: 'governanceCenter.modules.config.description',
				href: '/governance/config',
				group: 'system'
			},
			{
				id: 'auditLogs',
				titleKey: 'governanceCenter.modules.auditLogs.title',
				descriptionKey: 'governanceCenter.modules.auditLogs.description',
				href: '/governance/audit-logs',
				group: 'system'
			}
		]
	);

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		const projects = await projectOptionsStore.ensureLoaded();
		if (!selectedProjectId && projects.length > 0) {
			selectedProjectId = projects[0].id;
		}
		const decisionTask = aggregateMonthlyDecisions(projects.map((project) => project.id));
		await Promise.all([loadOverview(decisionTask), loadModuleStats(decisionTask)]);
		loading = false;
	}

	async function handleProjectChange() {
		const decisionTask = aggregateMonthlyDecisions(projectOptions.map((project) => project.id));
		await Promise.all([loadOverview(decisionTask), loadModuleStats(decisionTask)]);
	}

	async function loadOverview(decisionsTask?: Promise<DecisionAggregate>) {
		const decisionTask = decisionsTask ?? aggregateMonthlyDecisions(projectOptions.map((project) => project.id));

		const [openRes, votingRes, decisionsRes, trustRes] = await Promise.all([
			proposalsApi.list({ status: 'open', page: 1, per_page: 1 }),
			proposalsApi.list({ status: 'voting', page: 1, per_page: 1 }),
			decisionTask,
			trustApi.listTrustScores({ domain: 'all', page: 1, per_page: 200 })
		]);

		if (openRes.code === 0 && openRes.data && votingRes.code === 0 && votingRes.data) {
			activeProposals = (openRes.data.total ?? 0) + (votingRes.data.total ?? 0);
			pendingVotes = votingRes.data.total ?? 0;
		} else {
			activeProposals = 0;
			pendingVotes = 0;
		}

		if (decisionsRes.code === 0) {
			monthlyDecisions = decisionsRes.total;
		} else {
			monthlyDecisions = 0;
		}

		if (trustRes.code === 0 && trustRes.data) {
			const scores = trustRes.data.items.map((item) => item.score);
			averageTrustScore = scores.length > 0 ? scores.reduce((sum, value) => sum + value, 0) / scores.length : 0;
		} else {
			averageTrustScore = 0;
		}

		if (openRes.code !== 0 || votingRes.code !== 0 || decisionsRes.code !== 0 || trustRes.code !== 0) {
			toast.error(get(t)('governanceCenter.overview.loadFailed'));
		}
	}

	async function loadModuleStats(decisionsTask?: Promise<DecisionAggregate>) {
		moduleStatsLoading = true;
		const decisionTask = decisionsTask ?? aggregateMonthlyDecisions(projectOptions.map((project) => project.id));

		const [openRes, votingRes, allProposalsRes, decisionsRes, trustRes, impactPendingRes, impactDoneRes, appealsPendingRes, appealsAllRes] =
			await Promise.all([
				proposalsApi.list({ status: 'open', page: 1, per_page: 1 }),
				proposalsApi.list({ status: 'voting', page: 1, per_page: 1 }),
				proposalsApi.list({ page: 1, per_page: 1 }),
				decisionTask,
				trustApi.listTrustScores({ domain: 'all', page: 1, per_page: 200 }),
				impactReviewsApi.list({ status: 'pending', page: 1, per_page: 1 }),
				impactReviewsApi.list({ status: 'completed', page: 1, per_page: 1 }),
				trustApi.listAppeals({ status: 'pending' }),
				trustApi.listAppeals({ status: 'all' })
			]);

		const nextStats: Record<GovernanceModuleId, string> = {
			...moduleStats
		};

		if (openRes.code === 0 && openRes.data && votingRes.code === 0 && votingRes.data && allProposalsRes.code === 0 && allProposalsRes.data) {
			const active = (openRes.data.total ?? 0) + (votingRes.data.total ?? 0);
			nextStats.proposals = `活跃 ${active} 个 · 总计 ${allProposalsRes.data.total ?? 0} 个`;
		}

		if (decisionsRes.code === 0) {
			nextStats.decisions = `本月 ${decisionsRes.total} 项决策 · 通过率 ${formatPercent(decisionsRes.passRate)}`;
		}

		if (trustRes.code === 0 && trustRes.data) {
			const members = trustRes.data.total ?? trustRes.data.items.length;
			const scoreList = trustRes.data.items.map((item) => item.score);
			const avg = scoreList.length > 0 ? scoreList.reduce((sum, score) => sum + score, 0) / scoreList.length : 0;
			nextStats.trustBoard = `${members} 名成员 · 平均 ${avg.toFixed(1)} 分`;
		}

		if (impactPendingRes.code === 0 && impactPendingRes.data && impactDoneRes.code === 0 && impactDoneRes.data) {
			nextStats.impactReviews = `待评审 ${impactPendingRes.data.total ?? 0} · 已完成 ${impactDoneRes.data.total ?? 0}`;
		}

		if (selectedProjectId) {
			const [templateRes, auditReportsRes, configRes, auditLogsTodayRes] = await Promise.all([
				proposalTemplatesApi.list({ project_id: selectedProjectId }),
				governanceExtApi.listProjectAuditReports(selectedProjectId, { page: 1, per_page: 1 }),
				governanceApi.getConfig(selectedProjectId),
				governanceApi.listAuditLogs({
					project_id: selectedProjectId,
					start_at: getTodayStartIso(),
					end_at: new Date().toISOString(),
					page: 1,
					per_page: 1
				})
			]);

			if (templateRes.code === 0 && templateRes.data) {
				nextStats.proposalTemplates = `${templateRes.data.total ?? 0} 个模板`;
			}

			if (auditReportsRes.code === 0 && auditReportsRes.data) {
				if (auditReportsRes.data.items.length > 0) {
					nextStats.auditReports = `最近报告：${formatDateLabel(auditReportsRes.data.items[0].generated_at)}`;
				} else {
					nextStats.auditReports = '暂无报告';
				}
			}

			if (configRes.code === 0 && configRes.data) {
				nextStats.config = `最后更新：${formatDateLabel(configRes.data.updated_at)}`;
			}

			if (auditLogsTodayRes.code === 0 && auditLogsTodayRes.data) {
				nextStats.auditLogs = `今日 ${auditLogsTodayRes.data.total ?? 0} 条记录`;
			}
		}

		if (appealsPendingRes.code === 0 && appealsPendingRes.data && appealsAllRes.code === 0 && appealsAllRes.data) {
			const pending = appealsPendingRes.data.total ?? 0;
			const processed = Math.max(0, (appealsAllRes.data.total ?? 0) - pending);
			nextStats.trustAppeals = `待处理 ${pending} · 已处理 ${processed}`;
		}

		moduleStats = nextStats;
		moduleStatsLoading = false;
	}

	async function aggregateMonthlyDecisions(decisionProjectIds: string[]): Promise<DecisionAggregate> {
		const monthStart = new Date();
		monthStart.setDate(1);
		monthStart.setHours(0, 0, 0, 0);

		const decisionParams = {
			start_at: monthStart.toISOString(),
			end_at: new Date().toISOString()
		};
		if (decisionProjectIds.length === 0) {
			return { total: 0, approved: 0, passRate: 0, code: 0 };
		}

		const decisionResponses = await Promise.all(
			decisionProjectIds.map((projectId) =>
				governanceExtApi.getDecisionAnalytics({
					project_id: projectId,
					...decisionParams
				})
			)
		);

		const hasFailure = decisionResponses.some((res) => res.code !== 0 || !res.data);
		if (hasFailure) {
			return { total: 0, approved: 0, passRate: 0, code: -1 };
		}

		const { total, approved } = decisionResponses.reduce(
			(acc, res) => {
				const overview = res.data?.overview;
				if (!overview) return acc;
				return {
					total: acc.total + overview.total_decisions,
					approved: acc.approved + overview.approved_count
				};
			},
			{ total: 0, approved: 0 }
		);

		return {
			total,
			approved,
			passRate: total > 0 ? (approved / total) * 100 : 0,
			code: 0
		};
	}

	function formatMetricValue(metric: GovernanceMetric): string {
		if (metric.format === 'decimal') {
			return metric.value.toFixed(1);
		}
		return String(Math.round(metric.value));
	}

	function getIconColorClass(group: GovernanceModuleGroup): string {
		if (group === 'core') return 'text-blue-500';
		if (group === 'review') return 'text-violet-500';
		return 'text-slate-500';
	}

	function formatPercent(value: number): string {
		return `${value.toFixed(1)}%`;
	}

	function getTodayStartIso(): string {
		const todayStart = new Date();
		todayStart.setHours(0, 0, 0, 0);
		return todayStart.toISOString();
	}

	function formatDateLabel(value?: string): string {
		if (!value) return '--';
		const date = new Date(value);
		if (Number.isNaN(date.getTime())) return '--';
		return date.toLocaleString();
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.governanceCenter')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-6">
	<section class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm dark:border-slate-700 dark:bg-slate-800">
		<div class="flex items-start justify-between gap-4">
			<div>
				<h1 class="text-2xl font-semibold text-slate-900 dark:text-slate-100">{$t('governanceCenter.title')}</h1>
				<p class="mt-1 text-sm text-slate-500">{$t('governanceCenter.subtitle')}</p>
			</div>
			{#if projectOptions.length > 0}
				<div class="w-full max-w-xs">
					<label for="governance-center-project" class="mb-1 block text-xs text-slate-500">{$t('governanceCenter.currentProject')}</label>
					<select
						id="governance-center-project"
						bind:value={selectedProjectId}
						onchange={handleProjectChange}
						class="w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
					>
						{#each groupedProjectOptions as group}
							<optgroup label={group.workspaceName}>
								{#each group.items as project}
									<option value={project.id}>{project.name}</option>
								{/each}
							</optgroup>
						{/each}
					</select>
				</div>
			{/if}
		</div>

		<div class="mt-5 grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
			{#each overviewMetrics as metric}
				<div class="rounded-xl border border-slate-200 bg-slate-50 p-4 dark:border-slate-700 dark:bg-slate-900">
					<p class="text-xs uppercase tracking-wide text-slate-500">{$t(metric.labelKey)}</p>
					<p class="mt-2 text-2xl font-semibold text-slate-900 dark:text-slate-100">{loading ? '...' : formatMetricValue(metric)}</p>
					{#if metric.hintKey}
						<p class="mt-1 text-xs text-slate-500">{$t(metric.hintKey)}</p>
					{/if}
				</div>
			{/each}
		</div>
	</section>

	<section class="space-y-3">
		<h2 class="text-sm font-semibold uppercase tracking-wide text-slate-500">{$t('governanceCenter.groups.core')}</h2>
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
			{#each coreModules as module}
				<a href={module.href} class="group rounded-xl border border-slate-200 bg-white p-5 shadow-sm transition hover:-translate-y-0.5 hover:shadow-md dark:border-slate-700 dark:bg-slate-800">
					<div class="flex items-start justify-between gap-3">
						<div>
							<span class={`inline-flex ${getIconColorClass(module.group)}`}>
								{#if module.id === 'proposals'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M14 3v5h5" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M9 13h6M9 17h6M15.5 11.5l2-2 1.5 1.5-2 2L15 13z" />
									</svg>
								{:else if module.id === 'decisions'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M4 19h16" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M7 15V9m5 6V5m5 10v-3" />
										<path stroke-linecap="round" stroke-linejoin="round" d="m6 8 4-3 4 2 4-3" />
									</svg>
								{:else if module.id === 'trustBoard'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M8 4h8v2a4 4 0 0 1-4 4 4 4 0 0 1-4-4z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M6 6H4a3 3 0 0 0 3 3m11-3h2a3 3 0 0 1-3 3" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M10 14h4m-6 6h8a2 2 0 0 0-2-2h-4a2 2 0 0 0-2 2z" />
									</svg>
								{:else if module.id === 'impactReviews'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<circle cx="11" cy="11" r="6" />
										<path stroke-linecap="round" stroke-linejoin="round" d="m20 20-4.2-4.2M8.5 12.5l1.8 1.8 3.2-3.2" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M4 20h7M4 17h5" />
									</svg>
								{:else if module.id === 'proposalTemplates'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<rect x="4" y="5" width="10" height="12" rx="2" />
										<rect x="10" y="8" width="10" height="12" rx="2" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M7 9h4m-4 3h4" />
									</svg>
								{:else if module.id === 'auditReports'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<rect x="6" y="4" width="12" height="16" rx="2" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M9 4h6v3H9zM9 11h6m-6 3h3m2 1.5 1.5 1.5 2.5-2.5" />
									</svg>
								{:else if module.id === 'trustAppeals'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M12 5v14M5 9h14" />
										<path stroke-linecap="round" stroke-linejoin="round" d="m7 9-2 4h4zm12 0-2 4h4z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M9 19h6" />
									</svg>
								{:else if module.id === 'config'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.2a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.2a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3h.1A1.7 1.7 0 0 0 10 3.2V3a2 2 0 1 1 4 0v.2a1.7 1.7 0 0 0 1 1.5h.1a1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8v.1a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.2a1.7 1.7 0 0 0-1.5 1z" />
									</svg>
								{:else if module.id === 'auditLogs'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<circle cx="8" cy="8" r="3" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M8 6.5V8h1.5M13 6h7M13 10h7M4 16h16M4 20h10" />
									</svg>
								{/if}
							</span>
							<h3 class="mt-3 text-base font-semibold text-slate-900 dark:text-slate-100">{$t(module.titleKey)}</h3>
							<p class="mt-1 text-sm text-slate-500">{$t(module.descriptionKey)}</p>
							{#if moduleStatsLoading}
								<div class="mt-2 h-4 w-44 animate-pulse rounded bg-slate-200 dark:bg-slate-700"></div>
							{:else}
								<p class="mt-2 text-xs text-slate-500">{moduleStats[module.id]}</p>
							{/if}
						</div>
						<span class="text-lg text-slate-400 transition group-hover:text-slate-700 dark:group-hover:text-slate-300">→</span>
					</div>
				</a>
			{/each}
		</div>
	</section>

	<section class="space-y-3">
		<h2 class="text-sm font-semibold uppercase tracking-wide text-slate-500">{$t('governanceCenter.groups.reviewAndTemplate')}</h2>
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
			{#each reviewModules as module}
				<a
					href={module.href}
					class={`group rounded-xl border border-slate-200 bg-white p-5 shadow-sm transition hover:-translate-y-0.5 hover:shadow-md dark:border-slate-700 dark:bg-slate-800 ${module.disabled
						? 'pointer-events-none opacity-60'
						: ''}`}
				>
					<div class="flex items-start justify-between gap-3">
						<div>
							<span class={`inline-flex ${getIconColorClass(module.group)}`}>
								{#if module.id === 'proposals'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M14 3v5h5" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M9 13h6M9 17h6M15.5 11.5l2-2 1.5 1.5-2 2L15 13z" />
									</svg>
								{:else if module.id === 'decisions'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M4 19h16" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M7 15V9m5 6V5m5 10v-3" />
										<path stroke-linecap="round" stroke-linejoin="round" d="m6 8 4-3 4 2 4-3" />
									</svg>
								{:else if module.id === 'trustBoard'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M8 4h8v2a4 4 0 0 1-4 4 4 4 0 0 1-4-4z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M6 6H4a3 3 0 0 0 3 3m11-3h2a3 3 0 0 1-3 3" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M10 14h4m-6 6h8a2 2 0 0 0-2-2h-4a2 2 0 0 0-2 2z" />
									</svg>
								{:else if module.id === 'impactReviews'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<circle cx="11" cy="11" r="6" />
										<path stroke-linecap="round" stroke-linejoin="round" d="m20 20-4.2-4.2M8.5 12.5l1.8 1.8 3.2-3.2" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M4 20h7M4 17h5" />
									</svg>
								{:else if module.id === 'proposalTemplates'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<rect x="4" y="5" width="10" height="12" rx="2" />
										<rect x="10" y="8" width="10" height="12" rx="2" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M7 9h4m-4 3h4" />
									</svg>
								{:else if module.id === 'auditReports'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<rect x="6" y="4" width="12" height="16" rx="2" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M9 4h6v3H9zM9 11h6m-6 3h3m2 1.5 1.5 1.5 2.5-2.5" />
									</svg>
								{:else if module.id === 'trustAppeals'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M12 5v14M5 9h14" />
										<path stroke-linecap="round" stroke-linejoin="round" d="m7 9-2 4h4zm12 0-2 4h4z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M9 19h6" />
									</svg>
								{:else if module.id === 'config'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.2a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.2a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3h.1A1.7 1.7 0 0 0 10 3.2V3a2 2 0 1 1 4 0v.2a1.7 1.7 0 0 0 1 1.5h.1a1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8v.1a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.2a1.7 1.7 0 0 0-1.5 1z" />
									</svg>
								{:else if module.id === 'auditLogs'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<circle cx="8" cy="8" r="3" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M8 6.5V8h1.5M13 6h7M13 10h7M4 16h16M4 20h10" />
									</svg>
								{/if}
							</span>
							<h3 class="mt-3 text-base font-semibold text-slate-900 dark:text-slate-100">{$t(module.titleKey)}</h3>
							<p class="mt-1 text-sm text-slate-500">{$t(module.descriptionKey)}</p>
							{#if moduleStatsLoading}
								<div class="mt-2 h-4 w-44 animate-pulse rounded bg-slate-200 dark:bg-slate-700"></div>
							{:else}
								<p class="mt-2 text-xs text-slate-500">{moduleStats[module.id]}</p>
							{/if}
							{#if module.disabled}
								<p class="mt-2 text-xs text-slate-500">{$t('governanceCenter.auditReportsProjectHint')}</p>
							{/if}
						</div>
						<span class="text-lg text-slate-400 transition group-hover:text-slate-700 dark:group-hover:text-slate-300">→</span>
					</div>
				</a>
			{/each}
		</div>
	</section>

	<section class="space-y-3">
		<h2 class="text-sm font-semibold uppercase tracking-wide text-slate-500">{$t('governanceCenter.groups.system')}</h2>
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
			{#each systemModules as module}
				<a href={module.href} class="group rounded-xl border border-slate-200 bg-white p-5 shadow-sm transition hover:-translate-y-0.5 hover:shadow-md dark:border-slate-700 dark:bg-slate-800">
					<div class="flex items-start justify-between gap-3">
						<div>
							<span class={`inline-flex ${getIconColorClass(module.group)}`}>
								{#if module.id === 'proposals'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M14 3v5h5" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M9 13h6M9 17h6M15.5 11.5l2-2 1.5 1.5-2 2L15 13z" />
									</svg>
								{:else if module.id === 'decisions'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M4 19h16" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M7 15V9m5 6V5m5 10v-3" />
										<path stroke-linecap="round" stroke-linejoin="round" d="m6 8 4-3 4 2 4-3" />
									</svg>
								{:else if module.id === 'trustBoard'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M8 4h8v2a4 4 0 0 1-4 4 4 4 0 0 1-4-4z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M6 6H4a3 3 0 0 0 3 3m11-3h2a3 3 0 0 1-3 3" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M10 14h4m-6 6h8a2 2 0 0 0-2-2h-4a2 2 0 0 0-2 2z" />
									</svg>
								{:else if module.id === 'impactReviews'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<circle cx="11" cy="11" r="6" />
										<path stroke-linecap="round" stroke-linejoin="round" d="m20 20-4.2-4.2M8.5 12.5l1.8 1.8 3.2-3.2" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M4 20h7M4 17h5" />
									</svg>
								{:else if module.id === 'proposalTemplates'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<rect x="4" y="5" width="10" height="12" rx="2" />
										<rect x="10" y="8" width="10" height="12" rx="2" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M7 9h4m-4 3h4" />
									</svg>
								{:else if module.id === 'auditReports'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<rect x="6" y="4" width="12" height="16" rx="2" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M9 4h6v3H9zM9 11h6m-6 3h3m2 1.5 1.5 1.5 2.5-2.5" />
									</svg>
								{:else if module.id === 'trustAppeals'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M12 5v14M5 9h14" />
										<path stroke-linecap="round" stroke-linejoin="round" d="m7 9-2 4h4zm12 0-2 4h4z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M9 19h6" />
									</svg>
								{:else if module.id === 'config'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<path stroke-linecap="round" stroke-linejoin="round" d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.2a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.2a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3h.1A1.7 1.7 0 0 0 10 3.2V3a2 2 0 1 1 4 0v.2a1.7 1.7 0 0 0 1 1.5h.1a1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8v.1a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.2a1.7 1.7 0 0 0-1.5 1z" />
									</svg>
								{:else if module.id === 'auditLogs'}
									<svg class="h-8 w-8" fill="none" stroke="currentColor" stroke-width="1.8" viewBox="0 0 24 24" aria-hidden="true">
										<circle cx="8" cy="8" r="3" />
										<path stroke-linecap="round" stroke-linejoin="round" d="M8 6.5V8h1.5M13 6h7M13 10h7M4 16h16M4 20h10" />
									</svg>
								{/if}
							</span>
							<h3 class="mt-3 text-base font-semibold text-slate-900 dark:text-slate-100">{$t(module.titleKey)}</h3>
							<p class="mt-1 text-sm text-slate-500">{$t(module.descriptionKey)}</p>
							{#if moduleStatsLoading}
								<div class="mt-2 h-4 w-44 animate-pulse rounded bg-slate-200 dark:bg-slate-700"></div>
							{:else}
								<p class="mt-2 text-xs text-slate-500">{moduleStats[module.id]}</p>
							{/if}
						</div>
						<span class="text-lg text-slate-400 transition group-hover:text-slate-700 dark:group-hover:text-slate-300">→</span>
					</div>
				</a>
			{/each}
		</div>
	</section>
</div>

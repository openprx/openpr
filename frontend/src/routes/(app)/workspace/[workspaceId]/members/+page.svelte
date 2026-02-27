<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { botsApi, type Bot, type CreateBotResponse } from '$lib/api/bots';
	import { membersApi, type WorkspaceMember, type WorkspaceMemberRole } from '$lib/api/members';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import Button from '$lib/components/Button.svelte';
	import BotIcon from '$lib/components/BotIcon.svelte';
	import Input from '$lib/components/Input.svelte';
	import Modal from '$lib/components/Modal.svelte';

	type EditableRole = Exclude<WorkspaceMemberRole, 'owner'>;
	type BotPermission = 'read' | 'write' | 'admin';

	interface SearchUser {
		id: string;
		email: string;
		name: string;
	}

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const botPermissionOptions: BotPermission[] = ['read', 'write', 'admin'];

	let loading = $state(true);
	let removingMemberId = $state<string | null>(null);
	let changingRoleMemberId = $state<string | null>(null);
	let addingUserId = $state<string | null>(null);
	let searchingUsers = $state(false);

	let members = $state<WorkspaceMember[]>([]);
	let showInviteModal = $state(false);
	let selectedRole = $state<EditableRole>('member');
	let searchKeyword = $state('');
	let searchResults = $state<SearchUser[]>([]);

	let botsLoading = $state(true);
	let creatingToken = $state(false);
	let revokingBotId = $state<string | null>(null);
	let showCreateTokenModal = $state(false);
	let showCreatedTokenModal = $state(false);
	let tokenName = $state('');
	let tokenExpiresAt = $state('');
	let tokenPermissions = $state<BotPermission[]>(['read']);
	let bots = $state<Bot[]>([]);
	let createdToken = $state<CreateBotResponse | null>(null);

	const memberIds = $derived(new Set(members.map((member) => member.user_id)));

	onMount(async () => {
		await Promise.all([loadMembers(), loadBots()]);
	});

	async function loadMembers() {
		loading = true;
		const response = await membersApi.list(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			members = response.data.items ?? [];
		}
		loading = false;
	}

	async function loadBots() {
		botsLoading = true;
		const response = await botsApi.list(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
			bots = [];
		} else {
			bots = response.data ?? [];
		}
		botsLoading = false;
	}

	async function searchInternalUsers() {
		searchingUsers = true;
		const response = await membersApi.searchUsers(workspaceId, {
			search: searchKeyword.trim(),
			limit: 20
		});
		if (response.code !== 0) {
			toast.error(response.message);
			searchResults = [];
		} else {
			searchResults = response.data?.items ?? [];
		}
		searchingUsers = false;
	}

	async function addInternalUser(userId: string) {
		addingUserId = userId;
		const response = await membersApi.add(workspaceId, {
			user_id: userId,
			role: selectedRole
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('members.addSuccess'));
			await loadMembers();
		}

		addingUserId = null;
	}

	async function removeMember(member: WorkspaceMember) {
		const name = member.name || member.email || member.user_id;
		if (!confirm(get(t)('members.removeConfirm', { values: { name } }))) {
			return;
		}

		removingMemberId = member.user_id;
		const response = await membersApi.remove(workspaceId, member.user_id);
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('members.removeSuccess'));
			await loadMembers();
		}
		removingMemberId = null;
	}

	async function changeRole(member: WorkspaceMember, nextRole: WorkspaceMemberRole) {
		if (member.role === nextRole || nextRole === 'owner') {
			return;
		}

		if (!confirm(get(t)('members.changeRoleConfirm', { values: { email: member.email, role: nextRole } }))) {
			return;
		}

		changingRoleMemberId = member.user_id;
		const response = await membersApi.updateRole(workspaceId, member.user_id, nextRole);

		if (response.code !== 0) {
			toast.error(get(t)('members.changeRoleFailed', { values: { message: response.message } }));
		} else {
			toast.success(get(t)('members.changeRoleSuccess'));
			await loadMembers();
		}
		changingRoleMemberId = null;
	}

	function formatDate(dateText: string | null | undefined): string {
		if (!dateText) return '-';
		const parsed = new Date(dateText);
		if (Number.isNaN(parsed.getTime())) return '-';
		return parsed.toLocaleString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function isBotMember(member: WorkspaceMember): boolean {
		return member.entity_type === 'bot';
	}

	function getBotRoleLabel(member: WorkspaceMember): string {
		const botLabel = get(t)('admin.botBadge');
		if (member.agent_type === 'openclaw') return `${botLabel} (${get(t)('agentTypes.openclaw')})`;
		if (member.agent_type === 'webhook') return `${botLabel} (${get(t)('agentTypes.webhook')})`;
		if (member.agent_type === 'custom') return `${botLabel} (${get(t)('agentTypes.custom')})`;
		return botLabel;
	}

	function openCreateTokenModal() {
		tokenName = '';
		tokenExpiresAt = '';
		tokenPermissions = ['read'];
		showCreateTokenModal = true;
	}

	function togglePermission(permission: BotPermission) {
		if (tokenPermissions.includes(permission)) {
			if (tokenPermissions.length === 1) return;
			tokenPermissions = tokenPermissions.filter((value) => value !== permission);
			return;
		}
		tokenPermissions = [...tokenPermissions, permission];
	}

	function buildExpiresAt(dateInput: string): string | undefined {
		if (!dateInput) return undefined;
		const parsed = new Date(`${dateInput}T23:59:59`);
		if (Number.isNaN(parsed.getTime())) return undefined;
		return parsed.toISOString();
	}

	async function createToken() {
		if (!tokenName.trim()) {
			toast.error(get(t)('members.tokenNameRequired'));
			return;
		}

		creatingToken = true;
		const expiresAt = buildExpiresAt(tokenExpiresAt);
		const response = await botsApi.create(workspaceId, {
			name: tokenName.trim(),
			permissions: tokenPermissions,
			expires_at: expiresAt
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			createdToken = response.data;
			showCreateTokenModal = false;
			showCreatedTokenModal = true;
			toast.success(get(t)('members.createTokenSuccess'));
			await loadBots();
		}

		creatingToken = false;
	}

	async function revokeToken(bot: Bot) {
		if (!confirm(get(t)('members.revokeConfirm', { values: { name: bot.name } }))) {
			return;
		}

		revokingBotId = bot.id;
		const response = await botsApi.revoke(workspaceId, bot.id);
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('members.revokeSuccess'));
			await loadBots();
		}
		revokingBotId = null;
	}

	async function copyCreatedToken() {
		if (!createdToken?.token) return;

		try {
			if (navigator.clipboard?.writeText) {
				await navigator.clipboard.writeText(createdToken.token);
			} else {
				const ta = document.createElement('textarea');
				ta.value = createdToken.token;
				ta.style.position = 'fixed';
				ta.style.opacity = '0';
				document.body.appendChild(ta);
				ta.select();
				document.execCommand('copy');
				document.body.removeChild(ta);
			}
			toast.success(get(t)('members.copyTokenSuccess'));
		} catch {
			toast.error(get(t)('members.copyTokenFailed'));
		}
	}

	function closeCreatedTokenModal() {
		showCreatedTokenModal = false;
		createdToken = null;
	}

	function getPermissionClass(permission: string): string {
		if (permission === 'admin') {
			return 'bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-300';
		}
		if (permission === 'write') {
			return 'bg-amber-100 text-amber-700 dark:bg-amber-900/30 dark:text-amber-300';
		}
		return 'bg-blue-100 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300';
	}

	function getPermissionLabel(permission: string): string {
		const normalized = permission.toLowerCase();
		if (normalized === 'admin' || normalized === 'write' || normalized === 'read') {
			return get(t)(`members.permission.${normalized}`);
		}
		return permission;
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('members.title')}</h1>
			<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('members.subtitle')}</p>
		</div>
		<Button onclick={() => (showInviteModal = true)}>{$t('members.addInternalUser')}</Button>
	</div>

	<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-blue-50 p-4 text-sm text-blue-800">
		{$t('members.roleDescription')}
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
	{:else if members.length === 0}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-10 text-center text-slate-500 dark:text-slate-400">{$t('members.empty')}</div>
	{:else}
		<div class="overflow-hidden rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900">
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-slate-200 dark:divide-slate-700">
					<thead class="bg-slate-50 dark:bg-slate-950">
						<tr>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('members.member')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('admin.email')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('admin.role')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('members.joinedAt')}</th>
							<th class="px-4 py-3 text-right text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.actions')}</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-200 dark:divide-slate-700 bg-white dark:bg-slate-900">
						{#each members as member (member.user_id)}
							<tr>
								<td class="px-4 py-3 text-sm font-medium text-slate-900 dark:text-slate-100">
									<div class="flex items-center gap-1.5">
										{#if isBotMember(member)}
											<BotIcon class="h-4 w-4 text-cyan-700 dark:text-cyan-300" title={$t('admin.botBadge')} />
										{/if}
										<span>{member.name || member.user_id}</span>
									</div>
								</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">{member.email || '-'}</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">
									{#if isBotMember(member)}
										<span class="inline-flex rounded-full bg-cyan-100 px-2.5 py-1 text-xs font-medium text-cyan-700 dark:bg-cyan-900/30 dark:text-cyan-300">
											{getBotRoleLabel(member)}
										</span>
									{:else}
										<select
											value={member.role}
											disabled={member.role === 'owner' || changingRoleMemberId === member.user_id}
											onchange={(event) => {
												const target = event.currentTarget as HTMLSelectElement;
												changeRole(member, target.value as WorkspaceMemberRole);
											}}
											class="rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-2 py-1 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400 disabled:cursor-not-allowed disabled:bg-slate-100 dark:disabled:bg-slate-700"
										>
											<option value="owner">{$t('roles.owner')}</option>
											<option value="admin">{$t('roles.admin')}</option>
											<option value="member">{$t('roles.member')}</option>
										</select>
									{/if}
								</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">{formatDate(member.joined_at)}</td>
								<td class="px-4 py-3 text-right">
									<Button
										variant="danger"
										size="sm"
										disabled={member.role === 'owner'}
										loading={removingMemberId === member.user_id}
										onclick={() => removeMember(member)}
									>
										{$t('members.remove')}
									</Button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</div>
	{/if}

	<div class="overflow-hidden rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900">
		<div class="flex flex-col gap-3 border-b border-slate-200 dark:border-slate-700 p-4 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('members.apiTokensTitle')}</h2>
				<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('members.apiTokensSubtitle')}</p>
			</div>
			<Button size="sm" onclick={openCreateTokenModal}>{$t('members.createToken')}</Button>
		</div>

		{#if botsLoading}
			<div class="p-6 text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
		{:else if bots.length === 0}
			<div class="p-10 text-center text-sm text-slate-500 dark:text-slate-400">{$t('members.apiTokensEmpty')}</div>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-slate-200 dark:divide-slate-700">
					<thead class="bg-slate-50 dark:bg-slate-950">
						<tr>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('members.tokenName')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('members.tokenPrefix')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('members.permissions')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.status')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('members.lastUsedAt')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.createdAt')}</th>
							<th class="px-4 py-3 text-right text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.actions')}</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-200 dark:divide-slate-700 bg-white dark:bg-slate-900">
						{#each bots as bot (bot.id)}
							<tr>
								<td class="px-4 py-3 text-sm font-medium text-slate-900 dark:text-slate-100">{bot.name}</td>
								<td class="px-4 py-3 text-sm font-mono text-slate-600 dark:text-slate-300">{bot.token_prefix}</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">
									<div class="flex flex-wrap gap-1.5">
										{#if bot.permissions.length === 0}
											<span>-</span>
										{:else}
											{#each bot.permissions as permission}
												<span class={`inline-flex rounded-full px-2.5 py-1 text-xs font-medium ${getPermissionClass(permission)}`}>
													{getPermissionLabel(permission)}
												</span>
											{/each}
										{/if}
									</div>
								</td>
								<td class="px-4 py-3 text-sm">
									{#if bot.is_active}
										<span class="inline-flex rounded-full bg-emerald-100 px-2.5 py-1 text-xs font-medium text-emerald-700 dark:bg-emerald-900/30 dark:text-emerald-300">
											{$t('members.tokenStatus.active')}
										</span>
									{:else}
										<span class="inline-flex rounded-full bg-slate-200 px-2.5 py-1 text-xs font-medium text-slate-700 dark:bg-slate-700 dark:text-slate-300">
											{$t('members.tokenStatus.disabled')}
										</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">{formatDate(bot.last_used_at)}</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">{formatDate(bot.created_at)}</td>
								<td class="px-4 py-3 text-right">
									<Button
										variant="danger"
										size="sm"
										disabled={!bot.is_active}
										loading={revokingBotId === bot.id}
										onclick={() => revokeToken(bot)}
									>
										{$t('members.revoke')}
									</Button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>
</div>

<Modal bind:open={showInviteModal} title={$t('members.addInternalUser')}>
	<div class="space-y-4">
		<form
			onsubmit={(event) => {
				event.preventDefault();
				searchInternalUsers();
			}}
			class="grid grid-cols-1 gap-3 sm:grid-cols-[1fr_auto]"
		>
			<Input type="search" label={$t('members.usernameOrEmail')} bind:value={searchKeyword} placeholder={$t('members.usernameOrEmailPlaceholder')} />
			<div class="flex items-end">
				<Button type="submit" loading={searchingUsers}>{$t('common.search')}</Button>
			</div>
		</form>

		<div>
			<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="inviteRole">{$t('admin.role')}</label>
			<select
				id="inviteRole"
				bind:value={selectedRole}
				class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
			>
				<option value="member">{$t('roles.member')}</option>
				<option value="admin">{$t('roles.admin')}</option>
			</select>
		</div>

		<div class="max-h-72 space-y-2 overflow-y-auto rounded-md border border-slate-200 dark:border-slate-700 p-2">
			{#if searchingUsers}
				<p class="p-2 text-sm text-slate-500 dark:text-slate-400">{$t('search.searching')}</p>
			{:else if searchResults.length === 0}
				<p class="p-2 text-sm text-slate-500 dark:text-slate-400">{$t('members.noSearchResults')}</p>
			{:else}
				{#each searchResults as user (user.id)}
					<div class="flex items-center justify-between rounded-md px-2 py-2 hover:bg-slate-50 dark:hover:bg-slate-800 dark:bg-slate-950">
						<div>
							<p class="text-sm font-medium text-slate-900 dark:text-slate-100">{user.name}</p>
							<p class="text-xs text-slate-500 dark:text-slate-400">{user.email}</p>
						</div>
						{#if memberIds.has(user.id)}
							<Button size="sm" variant="secondary" disabled>{$t('members.added')}</Button>
						{:else}
							<Button
								size="sm"
								loading={addingUserId === user.id}
								onclick={() => addInternalUser(user.id)}
							>
								{$t('common.create')}
							</Button>
						{/if}
					</div>
				{/each}
			{/if}
		</div>

		<div class="flex justify-end gap-2 pt-2">
			<Button variant="secondary" onclick={() => (showInviteModal = false)}>{$t('common.close')}</Button>
		</div>
	</div>
</Modal>

<Modal bind:open={showCreateTokenModal} title={$t('members.createToken')}>
	<form
		onsubmit={(event) => {
			event.preventDefault();
			createToken();
		}}
		class="space-y-4"
	>
		<Input
			label={$t('members.tokenName')}
			placeholder={$t('members.tokenNamePlaceholder')}
			bind:value={tokenName}
			required
		/>

		<div>
			<p class="mb-2 block text-sm font-medium text-slate-700 dark:text-slate-300">{$t('members.permissions')}</p>
			<div class="flex flex-wrap gap-2">
				{#each botPermissionOptions as permission}
					<label class="inline-flex items-center gap-2 rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-1.5 text-sm text-slate-700 dark:text-slate-300">
						<input
							type="checkbox"
							checked={tokenPermissions.includes(permission)}
							onchange={() => togglePermission(permission)}
							class="h-4 w-4 rounded border-slate-300 text-blue-600 focus:ring-blue-500 dark:border-slate-600 dark:bg-slate-800"
						/>
						<span>{$t(`members.permission.${permission}`)}</span>
					</label>
				{/each}
			</div>
		</div>

		<div>
			<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="tokenExpiresAt">{$t('members.expiresAt')}</label>
			<input
				id="tokenExpiresAt"
				type="date"
				bind:value={tokenExpiresAt}
				class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
			/>
			<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">{$t('members.expiresAtHint')}</p>
		</div>

		<div class="flex justify-end gap-2 pt-2">
			<Button variant="secondary" onclick={() => (showCreateTokenModal = false)}>{$t('common.cancel')}</Button>
			<Button type="submit" loading={creatingToken}>{$t('members.createToken')}</Button>
		</div>
	</form>
</Modal>

<Modal
	bind:open={showCreatedTokenModal}
	title={$t('members.tokenCreatedTitle')}
	maxWidthClass="max-w-2xl"
	onclose={closeCreatedTokenModal}
>
	{#if createdToken}
		<div class="space-y-4">
			<div class="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800">
				{$t('members.tokenCreatedHint')}
			</div>

			<div>
				<p class="mb-1 text-sm font-medium text-slate-700 dark:text-slate-300">{$t('members.fullToken')}</p>
				<code class="block rounded-md bg-slate-100 dark:bg-slate-950 p-3 text-xs sm:text-sm text-slate-900 dark:text-slate-100 break-all">{createdToken.token}</code>
			</div>

			<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
				<div>
					<p class="text-xs text-slate-500 dark:text-slate-400">{$t('members.tokenPrefix')}</p>
					<p class="text-sm font-mono text-slate-700 dark:text-slate-200">{createdToken.token_prefix}</p>
				</div>
				<div>
					<p class="text-xs text-slate-500 dark:text-slate-400">{$t('common.createdAt')}</p>
					<p class="text-sm text-slate-700 dark:text-slate-200">{formatDate(createdToken.created_at)}</p>
				</div>
			</div>

			<div class="flex justify-end gap-2 pt-2">
				<Button variant="secondary" onclick={closeCreatedTokenModal}>{$t('common.close')}</Button>
				<Button onclick={copyCreatedToken}>{$t('members.copyToken')}</Button>
			</div>
		</div>
	{/if}
</Modal>

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { membersApi, type WorkspaceMember, type WorkspaceMemberRole } from '$lib/api/members';
	import { toast } from '$lib/stores/toast';
	import { requireRouteParam } from '$lib/utils/route-params';
	import Button from '$lib/components/Button.svelte';
	import BotIcon from '$lib/components/BotIcon.svelte';
	import Input from '$lib/components/Input.svelte';
	import Modal from '$lib/components/Modal.svelte';

	type EditableRole = Exclude<WorkspaceMemberRole, 'owner'>;
	interface SearchUser {
		id: string;
		email: string;
		name: string;
	}

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');

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

	const memberIds = $derived(new Set(members.map((member) => member.user_id)));

	onMount(async () => {
		await loadMembers();
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

	function formatDate(dateText: string): string {
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

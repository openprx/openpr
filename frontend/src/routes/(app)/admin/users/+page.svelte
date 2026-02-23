<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { authApi } from '$lib/api/auth';
	import { adminApi, type AdminUser } from '$lib/api/admin';
	import { toast } from '$lib/stores/toast';
	import { isAdminUser } from '$lib/utils/auth';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import BotIcon from '$lib/components/BotIcon.svelte';
	import Modal from '$lib/components/Modal.svelte';

	interface ManagedUser extends AdminUser {
		role: 'admin' | 'user';
		status: 'active' | 'disabled';
	}

	let users = $state<ManagedUser[]>([]);
	let loading = $state(true);
	let creating = $state(false);
	let keyword = $state('');
	let roleFilter = $state<'all' | 'admin' | 'user'>('all');
	let statusFilter = $state<'all' | 'active' | 'disabled'>('all');
	let actionLoadingUserId = $state<string | null>(null);

	let showCreateModal = $state(false);
	let createForm = $state({ email: '', password: '', name: '' });
	type EntityType = 'human' | 'bot';
	type AgentType = 'openclaw' | 'webhook' | 'custom';
	let entityType = $state<EntityType>('human');
	let agentType = $state<AgentType>('openclaw');
	let openclawConfig = $state({ command: '', channel: '', target: '' });
	let webhookConfig = $state({ url: '', secret: '' });
	let customConfig = $state({ command: '', args: '' });

	let showEditModal = $state(false);
	let editSaving = $state(false);
	let editingUserId = $state<string | null>(null);
	let editForm = $state({ name: '', email: '', role: 'user' as 'admin' | 'user' });

	let showPasswordModal = $state(false);
	let passwordSaving = $state(false);
	let passwordUser = $state<ManagedUser | null>(null);
	let passwordForm = $state({ newPassword: '', confirmPassword: '' });

	onMount(async () => {
		const meResponse = await authApi.me();
		if (meResponse.code !== 0 || !isAdminUser(meResponse.data?.user ?? null)) {
			toast.error(get(t)('admin.usersAdminOnly'));
			goto('/workspace');
			return;
		}

		await loadUsers();
		loading = false;
	});

	async function loadUsers() {
		const usersResponse = await adminApi.listUsers();
		if (usersResponse.code !== 0 || !usersResponse.data) {
			toast.error(usersResponse.message || get(t)('admin.loadUsersFailed'));
			return;
		}

		users = (usersResponse.data.items || []).map(mapUser);
	}

	function mapUser(user: AdminUser): ManagedUser {
		return {
			...user,
			role: user.role === 'admin' ? 'admin' : 'user',
			status: user.is_active === false ? 'disabled' : 'active'
		};
	}

	const filteredUsers = $derived.by(() => {
		const q = keyword.trim().toLowerCase();
		return users.filter((user) => {
			const queryMatched =
				!q ||
				user.name.toLowerCase().includes(q) ||
				user.email.toLowerCase().includes(q) ||
				user.id.toLowerCase().includes(q);
			const roleMatched = roleFilter === 'all' || user.role === roleFilter;
			const statusMatched = statusFilter === 'all' || user.status === statusFilter;
			return queryMatched && roleMatched && statusMatched;
		});
	});

	async function handleCreateUser() {
		if (creating) return;
		if (!createForm.email.trim() || !createForm.name.trim()) {
			toast.error(get(t)('admin.fillUserFields'));
			return;
		}
		if (entityType === 'human' && !createForm.password.trim()) {
			toast.error(get(t)('admin.fillUserFields'));
			return;
		}
		if (entityType === 'human' && createForm.password.trim().length < 8) {
			toast.error(get(t)('settings.passwordLength'));
			return;
		}
		if (entityType === 'bot') {
			if (
				(agentType === 'openclaw' &&
					(!openclawConfig.command.trim() || !openclawConfig.channel.trim() || !openclawConfig.target.trim())) ||
				(agentType === 'webhook' && !webhookConfig.url.trim()) ||
				(agentType === 'custom' && !customConfig.command.trim())
			) {
				toast.error(get(t)('admin.fillUserFields'));
				return;
			}
		}

		creating = true;
		const body: {
			name: string;
			email: string;
			password?: string;
			entity_type: EntityType;
			agent_type?: AgentType;
			agent_config?: Record<string, string>;
		} = {
			name: createForm.name.trim(),
			email: createForm.email.trim(),
			entity_type: entityType
		};
		if (entityType === 'human') {
			body.password = createForm.password;
		} else {
			body.agent_type = agentType;
			if (agentType === 'openclaw') {
				body.agent_config = {
					command: openclawConfig.command.trim(),
					channel: openclawConfig.channel.trim(),
					target: openclawConfig.target.trim()
				};
			} else if (agentType === 'webhook') {
				body.agent_config = {
					url: webhookConfig.url.trim(),
					secret: webhookConfig.secret.trim()
				};
			} else {
				body.agent_config = {
					command: customConfig.command.trim(),
					args: customConfig.args.trim()
				};
			}
		}
		const response = await adminApi.createUser(body);

		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			showCreateModal = false;
			createForm = { email: '', password: '', name: '' };
			entityType = 'human';
			agentType = 'openclaw';
			openclawConfig = { command: '', channel: '', target: '' };
			webhookConfig = { url: '', secret: '' };
			customConfig = { command: '', args: '' };
			await loadUsers();
			toast.success(get(t)('admin.userCreated'));
		}

		creating = false;
	}

	function openEditUser(user: ManagedUser) {
		editingUserId = user.id;
		editForm = {
			name: user.name,
			email: user.email,
			role: user.role
		};
		showEditModal = true;
	}

	async function saveUserProfile() {
		if (!editingUserId || editSaving) return;

		const name = editForm.name.trim();
		const email = editForm.email.trim().toLowerCase();
		if (!name || !email) {
			toast.error(get(t)('admin.nameEmailRequired'));
			return;
		}

		editSaving = true;
		const result = await adminApi.updateUser(editingUserId, {
			name,
			email,
			role: editForm.role
		});

		if (result.code !== 0) {
			toast.error(result.message);
		} else {
			showEditModal = false;
			editingUserId = null;
			await loadUsers();
			toast.success(get(t)('admin.userUpdated'));
		}
		editSaving = false;
	}

	function openResetPassword(user: ManagedUser) {
		passwordUser = user;
		passwordForm = { newPassword: '', confirmPassword: '' };
		showPasswordModal = true;
	}

	async function savePassword() {
		if (!passwordUser || passwordSaving) return;

		const newPassword = passwordForm.newPassword;
		const confirmPassword = passwordForm.confirmPassword;
		if (newPassword.length < 8) {
			toast.error(get(t)('settings.passwordLength'));
			return;
		}
		if (newPassword !== confirmPassword) {
			toast.error(get(t)('settings.passwordMismatch'));
			return;
		}

		passwordSaving = true;
		const result = await adminApi.resetPassword(passwordUser.id, newPassword);
		if (result.code !== 0) {
			toast.error(result.message);
		} else {
			showPasswordModal = false;
			passwordUser = null;
			passwordForm = { newPassword: '', confirmPassword: '' };
			toast.success(get(t)('admin.passwordResetSuccess'));
		}
		passwordSaving = false;
	}

	async function toggleUserStatus(user: ManagedUser) {
		if (actionLoadingUserId) return;

		const actionText = user.status === 'active' ? get(t)('admin.disabled') : get(t)('admin.active');
		if (!confirm(get(t)('admin.toggleStatusConfirm', { values: { action: actionText, email: user.email } }))) {
			return;
		}

		actionLoadingUserId = user.id;
		const result = await adminApi.toggleUserStatus(user.id);
		if (result.code !== 0) {
			toast.error(result.message);
		} else {
			await loadUsers();
			toast.success(get(t)('admin.toggleStatusSuccess', { values: { action: actionText } }));
		}
		actionLoadingUserId = null;
	}

	function formatDate(dateText: string): string {
		return new Date(dateText).toLocaleString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function openCreateModal() {
		showCreateModal = true;
		createForm = { email: '', password: '', name: '' };
		entityType = 'human';
		agentType = 'openclaw';
		openclawConfig = { command: '', channel: '', target: '' };
		webhookConfig = { url: '', secret: '' };
		customConfig = { command: '', args: '' };
	}

	function isBot(user: ManagedUser): boolean {
		return user.entity_type === 'bot';
	}

	function getAgentTypeLabel(agentTypeValue: string | null | undefined): string {
		switch (agentTypeValue) {
			case 'openclaw':
				return get(t)('agentTypes.openclaw');
			case 'webhook':
				return get(t)('agentTypes.webhook');
			case 'custom':
				return get(t)('agentTypes.custom');
			default:
				return get(t)('admin.botBadge');
		}
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">{$t('admin.users')}</h1>
			<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('admin.usersSubtitle')}</p>
		</div>
		<Button onclick={openCreateModal}>{$t('admin.createUser')}</Button>
	</div>

	<div class="grid grid-cols-1 gap-3 rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-4 lg:grid-cols-4">
		<div class="lg:col-span-2">
			<Input type="search" label={$t('common.search')} placeholder={$t('admin.userSearchPlaceholder')} bind:value={keyword} />
		</div>
		<div>
			<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="roleFilter">{$t('admin.role')}</label>
			<select
				id="roleFilter"
				bind:value={roleFilter}
				class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
			>
				<option value="all">{$t('issue.all')}</option>
				<option value="admin">{$t('roles.admin')}</option>
				<option value="user">{$t('roles.user')}</option>
			</select>
		</div>
		<div>
			<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="statusFilter">{$t('admin.status')}</label>
			<select
				id="statusFilter"
				bind:value={statusFilter}
				class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
			>
				<option value="all">{$t('issue.all')}</option>
				<option value="active">{$t('admin.active')}</option>
				<option value="disabled">{$t('admin.disabled')}</option>
			</select>
		</div>
	</div>

	{#if loading}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-6 text-slate-500 dark:text-slate-400">{$t('common.loading')}</div>
	{:else if filteredUsers.length === 0}
		<div class="rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 p-10 text-center text-slate-500 dark:text-slate-400">{$t('admin.noMatchedUsers')}</div>
	{:else}
		<div class="overflow-hidden rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900">
			<div class="space-y-3 p-3 md:hidden">
				{#each filteredUsers as user (user.id)}
					<div class="rounded-lg border border-slate-200 dark:border-slate-700 p-4 {isBot(user)
						? 'bg-cyan-50 dark:bg-cyan-900/20'
						: 'bg-white dark:bg-slate-900'}">
						<div class="flex items-start justify-between gap-3">
							<div class="min-w-0">
								<p class="text-sm font-semibold text-slate-900 dark:text-slate-100 flex items-center gap-1.5">
									{#if isBot(user)}
										<BotIcon class="h-4 w-4 text-cyan-700 dark:text-cyan-300" title={$t('admin.botBadge')} />
									{/if}
									<span>{user.name}</span>
								</p>
								<p class="mt-1 break-all text-sm text-slate-600 dark:text-slate-300">{user.email}</p>
								<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">{$t('common.createdAt')} {formatDate(user.created_at)}</p>
							</div>
							<span class="inline-flex rounded-full px-2.5 py-1 text-xs font-medium {user.status === 'active'
								? 'bg-green-100 text-green-700'
								: 'bg-red-100 text-red-700'}">
								{user.status}
							</span>
						</div>
						<div class="mt-3 flex flex-wrap items-center gap-2 text-xs text-slate-500 dark:text-slate-400">
							<span>{$t('admin.role')}：{user.role}</span>
							{#if isBot(user)}
								<span class="inline-flex rounded-full bg-cyan-100 px-2 py-0.5 text-xs font-medium text-cyan-700 dark:bg-cyan-900/30 dark:text-cyan-300">
									{getAgentTypeLabel(user.agent_type)}
								</span>
							{/if}
						</div>
						<div class="mt-3 flex flex-wrap gap-2">
							<Button variant="ghost" size="sm" onclick={() => openEditUser(user)}>{$t('common.edit')}</Button>
							{#if !isBot(user)}
								<Button variant="secondary" size="sm" onclick={() => openResetPassword(user)}>{$t('admin.changePassword')}</Button>
							{/if}
							<Button
								variant={user.status === 'active' ? 'danger' : 'primary'}
								size="sm"
								loading={actionLoadingUserId === user.id}
								onclick={() => toggleUserStatus(user)}
							>
								{user.status === 'active' ? $t('admin.disabled') : $t('admin.active')}
							</Button>
						</div>
					</div>
				{/each}
			</div>
			<div class="hidden overflow-x-auto md:block">
				<table class="min-w-full divide-y divide-slate-200 dark:divide-slate-700">
					<thead class="bg-slate-50 dark:bg-slate-950">
						<tr>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('admin.name')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('admin.email')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('admin.role')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.createdAt')}</th>
							<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('admin.status')}</th>
							<th class="px-4 py-3 text-right text-xs font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">{$t('common.actions')}</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-200 dark:divide-slate-700 bg-white dark:bg-slate-900">
						{#each filteredUsers as user (user.id)}
							<tr class={isBot(user) ? 'bg-cyan-50/70 dark:bg-cyan-900/10' : ''}>
								<td class="px-4 py-3 text-sm text-slate-900 dark:text-slate-100">
									<div class="flex items-center gap-1.5">
										{#if isBot(user)}
											<BotIcon class="h-4 w-4 text-cyan-700 dark:text-cyan-300" title={$t('admin.botBadge')} />
										{/if}
										<span>{user.name}</span>
									</div>
								</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">{user.email}</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">
									<div class="flex items-center gap-2">
										<span>{user.role}</span>
										{#if isBot(user)}
											<span class="inline-flex rounded-full bg-cyan-100 px-2 py-0.5 text-xs font-medium text-cyan-700 dark:bg-cyan-900/30 dark:text-cyan-300">
												{getAgentTypeLabel(user.agent_type)}
											</span>
										{/if}
									</div>
								</td>
								<td class="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">{formatDate(user.created_at)}</td>
								<td class="px-4 py-3 text-sm">
									<span class="inline-flex rounded-full px-2.5 py-1 text-xs font-medium {user.status === 'active'
										? 'bg-green-100 text-green-700'
										: 'bg-red-100 text-red-700'}">
										{user.status}
									</span>
								</td>
								<td class="px-4 py-3 text-right">
									<div class="flex justify-end gap-2">
										<Button variant="ghost" size="sm" onclick={() => openEditUser(user)}>{$t('common.edit')}</Button>
										{#if !isBot(user)}
											<Button variant="secondary" size="sm" onclick={() => openResetPassword(user)}>
												{$t('admin.changePassword')}
											</Button>
										{/if}
										<Button
											variant={user.status === 'active' ? 'danger' : 'primary'}
											size="sm"
											loading={actionLoadingUserId === user.id}
											onclick={() => toggleUserStatus(user)}
										>
											{user.status === 'active' ? $t('admin.disabled') : $t('admin.active')}
										</Button>
									</div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</div>
	{/if}
</div>

<Modal bind:open={showCreateModal} title={$t('admin.createNewUser')}>
	<form
		onsubmit={(event) => {
			event.preventDefault();
			handleCreateUser();
		}}
		class="space-y-4"
	>
		<div>
			<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="entityType">{$t('admin.entityType')}</label>
			<select
				id="entityType"
				bind:value={entityType}
				class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
			>
				<option value="human">{$t('admin.human')}</option>
				<option value="bot">{$t('admin.bot')}</option>
			</select>
		</div>
		<Input type="email" label={$t('admin.email')} bind:value={createForm.email} required />
		{#if entityType === 'human'}
			<Input type="password" label={$t('auth.password')} bind:value={createForm.password} required minlength={8} />
		{/if}
		<Input label={$t('admin.name')} bind:value={createForm.name} required />
		{#if entityType === 'bot'}
			<div>
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="agentType">{$t('admin.agentType')}</label>
				<select
					id="agentType"
					bind:value={agentType}
					class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
				>
					<option value="openclaw">{$t('agentTypes.openclaw')}</option>
					<option value="webhook">{$t('agentTypes.webhook')}</option>
					<option value="custom">{$t('agentTypes.custom')}</option>
				</select>
			</div>

			<div class="space-y-3 rounded-md border border-slate-200 dark:border-slate-700 p-3">
				<p class="text-sm font-medium text-slate-700 dark:text-slate-300">{$t('admin.agentConfig')}</p>
				{#if agentType === 'openclaw'}
					<Input label={$t('formLabels.command')} bind:value={openclawConfig.command} placeholder={$t('placeholders.openclawCommand')} required />
					<Input label={$t('formLabels.channel')} bind:value={openclawConfig.channel} placeholder={$t('placeholders.openclawChannel')} required />
					<Input label={$t('formLabels.target')} bind:value={openclawConfig.target} placeholder={$t('placeholders.openclawTarget')} required />
				{:else if agentType === 'webhook'}
					<Input type="url" label={$t('formLabels.url')} bind:value={webhookConfig.url} placeholder={$t('placeholders.webhookUrlInput')} required />
					<Input label={$t('formLabels.secret')} bind:value={webhookConfig.secret} placeholder={$t('placeholders.webhookSecret')} />
				{:else if agentType === 'custom'}
					<Input label={$t('formLabels.command')} bind:value={customConfig.command} placeholder={$t('placeholders.customCommand')} required />
					<Input label={$t('formLabels.args')} bind:value={customConfig.args} placeholder={$t('placeholders.customArgs')} />
				{/if}
			</div>
		{/if}
		<div class="flex justify-end gap-2 pt-2">
			<Button variant="secondary" onclick={() => (showCreateModal = false)}>{$t('common.cancel')}</Button>
			<Button type="submit" loading={creating}>{$t('common.create')}</Button>
		</div>
	</form>
</Modal>

<Modal bind:open={showEditModal} title={$t('admin.editUser')}>
	{#if editingUserId}
		<form
			onsubmit={(event) => {
				event.preventDefault();
				saveUserProfile();
			}}
			class="space-y-4"
		>
			<Input label={$t('admin.name')} bind:value={editForm.name} required />
			<Input type="email" label={$t('admin.email')} bind:value={editForm.email} required />
			<div>
				<label class="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300" for="editRole">{$t('admin.role')}</label>
					<select
						id="editRole"
						bind:value={editForm.role}
						class="w-full rounded-md border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-800 px-3 py-2 text-sm text-slate-900 dark:text-slate-100 focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500 dark:focus:ring-blue-400"
					>
					<option value="user">{$t('roles.user')}</option>
					<option value="admin">{$t('roles.admin')}</option>
				</select>
			</div>
			<div class="flex justify-end gap-2 pt-2">
				<Button variant="secondary" onclick={() => (showEditModal = false)}>{$t('common.cancel')}</Button>
				<Button type="submit" loading={editSaving}>{$t('common.save')}</Button>
			</div>
		</form>
	{/if}
</Modal>

<Modal bind:open={showPasswordModal} title={$t('admin.changePassword')}>
	{#if passwordUser}
		<form
			onsubmit={(event) => {
				event.preventDefault();
				savePassword();
			}}
			class="space-y-4"
		>
			<div class="text-sm text-slate-600 dark:text-slate-300">{$t('common.user')}：{passwordUser.email}</div>
			<Input type="password" label={$t('settings.newPassword')} bind:value={passwordForm.newPassword} required minlength={8} />
			<Input
				type="password"
				label={$t('settings.confirmPassword')}
				bind:value={passwordForm.confirmPassword}
				required
				minlength={8}
			/>
			<div class="flex justify-end gap-2 pt-2">
				<Button variant="secondary" onclick={() => (showPasswordModal = false)}>{$t('common.cancel')}</Button>
				<Button type="submit" loading={passwordSaving}>{$t('common.save')}</Button>
			</div>
		</form>
	{/if}
</Modal>

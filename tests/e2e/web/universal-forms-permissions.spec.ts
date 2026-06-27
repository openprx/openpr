import { expect, test, type Page } from '@playwright/test';

const email = process.env.TEST_EMAIL ?? 'demo@openpr.local';
const password = process.env.TEST_PASSWORD ?? 'OpenPRDemo123!';
const workspaceId = process.env.OPENPR_WORKSPACE_ID ?? '07f6e023-6b0a-425c-bdac-442b5d36cd0c';
const projectId = process.env.OPENPR_PROJECT_ID ?? '8f7f7726-948e-4ea4-b149-06f25753b525';

type LoginData = {
	tokens: { access_token: string; refresh_token: string };
	user: unknown;
};

type UniversalForm = {
	id: string;
	key: string;
	name: string;
};

type FormPermissions = {
	policies: Array<{
		subject_type: string;
		subject_id: string;
		policy: {
			record_scope?: string;
			fields?: Record<string, { read?: boolean; write?: boolean }>;
		};
	}>;
};

async function login(page: Page): Promise<LoginData> {
	const response = await page.request.post('/api/v1/auth/login', {
		data: { email, password }
	});
	expect(response.ok()).toBe(true);
	const body = await response.json();
	expect(body.code).toBe(0);
	const loginData = body.data as LoginData;

	await page.addInitScript(
		({ accessToken, refreshToken, authUser }) => {
			localStorage.setItem('auth_token', accessToken);
			localStorage.setItem('refresh_token', refreshToken);
			localStorage.setItem('auth_user', JSON.stringify(authUser));
			localStorage.setItem('locale', 'zh');
		},
		{
			accessToken: loginData.tokens.access_token,
			refreshToken: loginData.tokens.refresh_token,
			authUser: loginData.user
		}
	);

	return loginData;
}

async function createTemporaryForm(page: Page, token: string, suffix: string): Promise<UniversalForm> {
	const response = await page.request.post(`/api/v1/projects/${projectId}/forms`, {
		headers: { Authorization: `Bearer ${token}` },
		data: {
			key: `qa_perm_${suffix}`,
			name: `QA Permissions ${suffix}`,
			description: 'Temporary Playwright form for permissions coverage',
			title_template: '{title}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'title', label: 'Title', type: 'text', required: true },
					{ key: 'secret', label: 'Secret', type: 'text', required: false }
				]
			}
		}
	});
	expect(response.ok()).toBe(true);
	const body = await response.json();
	expect(body.code).toBe(0);
	return body.data as UniversalForm;
}

function formsPath(formId: string) {
	return `/workspace/${workspaceId}/projects/${projectId}/forms?form=${formId}`;
}

test.describe('Universal forms / permissions', () => {
	test('saves member record scope and field read/write policy on a temporary form', async ({
		page
	}, testInfo) => {
		const loginData = await login(page);
		const suffix = Date.now().toString(36);
		const form = await createTemporaryForm(page, loginData.tokens.access_token, suffix);

			try {
				await page.goto(formsPath(form.id));
				await expect(page.getByRole('heading', { name: '记录列表' })).toBeVisible();
				await expect(page.locator('#form-switcher')).toHaveValue(form.id);

				await page.getByRole('button', { name: '权限' }).click();
			await expect(page.getByText(`这里配置 ${form.name} 的成员动作、记录范围和字段读写策略`)).toBeVisible();
			await expect(page.getByRole('heading', { name: '权限' })).toBeVisible();
			await expect(page.locator('[data-form-mode="permissions"]')).toBeVisible();
			await expect(page.locator('[data-permission-state-summary]')).toHaveCount(0);

			await page.getByRole('combobox', { name: '记录范围' }).selectOption('owned');
			await page.locator('[data-field-read-permission="secret"]').uncheck();
			await expect(page.locator('[data-field-read-permission="secret"]')).not.toBeChecked();
			await expect(page.locator('[data-field-write-permission="secret"]')).not.toBeChecked();
			await expect(page.locator('[data-permission-effect-hidden-fields]')).toHaveAttribute(
				'data-permission-effect-hidden-fields',
				'Secret'
			);
			await expect(page.locator('[data-field-read-denied-count]')).toHaveAttribute(
				'data-field-read-denied-count',
				'1'
			);
			await expect(page.locator('[data-field-write-denied-count]')).toHaveAttribute(
				'data-field-write-denied-count',
				'1'
			);

			await page.getByRole('button', { name: '保存权限' }).click();
			await expect(page.getByText('权限已保存')).toBeVisible();

			const permissionsResponse = await page.request.get(`/api/v1/forms/${form.id}/permissions`, {
				headers: { Authorization: `Bearer ${loginData.tokens.access_token}` }
			});
			expect(permissionsResponse.ok()).toBe(true);
			const permissionsBody = await permissionsResponse.json();
			expect(permissionsBody.code).toBe(0);
			const permissions = permissionsBody.data as FormPermissions;
			const memberPolicy = permissions.policies.find(
				(policy) => policy.subject_type === 'role' && policy.subject_id === 'member'
			);
			expect(memberPolicy).toBeTruthy();
			expect(memberPolicy?.policy.record_scope).toBe('owned');
			expect(memberPolicy?.policy.fields?.secret).toEqual({ read: false, write: false });

			await page.getByRole('button', { name: '字段设计' }).click();
			await expect(page.locator('[data-designer-field-permission-hidden="secret"]')).toBeVisible();
			await expect(page.locator('[data-designer-field-permission-locked="secret"]')).toBeVisible();

			await page.screenshot({
				path: '/opt/worker/task/openpr/test/artifacts/universal-forms-permissions/permissions-saved-desktop.png',
				fullPage: true
			});
			await testInfo.attach('permissions-saved-desktop', {
				path: '/opt/worker/task/openpr/test/artifacts/universal-forms-permissions/permissions-saved-desktop.png',
				contentType: 'image/png'
			});
		} finally {
			await page.request.delete(`/api/v1/forms/${form.id}`, {
				headers: { Authorization: `Bearer ${loginData.tokens.access_token}` }
			});
		}
	});
});

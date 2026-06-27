import { expect, test, type Page } from '@playwright/test';

const email = process.env.TEST_EMAIL ?? 'demo@openpr.local';
const password = process.env.TEST_PASSWORD ?? 'OpenPRDemo123!';
const workspaceId = process.env.OPENPR_WORKSPACE_ID ?? '07f6e023-6b0a-425c-bdac-442b5d36cd0c';
const projectId = process.env.OPENPR_PROJECT_ID ?? '8f7f7726-948e-4ea4-b149-06f25753b525';
const codeTaskFormId = process.env.OPENPR_CODE_TASK_FORM_ID ?? 'db4d73cc-9639-44b9-9875-21be69b0831f';

type LoginData = {
	tokens: { access_token: string; refresh_token: string };
	user: unknown;
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

async function createRecord(page: Page, token: string, suffix: string) {
	const repo = `qa-detail-${suffix}`;
	const branch = `feature/${suffix}`;
	const response = await page.request.post(`/api/v1/forms/${codeTaskFormId}/records`, {
		headers: { Authorization: `Bearer ${token}` },
		data: {
			values: {
				repo,
				directory: '/opt/worker/code/openpr',
				branch,
				ci_status: 'passing',
				risk: 'medium',
				verification: 'Initial detail/edit/delete fixture'
			},
			source: { type: 'playwright' }
		}
	});
	expect(response.ok()).toBe(true);
	const body = await response.json();
	expect(body.code).toBe(0);
	return {
		id: body.data.id as string,
		title: body.data.title as string,
		repo,
		branch
	};
}

function formsPath() {
	return `/workspace/${workspaceId}/projects/${projectId}/forms?form=${codeTaskFormId}`;
}

test.describe('Universal forms / Code Task detail edit delete', () => {
	test('opens detail, edits a record, then deletes it from the grid', async ({ page }) => {
		const loginData = await login(page);
		const suffix = Date.now().toString(36);
		const record = await createRecord(page, loginData.tokens.access_token, suffix);
		const updatedVerification = `Updated by Playwright ${suffix}`;
		let cleanupNeeded = true;

		try {
			await page.goto(formsPath());
				const row = page.getByRole('row', { name: new RegExp(record.repo) });
				await expect(row).toBeVisible();

				await row.getByRole('button', { name: record.title }).click();
				await expect(page.locator('[data-record-editor-drawer]')).toBeVisible();
				await expect(page.getByRole('heading', { name: '编辑记录' })).toBeVisible();
				await page.getByRole('textbox', { name: 'Verification' }).fill(updatedVerification);
				await page.getByRole('button', { name: '保存记录' }).click();
				await expect(page.getByText('记录已更新')).toBeVisible();
				await expect(page.getByText(updatedVerification)).toBeVisible();

			const updatedRow = page.getByRole('row', { name: new RegExp(record.repo) });
			await updatedRow.getByRole('button', { name: '删除记录' }).click();
			await expect(page.getByText('记录已删除')).toBeVisible();
			await expect(page.getByRole('row', { name: new RegExp(record.repo) })).toHaveCount(0);
			cleanupNeeded = false;
		} finally {
			if (cleanupNeeded) {
				await page.request.delete(`/api/v1/form-records/${record.id}`, {
					headers: { Authorization: `Bearer ${loginData.tokens.access_token}` }
				});
			}
		}
	});
});

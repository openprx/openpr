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

function formsPath() {
	return `/workspace/${workspaceId}/projects/${projectId}/forms?form=${codeTaskFormId}`;
}

test.describe('Universal forms / Code Task record CRUD', () => {
	test('creates a Code Task record through the human data-entry form and cleans it up', async ({
		page
	}) => {
		const loginData = await login(page);
		const suffix = Date.now().toString(36);
		const repo = `qa-human-${suffix}`;
		const branch = `feature/${suffix}`;
		let createdRecordId: string | null = null;

			try {
				await page.goto(formsPath());
				await page.getByRole('button', { name: '新增', exact: true }).click();
				await expect(page.getByRole('heading', { name: '新建记录' })).toBeVisible();

			await page.getByRole('textbox', { name: 'Repository *' }).fill(repo);
			await page.getByRole('textbox', { name: 'Directory' }).fill('/opt/worker/code/openpr');
			await page.getByRole('textbox', { name: 'Branch' }).fill(branch);
			await page.getByRole('combobox', { name: 'CI status' }).selectOption('passing');
			await page.getByRole('combobox', { name: 'Risk' }).selectOption('low');
			await page.getByRole('textbox', { name: 'Verification' }).fill('Playwright QA record creation flow');

			await page.getByRole('button', { name: '添加记录' }).click();
			await expect(page.getByText('记录已保存')).toBeVisible();

			const recordsResponse = await page.request.get(`/api/v1/forms/${codeTaskFormId}/records?per_page=50`, {
				headers: {
					Authorization: `Bearer ${loginData.tokens.access_token}`
				}
			});
			expect(recordsResponse.ok()).toBe(true);
			const recordsBody = await recordsResponse.json();
			const created = recordsBody.data.items.find(
				(item: { values?: Record<string, unknown> }) => item.values?.repo === repo
			);
			expect(created).toBeTruthy();
			createdRecordId = created.id;
			await expect(page.getByRole('button', { name: `${repo}:${branch}` })).toBeVisible();
			await expect(page.getByText(branch, { exact: true })).toBeVisible();
		} finally {
			if (createdRecordId) {
				await page.request.delete(`/api/v1/form-records/${createdRecordId}`, {
					headers: {
						Authorization: `Bearer ${loginData.tokens.access_token}`
					}
				});
			}
		}
	});
});

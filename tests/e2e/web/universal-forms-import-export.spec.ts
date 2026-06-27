import { expect, test, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';

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

type FormRecord = {
	id: string;
	title: string;
	values: Record<string, unknown>;
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
			key: `qa_import_${suffix}`,
			name: `QA Import ${suffix}`,
			description: 'Temporary Playwright form for import/export coverage',
			title_template: '{title}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'repo', label: 'Repository', type: 'text', required: true },
					{ key: 'status', label: 'Status', type: 'single_select', options: ['open', 'closed'], required: false }
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

test.describe('Universal forms / import export', () => {
	test('imports a CSV row through the modal and exports the current view as JSON', async ({
		page
	}, testInfo) => {
		const loginData = await login(page);
		const suffix = Date.now().toString(36);
		const form = await createTemporaryForm(page, loginData.tokens.access_token, suffix);
		const repo = `qa-import-${suffix}`;
		let importedRecordId: string | null = null;

			try {
				await page.goto(formsPath(form.id));
				await expect(page.getByRole('heading', { name: '记录列表' })).toBeVisible();
				await expect(page.locator('#form-switcher')).toHaveValue(form.id);
				await page.getByRole('button', { name: '导入导出' }).click();
			await expect(page.locator('[data-form-mode="import-export"]')).toBeVisible();

			await page.getByRole('button', { name: '导入记录' }).click();
			await expect(page.getByRole('heading', { name: '导入记录' })).toBeVisible();
			await expect(page.locator('[data-import-wizard]')).toHaveAttribute('data-import-wizard-step', 'source');
			await expect(page.locator('[data-import-wizard-panel="source"]')).toBeVisible();
			await page.locator('#form-import-text').fill(`Title,repo,status\n${repo},${repo},open\n`);
			await page.getByRole('button', { name: '下一步' }).click();
			await expect(page.locator('[data-import-wizard]')).toHaveAttribute('data-import-wizard-step', 'mapping');
			await expect(page.locator('[data-import-mapping-wizard]')).toBeVisible();

			await page.getByRole('button', { name: '预览导入' }).click();
			await expect(page.getByText('导入预览已生成')).toBeVisible();
			await expect(page.locator('[data-import-wizard]')).toHaveAttribute('data-import-wizard-step', 'preview');
			await expect(page.getByText('总计 1 行，可导入 1 行，错误 0 行')).toBeVisible();

				await page.getByRole('button', { name: '提交导入' }).click();
				await expect(page.getByText('已导入 1 条记录')).toBeVisible();
				await page.getByRole('button', { name: '返回记录列表' }).click();
				await expect(page.getByRole('row', { name: new RegExp(repo) })).toBeVisible();

			const recordsResponse = await page.request.get(`/api/v1/forms/${form.id}/records?per_page=20`, {
				headers: { Authorization: `Bearer ${loginData.tokens.access_token}` }
			});
			expect(recordsResponse.ok()).toBe(true);
			const recordsBody = await recordsResponse.json();
			expect(recordsBody.code).toBe(0);
			const importedRecord = recordsBody.data.items.find(
				(record: FormRecord) => record.values.repo === repo
			) as FormRecord | undefined;
			expect(importedRecord).toBeTruthy();
			importedRecordId = importedRecord?.id ?? null;

			const downloadPromise = page.waitForEvent('download');
			await page.getByRole('button', { name: '导入导出' }).click();
			await page.getByRole('button', { name: '导出当前视图 JSON' }).click();
			const download = await downloadPromise;
			expect(download.suggestedFilename()).toContain(form.key);
			const downloadPath = await download.path();
			expect(downloadPath).toBeTruthy();
			const exported = JSON.parse(readFileSync(downloadPath as string, 'utf8')) as {
				rows: Array<{ record_id: string; title: string; values: Record<string, string> }>;
			};
			expect(exported.rows).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						record_id: importedRecordId,
						values: expect.objectContaining({ repo })
					})
				])
			);
			await expect(page.getByText('记录 JSON 已导出')).toBeVisible();

			await page.screenshot({
				path: '/opt/worker/task/openpr/test/artifacts/universal-forms-import-export/import-export-desktop.png',
				fullPage: true
			});
			await testInfo.attach('import-export-desktop', {
				path: '/opt/worker/task/openpr/test/artifacts/universal-forms-import-export/import-export-desktop.png',
				contentType: 'image/png'
			});
		} finally {
			if (importedRecordId) {
				await page.request.delete(`/api/v1/form-records/${importedRecordId}`, {
					headers: { Authorization: `Bearer ${loginData.tokens.access_token}` }
				});
			}
			await page.request.delete(`/api/v1/forms/${form.id}`, {
				headers: { Authorization: `Bearer ${loginData.tokens.access_token}` }
			});
		}
	});
});

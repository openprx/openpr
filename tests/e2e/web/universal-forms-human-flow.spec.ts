import { mkdirSync } from 'node:fs';
import { expect, test, type Page, type TestInfo } from '@playwright/test';

const email = process.env.TEST_EMAIL ?? 'demo@openpr.local';
const password = process.env.TEST_PASSWORD ?? 'OpenPRDemo123!';
const workspaceId = process.env.OPENPR_WORKSPACE_ID ?? '07f6e023-6b0a-425c-bdac-442b5d36cd0c';
const projectId = process.env.OPENPR_PROJECT_ID ?? '8f7f7726-948e-4ea4-b149-06f25753b525';
const codeTaskFormId = process.env.OPENPR_CODE_TASK_FORM_ID ?? 'db4d73cc-9639-44b9-9875-21be69b0831f';
const artifactDir = '/opt/worker/task/openpr/test/artifacts/universal-forms-human-flow';

async function login(page: Page) {
	const response = await page.request.post('/api/v1/auth/login', {
		data: { email, password }
	});
	expect(response.ok()).toBe(true);
	const body = await response.json();
	expect(body.code).toBe(0);
	const { tokens, user } = body.data;

	await page.addInitScript(
		({ accessToken, refreshToken, authUser }) => {
			localStorage.setItem('auth_token', accessToken);
			localStorage.setItem('refresh_token', refreshToken);
			localStorage.setItem('auth_user', JSON.stringify(authUser));
			localStorage.setItem('locale', 'zh');
		},
		{
			accessToken: tokens.access_token,
			refreshToken: tokens.refresh_token,
			authUser: user
		}
	);
}

function formsPath() {
	return `/workspace/${workspaceId}/projects/${projectId}/forms?form=${codeTaskFormId}`;
}

async function attachScreenshot(page: Page, testInfo: TestInfo, name: string) {
	mkdirSync(artifactDir, { recursive: true });
	const path = `${artifactDir}/${name}.png`;
	await page.screenshot({ path, fullPage: true });
	await testInfo.attach(name, { path, contentType: 'image/png' });
}

test.describe('Universal forms / human Code Task flow', () => {
	test('opens Code Task in data-entry mode with advanced view tools collapsed', async ({ page }, testInfo) => {
		const consoleErrors: string[] = [];
		page.on('console', (message) => {
			if (message.type() === 'error' && !message.text().includes('favicon.ico')) {
				consoleErrors.push(message.text());
			}
		});

		await login(page);
		await page.goto(formsPath());

		await expect(page.getByRole('heading', { name: '记录列表' })).toBeVisible();
		await expect(page.locator('#form-switcher')).toHaveValue(codeTaskFormId);
		await expect(page.getByRole('button', { name: '保存视图' })).toBeVisible();
		await page.getByRole('button', { name: '新增', exact: true }).click();
		await expect(page.locator('[data-record-editor-drawer]')).toBeVisible();
		await expect(page.getByRole('heading', { name: '新建记录' })).toBeVisible();
		await expect(page.getByRole('button', { name: '添加记录' })).toBeVisible();
		await expect(page.getByText('视图标识')).toHaveCount(0);
		await attachScreenshot(page, testInfo, 'code-task-data-desktop');
		expect(consoleErrors).toEqual([]);
	});

	test('can expand view tools and switch to field design without losing context', async ({ page }, testInfo) => {
		await login(page);
		await page.goto(formsPath());

		await page.getByRole('button', { name: '保存视图' }).click();
		const expandedTools = page.locator('[data-view-tools-expanded-panel]');
		await expect(expandedTools).toBeVisible();
		await expect(page.getByText('视图设置')).toBeVisible();
		await expect(expandedTools.locator('[data-view-columns-disclosure]')).toBeVisible();
		await expect(page.getByText('视图标识')).toBeHidden();
		await expect(page.getByRole('button', { name: '导入记录' })).toHaveCount(0);
		await expect(page.getByRole('button', { name: '导入导出' })).toBeVisible();
		await attachScreenshot(page, testInfo, 'code-task-view-tools-expanded-desktop');

		await page.getByRole('button', { name: '字段设计' }).click();
		await expect(page.getByText('这里只修改 Code Task 的字段结构')).toBeVisible();
		await expect(page.getByRole('heading', { name: '字段库' })).toBeVisible();
		await expect(page.getByRole('button', { name: '保存设计' })).toBeVisible();
		await expect(page.locator('[data-form-mode="field-design"] [data-detail-header-builder]')).toHaveCount(0);
		await expect(page.getByRole('button', { name: '保存头部' })).toHaveCount(0);
		await expect(page.getByText('字段安全')).toBeVisible();
		await attachScreenshot(page, testInfo, 'code-task-field-design-desktop');
	});
});

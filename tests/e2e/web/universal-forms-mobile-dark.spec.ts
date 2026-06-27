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

async function login(page: Page, theme: 'light' | 'dark' = 'light'): Promise<LoginData> {
	const response = await page.request.post('/api/v1/auth/login', {
		data: { email, password }
	});
	expect(response.ok()).toBe(true);
	const body = await response.json();
	expect(body.code).toBe(0);
	const loginData = body.data as LoginData;

	await page.addInitScript(
		({ accessToken, refreshToken, authUser, selectedTheme }) => {
			localStorage.setItem('auth_token', accessToken);
			localStorage.setItem('refresh_token', refreshToken);
			localStorage.setItem('auth_user', JSON.stringify(authUser));
			localStorage.setItem('locale', 'zh');
			localStorage.setItem('theme', selectedTheme);
		},
		{
			accessToken: loginData.tokens.access_token,
			refreshToken: loginData.tokens.refresh_token,
			authUser: loginData.user,
			selectedTheme: theme
		}
	);

	return loginData;
}

function formsPath() {
	return `/workspace/${workspaceId}/projects/${projectId}/forms?form=${codeTaskFormId}`;
}

async function pageHasNoDocumentHorizontalOverflow(page: Page) {
	return page.evaluate(() => {
		const root = document.documentElement;
		const body = document.body;
		return root.scrollWidth <= window.innerWidth + 1 && body.scrollWidth <= window.innerWidth + 1;
	});
}

test.describe('Universal forms / mobile and dark mode', () => {
	test('keeps Code Task data-entry usable on mobile without horizontal overflow', async ({
		page
	}, testInfo) => {
		await page.setViewportSize({ width: 390, height: 844 });
		await login(page, 'light');
		await page.goto(formsPath());

			await expect(page.getByRole('heading', { name: '记录列表' })).toBeVisible();
			await expect(page.locator('#form-switcher')).toHaveValue(codeTaskFormId);
			await expect(page.getByRole('button', { name: '保存视图' })).toBeVisible();
			await page.getByRole('button', { name: '新增', exact: true }).click();
			await expect(page.locator('[data-record-editor-drawer]')).toBeVisible();
			await expect(page.getByRole('heading', { name: '新建记录' })).toBeVisible();
		await expect(await pageHasNoDocumentHorizontalOverflow(page)).toBe(true);

		await page.screenshot({
			path: '/opt/worker/task/openpr/test/artifacts/universal-forms-mobile-dark/code-task-mobile.png',
			fullPage: true
		});
		await testInfo.attach('code-task-mobile', {
			path: '/opt/worker/task/openpr/test/artifacts/universal-forms-mobile-dark/code-task-mobile.png',
			contentType: 'image/png'
		});
	});

	test('keeps field design actionability in dark mode', async ({ page }, testInfo) => {
		await page.setViewportSize({ width: 1280, height: 720 });
		await login(page, 'dark');
		await page.goto(formsPath());

		await expect(page.locator('html')).toHaveClass(/dark/);
		await page.getByRole('button', { name: '字段设计' }).click();
		await expect(page.getByText('这里只修改 Code Task 的字段结构')).toBeVisible();
		await expect(page.getByRole('heading', { name: '字段库' })).toBeVisible();
		await expect(page.locator('[data-form-mode="field-design"] [data-detail-header-builder]')).toHaveCount(0);
		await expect(await pageHasNoDocumentHorizontalOverflow(page)).toBe(true);
		await page.getByRole('button', { name: '保存设计' }).click({ trial: true });

		await page.screenshot({
			path: '/opt/worker/task/openpr/test/artifacts/universal-forms-mobile-dark/code-task-field-design-dark.png',
			fullPage: true
		});
		await testInfo.attach('code-task-field-design-dark', {
			path: '/opt/worker/task/openpr/test/artifacts/universal-forms-mobile-dark/code-task-field-design-dark.png',
			contentType: 'image/png'
		});
	});
});

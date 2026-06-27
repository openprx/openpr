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
	schema_version: number;
	schema: {
		fields?: Array<Record<string, unknown>>;
	};
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
			key: `qa_design_${suffix}`,
			name: `QA Design ${suffix}`,
			description: 'Temporary Playwright form for field designer save coverage',
			title_template: '{title}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{
						key: 'title',
						label: 'Title',
						type: 'text',
						required: true
					}
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

test.describe('Universal forms / field design save', () => {
	test('adds a field in the designer, saves it, and archives the temporary form', async ({
		page
	}, testInfo) => {
		const loginData = await login(page);
		const suffix = Date.now().toString(36);
		const form = await createTemporaryForm(page, loginData.tokens.access_token, suffix);
		const fieldKey = `qa_notes_${suffix}`;
		const fieldLabel = `QA Notes ${suffix}`;
		let cleanupNeeded = true;

			try {
				await page.goto(formsPath(form.id));
				await expect(page.getByRole('heading', { name: '记录列表' })).toBeVisible();
				await expect(page.locator('#form-switcher')).toHaveValue(form.id);

				await page.getByRole('button', { name: '字段设计' }).click();
			await expect(page.getByText(`这里只修改 ${form.name} 的字段结构`)).toBeVisible();
			await expect(page.getByRole('heading', { name: '字段库' })).toBeVisible();

			await page.locator('[data-field-library-type="textarea"]').click();
			const newField = page.locator('[data-designer-field-type="textarea"]').last();
			await expect(newField).toBeVisible();
			await expect(newField).toContainText('textarea_2');

			await page.getByLabel('字段名称').fill(fieldLabel);
			await page.getByLabel('字段标识').fill(fieldKey);
			await page.getByRole('button', { name: '保存设计' }).click();
			await expect(page.getByText('表单设计已保存')).toBeVisible();
			await expect(page.locator(`[data-designer-field-key="${fieldKey}"]`)).toContainText(fieldLabel);

			const formResponse = await page.request.get(`/api/v1/forms/${form.id}`, {
				headers: { Authorization: `Bearer ${loginData.tokens.access_token}` }
			});
			expect(formResponse.ok()).toBe(true);
			const formBody = await formResponse.json();
			expect(formBody.code).toBe(0);
			const updatedForm = formBody.data as UniversalForm;
			expect(updatedForm.schema_version).toBeGreaterThan(form.schema_version);
			expect(updatedForm.schema.fields).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						key: fieldKey,
						label: fieldLabel,
						type: 'textarea',
						required: false
					})
				])
			);

			await page.screenshot({
				path: `/opt/worker/task/openpr/test/artifacts/universal-forms-field-design-save/field-saved-desktop.png`,
				fullPage: true
			});
			await testInfo.attach('field-saved-desktop', {
				path: '/opt/worker/task/openpr/test/artifacts/universal-forms-field-design-save/field-saved-desktop.png',
				contentType: 'image/png'
			});
		} finally {
			if (cleanupNeeded) {
				await page.request.delete(`/api/v1/forms/${form.id}`, {
					headers: { Authorization: `Bearer ${loginData.tokens.access_token}` }
				});
				cleanupNeeded = false;
			}
		}
	});
});

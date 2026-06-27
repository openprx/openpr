#!/usr/bin/env node

const frontendUrl = (process.env.OPENPR_FRONTEND_URL ?? 'http://10.72.0.3:3000').replace(/\/+$/, '');
const email = process.env.OPENPR_DEMO_EMAIL ?? 'demo@openpr.local';
const password = process.env.OPENPR_DEMO_PASSWORD ?? 'OpenPRDemo123!';
const workspaceOverride = process.env.OPENPR_WORKSPACE_ID;
const projectOverride = process.env.OPENPR_PROJECT_ID;
const expectedBackend = process.env.OPENPR_EXPECT_OBJECT_STORAGE_BACKEND;
const cleanupCreated = process.env.OPENPR_ATTACHMENT_LIFECYCLE_CLEANUP !== '0';
const suffix = Date.now().toString(36);

let accessToken = '';
let workspaceId = '';
let projectId = '';
let createdFormId = '';

const pngBytes = Buffer.from(
	'iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAACXBIWXMAAAABAAAAAQBPJcTWAAAAFklEQVR4nGO'
		+ 'wrz9AEmIY1TCqYfhqAACUSH4QSDwlWwAAAABJRU5ErkJggg==',
	'base64'
);

function usage() {
	console.log(`Usage:
  OPENPR_FRONTEND_URL=https://openpr.example \\
  OPENPR_DEMO_EMAIL=admin@example.com \\
  OPENPR_DEMO_PASSWORD=... \\
  OPENPR_EXPECT_OBJECT_STORAGE_BACKEND=s3 \\
  scripts/smoke-universal-forms-production-attachment-lifecycle.mjs

Optional:
  OPENPR_WORKSPACE_ID                         Workspace to use. Defaults to first visible workspace.
  OPENPR_PROJECT_ID                           Project to use. Defaults to first custom_form project, then first project.
  OPENPR_EXPECT_OBJECT_STORAGE_BACKEND        Assert upload response storage_backend, for example s3 or local.
  OPENPR_ATTACHMENT_LIFECYCLE_CLEANUP=0       Keep the temporary form for inspection.

This deployed smoke uploads a private image attachment, creates record-scoped
attachment metadata, verifies signed download, archives the attachment and
checks that active listing and signed download reject it, restores the
attachment, and verifies signed download works again.`);
}

if (process.argv.includes('--help') || process.argv.includes('-h')) {
	usage();
	process.exit(0);
}

function assert(condition, message) {
	if (!condition) throw new Error(message);
}

function absoluteUrl(pathOrUrl) {
	return pathOrUrl.startsWith('http://') || pathOrUrl.startsWith('https://')
		? pathOrUrl
		: `${frontendUrl}${pathOrUrl}`;
}

async function rawJsonApi(path, options = {}) {
	const headers = new Headers(options.headers ?? {});
	headers.set('Content-Type', 'application/json');
	if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`);
	const response = await fetch(`${frontendUrl}${path}`, {
		...options,
		headers,
		body: options.body === undefined || typeof options.body === 'string' ? options.body : JSON.stringify(options.body)
	});
	const text = await response.text();
	const parsed = text ? JSON.parse(text) : { code: response.status, message: response.statusText, data: null };
	return { response, parsed, text };
}

async function api(path, options = {}) {
	const { response, parsed, text } = await rawJsonApi(path, options);
	if (!response.ok || parsed.code !== 0) {
		throw new Error(`${options.method ?? 'GET'} ${path} failed: HTTP ${response.status} ${text}`);
	}
	return parsed.data;
}

async function expectApiFailure(path, options, expectedStatus, label) {
	const { response, parsed, text } = await rawJsonApi(path, options);
	assert(
		response.status === expectedStatus || parsed.code === expectedStatus,
		`${label} expected HTTP/envelope ${expectedStatus}, got HTTP ${response.status}, code ${parsed.code}: ${text}`
	);
	return text;
}

async function fetchBinary(pathOrUrl, label) {
	const headers = new Headers();
	if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`);
	const response = await fetch(absoluteUrl(pathOrUrl), { headers });
	const bytes = Buffer.from(await response.arrayBuffer());
	if (!response.ok || bytes.length === 0) {
		throw new Error(`${label} read failed: HTTP ${response.status}, bytes=${bytes.length}`);
	}
	return { response, bytes };
}

async function uploadFile(fileName, contentType, bytes) {
	const formData = new FormData();
	formData.append('file', new Blob([bytes], { type: contentType }), fileName);
	const headers = new Headers();
	if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`);
	const response = await fetch(`${frontendUrl}/api/v1/upload`, {
		method: 'POST',
		headers,
		body: formData
	});
	const text = await response.text();
	const parsed = text ? JSON.parse(text) : { code: response.status, message: response.statusText, data: null };
	if (!response.ok || parsed.code !== 0) {
		throw new Error(`POST /api/v1/upload failed for ${fileName}: HTTP ${response.status} ${text}`);
	}
	const uploaded = parsed.data;
	assert(uploaded.url, `${fileName} upload missing url`);
	assert(uploaded.object_key, `${fileName} upload missing object_key`);
	assert(uploaded.storage_backend, `${fileName} upload missing storage_backend`);
	assert(uploaded.thumbnail_url, `${fileName} upload missing thumbnail_url`);
	if (expectedBackend) {
		assert(
			uploaded.storage_backend === expectedBackend,
			`${fileName} storage backend mismatch: expected ${expectedBackend}, got ${uploaded.storage_backend}`
		);
	}
	return uploaded;
}

async function login() {
	const result = await api('/api/v1/auth/login', {
		method: 'POST',
		body: { email, password }
	});
	accessToken = result.tokens?.access_token;
	assert(accessToken, 'login did not return an access token');
}

async function chooseWorkspace() {
	if (workspaceOverride) return workspaceOverride;
	const workspaces = await api('/api/v1/workspaces');
	const workspace = workspaces.items?.[0] ?? workspaces[0];
	assert(workspace?.id, 'no workspace available');
	return workspace.id;
}

async function chooseProject(selectedWorkspaceId) {
	if (projectOverride) return projectOverride;
	const projects = await api(`/api/v1/workspaces/${selectedWorkspaceId}/projects?per_page=100`);
	const items = projects.items ?? projects;
	const project = items.find((item) => item.type_key === 'custom_form') ?? items[0];
	assert(project?.id, 'no project available');
	return project.id;
}

async function createForm() {
	const form = await api(`/api/v1/projects/${projectId}/forms`, {
		method: 'POST',
		body: {
			key: `attachment_lifecycle_acceptance_${suffix}`,
			name: `Attachment Lifecycle Acceptance ${suffix}`,
			description: 'Temporary form for deployed attachment lifecycle acceptance.',
			title_template: '{name}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'name', label: 'Name', type: 'text', required: true },
					{
						key: 'receipt_image',
						label: 'Receipt Image',
						type: 'image',
						attachment: {
							storage_policy: 'private',
							accept: ['image/png', '.png'],
							max_size_mb: 1,
							thumbnail: true,
							thumbnail_max_dimension: 128,
							signed_url_ttl_minutes: 15
						}
					}
				]
			}
		}
	});
	createdFormId = form.id;
	return form;
}

async function cleanup() {
	if (!cleanupCreated || !createdFormId) return;
	try {
		await api(`/api/v1/forms/${createdFormId}`, { method: 'DELETE' });
	} catch (error) {
		console.error(`form cleanup failed: ${error.message}`);
	}
}

async function listAttachments(formId, recordId, includeArchived = false) {
	const query = new URLSearchParams({
		record_id: recordId,
		field_key: 'receipt_image',
		per_page: '20'
	});
	if (includeArchived) query.set('include_archived', 'true');
	return api(`/api/v1/forms/${formId}/attachments?${query.toString()}`);
}

function findAttachment(page, attachmentId) {
	return page.items?.find((item) => item.id === attachmentId);
}

async function signedDownload(attachmentId, label) {
	const signed = await api(`/api/v1/form-attachments/${attachmentId}/signed-url`);
	assert(signed.url?.includes(`/api/v1/form-attachments/${attachmentId}/download?`), `${label} signed URL mismatch: ${JSON.stringify(signed)}`);
	const downloaded = await fetchBinary(signed.url, `${label} signed attachment`);
	return { signed, downloaded };
}

async function main() {
	await login();
	workspaceId = await chooseWorkspace();
	projectId = await chooseProject(workspaceId);
	const form = await createForm();

	const imageUpload = await uploadFile(`receipt-${suffix}.png`, 'image/png', pngBytes);
	const record = await api(`/api/v1/forms/${form.id}/records`, {
		method: 'POST',
		body: {
			values: {
				name: `Attachment Lifecycle ${suffix}`,
				receipt_image: imageUpload.url
			},
			idempotency_key: `attachment-lifecycle-record-${suffix}`
		}
	});
	assert(record.id, `record create did not return id: ${JSON.stringify(record)}`);

	const attachment = await api(`/api/v1/forms/${form.id}/attachments`, {
		method: 'POST',
		body: {
			field_key: 'receipt_image',
			record_id: record.id,
			file_name: `receipt-${suffix}.png`,
			content_type: 'image/png',
			byte_size: pngBytes.length,
			storage_key: imageUpload.object_key,
			url: imageUpload.url,
			thumbnail_url: imageUpload.thumbnail_url
		}
	});
	assert(attachment.id, 'attachment metadata did not return id');
	assert(attachment.archived_at === null, `new attachment should not be archived: ${JSON.stringify(attachment)}`);
	assert(attachment.thumbnail_url, 'attachment thumbnail_url missing');

	const sourceRead = await fetchBinary(attachment.url, 'attachment source object');
	const thumbnailRead = await fetchBinary(attachment.thumbnail_url, 'attachment thumbnail object');
	const initialDownload = await signedDownload(attachment.id, 'initial');

	const activeBeforeArchive = await listAttachments(form.id, record.id);
	assert(findAttachment(activeBeforeArchive, attachment.id), `active attachment list missing attachment: ${JSON.stringify(activeBeforeArchive)}`);

	await api(`/api/v1/form-attachments/${attachment.id}`, { method: 'DELETE' });
	await expectApiFailure(`/api/v1/form-attachments/${attachment.id}/signed-url`, {}, 404, 'archived attachment signed-url');
	const activeAfterArchive = await listAttachments(form.id, record.id);
	assert(!findAttachment(activeAfterArchive, attachment.id), `archived attachment leaked into active list: ${JSON.stringify(activeAfterArchive)}`);
	const archivedPage = await listAttachments(form.id, record.id, true);
	const archivedAttachment = findAttachment(archivedPage, attachment.id);
	assert(archivedAttachment?.archived_at, `include_archived did not return archived attachment: ${JSON.stringify(archivedPage)}`);

	const restored = await api(`/api/v1/form-attachments/${attachment.id}/restore`, { method: 'POST' });
	assert(restored.archived_at === null, `restored attachment should clear archived_at: ${JSON.stringify(restored)}`);
	const activeAfterRestore = await listAttachments(form.id, record.id);
	assert(findAttachment(activeAfterRestore, attachment.id), `restored attachment missing from active list: ${JSON.stringify(activeAfterRestore)}`);
	const restoredDownload = await signedDownload(attachment.id, 'restored');

	console.log(JSON.stringify({
		ok: true,
		frontend_url: frontendUrl,
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: form.id,
		record_id: record.id,
		attachment_id: attachment.id,
		storage_backend: imageUpload.storage_backend,
		expected_backend: expectedBackend ?? null,
		source_object_key: imageUpload.object_key,
		thumbnail_url: attachment.thumbnail_url,
		archived_at: archivedAttachment.archived_at,
		restored_archived_at: restored.archived_at,
		bytes: {
			source: sourceRead.bytes.length,
			thumbnail: thumbnailRead.bytes.length,
			initial_signed_download: initialDownload.downloaded.bytes.length,
			restored_signed_download: restoredDownload.downloaded.bytes.length
		},
		cleanup: cleanupCreated
	}, null, 2));
}

main()
	.catch(async (error) => {
		console.error(error.stack ?? error.message);
		process.exitCode = 1;
	})
	.finally(cleanup);

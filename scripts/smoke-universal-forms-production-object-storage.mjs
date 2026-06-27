#!/usr/bin/env node

const frontendUrl = (process.env.OPENPR_FRONTEND_URL ?? 'http://10.72.0.3:3000').replace(/\/+$/, '');
const email = process.env.OPENPR_DEMO_EMAIL ?? 'demo@openpr.local';
const password = process.env.OPENPR_DEMO_PASSWORD ?? 'OpenPRDemo123!';
const workspaceOverride = process.env.OPENPR_WORKSPACE_ID;
const projectOverride = process.env.OPENPR_PROJECT_ID;
const expectedBackend = process.env.OPENPR_EXPECT_OBJECT_STORAGE_BACKEND;
const cleanupCreated = process.env.OPENPR_OBJECT_STORAGE_CLEANUP !== '0';
const timeoutMs = Number.parseInt(process.env.OPENPR_OBJECT_STORAGE_TIMEOUT_MS ?? '90000', 10);
const pollMs = Number.parseInt(process.env.OPENPR_OBJECT_STORAGE_POLL_MS ?? '1500', 10);
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
  scripts/smoke-universal-forms-production-object-storage.mjs

Optional:
  OPENPR_WORKSPACE_ID                  Workspace to use. Defaults to first visible workspace.
  OPENPR_PROJECT_ID                    Project to use. Defaults to first custom_form project, then first project.
  OPENPR_EXPECT_OBJECT_STORAGE_BACKEND Assert upload response storage_backend, for example s3 or local.
  OPENPR_OBJECT_STORAGE_CLEANUP=0      Keep the temporary form for inspection.
  OPENPR_OBJECT_STORAGE_TIMEOUT_MS     Package job poll timeout. Default: 90000.

This deployed smoke uploads an image and CSV through /api/v1/upload, reads back
the source and thumbnail objects, imports records from the uploaded CSV file,
creates image attachment metadata, forces server JPEG/WebP thumbnail/preview/variant
derivatives, creates an attachment package job, and downloads the ZIP artifact.`);
}

if (process.argv.includes('--help') || process.argv.includes('-h')) {
	usage();
	process.exit(0);
}

function sleep(ms) {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

function assert(condition, message) {
	if (!condition) throw new Error(message);
}

function assertUploadContract(uploaded, fileName) {
	const missing = [];
	if (!uploaded.url) missing.push('url');
	if (!uploaded.object_key) missing.push('object_key');
	if (!uploaded.storage_backend) missing.push('storage_backend');
	if (fileName.endsWith('.png') && !uploaded.thumbnail_url) missing.push('thumbnail_url');
	if (missing.length) {
		throw new Error(
			`${fileName} upload response is missing required object-storage fields: ${missing.join(', ')}. `
			+ `Response fields: ${Object.keys(uploaded).sort().join(', ') || '<none>'}. `
			+ 'Deploy an API build that exposes storage_backend, object_key, and image thumbnail_url from /api/v1/upload.'
		);
	}
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
	assertUploadContract(uploaded, fileName);
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
			key: `object_storage_acceptance_${suffix}`,
			name: `Object Storage Acceptance ${suffix}`,
			description: 'Temporary form for deployed object-storage acceptance.',
			title_template: '{name}',
			schema: {
				version: 'openpr.form.schema.v1',
				fields: [
					{ key: 'name', label: 'Name', type: 'text', required: true },
					{
						key: 'stored_image',
						label: 'Stored Image',
						type: 'image',
						attachment: {
							storage_policy: 'private',
							accept: ['image/png', '.png'],
							max_size_mb: 1,
							thumbnail: true,
							thumbnail_max_dimension: 128,
							thumbnail_format: 'jpeg',
							preview: true,
							preview_max_dimension: 256,
							preview_format: 'webp',
							variants: [
								{ kind: 'gallery', max_dimension: 192, format: 'jpeg' },
								{ kind: 'card', max_dimension: 96, format: 'webp' }
							]
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

async function waitForPackageJob(jobId) {
	const deadline = Date.now() + timeoutMs;
	let last = '';
	while (Date.now() < deadline) {
		const job = await api(`/api/v1/form-attachment-package-jobs/${jobId}`);
		if (job.status === 'completed') return job;
		if (job.status === 'failed') throw new Error(`attachment package job failed: ${job.error ?? JSON.stringify(job)}`);
		last = `status=${job.status}`;
		await sleep(pollMs);
	}
	throw new Error(`Timed out waiting for attachment package job ${jobId}: ${last}`);
}

async function main() {
	await login();
	workspaceId = await chooseWorkspace();
	projectId = await chooseProject(workspaceId);
	const form = await createForm();

	const imageUpload = await uploadFile(`stored-image-${suffix}.png`, 'image/png', pngBytes);
	const source = await fetchBinary(imageUpload.url, 'uploaded source image');
	const uploadThumbnail = await fetchBinary(imageUpload.thumbnail_url, 'uploaded thumbnail image');

	const csv = Buffer.from(`name\nImported Object Storage ${suffix}\n`, 'utf8');
	const csvUpload = await uploadFile(`object-storage-import-${suffix}.csv`, 'text/csv', csv);
	const preview = await api(`/api/v1/forms/${form.id}/records/import-file-preview`, {
		method: 'POST',
		body: { file_url: csvUpload.url }
	});
	assert(preview.valid_rows === 1 && preview.invalid_rows === 0, `unexpected import preview result: ${JSON.stringify(preview)}`);
	const imported = await api(`/api/v1/forms/${form.id}/records/import-file`, {
		method: 'POST',
		body: {
			file_url: csvUpload.url,
			idempotency_key: `object-storage-import-${suffix}`
		}
	});
	assert(imported.created_count === 1, `import-file did not create one record: ${JSON.stringify(imported)}`);
	const importedRecord = imported.records?.[0];
	assert(importedRecord?.id, 'import-file did not return created record');

	const attachment = await api(`/api/v1/forms/${form.id}/attachments`, {
		method: 'POST',
		body: {
			field_key: 'stored_image',
			record_id: importedRecord.id,
			file_name: `stored-image-${suffix}.png`,
			content_type: 'image/png',
			byte_size: pngBytes.length,
			storage_key: imageUpload.object_key,
			url: imageUpload.url
		}
	});
	assert(attachment.id, 'attachment metadata did not return id');
	assert(attachment.thumbnail_url, 'attachment media did not generate thumbnail_url');
	assert(attachment.media_metadata?.thumbnail?.object_key, 'attachment thumbnail object_key missing');
	assert(attachment.media_metadata?.preview?.object_key, 'attachment preview object_key missing');
	assert(attachment.media_metadata.thumbnail.format === 'jpeg', `attachment thumbnail format mismatch: ${JSON.stringify(attachment.media_metadata.thumbnail)}`);
	assert(attachment.media_metadata.preview.format === 'webp', `attachment preview format mismatch: ${JSON.stringify(attachment.media_metadata.preview)}`);
	assert(
		Array.isArray(attachment.media_metadata?.variants) && attachment.media_metadata.variants.some((item) => item.kind === 'gallery'),
		'attachment gallery variant metadata missing'
	);
	assert(
		Array.isArray(attachment.media_metadata?.variants) && attachment.media_metadata.variants.some((item) => item.kind === 'card'),
		'attachment card variant metadata missing'
	);
	const attachmentThumbnail = await fetchBinary(attachment.thumbnail_url, 'attachment thumbnail derivative');
	assert(
		attachmentThumbnail.response.headers.get('content-type')?.includes('image/jpeg'),
		`attachment thumbnail content-type is not image/jpeg: ${attachmentThumbnail.response.headers.get('content-type')}`
	);
	const previewUrl = attachment.media_metadata.preview.url;
	const attachmentPreview = await fetchBinary(previewUrl, 'attachment preview derivative');
	assert(
		attachmentPreview.response.headers.get('content-type')?.includes('image/webp'),
		`attachment preview content-type is not image/webp: ${attachmentPreview.response.headers.get('content-type')}`
	);
	const galleryVariant = attachment.media_metadata.variants.find((item) => item.kind === 'gallery');
	const galleryVariantRead = await fetchBinary(galleryVariant.policy.url, 'attachment gallery variant derivative');
	assert(galleryVariant.policy.format === 'jpeg', `gallery variant format mismatch: ${JSON.stringify(galleryVariant)}`);
	assert(
		galleryVariantRead.response.headers.get('content-type')?.includes('image/jpeg'),
		`gallery variant content-type is not image/jpeg: ${galleryVariantRead.response.headers.get('content-type')}`
	);
	const cardVariant = attachment.media_metadata.variants.find((item) => item.kind === 'card');
	assert(cardVariant?.policy?.format === 'webp', `card variant format mismatch: ${JSON.stringify(cardVariant)}`);
	const cardVariantRead = await fetchBinary(cardVariant.policy.url, 'attachment card variant derivative');
	assert(
		cardVariantRead.response.headers.get('content-type')?.includes('image/webp'),
		`card variant content-type is not image/webp: ${cardVariantRead.response.headers.get('content-type')}`
	);

	const packageJob = await api(`/api/v1/forms/${form.id}/attachments/package-jobs`, {
		method: 'POST',
		body: {}
	});
	assert(packageJob.id, 'attachment package job did not return id');
	const completedJob = await waitForPackageJob(packageJob.id);
	const result = completedJob.result ?? {};
	assert(result.artifact_storage_key, `package job result missing artifact_storage_key: ${JSON.stringify(result)}`);
	assert(result.artifact_storage?.key, `package job result missing artifact_storage.key: ${JSON.stringify(result)}`);
	assert(result.download_url, `package job result missing download_url: ${JSON.stringify(result)}`);
	assert(result.attachment_count >= 1, `package job attachment_count invalid: ${JSON.stringify(result)}`);
	assert(result.binary_file_count >= 1, `package job binary_file_count invalid: ${JSON.stringify(result)}`);
	assert(result.byte_size > 0, `package job byte_size invalid: ${JSON.stringify(result)}`);
	const zip = await fetchBinary(result.download_url, 'attachment package ZIP');
	assert(
		zip.response.headers.get('content-type')?.includes('application/zip'),
		`package download content-type is not application/zip: ${zip.response.headers.get('content-type')}`
	);

	console.log(JSON.stringify({
		ok: true,
		frontend_url: frontendUrl,
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: form.id,
		storage_backend: imageUpload.storage_backend,
		expected_backend: expectedBackend ?? null,
		source_object_key: imageUpload.object_key,
		upload_thumbnail_url: imageUpload.thumbnail_url,
		csv_object_key: csvUpload.object_key,
		import_record_id: importedRecord.id,
		attachment_id: attachment.id,
		attachment_thumbnail_url: attachment.thumbnail_url,
		attachment_preview_url: previewUrl,
		attachment_gallery_variant_url: galleryVariant.policy.url,
		attachment_card_variant_url: cardVariant.policy.url,
		package_job_id: completedJob.id,
		package_artifact_key: result.artifact_storage_key,
		bytes: {
			source: source.bytes.length,
			upload_thumbnail: uploadThumbnail.bytes.length,
			attachment_thumbnail: attachmentThumbnail.bytes.length,
			attachment_preview: attachmentPreview.bytes.length,
			attachment_gallery_variant: galleryVariantRead.bytes.length,
			attachment_card_variant: cardVariantRead.bytes.length,
			package_zip: zip.bytes.length
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

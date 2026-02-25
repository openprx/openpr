export const MAX_UPLOAD_SIZE_BYTES = 200 * 1024 * 1024;

export const IMAGE_UPLOAD_MIME_TYPES = [
	'image/png',
	'image/jpeg',
	'image/jpg',
	'image/gif',
	'image/webp'
] as const;

export const VIDEO_UPLOAD_MIME_TYPES = [
	'video/mp4',
	'video/webm',
	'video/quicktime',
	'video/x-msvideo'
] as const;

export const ALLOWED_UPLOAD_MIME_TYPES = [
	...IMAGE_UPLOAD_MIME_TYPES,
	...VIDEO_UPLOAD_MIME_TYPES
] as const;

export const UPLOAD_ACCEPT_ATTR = ALLOWED_UPLOAD_MIME_TYPES.join(',');

export function isAllowedUploadMime(type: string): boolean {
	return ALLOWED_UPLOAD_MIME_TYPES.includes(type.toLowerCase() as (typeof ALLOWED_UPLOAD_MIME_TYPES)[number]);
}

export function isImageUploadMime(type: string): boolean {
	return IMAGE_UPLOAD_MIME_TYPES.includes(type.toLowerCase() as (typeof IMAGE_UPLOAD_MIME_TYPES)[number]);
}

export function mediaMarkdown(url: string, mimeType: string): string {
	if (isImageUploadMime(mimeType)) {
		return `![image](${url})`;
	}
	return `<video controls src="${url}"></video>`;
}

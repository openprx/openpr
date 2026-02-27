export interface UploadedAttachment {
	url: string;
	filename: string;
	size: number;
	mimeType: string;
}

export interface MarkdownAttachmentLink {
	url: string;
	filename: string;
	isImage: boolean;
}

export const ISSUE_ATTACHMENT_MAX_SIZE_BYTES = 50 * 1024 * 1024;

export const ISSUE_ATTACHMENT_ACCEPT =
	'.png,.jpg,.jpeg,.gif,.webp,.mp4,.webm,.mov,.avi,.zip,.gz,.tar.gz,.log,.txt,.pdf,.json,.csv,.xml';

const ATTACHMENT_EXTENSIONS = ISSUE_ATTACHMENT_ACCEPT.split(',').map((item) => item.trim().toLowerCase());

function fileNameFromUrl(url: string): string {
	const noQuery = url.split(/[?#]/, 1)[0] ?? '';
	const encodedName = noQuery.slice(noQuery.lastIndexOf('/') + 1).trim();
	if (!encodedName) {
		return 'file';
	}

	try {
		return decodeURIComponent(encodedName);
	} catch {
		return encodedName;
	}
}

function isImageByName(filename: string): boolean {
	const lower = filename.toLowerCase();
	return (
		lower.endsWith('.png') ||
		lower.endsWith('.jpg') ||
		lower.endsWith('.jpeg') ||
		lower.endsWith('.gif') ||
		lower.endsWith('.webp')
	);
}

function isAttachmentUrl(url: string): boolean {
	const lower = (url.split(/[?#]/, 1)[0] ?? '').toLowerCase();
	return ATTACHMENT_EXTENSIONS.some((ext) => lower.endsWith(ext));
}

function isImageAttachment(attachment: Pick<UploadedAttachment, 'filename' | 'mimeType' | 'url'>): boolean {
	if (attachment.mimeType?.toLowerCase().startsWith('image/')) {
		return true;
	}
	if (isImageByName(attachment.filename)) {
		return true;
	}
	return isImageByName(fileNameFromUrl(attachment.url));
}

export function attachmentToMarkdown(attachment: UploadedAttachment): string {
	if (isImageAttachment(attachment)) {
		return `![${attachment.filename}](${attachment.url})`;
	}
	return `[📎 ${attachment.filename}](${attachment.url})`;
}

export function appendAttachmentsMarkdown(content: string, attachments: UploadedAttachment[]): string {
	if (attachments.length === 0) {
		return content;
	}

	const trimmedContent = content.trimEnd();
	const attachmentLines = attachments.map((item) => attachmentToMarkdown(item)).join('\n');
	const separator = trimmedContent.length > 0 ? '\n\n' : '';
	return `${trimmedContent}${separator}${attachmentLines}`;
}

export function extractMarkdownAttachments(content: string): MarkdownAttachmentLink[] {
	const result: MarkdownAttachmentLink[] = [];
	const seen = new Set<string>();

	const imageRegex = /!\[([^\]]*)\]\(([^)\s]+)\)/g;
	for (const match of content.matchAll(imageRegex)) {
		const label = (match[1] ?? '').trim();
		const url = (match[2] ?? '').trim();
		if (!url || !isAttachmentUrl(url) || seen.has(url)) {
			continue;
		}
		seen.add(url);
		result.push({
			url,
			filename: label || fileNameFromUrl(url),
			isImage: true
		});
	}

	const fileRegex = /\[([^\]]+)\]\(([^)\s]+)\)/g;
	for (const match of content.matchAll(fileRegex)) {
		const index = match.index ?? -1;
		if (index > 0 && content[index - 1] === '!') {
			continue;
		}

		const label = (match[1] ?? '').trim();
		const url = (match[2] ?? '').trim();
		if (!url || !isAttachmentUrl(url) || seen.has(url)) {
			continue;
		}
		seen.add(url);

		const normalizedLabel = label.replace(/^📎\s*/, '').trim();
		result.push({
			url,
			filename: normalizedLabel || fileNameFromUrl(url),
			isImage: isImageByName(normalizedLabel || fileNameFromUrl(url))
		});
	}

	return result;
}

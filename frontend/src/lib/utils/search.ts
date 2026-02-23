export function stripMarkdown(input: string): string {
	return input
		.replace(/!\[[^\]]*]\([^)]*\)/g, ' ')
		.replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
		.replace(/`{1,3}[^`]*`{1,3}/g, ' ')
		.replace(/[#>*_~\-]+/g, ' ')
		.replace(/\r?\n+/g, ' ')
		.replace(/\s+/g, ' ')
		.trim();
}

export function previewText(input: string | null | undefined, maxLength = 100): string {
	const plain = stripMarkdown(input ?? '');
	if (plain.length <= maxLength) {
		return plain;
	}

	return `${plain.slice(0, maxLength)}...`;
}

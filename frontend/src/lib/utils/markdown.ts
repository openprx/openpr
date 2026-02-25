function escapeHtml(input: string): string {
	return input
		.replaceAll('&', '&amp;')
		.replaceAll('<', '&lt;')
		.replaceAll('>', '&gt;')
		.replaceAll('"', '&quot;')
		.replaceAll("'", '&#39;');
}

function renderInline(input: string): string {
	let text = escapeHtml(input);
	text = text.replace(
		/!\[([^\]]*)\]\(((?:https?:\/\/|\/)[^\s)]+)\)/g,
		'<img src="$2" alt="$1" />'
	);
	text = text.replace(/`([^`]+)`/g, '<code>$1</code>');
	text = text.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
	text = text.replace(/\*([^*]+)\*/g, '<em>$1</em>');
	text = text.replace(
		/\[([^\]]+)\]\(((?:https?:\/\/|\/)[^\s)]+)\)/g,
		'<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>'
	);
	return text;
}

export function renderMarkdown(content: string): string {
	const source = content.trim();
	if (!source) {
		return '';
	}

	// Preserve <video> tags before processing
	const videoPlaceholders: string[] = [];
	let processed = source.replace(/<video\b[^>]*>[\s\S]*?<\/video>/gi, (match) => {
		const idx = videoPlaceholders.length;
		videoPlaceholders.push(match);
		return `%%VIDEO_PLACEHOLDER_${idx}%%`;
	});

	const lines = processed.split(/\r?\n/);
	const output: string[] = [];
	let inCodeBlock = false;
	let listItems: string[] = [];

	function flushList() {
		if (listItems.length === 0) {
			return;
		}
		output.push(`<ul>${listItems.join('')}</ul>`);
		listItems = [];
	}

	for (const rawLine of lines) {
		const line = rawLine.trimEnd();

		if (line.startsWith('```')) {
			flushList();
			if (inCodeBlock) {
				output.push('</code></pre>');
				inCodeBlock = false;
			} else {
				output.push('<pre><code>');
				inCodeBlock = true;
			}
			continue;
		}

		if (inCodeBlock) {
			output.push(`${escapeHtml(rawLine)}\n`);
			continue;
		}

		if (!line.trim()) {
			flushList();
			continue;
		}

		const heading = line.match(/^(#{1,3})\s+(.+)$/);
		if (heading) {
			flushList();
			const level = heading[1].length;
			output.push(`<h${level}>${renderInline(heading[2])}</h${level}>`);
			continue;
		}

		const list = line.match(/^[-*]\s+(.+)$/);
		if (list) {
			listItems.push(`<li>${renderInline(list[1])}</li>`);
			continue;
		}

		flushList();
		output.push(`<p>${renderInline(line)}</p>`);
	}

	flushList();

	if (inCodeBlock) {
		output.push('</code></pre>');
	}

	let result = output.join('');

	// Restore <video> tags
	for (let i = 0; i < videoPlaceholders.length; i++) {
		result = result.replace(`%%VIDEO_PLACEHOLDER_${i}%%`, videoPlaceholders[i]);
	}

	return result;
}

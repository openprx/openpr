// Moves SvelteKit's inline start script out of index.html into its own file.
//
// The built index.html boots the whole app from an inline <script>. A
// `script-src 'self'` Content-Security-Policy forbids inline scripts, so under
// that policy the app never hydrates and every route renders a blank page --
// the SPA fallback means index.html serves every URL, so nothing works at all.
//
// Allow-listing the script's hash instead would break on every build, because
// the inline source embeds the content-hashed entry module names. Giving the
// script its own file keeps `script-src 'self'` correct forever.
//
// Runs after `vite build`; see the frontend `build` script.

import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const buildDir = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'build');
const indexPath = join(buildDir, 'index.html');

const html = readFileSync(indexPath, 'utf8');

// Only scripts without a `src` carry inline code. Anything already external is
// left alone.
const inlineScripts = [...html.matchAll(/<script(?![^>]*\ssrc=)([^>]*)>([\s\S]*?)<\/script>/g)];

if (inlineScripts.length !== 1) {
	throw new Error(
		`expected exactly 1 inline <script> in ${indexPath}, found ${inlineScripts.length}. ` +
			'SvelteKit changed its bootstrap shape -- update this script rather than ' +
			'relaxing the Content-Security-Policy.'
	);
}

const [match, attributes, code] = inlineScripts[0];

if (attributes.trim() !== '') {
	throw new Error(
		`inline <script> carries attributes (${attributes.trim()}); externalising would drop them`
	);
}

// The bootstrap imports the entry modules by absolute path. Reusing that same
// directory keeps the new file under whatever `paths.base` the build used and
// inherits the immutable-asset caching rules that already cover it.
const entryImport = code.match(/import\("([^"]*\/_app\/immutable\/entry\/)[^"]+"\)/);

if (!entryImport) {
	throw new Error('could not locate the entry module directory in the bootstrap script');
}

const entryDir = entryImport[1];
const digest = createHash('sha256').update(code).digest('hex').slice(0, 8);
const bootstrapUrl = `${entryDir}bootstrap.${digest}.js`;

// `entryDir` is a URL path; map it back onto the build directory to write the file.
const bootstrapPath = join(buildDir, bootstrapUrl.replace(/^.*?\/_app\//, '_app/'));

writeFileSync(bootstrapPath, code, 'utf8');
writeFileSync(indexPath, html.replace(match, `<script src="${bootstrapUrl}"></script>`), 'utf8');

console.log(`externalised SvelteKit bootstrap -> ${bootstrapUrl}`);

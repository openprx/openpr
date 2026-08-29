// Minimal static file server that ACTUALLY sends `Content-Encoding: gzip` -- `vite preview`
// (used for every other measurement in this delivery) does not compress responses at all (see
// the `curl -I` finding in the report), which makes its Fast-4G cold-start numbers a pessimistic
// worst case rather than a production-representative one. This server exists solely to close that
// gap: precompresses every file under `--dir` once at startup with real `zlib.gzipSync`, and on
// each request, if the client's `Accept-Encoding` includes `gzip`, serves the precompressed bytes
// with a real `Content-Encoding: gzip` header (verified with `curl -I` before any measurement is
// trusted -- see the report's "response header verified" step). SPA-falls back to `index.html`
// for any path with no matching file, matching `vite preview`'s default `appType: 'spa'` behavior
// (see `bundle-loro/vite.config.ts` and the report's adapter-static discussion) so `/flow/*`
// continues to resolve.
//
// R6 additions (coordinator directive 2026-08-29, throughput-calibration predregistration fix):
//  1. Always additionally serves `probe/calibration-256k.bin` (a fixed, coordinator-generated,
//     deterministic-content file -- see lib/common.mjs's CALIBRATION_ASSET_* constants for its
//     frozen SHA-256/size) at the fixed path `/calibration-256k.bin`, on BOTH candidates' origins
//     regardless of which `--dir` this instance was started with, so both candidates calibrate
//     against the byte-identical neutral resource (the previous "largest file in each candidate's
//     own dist/" selection was asymmetric across candidates -- ~1MB for loro vs ~57KB for
//     yrs-yjs -- and got rejected for that reason).
//  2. Gzip is no longer applied unconditionally: a file is only served gzip-encoded if compression
//     actually shrinks it. `calibration-256k.bin`'s content is a SHA-256-derived byte stream
//     (coordinator: "sha256('sylvode-coldstart-calib-v1' || le_u32(i)) 拼接流"), which is
//     high-entropy and INCOMPRESSIBLE (gzip-9 on it produces 262242 bytes, larger than the 262144
//     raw), so it is always served with an explicit `Content-Encoding: identity` header -- the
//     literal exception the R6 compression clause names ("这是压缩条款下唯一的合法例外"), not an
//     omitted header a reader would have to infer.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, extname, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const HERE = dirname(fileURLToPath(import.meta.url));

const dir = process.argv[2];
const port = Number(process.argv[3]);
if (dir === undefined || Number.isNaN(port)) {
  console.error("usage: node gzip-static-server.mjs <dist-dir> <port>");
  process.exit(2);
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".ico": "image/x-icon",
  ".bin": "application/octet-stream",
};

function walk(base, rel = "") {
  const out = [];
  for (const entry of readdirSync(join(base, rel))) {
    const relPath = rel === "" ? entry : `${rel}/${entry}`;
    const full = join(base, relPath);
    if (statSync(full).isDirectory()) {
      out.push(...walk(base, relPath));
    } else {
      out.push(relPath);
    }
  }
  return out;
}

// { raw: Buffer, gz: Buffer | null, contentType: string } -- `gz === null` means "compression did
// not shrink this file, always serve raw with an explicit identity encoding" (see header comment).
function loadEntry(fullPath, relPath) {
  const raw = readFileSync(fullPath);
  const gzAttempt = gzipSync(raw, { level: 9 });
  const gz = gzAttempt.length < raw.length ? gzAttempt : null;
  const contentType = MIME[extname(relPath)] ?? "application/octet-stream";
  return { raw, gz, contentType };
}

const files = new Map(); // urlPath -> { raw, gz, contentType }
for (const relPath of walk(dir)) {
  files.set(`/${relPath}`, loadEntry(join(dir, relPath), relPath));
}
const indexEntry = files.get("/index.html");
if (indexEntry === undefined) {
  throw new Error(`gzip-static-server: no index.html found under ${dir}`);
}

// R6 item 1: always merge in the fixed calibration asset, independent of `dir`.
const CALIBRATION_ASSET_PATH = join(HERE, "probe", "calibration-256k.bin");
files.set("/calibration-256k.bin", loadEntry(CALIBRATION_ASSET_PATH, "calibration-256k.bin"));

// BUG FOUND & FIXED (formal-run integrity issue, this implementation round): this readiness log
// used to be printed HERE, before Bun.serve() below actually attempts to bind the port. The
// runner's startServer() (measure-flow-route-cold-load.mjs) watches stdout for the substring
// "serving" to decide the server is ready. Because that used to be printed unconditionally before
// the bind attempt, an EADDRINUSE crash immediately after (Bun.serve() throwing because a STALE
// process from an earlier run was still holding the port) went UNDETECTED: the runner had already
// marked the server "ready" from the premature log line, so the crash+exit that followed did not
// trip its `if (!ready) reject(...)` safety net -- the run silently continued, and requests to
// that port were actually served by whatever stale process still held it, not this instance.
// Confirmed via `ps`/mtime cross-check during a formal run: the stale process happened to be
// running up-to-date, correct code in that specific incident, but that was luck, not a property
// this design guaranteed. Fix: the "serving"-bearing readiness line now prints ONLY after
// Bun.serve() has returned successfully (i.e. the port really is bound), so a bind failure can
// never be mistaken for a successful start.
console.log(`gzip-static-server: loaded ${files.size} files from ${dir} (+ probe/calibration-256k.bin), binding :${port}...`);
for (const [path, entry] of files) {
  console.log(`  ${path}  raw=${entry.raw.length}  gzip=${entry.gz === null ? "n/a (incompressible, served identity)" : entry.gz.length}`);
}

const server = Bun.serve({
  port,
  fetch(req) {
    const url = new URL(req.url);
    const acceptsGzip = (req.headers.get("accept-encoding") ?? "").includes("gzip");
    const entry = files.get(url.pathname) ?? indexEntry; // SPA fallback, matches vite preview's appType:'spa'
    const useGzip = acceptsGzip && entry.gz !== null;
    const body = useGzip ? entry.gz : entry.raw;
    const headers = {
      "Content-Type": entry.contentType,
      "Content-Length": String(body.length),
      "Cache-Control": "no-cache",
    };
    if (useGzip) {
      headers["Content-Encoding"] = "gzip";
      headers.Vary = "Accept-Encoding";
    } else if (entry.gz === null) {
      // Explicit, not omitted -- see header comment's R6 item 2.
      headers["Content-Encoding"] = "identity";
    }
    return new Response(body, { headers });
  },
});
// Readiness signal, printed ONLY after a real, successful bind (see the fix comment above) --
// `server.port` also cross-checks Bun actually bound the port we asked for, not some other one.
console.log(`gzip-static-server: serving on :${server.port} (bound OK)`);

// Minimal CDP-event-driven HAR 1.2 recorder + the ADR-0015 3.4 gzip assertion
// ("Content-Encoding 逐响应断言;计入资源集合 = 该路由加载的全部 HTML/JS/WASM;
// 允许的无编码例外仅限已压缩格式（如 .woff2）,须逐条列出"). No external HAR library --
// puppeteer-core's CDPSession already delivers everything a HAR entry needs.
const COMPRESSED_EXTENSION_EXCEPTIONS = [".woff2"]; // only .woff2 may skip Content-Encoding: gzip
const ASSERTED_EXTENSIONS = [".html", ".js", ".wasm", ""]; // "" covers extension-less HTML routes

function extname(pathname) {
  const i = pathname.lastIndexOf(".");
  return i === -1 ? "" : pathname.slice(i);
}

export class HarRecorder {
  constructor(pageStartWallTimeIso) {
    this.pageStartWallTimeIso = pageStartWallTimeIso;
    this.entriesByRequestId = new Map();
    this.encodingAssertions = []; // { url, contentEncoding, asserted, ok }
  }

  attach(cdpSession) {
    cdpSession.on("Network.requestWillBeSent", (e) => {
      this.entriesByRequestId.set(e.requestId, {
        requestId: e.requestId,
        url: e.request.url,
        method: e.request.method,
        requestHeaders: e.request.headers,
        wallTime: e.wallTime,
        timestamp: e.timestamp,
      });
    });
    cdpSession.on("Network.responseReceived", (e) => {
      const entry = this.entriesByRequestId.get(e.requestId);
      if (entry === undefined) return;
      // R6 throughput-calibration fix (coordinator directive 2026-08-29): the event's own
      // top-level `timestamp` (CDP monotonic clock, same domain as requestWillBeSent/
      // loadingFinished) is "firstByte_ts" -- the point response data became available, which
      // R6's frozen formula uses as the calibration window's start instead of subtracting an
      // assumed latency constant from the navigation start. See measure-flow-route-cold-load.mjs.
      entry.responseReceivedTimestamp = e.timestamp;
      entry.status = e.response.status;
      entry.statusText = e.response.statusText;
      entry.responseHeaders = e.response.headers;
      entry.mimeType = e.response.mimeType;
      entry.encodedDataLength = e.response.encodedDataLength ?? 0;
      entry.timing = e.response.timing ?? null;
      entry.protocol = e.response.protocol ?? null;

      const u = (() => {
        try {
          return new URL(e.response.url);
        } catch {
          return null;
        }
      })();
      const ext = u ? extname(u.pathname) : "";
      if (ASSERTED_EXTENSIONS.includes(ext) && u !== null) {
        const headerEntry = Object.entries(e.response.headers).find(([k]) => k.toLowerCase() === "content-encoding");
        const contentEncoding = headerEntry ? headerEntry[1] : null;
        const exempt = COMPRESSED_EXTENSION_EXCEPTIONS.includes(ext);
        const ok = exempt || contentEncoding === "gzip";
        this.encodingAssertions.push({ url: e.response.url, extension: ext || "(none)", contentEncoding, exempt, ok });
      }
    });
    cdpSession.on("Network.loadingFinished", (e) => {
      const entry = this.entriesByRequestId.get(e.requestId);
      if (entry !== undefined) {
        entry.finishedTimestamp = e.timestamp;
        // `Network.loadingFinished.encodedDataLength` is "Total number of bytes received for this
        // request" (CDP protocol doc) -- the authoritative final on-the-wire byte count.
        // `Network.responseReceived.response.encodedDataLength` (set above, in the
        // responseReceived handler) can be an early/partial value captured before the body has
        // finished streaming in (confirmed empirically here: for a ~1MB gzip transfer it was
        // observed at 185 bytes, i.e. header-only, at responseReceived time) -- overwrite it with
        // the loadingFinished value, which is what any byte-count-dependent computation (e.g.
        // throughput calibration, see measure-flow-route-cold-load.mjs) must use.
        entry.encodedDataLength = e.encodedDataLength;
      }
    });
    cdpSession.on("Network.loadingFailed", (e) => {
      const entry = this.entriesByRequestId.get(e.requestId);
      if (entry !== undefined) entry.failed = { errorText: e.errorText, canceled: e.canceled ?? false };
    });
  }

  toHar() {
    const entries = [...this.entriesByRequestId.values()].map((e) => ({
      startedDateTime: this.pageStartWallTimeIso,
      time: e.timing ? Math.max(0, (e.finishedTimestamp ?? e.timestamp) - e.timestamp) * 1000 : 0,
      request: {
        method: e.method,
        url: e.url,
        httpVersion: e.protocol ?? "unknown",
        headers: Object.entries(e.requestHeaders ?? {}).map(([name, value]) => ({ name, value: String(value) })),
        queryString: [],
        headersSize: -1,
        bodySize: -1,
      },
      response: {
        status: e.status ?? 0,
        statusText: e.statusText ?? "",
        httpVersion: e.protocol ?? "unknown",
        headers: Object.entries(e.responseHeaders ?? {}).map(([name, value]) => ({ name, value: String(value) })),
        content: {
          size: e.encodedDataLength ?? 0,
          mimeType: e.mimeType ?? "application/octet-stream",
        },
        redirectURL: "",
        headersSize: -1,
        bodySize: e.encodedDataLength ?? 0,
      },
      cache: {},
      timings: { send: 0, wait: 0, receive: 0 },
      _failed: e.failed ?? null,
    }));
    return {
      log: {
        version: "1.2",
        creator: { name: "sylvode-flow-coldstart-measure", version: "sylvode.flow.coldstart.v1" },
        entries,
      },
    };
  }
}

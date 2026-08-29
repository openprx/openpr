// Source of the script injected via `page.evaluateOnNewDocument` (both runners) BEFORE any
// candidate/app script executes. Implements, in one place shared by both metrics:
//
//  (a) WebAssembly instrumentation -- ADR-0015 3.3 "必须保存 compile / instantiate 分段耗时".
//      Patches BOTH the sync constructors (`new WebAssembly.Module(bytes)` /
//      `new WebAssembly.Instance(module, imports)` -- confirmed via
//      `grep -o 'WebAssembly\..*' bundle-loro/dist/assets/flow-entry-*.js` to be what
//      loro-crdt's browser build actually calls, NOT the async compile()/instantiate()
//      functions) and the async/streaming forms, in case a future build changes that.
//  (b) The ADR-0015 3.2 ready postconditions, as callable helpers `window.__coldstart.*` that
//      both runners drive identically (only the timer start differs between the two metrics --
//      see measure-engine-runtime-ready.mjs vs measure-flow-route-cold-load.mjs).
//
// `window.__coldstart` is intentionally NOT read by candidate code (candidates never self-report
// ready -- ADR-0015 3.2: "mark 一律由共享 harness 发出，候选 adapter 不得自报 ready"); it exists
// purely for the Node-side runner (via page.evaluate) to drive and interrogate.
// `wasmSyntheticAsset`, when provided ({ url, base64 }), makes the ADR-0015 3.3
// "候选专属资源的原始 bytes 计时前已在本地内存中可用，计时不含任何网络往返" requirement literal
// for the one resource where it matters most: loro-crdt's browser build loads its ~3.1MB .wasm
// via a SYNCHRONOUS XMLHttpRequest at module-eval time (confirmed empirically -- see
// measure-engine-runtime-ready.mjs's header comment). Routing that through Puppeteer request
// interception (a real Fetch-domain round trip, CDP-serialized) was measured to cost ~360-420ms
// BY ITSELF for this one file -- almost entirely IPC/serialization overhead of shipping ~3MB
// through the interception channel, not a real network transfer (loopback) and not WASM
// compile/instantiate (those measured at ~5ms combined, see the wasm_module_compile_sync /
// wasm_instance_instantiate_sync segments). That overhead is a measurement-tool artifact, not
// part of "the raw bytes are already in local memory" -- a real production runtime holding those
// bytes as an in-heap ArrayBuffer pays no such IPC tax. So for this specific synchronous-XHR
// pattern, the bytes are decoded from base64 into an in-page Uint8Array ONCE, before the timed
// window even opens (during the untimed harness-page setup), and XMLHttpRequest is patched to
// serve that resource synthetically -- no interception, no Fetch domain, no CDP round trip at
// all for this request. JS module chunks are NOT given this treatment (kept on the normal
// request-interception path): they're two orders of magnitude smaller (~90-185KB) and their
// resolution as real ES module graphs (relative imports between chunks) is exactly the kind of
// intra-candidate difference this measurement should be sensitive to; the report documents this
// as a residual ~30-40ms/sample interception-tool overhead common to both candidates.
export function buildHarnessSource({ wasmSyntheticAsset }) {
  const wasmSetupSrc =
    wasmSyntheticAsset === undefined || wasmSyntheticAsset === null
      ? "const __coldstartWasmUrl = null; const __coldstartWasmText = null;"
      : `
  const __coldstartWasmUrl = ${JSON.stringify(wasmSyntheticAsset.url)};
  const __coldstartWasmBytes = (() => {
    const bin = atob(${JSON.stringify(wasmSyntheticAsset.base64)});
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes;
  })();
  // Reproduce exactly what a real "text/plain; charset=x-user-defined" XHR responseText would
  // contain (one UTF-16 code unit per byte, value 0-255) -- see loro-crdt's own \`xn()\` loader
  // (grep 'x-user-defined' in bundle-loro/dist/assets/flow-entry-*.js) which decodes responseText
  // this exact way on the other end.
  const __coldstartWasmText = (() => {
    const CHUNK = 8192;
    let out = "";
    for (let i = 0; i < __coldstartWasmBytes.length; i += CHUNK) {
      out += String.fromCharCode.apply(null, __coldstartWasmBytes.subarray(i, i + CHUNK));
    }
    return out;
  })();
`;
  return `
(() => {
  const trace = [];
  function record(op, startMs, endMs) {
    trace.push({ op, startMs, endMs, durationMs: endMs - startMs });
  }
  ${wasmSetupSrc}

  // --- WASM instrumentation -------------------------------------------------------------
  const OrigModule = WebAssembly.Module;
  function PatchedModule(...args) {
    const start = performance.now();
    const mod = new OrigModule(...args);
    record("wasm_module_compile_sync", start, performance.now());
    return mod;
  }
  PatchedModule.prototype = OrigModule.prototype;
  Object.setPrototypeOf(PatchedModule, OrigModule);
  WebAssembly.Module = PatchedModule;

  const OrigInstance = WebAssembly.Instance;
  function PatchedInstance(...args) {
    const start = performance.now();
    const inst = new OrigInstance(...args);
    record("wasm_instance_instantiate_sync", start, performance.now());
    return inst;
  }
  PatchedInstance.prototype = OrigInstance.prototype;
  Object.setPrototypeOf(PatchedInstance, OrigInstance);
  WebAssembly.Instance = PatchedInstance;

  const OrigCompile = WebAssembly.compile.bind(WebAssembly);
  WebAssembly.compile = async function (...args) {
    const start = performance.now();
    const r = await OrigCompile(...args);
    record("wasm_compile_async", start, performance.now());
    return r;
  };
  const OrigInstantiate = WebAssembly.instantiate.bind(WebAssembly);
  WebAssembly.instantiate = async function (...args) {
    const start = performance.now();
    const r = await OrigInstantiate(...args);
    record("wasm_instantiate_async", start, performance.now());
    return r;
  };
  if (typeof WebAssembly.compileStreaming === "function") {
    const OrigCompileStreaming = WebAssembly.compileStreaming.bind(WebAssembly);
    WebAssembly.compileStreaming = async function (...args) {
      const start = performance.now();
      const r = await OrigCompileStreaming(...args);
      record("wasm_compileStreaming_async", start, performance.now());
      return r;
    };
  }
  if (typeof WebAssembly.instantiateStreaming === "function") {
    const OrigInstantiateStreaming = WebAssembly.instantiateStreaming.bind(WebAssembly);
    WebAssembly.instantiateStreaming = async function (...args) {
      const start = performance.now();
      const r = await OrigInstantiateStreaming(...args);
      record("wasm_instantiateStreaming_async", start, performance.now());
      return r;
    };
  }

  // --- XHR instrumentation ---------------------------------------------------------------
  // loro-crdt's browser build loads its .wasm bytes via a SYNCHRONOUS XMLHttpRequest at
  // module-eval time (confirmed via \`grep -n XMLHttpRequest bundle-loro/dist/assets/
  // flow-entry-*.js\`), not fetch()/instantiateStreaming. That call is intercepted by
  // Puppeteer request interception (zero real socket I/O -- see measure-engine-runtime-
  // ready.mjs), but still goes through the browser's XHR/IPC machinery for however many bytes
  // are being delivered, which is real wall-clock cost distinct from WASM compile/instantiate
  // proper. Recorded here so the report can show where engine_runtime_ready time actually goes,
  // not just the compile/instantiate segments.
  const OrigXHROpen = XMLHttpRequest.prototype.open;
  const OrigXHRSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    this.__coldstart_url = url;
    this.__coldstart_async = rest[0] !== false;
    this.__coldstart_synthetic = __coldstartWasmUrl !== null && String(url) === __coldstartWasmUrl;
    if (this.__coldstart_synthetic) return; // do not touch the real XHR machinery at all
    return OrigXHROpen.call(this, method, url, ...rest);
  };
  XMLHttpRequest.prototype.send = function (...args) {
    const start = performance.now();
    if (this.__coldstart_synthetic) {
      // Fulfilled entirely from the in-page Uint8Array decoded before the timed window opened --
      // no Fetch-domain interception, no CDP round trip, no browser network stack involvement.
      Object.defineProperty(this, "status", { value: 200, configurable: true });
      Object.defineProperty(this, "statusText", { value: "OK", configurable: true });
      Object.defineProperty(this, "readyState", { value: 4, configurable: true });
      Object.defineProperty(this, "responseText", { value: __coldstartWasmText, configurable: true });
      Object.defineProperty(this, "response", { value: __coldstartWasmText, configurable: true });
      record("wasm_xhr_synthetic_inmemory_fulfill", start, performance.now());
      if (this.__coldstart_async !== false) {
        queueMicrotask(() => {
          this.dispatchEvent(new Event("readystatechange"));
          this.dispatchEvent(new Event("load"));
          this.dispatchEvent(new Event("loadend"));
        });
      }
      return undefined;
    }
    const url = this.__coldstart_url;
    const finish = () => record("xhr:" + url, start, performance.now());
    if (this.__coldstart_async === false) {
      // synchronous XHR: send() itself blocks until the response is ready.
      const r = OrigXHRSend.apply(this, args);
      finish();
      return r;
    }
    this.addEventListener("loadend", finish, { once: true });
    return OrigXHRSend.apply(this, args);
  };

  // --- Shared ready-postcondition helpers (ADR-0015 3.2) ---------------------------------
  async function waitForProseMirror(timeoutMs) {
    const deadline = performance.now() + timeoutMs;
    while (performance.now() < deadline) {
      const el = document.querySelector(".ProseMirror");
      if (el !== null) return el;
      await new Promise((r) => requestAnimationFrame(r));
    }
    return null;
  }

  // postcondition 5 (flow_route_cold_load ONLY, per coordinator correction 2026-08-29 item 3:
  // engine_runtime_ready uses postconditions 1-4 exclusively and does not call this).
  async function waitOneRaf() {
    await new Promise((r) => requestAnimationFrame(r));
  }

  // postcondition 4: the shared read/write probe. Rebuilds the exact block-array shape documented
  // in probe/coldstart-probe-v1.json's flow_route_cold_load_probe from what the adapter actually rendered (DOM order,
  // tag name, text -- NOT candidate-reported), hashes the FULL canonical JSON (coordinator
  // correction 2026-08-29 item 5: full hash, not a prefix), and compares it itself -- the
  // candidate/adapter has no code path that runs after this and no way to influence \`ok\`.
  async function readBackAndHashBlocks(expectedFullHex) {
    const root = document.querySelector(".ProseMirror");
    if (root === null) return { ok: false, reason: "no-prosemirror-element" };
    const blocks = Array.from(root.children).map((el, index) => ({
      parent: "root",
      index,
      type: el.tagName.toLowerCase(),
      text: el.textContent,
    }));
    const canonicalJson = JSON.stringify(blocks);
    const enc = new TextEncoder().encode(canonicalJson);
    const digestBuf = await crypto.subtle.digest("SHA-256", enc);
    const hex = Array.from(new Uint8Array(digestBuf))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
    const hashOk = hex === expectedFullHex;
    const mountOk = root.isConnected && root.getAttribute("contenteditable") === "true";
    return { ok: hashOk && mountOk, blocks, canonicalJson, hex, hashOk, mountOk };
  }

  function checkFlowReadyMarkCount() {
    return performance.getEntriesByName("flow-ready").length;
  }

  function markOnce(name) {
    if (performance.getEntriesByName(name).length !== 0) {
      return { ok: false, reason: "mark-already-exists: " + name };
    }
    performance.mark(name);
    return { ok: true };
  }

  function markStart(name) {
    performance.mark(name);
  }

  function summary(startMarkName, endMarkName) {
    const s = performance.getEntriesByName(startMarkName)[0];
    const e = performance.getEntriesByName(endMarkName)[0];
    return {
      startMs: s ? s.startTime : null,
      endMs: e ? e.startTime : null,
      elapsedMs: s && e ? e.startTime - s.startTime : null,
      navigationStartOriginMs: performance.timeOrigin,
      wasmTrace: trace,
      allMarks: performance.getEntriesByType("mark").map((m) => ({ name: m.name, startTime: m.startTime })),
    };
  }

  window.__coldstart = {
    trace,
    waitForProseMirror,
    waitOneRaf,
    readBackAndHashBlocks,
    checkFlowReadyMarkCount,
    markOnce,
    markStart,
    summary,
  };
})();
`;
}

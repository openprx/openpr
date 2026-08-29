// Real-browser verification of `ui-surface-v1.md` item 6 ("engine chunk 只从 (app)/flow 动态
// import，login/Forms route bundle 不携带 engine") for both v0.3 candidates: drives an isolated
// headless Chromium instance (own --user-data-dir under scratchpad, so it never conflicts with
// any other chrome-devtools-mcp session sharing this host) against the real `vite preview`
// servers for spikes/bundle-loro (port 4310) and spikes/bundle-yrs-yjs (port 4311), and asserts
// on the real Network domain request list -- not just static reachability analysis.
import puppeteer from "puppeteer-core";

const CANDIDATES = [
  { name: "loro", base: "http://localhost:4310", engineMarkers: [/flow-entry-.*\.js$/, /loro_wasm_bg-.*\.wasm$/] },
  { name: "yrs-yjs", base: "http://localhost:4311", engineMarkers: [/flow-entry-.*\.js$/] },
];

async function collectRequests(page, url) {
  const urls = [];
  const onRequest = (req) => urls.push(req.url());
  page.on("request", onRequest);
  await page.goto(url, { waitUntil: "networkidle0" });
  page.off("request", onRequest);
  return urls;
}

async function main() {
  const browser = await puppeteer.launch({
    executablePath: "/usr/bin/chromium",
    headless: true,
    userDataDir: "/tmp/claude-1000/-opt-worker/1a6d3dc8-4092-4f3a-a0ea-849ea0c3ea47/scratchpad/chromium-profile-verify",
    args: ["--no-sandbox", "--disable-dev-shm-usage"],
  });

  const results = {};
  try {
    for (const candidate of CANDIDATES) {
      const page = await browser.newPage();
      await page.setCacheEnabled(false);

      const homeRequests = await collectRequests(page, `${candidate.base}/`);
      const homeCarriesEngine = candidate.engineMarkers.some((re) => homeRequests.some((u) => re.test(u)));

      const flowRequests = await collectRequests(page, `${candidate.base}/flow/verify-object-1`);
      const flowCarriesEngine = candidate.engineMarkers.every((re) => flowRequests.some((u) => re.test(u)));

      const routeDataset = await page.$eval("#app", (el) => el.dataset.route);

      await page.close();

      results[candidate.name] = {
        home_route_requests: homeRequests.map((u) => u.replace(candidate.base, "")),
        home_route_carries_engine_chunk: homeCarriesEngine,
        flow_route_requests: flowRequests.map((u) => u.replace(candidate.base, "")),
        flow_route_carries_all_engine_markers: flowCarriesEngine,
        flow_route_dataset_route_attr: routeDataset,
      };
    }
  } finally {
    await browser.close();
  }

  console.log(JSON.stringify(results, null, 2));
}

await main();

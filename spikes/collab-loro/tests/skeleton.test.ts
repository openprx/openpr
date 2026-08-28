import { describe, expect, test } from "bun:test";

import { candidate, dependencyVersions, webIsolationKind } from "../src";

describe("collab candidate skeleton", () => {
  test("locks the candidate and browser dependency versions", () => {
    expect(candidate).toBe("loro");
    expect(dependencyVersions).toEqual({
      browserEngine: "1.14.1",
      prosemirrorBinding: "0.4.3",
      offlineAdapter: "project-internal",
      prosemirrorModel: "1.25.11",
      prosemirrorState: "1.4.4",
      prosemirrorView: "1.42.2",
    });
  });

  test("reserves the browser path for a worker sandbox", () => {
    expect(webIsolationKind).toBe("worker_sandbox");
  });
});

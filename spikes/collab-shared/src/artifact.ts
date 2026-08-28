import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import { sha256Hex } from "./hash";

export type Artifact = Readonly<{ path: string; sha256: string }>;

/**
 * Writes `data` under `baseDir/relPath` and returns the
 * `docs/schemas/sylvode-flow-*-result-v1.schema.json` `artifact` shape
 * (`{path, sha256}`), where `path` is relative (schema requires
 * `pattern: "^(?!/)"`) and `sha256` is the real digest of the bytes on disk.
 */
export async function writeArtifact(baseDir: string, relPath: string, data: Uint8Array<ArrayBuffer> | string): Promise<Artifact> {
  if (relPath.startsWith("/")) {
    throw new RangeError(`artifact path must be relative, got "${relPath}"`);
  }
  const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
  const absolutePath = join(baseDir, relPath);
  await mkdir(dirname(absolutePath), { recursive: true });
  await writeFile(absolutePath, bytes);
  return { path: relPath, sha256: await sha256Hex(bytes) };
}

export async function writeJsonArtifact(baseDir: string, relPath: string, value: unknown): Promise<Artifact> {
  return writeArtifact(baseDir, relPath, `${JSON.stringify(value, null, 2)}\n`);
}

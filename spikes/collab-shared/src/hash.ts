/** Deterministic JSON canonicalization + sha256, used for semantic-state hashing. No `Math.random()`, no non-deterministic key order. */

function sortKeysDeep(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(sortKeysDeep);
  }
  if (value !== null && typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    const out: Record<string, unknown> = {};
    for (const [key, val] of entries) {
      out[key] = sortKeysDeep(val);
    }
    return out;
  }
  return value;
}

export function canonicalStringify(value: unknown): string {
  return JSON.stringify(sortKeysDeep(value));
}

export async function sha256Hex(data: Uint8Array<ArrayBuffer>): Promise<string> {
  const digestBuffer = await crypto.subtle.digest("SHA-256", data);
  const digest = new Uint8Array(digestBuffer);
  let hex = "";
  for (const byte of digest) {
    hex += byte.toString(16).padStart(2, "0");
  }
  return hex;
}

export async function semanticHash(value: unknown): Promise<string> {
  return sha256Hex(new TextEncoder().encode(canonicalStringify(value)));
}

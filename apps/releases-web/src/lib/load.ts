import { MAX_MANIFEST_BYTES, type Manifest, parseManifest } from "./catalog";

export async function loadReleases(signal: AbortSignal): Promise<Manifest> {
  const response = await fetch("/v1/releases", {
    signal,
    credentials: "omit",
    cache: "no-store",
    redirect: "error",
  });
  if (!response.ok || !response.body) throw new Error("RELEASES_UNAVAILABLE");
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > MAX_MANIFEST_BYTES) throw new Error("RELEASES_UNAVAILABLE");
      chunks.push(value);
    }
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return parseManifest(
    JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)),
  );
}

// Read a request body as text without ever holding more than a set number of bytes.
// No React, Next or server import, so `node --test` loads it as is.

/**
 * The body as UTF-8 text, or null when it is larger than `maxBytes`. A Content-Length over the
 * limit is refused before anything is read; without one (or with a wrong one) the read stops as
 * soon as the limit is passed.
 */
export async function readBodyCapped(
  req: { headers: Headers; body: ReadableStream<Uint8Array> | null },
  maxBytes: number,
): Promise<string | null> {
  const declared = req.headers.get("content-length");
  if (declared !== null && Number(declared) > maxBytes) return null;
  if (!req.body) return "";
  const reader = req.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > maxBytes) {
      await reader.cancel().catch(() => {});
      return null;
    }
    chunks.push(value);
  }
  const all = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.byteLength;
  }
  return new TextDecoder().decode(all);
}

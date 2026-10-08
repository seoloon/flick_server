import assert from "node:assert/strict";
import { test } from "node:test";

import { readBodyCapped } from "../src/lib/read-body.ts";

function streamOf(parts: string[]): ReadableStream<Uint8Array> {
  const enc = new TextEncoder();
  return new ReadableStream({
    start(c) {
      for (const p of parts) c.enqueue(enc.encode(p));
      c.close();
    },
  });
}

const req = (parts: string[] | null, length?: string) => ({
  headers: new Headers(length === undefined ? {} : { "content-length": length }),
  body: parts === null ? null : streamOf(parts),
});

test("a body within the limit is read whole, across chunks and multi-byte characters", async () => {
  assert.equal(await readBodyCapped(req(['{"values":', '{"A":"é"}}']), 64), '{"values":{"A":"é"}}');
  assert.equal(await readBodyCapped(req(null), 64), "");
  assert.equal(await readBodyCapped(req(["x".repeat(16)], "16"), 16), "x".repeat(16));
});

test("a Content-Length over the limit is refused before reading", async () => {
  const body = streamOf(["x".repeat(17)]);
  assert.equal(await readBodyCapped({ headers: new Headers({ "content-length": "17" }), body }, 16), null);
  assert.equal(body.locked, false); // never read
});

test("a body over the limit is cut off even without (or with a wrong) Content-Length", async () => {
  assert.equal(await readBodyCapped(req(["x".repeat(10), "x".repeat(7)]), 16), null);
  assert.equal(await readBodyCapped(req(["x".repeat(17)], "3"), 16), null);
  assert.equal(await readBodyCapped(req(["é".repeat(8), "é"]), 16), null); // 18 bytes
});

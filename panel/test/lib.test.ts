import assert from "node:assert/strict";
import { test } from "node:test";

import { AttemptLimiter } from "../src/lib/limiter.ts";
import { createSessionToken, passwordMatches, verifySessionToken } from "../src/lib/session.ts";
import {
  formatBytes,
  formatDuration,
  formatPosition,
  formatSpeed,
  livePosition,
} from "../src/lib/format.ts";

test("formatBytes and formatSpeed use binary units", () => {
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(1023), "1023 B");
  assert.equal(formatBytes(1536), "1.5 KiB");
  assert.equal(formatBytes(5 * 1024 ** 3), "5.0 GiB");
  assert.equal(formatSpeed(10 * 1024 * 1024), "10.0 MiB/s");
  assert.equal(formatBytes(Number.NaN), "n/a");
});

const PW = "correct horse battery staple";

test("a fresh session token verifies", () => {
  const t = createSessionToken(PW, 1_000_000);
  assert.equal(verifySessionToken(PW, t, 1_000_000), true);
});

test("a session token expires", () => {
  const t = createSessionToken(PW, 1_000_000, 60);
  assert.equal(verifySessionToken(PW, t, 1_000_000 + 59_000), true);
  assert.equal(verifySessionToken(PW, t, 1_000_000 + 61_000), false);
});

test("a token signed with another password, or tampered with, is refused", () => {
  const t = createSessionToken(PW, 1_000_000);
  assert.equal(verifySessionToken("another password", t, 1_000_000), false);
  const [exp, nonce, sig] = t.split(".");
  assert.equal(verifySessionToken(PW, `${Number(exp) + 99999}.${nonce}.${sig}`, 1_000_000), false);
  assert.equal(verifySessionToken(PW, `${exp}.${nonce}.${sig.slice(0, -2)}xx`, 1_000_000), false);
});

test("garbage and empty tokens are refused, and an empty password never verifies", () => {
  for (const bad of [undefined, "", "a", "a.b", "a.b.c", "a.b.c.d", "..", "1.2.3"]) {
    assert.equal(verifySessionToken(PW, bad, 0), false, String(bad));
  }
  assert.equal(verifySessionToken("", createSessionToken("", 0), 0), false);
});

test("two tokens for the same login differ", () => {
  assert.notEqual(createSessionToken(PW, 5), createSessionToken(PW, 5));
});

test("password comparison", () => {
  assert.equal(passwordMatches(PW, PW), true);
  assert.equal(passwordMatches(PW + "x", PW), false);
  assert.equal(passwordMatches("", PW), false);
});

test("the limiter blocks after max failures and recovers", () => {
  const l = new AttemptLimiter(3, 60_000);
  assert.equal(l.retryAfterSecs("ip", 0), 0);
  l.fail("ip", 0);
  l.fail("ip", 1000);
  assert.equal(l.retryAfterSecs("ip", 2000), 0);
  l.fail("ip", 2000);
  assert.ok(l.retryAfterSecs("ip", 3000) > 0);
  assert.equal(l.retryAfterSecs("other", 3000), 0, "per client");
  assert.equal(l.retryAfterSecs("ip", 125_000), 0, "window passed for every failure");
  l.fail("ip", 125_000);
  l.fail("ip", 125_500);
  l.fail("ip", 126_000);
  assert.ok(l.retryAfterSecs("ip", 126_500) > 0);
  l.success("ip");
  assert.equal(l.retryAfterSecs("ip", 126_500), 0);
});

test("formatting", () => {
  assert.equal(formatDuration(42), "42 s");
  assert.equal(formatDuration(7 * 60), "7 min");
  assert.equal(formatDuration(3600 + 5 * 60), "1 h 05 min");
  assert.equal(formatDuration(2 * 86400 + 4 * 3600), "2 d 4 h");
  assert.equal(formatPosition(245), "4:05");
  assert.equal(formatPosition(3600 + 23 * 60 + 45), "1:23:45");
});

test("a playing room's position advances from the server stamp, a paused one does not", () => {
  const p = { state: "playing", position: 100, rate: 2, server_time: 1_000 };
  assert.equal(livePosition(p, 4_000), 106);
  assert.equal(livePosition({ ...p, state: "paused" }, 4_000), 100);
  assert.equal(livePosition(p, 500), 100, "never goes backwards");
});

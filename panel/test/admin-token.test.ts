import assert from "node:assert/strict";
import { test } from "node:test";

import { deriveAdminToken, resolveAdminToken } from "../src/lib/admin-token.ts";

const PW = "correct horse battery staple";
// Same vector as `the_derived_token_matches_the_value_the_panel_computes` in src/admin_token.rs.
const TOKEN = "602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011";

test("the admin token matches the server's vector", () => {
  assert.equal(deriveAdminToken(PW), TOKEN);
});

test("the password is trimmed first, like on the server", () => {
  assert.equal(deriveAdminToken(`  ${PW}\n`), TOKEN);
});

test("a password under 10 bytes gives no token", () => {
  assert.equal(deriveAdminToken(""), "");
  assert.equal(deriveAdminToken("123456789"), "");
  assert.equal(deriveAdminToken("  123456789  "), "");
  assert.notEqual(deriveAdminToken("1234567890"), "");
});

test("the length is counted in bytes, as the server does", () => {
  // 5 characters, 10 bytes in UTF-8.
  assert.notEqual(deriveAdminToken("ééééé"), "");
});

test("a leftover FLICKSYNC_ADMIN_TOKEN is ignored", () => {
  assert.deepEqual(
    resolveAdminToken({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: "  legacy-token-0123456789 " }),
    { token: TOKEN, source: "password" },
  );
  assert.deepEqual(resolveAdminToken({ FLICKSYNC_ADMIN_TOKEN: "legacy-token-0123456789" }), {
    token: "",
    source: "none",
  });
});

test("the token comes from PANEL_PASSWORD alone", () => {
  assert.deepEqual(resolveAdminToken({ PANEL_PASSWORD: PW }), { token: TOKEN, source: "password" });
});

test("a short or missing password gives no token", () => {
  assert.deepEqual(resolveAdminToken({ PANEL_PASSWORD: "short" }), { token: "", source: "none" });
  assert.deepEqual(resolveAdminToken({}), { token: "", source: "none" });
});

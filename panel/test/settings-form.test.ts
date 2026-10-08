import assert from "node:assert/strict";
import { test } from "node:test";

import {
  type Edits,
  MAX_PATCH_CHARS,
  buildPatch,
  choiceOptions,
  fieldDisplay,
  fieldHint,
  fieldLabel,
  isSettingsScope,
  namedFields,
  parseBool,
  parsePatchBody,
  resetEdit,
  setEdit,
  undoEdit,
  visibleFields,
} from "../src/lib/settings-form.ts";
import type { SettingField, SettingsView } from "../src/lib/types.ts";

function field(over: Partial<SettingField> & Pick<SettingField, "name" | "kind">): SettingField {
  const secret = over.kind === "secret";
  return {
    secret,
    value: secret ? null : "",
    set: false,
    source: "default",
    default: secret ? null : "",
    ...over,
  };
}

const VIEW: SettingsView = {
  scope: "flickdd",
  revision: 3,
  fields: [
    field({ name: "FLICKDD_ENABLED", kind: "bool", value: "true", default: "false", source: "panel", set: true }),
    field({ name: "FLICKDD_MAX_PARALLEL", kind: "int", value: "10", default: "10" }),
    field({ name: "FLICKDD_MAX_GLOBAL", kind: "int", value: "50", default: "100", source: "panel", set: true }),
    field({ name: "FLICKDD_JELLYFIN_URL", kind: "text", value: "http://jellyfin:8096", source: "environment", set: true }),
    field({ name: "FLICKDD_JELLYFIN_API_KEY", kind: "secret", source: "environment", set: true }),
    field({ name: "FLICKDD_PLEX_TOKEN", kind: "secret", source: "panel", set: true }),
  ],
};

const byName = (name: string): SettingField => {
  const f = VIEW.fields.find((x) => x.name === name);
  if (!f) throw new Error(`no field ${name}`);
  return f;
};

test("scopes are a fixed list", () => {
  for (const s of ["server", "flicksync", "flickdd"]) assert.equal(isSettingsScope(s), true, s);
  for (const s of ["", "modules", "Server", "../server"]) assert.equal(isSettingsScope(s), false, s);
});

test("labels come from the variable name", () => {
  assert.equal(fieldLabel("FLICKSYNC_PUBLIC_URL"), "Public URL");
  assert.equal(fieldLabel("FLICKSYNC_MAX_ROOM_SIZE"), "Max room size");
  assert.equal(fieldLabel("FLICKDD_JELLYFIN_API_KEY"), "Jellyfin API key");
  assert.equal(fieldLabel("FLICKSYNC_WS_PING_INTERVAL"), "WS ping interval");
  assert.equal(fieldLabel("FLICKSYNC_SYNC_SEEK_COOLDOWN_MS"), "Sync seek cooldown ms");
  assert.equal(fieldLabel("FLICKSYNC_CORS_ORIGINS"), "CORS origins");
});

test("the module switches are left to Start and Stop", () => {
  assert.deepEqual(
    visibleFields(VIEW).map((f) => f.name),
    ["FLICKDD_MAX_PARALLEL", "FLICKDD_MAX_GLOBAL", "FLICKDD_JELLYFIN_URL", "FLICKDD_JELLYFIN_API_KEY", "FLICKDD_PLEX_TOKEN"],
  );
});

test("switch values are read like the server reads them", () => {
  for (const v of ["1", "true", "YES", " on "]) assert.equal(parseBool(v), true, v);
  for (const v of ["0", "false", "No", "off"]) assert.equal(parseBool(v), false, v);
  for (const v of ["", "perhaps", null, undefined]) assert.equal(parseBool(v), null, String(v));
});

test("nothing is sent until something really changes", () => {
  assert.deepEqual(buildPatch(VIEW, {}), {});
  const parallel = byName("FLICKDD_MAX_PARALLEL");
  let e = setEdit({}, parallel, "12");
  assert.deepEqual(buildPatch(VIEW, e), { FLICKDD_MAX_PARALLEL: "12" });
  e = setEdit(e, parallel, "10");
  assert.deepEqual(e, {}, "typing the current value back cancels the change");
  assert.deepEqual(buildPatch(VIEW, e), {});
});

test("a switch written differently in the environment is not a change", () => {
  const chat = field({
    name: "FLICKSYNC_CHAT_ENABLED", kind: "bool", value: "yes", default: "true", source: "environment", set: true,
  });
  assert.deepEqual(setEdit({}, chat, "true"), {});
  assert.deepEqual(setEdit({}, chat, "false"), { FLICKSYNC_CHAT_ENABLED: "false" });
  const odd = { ...chat, value: "perhaps" };
  assert.deepEqual(setEdit({}, odd, "true"), { FLICKSYNC_CHAT_ENABLED: "true" });
  assert.deepEqual(setEdit({}, odd, "false"), { FLICKSYNC_CHAT_ENABLED: "false" });
});

test("a secret is sent only when a new value is typed", () => {
  const key = byName("FLICKDD_JELLYFIN_API_KEY");
  assert.deepEqual(setEdit({}, key, ""), {});
  const e = setEdit({}, key, "new-key");
  assert.deepEqual(buildPatch(VIEW, e), { FLICKDD_JELLYFIN_API_KEY: "new-key" });
  assert.deepEqual(setEdit(e, key, ""), {}, "emptying the input cancels");
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_JELLYFIN_API_KEY: "" }), {});
});

test("reset sends null only for a value saved in the panel", () => {
  let e = resetEdit({}, byName("FLICKDD_MAX_GLOBAL"));
  e = resetEdit(e, byName("FLICKDD_JELLYFIN_URL"));
  e = resetEdit(e, byName("FLICKDD_PLEX_TOKEN"));
  assert.deepEqual(e, { FLICKDD_MAX_GLOBAL: null, FLICKDD_PLEX_TOKEN: null });
  assert.deepEqual(buildPatch(VIEW, e), { FLICKDD_MAX_GLOBAL: null, FLICKDD_PLEX_TOKEN: null });
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_JELLYFIN_URL: null }), {}, "null for a non-panel value is dropped");
  assert.deepEqual(undoEdit(e, "FLICKDD_MAX_GLOBAL"), { FLICKDD_PLEX_TOKEN: null });
});

test("edits for fields the form does not show are never sent", () => {
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_ENABLED: "false", FLICKSYNC_MAX_ROOMS: "5" }), {});
});

test("what a control shows", () => {
  const global = byName("FLICKDD_MAX_GLOBAL");
  assert.deepEqual(fieldDisplay(global, {}), { value: "50", changed: false, removing: false });
  assert.deepEqual(fieldDisplay(global, { FLICKDD_MAX_GLOBAL: "60" }), { value: "60", changed: true, removing: false });
  assert.deepEqual(fieldDisplay(global, { FLICKDD_MAX_GLOBAL: null }), { value: "100", changed: true, removing: true });
  assert.deepEqual(fieldDisplay(byName("FLICKDD_PLEX_TOKEN"), {}), { value: "", changed: false, removing: false });
});

test("the hint says where a value comes from", () => {
  const hint = (f: SettingField, e: Edits = {}) => fieldHint(f, fieldDisplay(f, e));
  assert.equal(hint(byName("FLICKDD_MAX_PARALLEL")), "Default");
  assert.equal(hint(byName("FLICKDD_MAX_GLOBAL")), "Saved in the panel · default 100");
  assert.equal(hint(byName("FLICKDD_JELLYFIN_URL")), "From the environment · default none");
  assert.equal(hint(byName("FLICKDD_JELLYFIN_API_KEY")), "From the environment · a value saved here overrides it");
  assert.equal(hint(byName("FLICKDD_PLEX_TOKEN")), "Saved in the panel");
  assert.equal(hint(field({ name: "FLICKDD_PLEX_TOKEN", kind: "secret" })), "");
  assert.equal(
    hint(byName("FLICKDD_MAX_GLOBAL"), { FLICKDD_MAX_GLOBAL: null }),
    "Save removes the panel's value: the environment value applies, or the default (100).",
  );
  assert.equal(hint(byName("FLICKDD_PLEX_TOKEN"), { FLICKDD_PLEX_TOKEN: null }), "Save clears the value saved in the panel.");
  assert.equal(hint(field({ name: "FLICKSYNC_CORS_ORIGINS", kind: "list" })), "Default · comma-separated");
  assert.equal(
    hint(field({ name: "FLICKSYNC_CHAT_ENABLED", kind: "bool", value: "perhaps", default: "true", source: "environment", set: true })),
    "From the environment · default true · 'perhaps' is not an on/off value",
  );
});

test("a choice keeps a current value that is not one of the choices", () => {
  const mode = field({
    name: "FLICKSYNC_DEFAULT_CONTROL_MODE", kind: "choice", value: "everyone", default: "everyone",
    choices: ["everyone", "host_only"],
  });
  assert.deepEqual(choiceOptions(mode, "everyone"), ["everyone", "host_only"]);
  assert.deepEqual(choiceOptions(mode, "hosts"), ["everyone", "host_only", "hosts"]);
});

test("only the settings a message names are flagged", () => {
  const names = ["FLICKSYNC_RATE_MIN", "FLICKSYNC_RATE_MAX", "FLICKSYNC_SYNC_DRIFT_SOFT", "FLICKSYNC_MAX_ROOMS"];
  assert.deepEqual(
    namedFields("FLICKSYNC_RATE_MIN (0.5) must be below FLICKSYNC_RATE_MAX (0.25). Nothing was saved. (SETTINGS_INVALID)", names),
    ["FLICKSYNC_RATE_MIN", "FLICKSYNC_RATE_MAX"],
  );
  assert.deepEqual(namedFields("FLICKSYNC_SYNC_DRIFT_SOFTER is not a known setting. (UNKNOWN_SETTING)", names), []);
  assert.deepEqual(namedFields("XFLICKSYNC_MAX_ROOMS and FLICKSYNC_MAX_ROOMS_2", names), []);
  assert.deepEqual(namedFields("Something else went wrong.", names), []);
});

test("the PUT body is checked before it is forwarded", () => {
  assert.deepEqual(parsePatchBody('{"values":{"FLICKSYNC_MAX_ROOMS":"200","FLICKSYNC_DEFAULT_CONTROL_MODE":null}}'), {
    ok: true,
    values: { FLICKSYNC_MAX_ROOMS: "200", FLICKSYNC_DEFAULT_CONTROL_MODE: null },
  });
  for (const raw of [
    "not json",
    "[]",
    "null",
    '{"values":[]}',
    '{"other":{}}',
    '{"values":{"lower_case":"1"}}',
    '{"values":{"FLICKSYNC_MAX_ROOMS":200}}',
    '{"values":{"FLICKSYNC_MAX_ROOMS":{"x":1}}}',
    '{"values":{}}',
    "x".repeat(MAX_PATCH_CHARS + 1),
  ]) {
    const r = parsePatchBody(raw);
    assert.equal(r.ok, false, raw.slice(0, 40));
    if (!r.ok) assert.ok(r.message.length > 0);
  }
});

test("an emptied number, choice or switch is a Reset, an emptied text is sent as is", () => {
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_MAX_GLOBAL: "" }), { FLICKDD_MAX_GLOBAL: null });
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_MAX_GLOBAL: "  " }), { FLICKDD_MAX_GLOBAL: null });
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_MAX_PARALLEL: "" }), {});
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_JELLYFIN_URL: "" }), { FLICKDD_JELLYFIN_URL: "" });
});

test("a cleared text field warns that the empty value hides the environment", () => {
  const url = byName("FLICKDD_JELLYFIN_URL");
  const hint = fieldHint(url, fieldDisplay(url, { FLICKDD_JELLYFIN_URL: "" }));
  assert.match(hint, /An empty value saved here hides the environment value\. Use Reset to go back to it\./);
  assert.doesNotMatch(fieldHint(url, fieldDisplay(url, {})), /empty value/);
});

test("unit labels keep their usual casing", () => {
  assert.equal(fieldLabel("FLICKDD_RATE_MBPS"), "Rate Mbps");
});

import assert from "node:assert/strict";
import { test } from "node:test";

import { checkPanelEnv, panelEnabled } from "../env-check.mjs";

const PW = "correct horse battery staple";

test("the panel starts only when ENABLE_WEB_PANEL is truthy", () => {
  for (const v of ["true", " TRUE ", "1", "yes", "on"]) {
    assert.equal(panelEnabled({ ENABLE_WEB_PANEL: v }), true, v);
  }
  for (const v of [undefined, "", "false", "0", "nope"]) {
    assert.equal(panelEnabled({ ENABLE_WEB_PANEL: v }), false, String(v));
  }
});

test("a missing, short or space-padded short password is refused", () => {
  for (const env of [{}, { PANEL_PASSWORD: "short" }, { PANEL_PASSWORD: "   123456789   " }]) {
    const { problems } = checkPanelEnv(env);
    assert.equal(problems.length, 1, JSON.stringify(env));
    assert.match(problems[0], /PANEL_PASSWORD/);
  }
});

test("a good password alone is enough: no FLICKSYNC_ADMIN_TOKEN needed", () => {
  assert.deepEqual(checkPanelEnv({ PANEL_PASSWORD: PW }), { problems: [], notes: [] });
  assert.deepEqual(checkPanelEnv({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: "  " }), { problems: [], notes: [] });
});

test("a leftover FLICKSYNC_ADMIN_TOKEN is ignored: no problem, no note", () => {
  for (const token of ["legacy-token-0123456789", "short", "  "]) {
    assert.deepEqual(checkPanelEnv({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: token }), {
      problems: [],
      notes: [],
    });
  }
});

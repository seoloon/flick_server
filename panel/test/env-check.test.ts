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

test("a legacy token is accepted with a deprecation note; a short one is refused", () => {
  const ok = checkPanelEnv({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: "legacy-token-0123456789" });
  assert.deepEqual(ok.problems, []);
  assert.equal(ok.notes.length, 1);
  assert.match(ok.notes[0], /deprecated/);

  const short = checkPanelEnv({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: "short" });
  assert.equal(short.problems.length, 1);
  assert.match(short.problems[0], /FLICKSYNC_ADMIN_TOKEN/);
});

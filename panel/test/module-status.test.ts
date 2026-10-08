import assert from "node:assert/strict";
import { test } from "node:test";

import {
  availableActions,
  confirmation,
  followUp,
  followUpText,
  isModuleAction,
  isModuleId,
  mergeStatus,
  offText,
  sinceText,
  stateBadge,
} from "../src/lib/module-status.ts";
import type { ModuleStatus } from "../src/lib/types.ts";

const status = (over: Partial<ModuleStatus> = {}): ModuleStatus => ({
  id: "flicksync",
  state: "stopped",
  message: null,
  enabled: false,
  pending_reload: false,
  since: null,
  ...over,
});

test("module ids and actions are fixed lists", () => {
  assert.equal(isModuleId("flicksync"), true);
  assert.equal(isModuleId("flickdd"), true);
  assert.equal(isModuleId("server"), false);
  for (const a of ["start", "stop", "reload"]) assert.equal(isModuleAction(a), true, a);
  for (const a of ["restart", "", "START"]) assert.equal(isModuleAction(a), false, a);
});

test("each state has its badge and its actions", () => {
  assert.deepEqual(stateBadge(status({ state: "running", since: 1 })), { label: "Running", tone: "strong" });
  assert.deepEqual(stateBadge(status()), { label: "Stopped", tone: "plain" });
  assert.deepEqual(stateBadge(status({ state: "failed", message: "x" })), { label: "Failed to start", tone: "warn" });
  assert.deepEqual(availableActions(status({ state: "running", since: 1 })), ["reload", "stop"]);
  assert.deepEqual(availableActions(status()), ["start"]);
  assert.deepEqual(availableActions(status({ state: "failed", message: "x" })), ["start", "stop"]);
});

test("stop and reload of a running module ask first and say what they interrupt", () => {
  const running = status({ state: "running", since: 0 });
  assert.equal(confirmation(running, "start"), null);
  const stop = confirmation(running, "stop");
  assert.equal(stop?.title, "Stop FlickSync?");
  assert.equal(stop?.confirm, "Stop FlickSync");
  assert.match(stop?.description ?? "", /Every room closes/);
  assert.match(stop?.description ?? "", /also after a server restart/);
  const reload = confirmation(status({ id: "flickdd", state: "running", since: 0 }), "reload");
  assert.equal(reload?.title, "Reload FlickDD?");
  assert.equal(reload?.confirm, "Reload FlickDD");
  assert.match(reload?.description ?? "", /Downloads in progress are cut/);
  assert.equal(confirmation(status({ state: "failed", message: "x" }), "stop"), null, "nothing to interrupt");
});

test("running time", () => {
  assert.equal(sinceText(status({ state: "running", since: 1_000 }), 1_000 + 7_500_000), "Running for 2 h 05 min");
  assert.equal(sinceText(status(), 5_000), null);
});

test("what a module page says when the module is off", () => {
  assert.match(offText(status()), /nobody can watch together/);
  assert.match(offText(status({ id: "flickdd" })), /Jellyfin or Plex/);
  assert.match(offText(status({ id: "flickdd", state: "failed", message: "x" })), /FlickDD is switched on but could not start/);
});

test("an action's answer replaces that module's status only", () => {
  const list = [status(), status({ id: "flickdd" })];
  const failed = status({ state: "failed", message: "FlickSync could not start.", enabled: true });
  assert.deepEqual(mergeStatus(list, failed), [failed, list[1]]);
  assert.deepEqual(mergeStatus([], failed), [failed]);
  assert.deepEqual(stateBadge(mergeStatus(list, failed)[0]).label, "Failed to start");
});

test("what a save offers next", () => {
  assert.equal(followUp("server", null), "apply-server");
  assert.equal(followUp("flicksync", null), "none");
  assert.equal(followUp("flicksync", status({ state: "running", since: 1, pending_reload: true })), "reload");
  assert.equal(followUp("flicksync", status({ state: "running", since: 1 })), "none");
  assert.equal(followUp("flickdd", status({ id: "flickdd", state: "failed", message: "x" })), "start-again");
  assert.equal(followUp("flickdd", status({ id: "flickdd" })), "applies-on-start");
  assert.equal(
    followUpText("server", "apply-server", 8),
    "Saved as revision 8. Apply the server settings to put them in force; running modules keep running.",
  );
  assert.equal(followUpText("flickdd", "reload", 7), "Saved as revision 7. FlickDD keeps its previous settings until it is reloaded.");
  assert.equal(followUpText("flickdd", "start-again", 7), "Saved as revision 7. Start FlickDD again to try them.");
  assert.equal(followUpText("flicksync", "applies-on-start", 2), "Saved as revision 2. FlickSync is stopped and uses them when it starts.");
  assert.equal(followUpText("flicksync", "none", 9), "Saved as revision 9.");
});

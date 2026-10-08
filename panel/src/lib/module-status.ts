// Pure logic of the module controls: labels, which actions to offer, confirmations, follow-ups.
// No React, Next or server import, so `node --test` loads it as is.
import { formatDuration } from "./format.ts";
import type { ModuleId, ModuleStatus, SettingsScope } from "./types.ts";

export const MODULE_IDS: readonly ModuleId[] = ["flicksync", "flickdd"];
export const MODULE_ACTIONS = ["start", "stop", "reload"] as const;
export type ModuleAction = (typeof MODULE_ACTIONS)[number];

export const MODULE_LABEL: Record<ModuleId, string> = { flicksync: "FlickSync", flickdd: "FlickDD" };

export function isModuleId(s: string): s is ModuleId {
  return (MODULE_IDS as readonly string[]).includes(s);
}

export function isModuleAction(s: string): s is ModuleAction {
  return (MODULE_ACTIONS as readonly string[]).includes(s);
}

export function stateBadge(s: ModuleStatus): { label: string; tone: "plain" | "strong" | "warn" } {
  if (s.state === "running") return { label: "Running", tone: "strong" };
  if (s.state === "failed") return { label: "Failed to start", tone: "warn" };
  return { label: "Stopped", tone: "plain" };
}

/** Running: Reload, Stop. Stopped: Start. Failed: Start (try again), Stop (switch it off). */
export function availableActions(s: ModuleStatus): ModuleAction[] {
  if (s.state === "running") return ["reload", "stop"];
  if (s.state === "failed") return ["start", "stop"];
  return ["start"];
}

export const ACTION_LABEL: Record<ModuleAction, { idle: string; busy: string }> = {
  start: { idle: "Start", busy: "Starting" },
  stop: { idle: "Stop", busy: "Stopping" },
  reload: { idle: "Reload", busy: "Reloading" },
};

export interface Confirmation {
  title: string;
  description: string;
  /** Label of the confirming button. */
  confirm: string;
}

const INTERRUPTS: Record<ModuleId, string> = {
  flicksync: "Every room closes and everyone watching together is disconnected.",
  flickdd: "Downloads in progress are cut (clients resume them later) and the download statistics are reset.",
};

/** Stop and Reload interrupt the users of a running module: ask first. Null: run at once. */
export function confirmation(s: ModuleStatus, action: ModuleAction): Confirmation | null {
  if (action === "start" || s.state !== "running") return null;
  const label = MODULE_LABEL[s.id];
  if (action === "stop") {
    return {
      title: `Stop ${label}?`,
      description: `${INTERRUPTS[s.id]} ${label} stays stopped, also after a server restart, until you start it again.`,
      confirm: `Stop ${label}`,
    };
  }
  return {
    title: `Reload ${label}?`,
    description: `${label} restarts with the saved settings. ${INTERRUPTS[s.id]}`,
    confirm: `Reload ${label}`,
  };
}

/** "Running for 2 h 05 min", or null when the module is not running. */
export function sinceText(s: ModuleStatus, nowMs: number): string | null {
  if (s.state !== "running" || s.since === null) return null;
  return `Running for ${formatDuration((nowMs - s.since) / 1000)}`;
}

/** What a module page says instead of its content while the module is stopped or failed. */
export function offText(s: ModuleStatus): string {
  const label = MODULE_LABEL[s.id];
  if (s.state === "failed") {
    return `${label} is switched on but could not start, for the reason above. Change the setting in ${label}'s settings, then start it again.`;
  }
  return s.id === "flicksync"
    ? "FlickSync is stopped, so nobody can watch together on this server. Start it to accept rooms again. It stays stopped, also after a server restart, until you start it."
    : "FlickDD is stopped, so offline downloads are not available. Set up a Jellyfin or Plex backend in its settings, then start it. It stays stopped, also after a server restart, until you start it.";
}

/** Replace one module's status in a /modules answer with the one an action returned. */
export function mergeStatus(list: ModuleStatus[], next: ModuleStatus): ModuleStatus[] {
  return list.some((m) => m.id === next.id)
    ? list.map((m) => (m.id === next.id ? next : m))
    : [...list, next];
}

export type FollowUp = "apply-server" | "reload" | "start-again" | "applies-on-start" | "none";

/** What the settings page offers once a save went through. */
export function followUp(scope: SettingsScope, s: ModuleStatus | null): FollowUp {
  if (scope === "server") return "apply-server";
  if (!s) return "none";
  if (s.state === "failed") return "start-again";
  if (s.state === "stopped") return "applies-on-start";
  return s.pending_reload ? "reload" : "none";
}

export function followUpText(scope: SettingsScope, f: FollowUp, revision: number): string {
  const saved = `Saved as revision ${revision}.`;
  const label = scope === "server" ? "The server" : MODULE_LABEL[scope];
  switch (f) {
    case "apply-server":
      return `${saved} Apply the server settings to put them in force; running modules keep running.`;
    case "reload":
      return `${saved} ${label} keeps its previous settings until it is reloaded.`;
    case "start-again":
      return `${saved} Start ${label} again to try them.`;
    case "applies-on-start":
      return `${saved} ${label} is stopped and uses them when it starts.`;
    default:
      return saved;
  }
}

/** "3 d 4 h", "2 h 05 min", "7 min", "42 s". */
export function formatDuration(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d} d ${h} h`;
  if (h > 0) return `${h} h ${String(m).padStart(2, "0")} min`;
  if (m > 0) return `${m} min`;
  return `${s} s`;
}

/** Playback position: "1:23:45" or "4:05". */
export function formatPosition(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return `${h > 0 ? `${h}:` : ""}${mm}:${String(sec).padStart(2, "0")}`;
}

export function formatCount(n: number): string {
  return new Intl.NumberFormat("en-GB").format(n);
}

export function formatMs(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "n/a";
  return ms < 10 ? `${ms.toFixed(1)} ms` : `${Math.round(ms)} ms`;
}

export function formatPercent(part: number, total: number): string {
  if (total <= 0) return "0 %";
  const p = (part / total) * 100;
  return `${p < 10 && p > 0 ? p.toFixed(1) : Math.round(p)} %`;
}

/** Position of a playing room, advanced from the moment the server stamped it. */
export function livePosition(
  p: { state: string; position: number; rate: number; server_time: number },
  nowMs: number,
): number {
  if (p.state !== "playing") return p.position;
  return p.position + Math.max(0, nowMs - p.server_time) / 1000 * p.rate;
}

const STATE_LABEL: Record<string, string> = {
  waiting: "Waiting",
  media_selected: "Ready",
  playing: "Playing",
  paused: "Paused",
  empty: "Empty",
  closed: "Closed",
};

export function stateLabel(state: string): string {
  return STATE_LABEL[state] ?? state;
}

export const PRESENCE_LABEL: Record<string, string> = {
  connected: "Connected",
  reconnecting: "Reconnecting",
  disconnected: "Not connected",
};

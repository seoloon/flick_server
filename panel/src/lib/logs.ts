// Pure logic of the Logs page: the cursor, merging answers, filters and line formatting.
// No React, Next or server import, so `node --test` loads it as is.
import type { LogEntry, LogLevel, LogsResponse } from "./types.ts";

/** Most severe first. */
export const LOG_LEVELS: readonly LogLevel[] = ["error", "warn", "info", "debug", "trace"];

export const LEVEL_LABEL: Record<LogLevel, string> = {
  error: "Error",
  warn: "Warn",
  info: "Info",
  debug: "Debug",
  trace: "Trace",
};

/** Time between two polls. */
export const POLL_MS = 2_000;
/** `limit` of each request: the server's maximum. */
export const PAGE_LIMIT = 1_000;
/** Lines the page keeps; the oldest go first. */
export const MAX_CLIENT_LINES = 5_000;
/** Pages fetched back to back while the server says there is more. */
export const MAX_PAGES_PER_POLL = 10;

export function isLogLevel(s: string): s is LogLevel {
  return (LOG_LEVELS as readonly string[]).includes(s);
}

/** Higher is more severe: error 4, warn 3, info 2, debug 1, trace 0. */
export function levelRank(level: LogLevel): number {
  return LOG_LEVELS.length - 1 - LOG_LEVELS.indexOf(level);
}

export type LogLine =
  | { kind: "entry"; key: string; entry: LogEntry }
  | { kind: "gap"; key: string; count: number; capacity: number }
  | { kind: "restart"; key: string };

export type MarkerLine = Exclude<LogLine, { kind: "entry" }>;

export interface LogView {
  /** The server run the lines come from (its `boot`); null before the first answer. */
  boot: number | null;
  /** The `after` of the next request. */
  cursor: number;
  lines: LogLine[];
  /** Lines removed from the top to stay under MAX_CLIENT_LINES since the last clear. */
  trimmed: number;
}

export function emptyView(): LogView {
  return { boot: null, cursor: 0, lines: [], trimmed: 0 };
}

/** The panel route for the next page after `cursor`. */
export function logsPath(cursor: number): string {
  return `/api/logs?after=${cursor}&limit=${PAGE_LIMIT}`;
}

function capped(view: LogView, lines: LogLine[], maxLines: number): Pick<LogView, "lines" | "trimmed"> {
  const removed = Math.max(0, lines.length - maxLines);
  return { lines: removed > 0 ? lines.slice(removed) : lines, trimmed: view.trimmed + removed };
}

/**
 * Add one answer of GET /logs to the view. `again`: ask for the next page at once, because the
 * server has more, or because it restarted and the view must start over from its first line.
 */
export function mergeLogs(
  view: LogView,
  res: LogsResponse,
  maxLines = MAX_CLIENT_LINES,
): { view: LogView; again: boolean } {
  if (view.boot !== null && res.boot !== view.boot) {
    // A new run numbers its lines from 1 again: this page (asked after an old cursor) is incomplete.
    const marker: LogLine = { kind: "restart", key: `restart:${res.boot}` };
    return {
      view: { boot: res.boot, cursor: 0, ...capped(view, [...view.lines, marker], maxLines) },
      again: true,
    };
  }
  const added: LogLine[] = [];
  if (res.dropped > 0 && view.cursor > 0) {
    added.push({ kind: "gap", key: `gap:${res.boot}:${view.cursor}`, count: res.dropped, capacity: res.capacity });
  }
  for (const entry of res.entries) {
    if (entry.seq > view.cursor) added.push({ kind: "entry", key: `${res.boot}:${entry.seq}`, entry });
  }
  return {
    view: {
      boot: res.boot,
      cursor: Math.max(view.cursor, res.next),
      ...capped(view, [...view.lines, ...added], maxLines),
    },
    again: res.more,
  };
}

/** Clear View: empty the page, keep the cursor so only newer lines come in. */
export function clearView(view: LogView): LogView {
  return { ...view, lines: [], trimmed: 0 };
}

/** Lines at least `level`, containing `search` (target, message or fields; case ignored). */
export function visibleLines(lines: readonly LogLine[], level: LogLevel, search: string): LogLine[] {
  const min = levelRank(level);
  const needle = search.trim().toLowerCase();
  return lines.filter((l) => {
    if (l.kind !== "entry") return needle === "";
    if (levelRank(l.entry.level) < min) return false;
    if (!needle) return true;
    const e = l.entry;
    return `${e.target} ${e.message} ${e.fields}`.toLowerCase().includes(needle);
  });
}

/** "14:03:12.123", in UTC like the server's console output. */
export function timeOfDay(ts: number): string {
  return new Date(ts).toISOString().slice(11, 23);
}

/** One line of text laid out like the console: ISO time, level, target, message, fields. */
export function formatEntry(e: LogEntry): string {
  const fields = e.fields ? ` ${e.fields}` : "";
  return `${new Date(e.ts).toISOString()} ${e.level.toUpperCase().padStart(5)} ${e.target}: ${e.message}${fields}`;
}

export function markerText(m: MarkerLine): string {
  if (m.kind === "restart") return "Flick Server restarted. The lines above are from its previous run.";
  const what = m.count === 1 ? "1 line was" : `${m.count} lines were`;
  return `${what} missed: the server keeps only its last ${m.capacity} lines.`;
}

export function lineText(l: LogLine): string {
  return l.kind === "entry" ? formatEntry(l.entry) : `[${markerText(l)}]`;
}

/** What Copy Lines and Download produce: one line each, newline-terminated. */
export function linesToText(lines: readonly LogLine[]): string {
  return lines.length === 0 ? "" : `${lines.map(lineText).join("\n")}\n`;
}

/** flick-server-logs-20261008-140312.txt (UTC). */
export function downloadName(nowMs: number): string {
  const iso = new Date(nowMs).toISOString();
  return `flick-server-logs-${iso.slice(0, 10).replaceAll("-", "")}-${iso.slice(11, 19).replaceAll(":", "")}.txt`;
}

export type LiveState = "live" | "paused" | "offline";

export const LIVE_BADGE: Record<LiveState, { label: string; tone: "plain" | "strong" | "warn" }> = {
  live: { label: "Live", tone: "strong" },
  paused: { label: "Paused", tone: "plain" },
  offline: { label: "Reconnecting", tone: "warn" },
};

export function liveState(paused: boolean, errorText: string | null): LiveState {
  if (paused) return "paused";
  return errorText ? "offline" : "live";
}

/** Shown after the error text while the server does not answer. */
export const STALE_NOTE =
  "The lines below were received earlier. New lines appear again as soon as the server answers.";

export const OFF_TEXT =
  "The server keeps no log lines in memory: FLICKSYNC_LOG_BUFFER is 0. Set it to the number of lines to keep (2000 by default) and restart the server to use this page.";

export function filterNote(logLevel: string): string {
  return `The server records only what its log level allows, now "${logLevel}". Change Log level in the server settings, then apply them, to record more or less.`;
}

/** The list's text when no line is shown; `total` counts the lines held before filtering. */
export function emptyText(total: number): string {
  return total === 0
    ? "No lines yet. New lines appear here as Flick Server writes them."
    : "No line matches the level and search above.";
}

export function summaryText(view: LogView, visible: readonly LogLine[]): string {
  const total = view.lines.filter((l) => l.kind === "entry").length;
  const shown = visible.filter((l) => l.kind === "entry").length;
  const base = shown === total ? `${total} ${total === 1 ? "line" : "lines"}.` : `${shown} of ${total} lines.`;
  return view.trimmed > 0
    ? `${base} Older lines were removed from this view, which keeps the last ${MAX_CLIENT_LINES}.`
    : base;
}

const LOGS_PARAMS: readonly string[] = ["after", "level", "limit"];

export type LogsQueryParse = { ok: true; query: string } | { ok: false; message: string };

/** The panel route's allow-list: `after` and `limit` as digits, `level` as one of the five, each once. */
export function parseLogsQuery(params: URLSearchParams): LogsQueryParse {
  const out = new URLSearchParams();
  for (const [name, value] of params) {
    if (!LOGS_PARAMS.includes(name)) {
      return { ok: false, message: `Unknown query parameter '${name.slice(0, 64)}'. Valid parameters: after, level, limit.` };
    }
    if (out.has(name)) return { ok: false, message: `The query parameter '${name}' is given twice.` };
    const good = name === "level" ? isLogLevel(value) : /^\d{1,20}$/.test(value);
    if (!good) {
      return {
        ok: false,
        message: name === "level" ? `level must be one of ${LOG_LEVELS.join(", ")}.` : `${name} must be a whole number.`,
      };
    }
    out.append(name, value);
  }
  return { ok: true, query: out.toString() };
}

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  LIVE_BADGE,
  LOG_LEVELS,
  type LogLine,
  type LogView,
  MAX_CLIENT_LINES,
  clearView,
  downloadName,
  emptyText,
  emptyView,
  filterNote,
  formatEntry,
  isLogLevel,
  levelRank,
  lineText,
  linesToText,
  liveState,
  logsPath,
  mergeLogs,
  parseLogsQuery,
  summaryText,
  timeOfDay,
  visibleLines,
} from "../src/lib/logs.ts";
import type { LogEntry, LogsResponse } from "../src/lib/types.ts";

const BOOT = 1_790_959_000_000;
const TS = Date.UTC(2026, 9, 8, 14, 3, 12, 123);

const entry = (seq: number, over: Partial<LogEntry> = {}): LogEntry => ({
  seq,
  ts: TS,
  level: "info",
  target: "flicksync::room",
  message: `line ${seq}`,
  fields: "",
  ...over,
});

const answer = (over: Partial<LogsResponse> = {}): LogsResponse => ({
  boot: BOOT,
  capacity: 2000,
  log_level: "info",
  entries: [],
  next: 0,
  more: false,
  dropped: 0,
  ...over,
});

const seqs = (v: LogView) => v.lines.map((l) => (l.kind === "entry" ? l.entry.seq : l.kind));

test("levels are a fixed list ordered by severity", () => {
  assert.deepEqual(LOG_LEVELS, ["error", "warn", "info", "debug", "trace"]);
  for (const l of LOG_LEVELS) assert.equal(isLogLevel(l), true, l);
  for (const l of ["", "WARN", "fatal"]) assert.equal(isLogLevel(l), false, l);
  assert.ok(levelRank("error") > levelRank("warn"));
  assert.ok(levelRank("info") > levelRank("debug"));
  assert.ok(levelRank("debug") > levelRank("trace"));
});

test("the first answer fills the view and moves the cursor", () => {
  const { view, again } = mergeLogs(emptyView(), answer({ entries: [entry(1), entry(2)], next: 2 }));
  assert.equal(again, false);
  assert.equal(view.boot, BOOT);
  assert.equal(view.cursor, 2);
  assert.deepEqual(seqs(view), [1, 2]);
});

test("later answers append, skip what the view already has, and ask again while there is more", () => {
  let v = mergeLogs(emptyView(), answer({ entries: [entry(1), entry(2)], next: 2 })).view;
  const r = mergeLogs(v, answer({ entries: [entry(2), entry(3)], next: 3, more: true }));
  assert.equal(r.again, true);
  v = r.view;
  assert.deepEqual(seqs(v), [1, 2, 3]);
  assert.equal(v.cursor, 3);
  // A page whose lines were all filtered out still moves the cursor.
  assert.equal(mergeLogs(v, answer({ next: 9 })).view.cursor, 9);
});

test("lost lines show as a gap, but not on the first load", () => {
  const first = mergeLogs(emptyView(), answer({ entries: [entry(41)], next: 41, dropped: 40 })).view;
  assert.deepEqual(seqs(first), [41], "older lines simply predate the view");
  const later = mergeLogs(first, answer({ entries: [entry(60)], next: 60, dropped: 18 })).view;
  assert.deepEqual(seqs(later), [41, "gap", 60]);
  assert.equal(lineText(later.lines[1]), "[18 lines were missed: the server keeps only its last 2000 lines.]");
  assert.equal(
    lineText({ kind: "gap", key: "g", count: 1, capacity: 100 }),
    "[1 line was missed: the server keeps only its last 100 lines.]",
  );
});

test("a server restart adds a marker and starts again from the first line", () => {
  const before = mergeLogs(emptyView(), answer({ entries: [entry(1), entry(2)], next: 2 })).view;
  const r = mergeLogs(before, answer({ boot: BOOT + 5_000, entries: [entry(3)], next: 3 }));
  assert.equal(r.again, true);
  assert.equal(r.view.boot, BOOT + 5_000);
  assert.equal(r.view.cursor, 0);
  assert.deepEqual(seqs(r.view), [1, 2, "restart"], "the incomplete page is not used");
  assert.equal(lineText(r.view.lines[2]), "[Flick Server restarted. The lines above are from its previous run.]");
  const after = mergeLogs(r.view, answer({ boot: BOOT + 5_000, entries: [entry(1, { message: "new run" })], next: 1 }))
    .view;
  assert.deepEqual(seqs(after), [1, 2, "restart", 1]);
  assert.equal(new Set(after.lines.map((l) => l.key)).size, 4, "keys stay unique across runs");
});

test("the view keeps at most MAX_CLIENT_LINES, dropping the oldest", () => {
  const many = Array.from({ length: MAX_CLIENT_LINES + 10 }, (_, i) => entry(i + 1));
  const v = mergeLogs(emptyView(), answer({ entries: many, next: many.length })).view;
  assert.equal(v.lines.length, MAX_CLIENT_LINES);
  assert.equal(v.trimmed, 10);
  assert.equal(seqs(v)[0], 11);
  const small = mergeLogs(emptyView(), answer({ entries: [entry(1), entry(2), entry(3)], next: 3 }), 2).view;
  assert.deepEqual(seqs(small), [2, 3]);
});

test("clearing empties the view but keeps the cursor", () => {
  const v = mergeLogs(emptyView(), answer({ entries: [entry(1), entry(2)], next: 2 })).view;
  const c = clearView(v);
  assert.deepEqual(c.lines, []);
  assert.equal(c.cursor, 2);
  assert.equal(c.boot, BOOT);
  assert.deepEqual(seqs(mergeLogs(c, answer({ entries: [entry(3)], next: 3 })).view), [3]);
});

test("level and search filter the lines; markers hide while searching", () => {
  const lines: LogLine[] = [
    { kind: "entry", key: "1", entry: entry(1, { level: "debug", message: "sync correction" }) },
    { kind: "entry", key: "2", entry: entry(2, { level: "warn", message: "admin API: invalid or missing token" }) },
    { kind: "gap", key: "g", count: 3, capacity: 2000 },
    {
      kind: "entry",
      key: "3",
      entry: entry(3, { level: "error", target: "flicksync::app", message: "server error", fields: 'error="Broken pipe"' }),
    },
  ];
  const keys = (ls: LogLine[]) => ls.map((l) => l.key);
  assert.deepEqual(keys(visibleLines(lines, "trace", "")), ["1", "2", "g", "3"]);
  assert.deepEqual(keys(visibleLines(lines, "warn", "")), ["2", "g", "3"]);
  assert.deepEqual(keys(visibleLines(lines, "trace", "  BROKEN ")), ["3"], "fields are searched, case ignored");
  assert.deepEqual(keys(visibleLines(lines, "trace", "flicksync::app")), ["3"], "the target is searched");
  assert.deepEqual(keys(visibleLines(lines, "error", "token")), []);
});

test("lines read like the server's console output", () => {
  assert.equal(timeOfDay(TS), "14:03:12.123");
  assert.equal(
    formatEntry(entry(7, { level: "warn", target: "flicksync::api::admin", message: "admin API: invalid or missing token" })),
    "2026-10-08T14:03:12.123Z  WARN flicksync::api::admin: admin API: invalid or missing token",
  );
  assert.equal(
    formatEntry(entry(8, { message: "room created", fields: 'room_id="R1"' })),
    '2026-10-08T14:03:12.123Z  INFO flicksync::room: room created room_id="R1"',
  );
  assert.equal(formatEntry(entry(9, { level: "error", message: "x" })).slice(25, 30), "ERROR");
  const v = mergeLogs(emptyView(), answer({ entries: [entry(1), entry(2)], next: 2 })).view;
  assert.equal(linesToText(v.lines), `${formatEntry(entry(1))}\n${formatEntry(entry(2))}\n`);
  assert.equal(linesToText([]), "");
  assert.equal(downloadName(TS), "flick-server-logs-20261008-140312.txt");
});

test("status, summary and notes", () => {
  assert.equal(liveState(false, null), "live");
  assert.equal(liveState(false, "Flick Server is unreachable."), "offline");
  assert.equal(liveState(true, "Flick Server is unreachable."), "paused");
  assert.deepEqual(LIVE_BADGE.offline, { label: "Reconnecting", tone: "warn" });
  assert.deepEqual(LIVE_BADGE.live, { label: "Live", tone: "strong" });
  const v = mergeLogs(emptyView(), answer({ entries: [entry(1), entry(2), entry(3)], next: 3 })).view;
  assert.equal(summaryText(v, v.lines), "3 lines.");
  assert.equal(summaryText(v, v.lines.slice(0, 1)), "1 of 3 lines.");
  assert.match(summaryText({ ...v, trimmed: 4 }, v.lines), /keeps the last 5000/);
  assert.equal(emptyText(0), "No lines yet. New lines appear here as Flick Server writes them.");
  assert.equal(emptyText(3), "No line matches the level and search above.");
  assert.match(filterNote("info,flicksync::room=debug"), /"info,flicksync::room=debug"/);
});

test("the panel route forwards only known, well-formed parameters", () => {
  assert.deepEqual(parseLogsQuery(new URLSearchParams("after=41&limit=1000&level=warn")), {
    ok: true,
    query: "after=41&limit=1000&level=warn",
  });
  assert.deepEqual(parseLogsQuery(new URLSearchParams("")), { ok: true, query: "" });
  for (const raw of [
    "after=-1",
    "after=1e3",
    `after=${"9".repeat(21)}`,
    "limit=ten",
    "level=WARN",
    "level=fatal",
    "since=1",
    "after=1&after=2",
    "token=abc",
  ]) {
    const r = parseLogsQuery(new URLSearchParams(raw));
    assert.equal(r.ok, false, raw);
    if (!r.ok) assert.ok(r.message.length > 0 && r.message.length < 200, raw);
  }
  assert.equal(logsPath(41), "/api/logs?after=41&limit=1000");
});

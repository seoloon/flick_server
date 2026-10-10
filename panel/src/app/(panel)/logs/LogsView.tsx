"use client";

import { useLayoutEffect, useMemo, useRef, useState } from "react";

import { Button, CopyButton, Notice, Panel, Pill, Segmented, Switch, TextField } from "@/components/flick/ui";
import {
  LEVEL_LABEL,
  LIVE_BADGE,
  LOG_LEVELS,
  OFF_TEXT,
  STALE_NOTE,
  downloadName,
  emptyText,
  filterNote,
  lineText,
  linesToText,
  liveState,
  summaryText,
  timeOfDay,
  visibleLines,
} from "@/lib/logs";
import type { LogLevel } from "@/lib/types";
import { useLogs } from "@/lib/use-logs";

const LEVEL_OPTIONS = LOG_LEVELS.map((value) => ({ value, label: LEVEL_LABEL[value] }));

function download(text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = downloadName(Date.now());
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

/** The live log list and its toolbar. What to show is decided in src/lib/logs.ts. */
export function LogsView() {
  const logs = useLogs();
  const [level, setLevel] = useState<LogLevel>("trace");
  const [search, setSearch] = useState("");
  const [autoScroll, setAutoScroll] = useState(true);
  const list = useRef<HTMLDivElement>(null);

  const visible = useMemo(() => visibleLines(logs.view.lines, level, search), [logs.view.lines, level, search]);
  const text = useMemo(() => linesToText(visible), [visible]);
  const total = useMemo(() => logs.view.lines.filter((l) => l.kind === "entry").length, [logs.view.lines]);
  const badge = LIVE_BADGE[liveState(logs.paused, logs.errorText)];

  useLayoutEffect(() => {
    const el = list.current;
    if (autoScroll && el) el.scrollTop = el.scrollHeight;
  }, [visible, autoScroll]);

  return (
    <>
      {logs.errorText && (
        <Notice tone="error">
          {logs.errorText} {STALE_NOTE}
        </Notice>
      )}
      {logs.capacity === 0 && <Notice tone="info">{OFF_TEXT}</Notice>}
      <Panel title="Server Log" actions={<Pill tone={badge.tone}>{badge.label}</Pill>}>
        <div className="logs-toolbar">
          <Segmented label="Minimum level" options={LEVEL_OPTIONS} value={level} onChange={setLevel} />
          <TextField label="Search" value={search} onChange={setSearch} />
          <span className="logs-toggle">
            <Switch id="logs-autoscroll" label="Auto-scroll" checked={autoScroll} onChange={setAutoScroll} />
            <label htmlFor="logs-autoscroll">Auto-scroll</label>
          </span>
        </div>
        <div className="row-actions">
          <Button size="sm" icon={logs.paused ? "play" : "pause"} onClick={() => logs.setPaused(!logs.paused)}>
            {logs.paused ? "Resume" : "Pause"}
          </Button>
          <CopyButton text={text} size="sm" variant="glass">
            Copy Lines
          </CopyButton>
          <Button size="sm" icon="download" disabled={!text} onClick={() => download(text)}>
            Download
          </Button>
          <Button size="sm" variant="ghost" icon="trash-2" onClick={logs.clear}>
            Clear View
          </Button>
        </div>
        <div ref={list} className="logs" role="log" aria-label="Server log lines" tabIndex={0}>
          {visible.length === 0 ? (
            <p className="logs__empty">{emptyText(total)}</p>
          ) : (
            visible.map((line) =>
              line.kind === "entry" ? (
                <div key={line.key} className="log" data-level={line.entry.level}>
                  <time className="log__time" dateTime={new Date(line.entry.ts).toISOString()}>
                    {timeOfDay(line.entry.ts)}
                  </time>
                  <span className="log__level">{line.entry.level.toUpperCase()}</span>
                  <span className="log__target" title={line.entry.target}>
                    {line.entry.target}
                  </span>
                  <span className="log__msg">
                    {line.entry.message}
                    {line.entry.fields && <span className="log__fields"> {line.entry.fields}</span>}
                  </span>
                </div>
              ) : (
                <div key={line.key} className="log log--marker">
                  {lineText(line)}
                </div>
              ),
            )
          )}
        </div>
        <p className="muted">
          {summaryText(logs.view, visible)} Times are in UTC.
          {logs.logLevel !== null && ` ${filterNote(logs.logLevel)}`}
        </p>
      </Panel>
    </>
  );
}

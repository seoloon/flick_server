"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import { ADMIN_ERROR_TEXT, adminError } from "./admin-errors";
import { type LogView, MAX_PAGES_PER_POLL, POLL_MS, clearView, emptyView, logsPath, mergeLogs } from "./logs";
import type { LogsResponse } from "./types";

export interface LogsState {
  view: LogView;
  /** From the last answer: lines the server keeps (0 = its buffer is off), and its log level. */
  capacity: number | null;
  logLevel: string | null;
  /** Why the last poll failed, as adminError text; null once the server answers again. */
  errorText: string | null;
  paused: boolean;
  setPaused: (paused: boolean) => void;
  clear: () => void;
}

/**
 * Poll /api/logs every `intervalMs` while the tab is visible and not paused, one request at a
 * time, following the cursor; pages back to back while the server has more. A failed poll keeps
 * the lines and retries on the next tick; an ended session goes to the login page.
 */
export function useLogs(intervalMs = POLL_MS): LogsState {
  const [view, setView] = useState<LogView>(emptyView);
  const viewRef = useRef<LogView>(view);
  const [meta, setMeta] = useState<{ capacity: number; logLevel: string } | null>(null);
  const [errorText, setErrorText] = useState<string | null>(null);
  const [paused, setPaused] = useState(false);

  const apply = useCallback((next: LogView) => {
    viewRef.current = next;
    setView(next);
  }, []);

  useEffect(() => {
    if (paused) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const abort = new AbortController();

    async function poll() {
      if (document.visibilityState === "visible") {
        for (let page = 0; page < MAX_PAGES_PER_POLL && !stopped; page++) {
          let again = false;
          try {
            const res = await fetch(logsPath(viewRef.current.cursor), { cache: "no-store", signal: abort.signal });
            if (res.status === 401) {
              window.location.assign("/login");
              return;
            }
            const body = await res.json().catch(() => null);
            if (stopped) return;
            if (!res.ok) {
              setErrorText(adminError(body).text);
              break;
            }
            const data = body as LogsResponse;
            const merged = mergeLogs(viewRef.current, data);
            apply(merged.view);
            setMeta({ capacity: data.capacity, logLevel: data.log_level });
            setErrorText(null);
            again = merged.again;
          } catch {
            if (stopped) return;
            setErrorText(ADMIN_ERROR_TEXT.UNREACHABLE);
            break;
          }
          if (!again) break;
        }
      }
      if (!stopped) timer = setTimeout(() => void poll(), intervalMs);
    }

    void poll();
    return () => {
      stopped = true;
      abort.abort();
      clearTimeout(timer);
    };
  }, [paused, intervalMs, apply]);

  const clear = useCallback(() => apply(clearView(viewRef.current)), [apply]);

  return {
    view,
    capacity: meta?.capacity ?? null,
    logLevel: meta?.logLevel ?? null,
    errorText,
    paused,
    setPaused,
    clear,
  };
}

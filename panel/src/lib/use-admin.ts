"use client";

import { type Dispatch, type SetStateAction, useCallback, useEffect, useRef, useState } from "react";

import { ADMIN_ERROR_TEXT, type AdminError, type AdminErrorCode, adminError } from "./admin-errors";

export { ADMIN_ERROR_TEXT, type AdminErrorCode, adminError };

export interface AdminState<T> {
  data: T | null;
  /** The error code: the server's (e.g. MODULE_DISABLED) or one of the panel's own. */
  error: string | null;
  /** What to show for `error`: the server's message with its code, or the panel's text. */
  errorText: string | null;
  loading: boolean;
  /** Fetch now, even if a poll is in flight; the newest answer wins. */
  refresh: () => Promise<void>;
  /** Replace the data locally (with an action's answer) ahead of the next poll. */
  mutate: Dispatch<SetStateAction<T | null>>;
}

/**
 * Load a panel API route, then poll it every `intervalMs` (null: load once, and on refresh).
 * Pauses while the tab is hidden, keeps the last good data on a failed refresh, and sends the
 * user to the login page when the session has ended.
 */
export function usePoll<T>(url: string, intervalMs: number | null): AdminState<T> {
  const [data, setData] = useState<T | null>(null);
  const [failure, setFailure] = useState<AdminError | null>(null);
  const [loading, setLoading] = useState(true);
  const seq = useRef(0);
  const inflight = useRef(0);

  const load = useCallback(
    async (force: boolean) => {
      if (!force && inflight.current > 0) return;
      const mine = ++seq.current;
      inflight.current += 1;
      try {
        const res = await fetch(url, { cache: "no-store" });
        if (res.status === 401) {
          window.location.assign("/login");
          return;
        }
        const body = await res.json().catch(() => null);
        if (mine !== seq.current) return; // a newer request or a mutate owns the state
        if (res.ok) {
          setData(body as T);
          setFailure(null);
        } else {
          setFailure(adminError(body));
        }
      } catch {
        if (mine === seq.current) setFailure({ code: "UNREACHABLE", text: ADMIN_ERROR_TEXT.UNREACHABLE });
      } finally {
        inflight.current -= 1;
        setLoading(false);
      }
    },
    [url],
  );

  useEffect(() => {
    void load(true);
    if (intervalMs === null) return;
    const tick = () => {
      if (document.visibilityState === "visible") void load(false);
    };
    const id = setInterval(tick, intervalMs);
    document.addEventListener("visibilitychange", tick);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", tick);
    };
  }, [load, intervalMs]);

  const refresh = useCallback(() => load(true), [load]);
  const mutate = useCallback<Dispatch<SetStateAction<T | null>>>((next) => {
    seq.current += 1;
    setData(next);
  }, []);

  return {
    data,
    error: failure?.code ?? null,
    errorText: failure?.text ?? null,
    loading,
    refresh,
    mutate,
  };
}

/** Poll a FlickSync resource (or, with base "flickdd", a FlickDD one) through the panel's proxy. */
export function useAdmin<T>(resource: string, intervalMs: number, base = "flicksync"): AdminState<T> {
  return usePoll<T>(`/api/${base}/${resource}`, intervalMs);
}

"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import { ADMIN_ERROR_TEXT, type AdminErrorCode, adminError } from "./admin-errors";

export { ADMIN_ERROR_TEXT, type AdminErrorCode, adminError };

export interface AdminState<T> {
  data: T | null;
  /** The error code: the server's (e.g. MODULE_DISABLED) or one of the panel's own. */
  error: string | null;
  /** What to show for `error`: the server's message with its code, or the panel's text. */
  errorText: string | null;
  loading: boolean;
  refresh: () => void;
}

/**
 * Poll a FlickSync admin resource through the panel's proxy. Pauses while the tab is hidden,
 * keeps the last good data on a failed refresh, and sends the user to the login page when the
 * session has ended.
 */
export function useAdmin<T>(
  resource: string,
  intervalMs: number,
  base = "flicksync",
): AdminState<T> {
  const [data, setData] = useState<T | null>(null);
  const [failure, setFailure] = useState<{ code: string; text: string } | null>(null);
  const [loading, setLoading] = useState(true);
  const inflight = useRef(false);

  const load = useCallback(async () => {
    if (inflight.current) return;
    inflight.current = true;
    try {
      const res = await fetch(`/api/${base}/${resource}`, { cache: "no-store" });
      if (res.status === 401) {
        window.location.assign("/login");
        return;
      }
      const body = await res.json().catch(() => null);
      if (res.ok) {
        setData(body as T);
        setFailure(null);
      } else {
        setFailure(adminError(body));
      }
    } catch {
      setFailure({ code: "UNREACHABLE", text: ADMIN_ERROR_TEXT.UNREACHABLE });
    } finally {
      inflight.current = false;
      setLoading(false);
    }
  }, [base, resource]);

  useEffect(() => {
    load();
    const tick = () => {
      if (document.visibilityState === "visible") load();
    };
    const id = setInterval(tick, intervalMs);
    document.addEventListener("visibilitychange", tick);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", tick);
    };
  }, [load, intervalMs]);

  return {
    data,
    error: failure?.code ?? null,
    errorText: failure?.text ?? null,
    loading,
    refresh: load,
  };
}

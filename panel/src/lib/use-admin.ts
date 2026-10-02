"use client";

import { useCallback, useEffect, useRef, useState } from "react";

export type AdminErrorCode =
  | "UNREACHABLE"
  | "ADMIN_TOKEN_MISSING"
  | "ADMIN_API_DISABLED"
  | "ADMIN_TOKEN_REJECTED"
  | "SIGNED_OUT"
  | "UNKNOWN";

export const ADMIN_ERROR_TEXT: Record<AdminErrorCode, string> = {
  UNREACHABLE:
    "FlickSync is unreachable. Check that the server is running and that FLICKSYNC_URL points to it, then try again.",
  ADMIN_TOKEN_MISSING:
    "The panel has no FLICKSYNC_ADMIN_TOKEN. Add it to .env and restart the panel.",
  ADMIN_API_DISABLED:
    "FlickSync's admin API is off. Set FLICKSYNC_ADMIN_TOKEN on the server, then restart it.",
  ADMIN_TOKEN_REJECTED:
    "FlickSync rejected the admin token. Make sure the panel and FlickSync use the same FLICKSYNC_ADMIN_TOKEN.",
  SIGNED_OUT: "Your session has ended. Sign in again.",
  UNKNOWN: "Something went wrong while talking to FlickSync. Try again.",
};

export interface AdminState<T> {
  data: T | null;
  error: AdminErrorCode | null;
  loading: boolean;
  refresh: () => void;
}

/**
 * Poll a FlickSync admin resource through the panel's proxy. Pauses while the tab is hidden,
 * keeps the last good data on a failed refresh, and sends the user to the login page when the
 * session has ended.
 */
export function useAdmin<T>(resource: string, intervalMs: number): AdminState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<AdminErrorCode | null>(null);
  const [loading, setLoading] = useState(true);
  const inflight = useRef(false);

  const load = useCallback(async () => {
    if (inflight.current) return;
    inflight.current = true;
    try {
      const res = await fetch(`/api/flicksync/${resource}`, { cache: "no-store" });
      if (res.status === 401) {
        window.location.assign("/login");
        return;
      }
      const body = await res.json().catch(() => null);
      if (res.ok) {
        setData(body as T);
        setError(null);
      } else {
        const code = body?.error?.code as AdminErrorCode | undefined;
        setError(code && code in ADMIN_ERROR_TEXT ? code : "UNKNOWN");
      }
    } catch {
      setError("UNREACHABLE");
    } finally {
      inflight.current = false;
      setLoading(false);
    }
  }, [resource]);

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

  return { data, error, loading, refresh: load };
}

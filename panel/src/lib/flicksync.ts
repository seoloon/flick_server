import "server-only";

import { panelConfig } from "./config";

export const ADMIN_RESOURCES = ["overview", "invite", "rooms", "stats"] as const;
export type AdminResource = (typeof ADMIN_RESOURCES)[number];

export const DD_RESOURCES = ["overview", "active", "history", "stats"] as const;
export type DdResource = (typeof DD_RESOURCES)[number];

export interface UpstreamResult {
  status: number;
  body: unknown;
}

export type AdminMethod = "GET" | "DELETE" | "POST" | "PUT";

/**
 * Call the server's admin API. The admin token never leaves this process. `body` is sent as
 * JSON. Network problems are reported as 502 with a stable error code the UI can explain.
 */
export async function adminFetch(
  path: string,
  method: AdminMethod = "GET",
  body?: unknown,
): Promise<UpstreamResult> {
  if (!panelConfig.adminToken) {
    return { status: 503, body: { error: { code: "ADMIN_TOKEN_MISSING" } } };
  }
  try {
    const headers: Record<string, string> = { authorization: `Bearer ${panelConfig.adminToken}` };
    if (body !== undefined) headers["content-type"] = "application/json";
    const res = await fetch(`${panelConfig.flicksyncUrl}/admin/v1/${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      cache: "no-store",
      // Start, stop and reload close rooms and cut streams: give them longer than a read.
      signal: AbortSignal.timeout(method === "GET" ? 5_000 : 15_000),
    });
    if (res.status === 204) return { status: 204, body: null };
    if (res.status === 401) {
      return { status: 502, body: { error: { code: "ADMIN_TOKEN_REJECTED" } } };
    }
    const json = await res.json().catch(() => null);
    if (res.status === 404 && !hasErrorCode(json)) {
      // A bare 404 (no error body) means the admin API is off: no PANEL_PASSWORD on the server.
      return { status: 503, body: { error: { code: "ADMIN_API_DISABLED" } } };
    }
    // Error bodies are passed on as they are: their `message` explains the problem.
    return { status: res.status, body: json };
  } catch {
    return { status: 502, body: { error: { code: "UNREACHABLE" } } };
  }
}

function hasErrorCode(body: unknown): boolean {
  return typeof (body as { error?: { code?: unknown } } | null)?.error?.code === "string";
}

/**
 * Call a FlickDD admin route (`dd/<path>`). FlickDD answers 404 DOWNLOAD_NOT_FOUND on GET when
 * it is not running, which is reported as DD_DISABLED with the server's explanation (stopped,
 * or the reason it failed to start).
 */
export async function ddFetch(
  path: string,
  method: "GET" | "DELETE" = "GET",
): Promise<UpstreamResult> {
  const res = await adminFetch(`dd/${path}`, method);
  const error = (res.body as { error?: { code?: string; message?: string } } | null)?.error;
  if (method === "GET" && res.status === 404 && error?.code === "DOWNLOAD_NOT_FOUND") {
    return { status: 503, body: { error: { code: "DD_DISABLED", message: error.message } } };
  }
  return res;
}

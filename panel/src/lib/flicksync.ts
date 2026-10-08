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

/**
 * Call FlickSync's admin API. The admin token never leaves this process.
 * Network problems are reported as 502 with a stable error code the UI can explain.
 */
export async function adminFetch(
  path: string,
  method: "GET" | "DELETE" = "GET",
): Promise<UpstreamResult> {
  if (!panelConfig.adminToken) {
    return { status: 503, body: { error: { code: "ADMIN_TOKEN_MISSING" } } };
  }
  try {
    const res = await fetch(`${panelConfig.flicksyncUrl}/admin/v1/${path}`, {
      method,
      headers: { authorization: `Bearer ${panelConfig.adminToken}` },
      cache: "no-store",
      signal: AbortSignal.timeout(5000),
    });
    if (res.status === 204) return { status: 204, body: null };
    if (res.status === 401) {
      return { status: 502, body: { error: { code: "ADMIN_TOKEN_REJECTED" } } };
    }
    const body = await res.json().catch(() => null);
    if (res.status === 404 && method === "GET" && !hasErrorCode(body)) {
      // A bare 404 (no error body) means the admin API is off: no admin token on the server.
      return { status: 503, body: { error: { code: "ADMIN_API_DISABLED" } } };
    }
    // Error bodies are passed on as they are: their `message` explains the problem.
    return { status: res.status, body };
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

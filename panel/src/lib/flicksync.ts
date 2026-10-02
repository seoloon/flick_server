import "server-only";

import { panelConfig } from "./config";

export const ADMIN_RESOURCES = ["overview", "invite", "rooms", "stats"] as const;
export type AdminResource = (typeof ADMIN_RESOURCES)[number];

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
    if (res.status === 404 && method === "GET") {
      // The admin API answers 404 only when FLICKSYNC_ADMIN_TOKEN is unset on the server.
      return { status: 503, body: { error: { code: "ADMIN_API_DISABLED" } } };
    }
    if (res.status === 401) {
      return { status: 502, body: { error: { code: "ADMIN_TOKEN_REJECTED" } } };
    }
    const body = await res.json().catch(() => null);
    return { status: res.status, body };
  } catch {
    return { status: 502, body: { error: { code: "UNREACHABLE" } } };
  }
}

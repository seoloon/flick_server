// The admin API token, derived from PANEL_PASSWORD exactly like the server (src/admin_token.rs):
// hex(HMAC-SHA256(key = trimmed PANEL_PASSWORD, msg = "flick-admin-api-v1")).
import { createHmac } from "node:crypto";

export const ADMIN_TOKEN_DOMAIN = "flick-admin-api-v1";

/** The server's rule: at least 10 bytes once trimmed, or the admin API stays off. */
export const MIN_PASSWORD_BYTES = 10;

/** The token for `password`, or "" when the server would refuse to derive one. */
export function deriveAdminToken(password: string): string {
  const p = password.trim();
  if (Buffer.byteLength(p, "utf8") < MIN_PASSWORD_BYTES) return "";
  return createHmac("sha256", p).update(ADMIN_TOKEN_DOMAIN).digest("hex");
}

export type AdminTokenSource = "legacy" | "password" | "none";

/**
 * The token the panel sends. A non-blank FLICKSYNC_ADMIN_TOKEN (deprecated, still accepted by the
 * server) wins while it is set, so an old .env keeps working; otherwise the derived token.
 */
export function resolveAdminToken(env: Record<string, string | undefined>): {
  token: string;
  source: AdminTokenSource;
} {
  const legacy = env.FLICKSYNC_ADMIN_TOKEN?.trim() ?? "";
  if (legacy) return { token: legacy, source: "legacy" };
  const token = deriveAdminToken(env.PANEL_PASSWORD ?? "");
  return token ? { token, source: "password" } : { token: "", source: "none" };
}

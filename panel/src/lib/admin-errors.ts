/** Error codes the panel produces itself (its proxy and its fetch helpers). */
export type AdminErrorCode =
  | "UNREACHABLE"
  | "ADMIN_TOKEN_MISSING"
  | "ADMIN_API_DISABLED"
  | "ADMIN_TOKEN_REJECTED"
  | "DD_DISABLED"
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
  DD_DISABLED:
    "FlickDD is off. Set FLICKDD_ENABLED=true and a backend on the server, then restart it.",
  SIGNED_OUT: "Your session has ended. Sign in again.",
  UNKNOWN: "Something went wrong while talking to FlickSync. Try again.",
};

export interface AdminError {
  /** The server's code (e.g. SETTINGS_INVALID) or one of the panel's own. */
  code: string;
  /** What to show: the server's explanation when it gave one, else the panel's text. */
  text: string;
}

/** Read `{"error": {"code", "message"}}`, as sent by the server and by the panel's proxy. */
export function adminError(body: unknown): AdminError {
  const e = (body as { error?: { code?: unknown; message?: unknown } } | null)?.error;
  const code = typeof e?.code === "string" && e.code ? e.code : "UNKNOWN";
  const message = typeof e?.message === "string" ? e.message.trim() : "";
  if (message) return { code, text: `${message} (${code})` };
  if (code in ADMIN_ERROR_TEXT) return { code, text: ADMIN_ERROR_TEXT[code as AdminErrorCode] };
  return { code, text: `${ADMIN_ERROR_TEXT.UNKNOWN} (${code})` };
}

/** Error codes the panel produces itself (its proxy and its fetch helpers). */
export type AdminErrorCode =
  | "UNREACHABLE"
  | "ADMIN_TOKEN_MISSING"
  | "ADMIN_API_DISABLED"
  | "ADMIN_TOKEN_REJECTED"
  | "DD_DISABLED"
  | "SIGNED_OUT"
  | "FORBIDDEN"
  | "NOT_FOUND"
  | "UNKNOWN";

export const ADMIN_ERROR_TEXT: Record<AdminErrorCode, string> = {
  UNREACHABLE:
    "Flick Server is unreachable. Check that it is running and that FLICKSYNC_URL points to it, then try again.",
  ADMIN_TOKEN_MISSING:
    "The panel has no admin token: PANEL_PASSWORD is missing or shorter than 10 characters. Set it in .env and restart the panel.",
  ADMIN_API_DISABLED:
    "The server's admin API is off. Give the server the same PANEL_PASSWORD as the panel (10+ characters, same .env), then restart it.",
  ADMIN_TOKEN_REJECTED:
    "The server rejected the panel's admin token. The panel and the server must share the same PANEL_PASSWORD (and, if you still set it, the same FLICKSYNC_ADMIN_TOKEN). Fix .env, then restart both.",
  DD_DISABLED: "FlickDD is not running. Start it from the Overview or the FlickDD page.",
  SIGNED_OUT: "Your session has ended. Sign in again.",
  FORBIDDEN:
    "The request was refused because it did not come from this panel's page. Reload the page and try again.",
  NOT_FOUND: "The panel does not know this request. Reload the page and try again.",
  UNKNOWN: "Something went wrong while talking to Flick Server. Try again.",
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

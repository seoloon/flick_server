// Start-up checks of the panel's environment, shared by start.mjs and the tests.
// Plain JavaScript: start.mjs runs it before Next.js, also in the Docker image.

const TRUTHY = ["1", "true", "yes", "on"];

/** ENABLE_WEB_PANEL is the master switch. */
export function panelEnabled(env) {
  return TRUTHY.includes(String(env.ENABLE_WEB_PANEL ?? "").trim().toLowerCase());
}

/**
 * `problems` stop the panel; `notes` are printed and the panel starts. The password is trimmed
 * like the server does before it derives the admin token; 10 characters always make 10 bytes.
 */
export function checkPanelEnv(env) {
  const problems = [];
  const notes = [];
  const password = String(env.PANEL_PASSWORD ?? "").trim();
  if ([...password].length < 10) {
    problems.push(
      "PANEL_PASSWORD must be set and at least 10 characters, not counting leading and trailing spaces (e.g. `openssl rand -base64 18`). The panel signs in with it and derives the server's admin token from it.",
    );
  }
  const legacy = String(env.FLICKSYNC_ADMIN_TOKEN ?? "").trim();
  if (legacy && legacy.length < 16) {
    problems.push(
      "FLICKSYNC_ADMIN_TOKEN is deprecated: remove it from .env, the admin token now comes from PANEL_PASSWORD. If you keep it, it must be at least 16 characters, as on the server.",
    );
  } else if (legacy) {
    notes.push(
      "FLICKSYNC_ADMIN_TOKEN is deprecated. The panel sends it while it is set; remove it from .env (for the panel and the server) to use the token derived from PANEL_PASSWORD.",
    );
  }
  return { problems, notes };
}

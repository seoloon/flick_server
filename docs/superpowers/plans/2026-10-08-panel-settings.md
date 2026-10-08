# Flick Panel manages the server: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Flick Panel starts, stops and reloads FlickSync and FlickDD, edits the server, FlickSync and FlickDD settings through the admin API, and derives its admin token from `PANEL_PASSWORD` instead of requiring `FLICKSYNC_ADMIN_TOKEN`.

**Architecture:** Every decision (what a control shows, what Save sends, which actions a module offers, what to say after a save, the admin token) lives in pure TypeScript modules under `panel/src/lib/` that `node --test` loads directly. Thin React components render them with the existing Flick design system; thin Next route handlers under `panel/src/app/api/` relay to the server's `/admin/v1/*` with the session and same-origin checks the panel already uses.

**Tech Stack:** Next.js 16 (App Router), React 19, TypeScript 5.9 (`tsc --noEmit`), Node 22+ built-in test runner with type stripping (`node --test "test/*.test.ts"`), Node `crypto`. No new dependency.

**Spec:** `docs/superpowers/specs/2026-10-08-panel-settings-design.md` (server API: `docs/admin-api.md`)

## Global Constraints

Every task's requirements include these:

- No new runtime or dev dependency: `panel/package.json` and `panel/package-lock.json` stay unchanged.
- Admin token = `hex(HMAC-SHA256(key = PANEL_PASSWORD.trim(), msg = "flick-admin-api-v1"))`, empty when the trimmed password has fewer than 10 bytes; vector: `correct horse battery staple` -> `602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011`. A non-blank `FLICKSYNC_ADMIN_TOKEN` is sent instead while it is set.
- Every server or panel error is shown as `<message> (<CODE>)` through `adminError` in `panel/src/lib/admin-errors.ts` (panel codes without a message use its text table).
- Pure logic lives in `panel/src/lib/*.ts` files with no React, Next or `server-only` import. Between those files, value imports use the `.ts` extension (`import { formatDuration } from "./format.ts"`) so `node --test` can load them; type imports use `import type`.
- Panel API routes: `hasSession()` on every route, plus `sameOrigin()` on every mutation; ids, actions and scopes checked against fixed lists before anything is forwarded; answers `Cache-Control: no-store`.
- Secret values are never rendered and never come back from the server; secret inputs use `autoComplete="new-password"` and exist only after the user clicks Replace / Set Value.
- Save never reloads a module by itself; Stop and Reload of a running module always ask for confirmation.
- UI copy in English following the design system's rules: Title Case for navigation and buttons, sentence case elsewhere, British spelling, no emoji, no exclamation marks.
- Styling only with the tokens in `tokens.css` and the classes of `flick-components.css`; new layout CSS goes in `panel/src/styles/panel.css`; no accent colour.
- After every task, from `panel/`: `npm test` (0 failures) and `npm run typecheck` (no output) pass; tasks that touch `src/app`, `src/components` or `src/styles` also pass `npm run build`.
- Commit messages end with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

Inputs and conditions the spec implies that a person is likely to hit; each has a test in the task that owns the code:

1. The browser auto-fills or the user leaves a secret field empty: nothing is sent for it (Task 2: `buildPatch` with an empty secret edit and with no edits).
2. The user types the current value back, or the environment spells a switch `yes` while the panel sends `true`: no change is sent and Save stays disabled (Task 2: `setEdit` round trip and boolean spellings).
3. The server refuses a save with a message that names a setting whose name is a prefix of another (`..._DRIFT_SOFT` vs `..._DRIFT_SOFTER`): only the named field is flagged and every edit is kept (Task 2: `namedFields` word boundaries; Task 6 keeps `edits` on failure).
4. Reset on a value that comes from the environment: `null` would change nothing on the server, so nothing is sent (Task 2: `resetEdit` and `buildPatch` drop it).
5. A Start that fails answers `200` with `state: "failed"`: the card must switch to Failed to start with the message, not look like a success (Task 3: `mergeStatus`, `stateBadge`, `availableActions` for `failed`).

Also covered: a password with surrounding spaces gives the server's token (Task 1); a PUT body with a non-string value, a bad name or nothing in it is refused by the panel before it reaches the server (Task 2).

## File Structure

| File | Responsibility |
|---|---|
| `panel/src/lib/admin-token.ts` (new) | Token derivation and choice between derived and legacy token |
| `panel/env-check.mjs` + `panel/env-check.d.mts` (new) | Start-up checks shared by `start.mjs` and the tests |
| `panel/start.mjs`, `panel/src/lib/config.ts`, `Dockerfile` | Use the checks and the derived token; ship `env-check.mjs` in the image |
| `panel/src/lib/admin-errors.ts` | Panel error texts for the derived token, `FORBIDDEN`, `NOT_FOUND` |
| `panel/src/lib/types.ts` | Module status, settings view, `Overview.running` |
| `panel/src/lib/settings-form.ts` (new) | Scopes, labels, sources, edits, patch, hints, PUT body check |
| `panel/src/lib/module-status.ts` (new) | Module ids, actions, badges, confirmations, follow-ups |
| `panel/src/lib/flicksync.ts`, `panel/src/lib/proxy.ts` (new) | `adminFetch` with POST / PUT; route response helpers |
| `panel/src/app/api/modules/**`, `panel/src/app/api/settings/**` (new) | Panel proxy routes |
| `panel/src/lib/use-admin.ts`, `panel/src/lib/use-modules.ts` (new) | `usePoll`, `useAdmin`, `useModules` |
| `panel/src/components/ModuleCard.tsx`, `ModuleGate.tsx` (new) | Module card with actions; gate for module pages |
| `panel/src/app/(panel)/page.tsx`, `ModulesOverview.tsx` (new) | Overview with Server card and module cards |
| `panel/src/app/(panel)/settings/**` (new) | Settings pages, tabs, form, field rows |
| `panel/src/components/flick/ui.tsx`, `icons.tsx`, `PanelSidebar.tsx`, `panel/src/lib/modules.ts` | `Switch`, four icons, Settings nav entry, `settingsHref` |
| `panel/test/*.test.ts` | Unit tests |

---

### Task 1: Admin token derived from `PANEL_PASSWORD`

**Files:**
- Create: `panel/src/lib/admin-token.ts`, `panel/env-check.mjs`, `panel/env-check.d.mts`, `panel/test/admin-token.test.ts`, `panel/test/env-check.test.ts`
- Modify: `panel/start.mjs`, `panel/src/lib/config.ts`, `panel/src/lib/admin-errors.ts`, `panel/test/lib.test.ts`, `Dockerfile:31`, `panel/README.md`, `README.md:99-103`, `TECHNICAL.md:170-178`, `docs/deployment.md:38-39` and `:200-202`, `.env.example:49-53`

**Interfaces:**
- Produces:
  - `deriveAdminToken(password: string): string` and `resolveAdminToken(env: Record<string, string | undefined>): { token: string; source: AdminTokenSource }` with `type AdminTokenSource = "legacy" | "password" | "none"` (`src/lib/admin-token.ts`)
  - `panelEnabled(env): boolean`, `checkPanelEnv(env): { problems: string[]; notes: string[] }` (`env-check.mjs`)
  - `panelConfig.adminToken` now resolved by `resolveAdminToken(process.env)`
  - `AdminErrorCode` gains `"FORBIDDEN" | "NOT_FOUND"` (used by Task 4)

- [ ] **Step 1: Write the failing tests**

Create `panel/test/admin-token.test.ts`:

```ts
import assert from "node:assert/strict";
import { test } from "node:test";

import { deriveAdminToken, resolveAdminToken } from "../src/lib/admin-token.ts";

const PW = "correct horse battery staple";
// Same vector as `the_derived_token_matches_the_value_the_panel_computes` in src/admin_token.rs.
const TOKEN = "602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011";

test("the admin token matches the server's vector", () => {
  assert.equal(deriveAdminToken(PW), TOKEN);
});

test("the password is trimmed first, like on the server", () => {
  assert.equal(deriveAdminToken(`  ${PW}\n`), TOKEN);
});

test("a password under 10 bytes gives no token", () => {
  assert.equal(deriveAdminToken(""), "");
  assert.equal(deriveAdminToken("123456789"), "");
  assert.equal(deriveAdminToken("  123456789  "), "");
  assert.notEqual(deriveAdminToken("1234567890"), "");
});

test("the length is counted in bytes, as the server does", () => {
  // 5 characters, 10 bytes in UTF-8.
  assert.notEqual(deriveAdminToken("ééééé"), "");
});

test("the legacy FLICKSYNC_ADMIN_TOKEN wins while it is set", () => {
  assert.deepEqual(
    resolveAdminToken({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: "  legacy-token-0123456789 " }),
    { token: "legacy-token-0123456789", source: "legacy" },
  );
  assert.deepEqual(resolveAdminToken({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: "   " }), {
    token: TOKEN,
    source: "password",
  });
  assert.deepEqual(resolveAdminToken({ PANEL_PASSWORD: PW }), { token: TOKEN, source: "password" });
  assert.deepEqual(resolveAdminToken({ PANEL_PASSWORD: "short" }), { token: "", source: "none" });
  assert.deepEqual(resolveAdminToken({}), { token: "", source: "none" });
});
```

Create `panel/test/env-check.test.ts`:

```ts
import assert from "node:assert/strict";
import { test } from "node:test";

import { checkPanelEnv, panelEnabled } from "../env-check.mjs";

const PW = "correct horse battery staple";

test("the panel starts only when ENABLE_WEB_PANEL is truthy", () => {
  for (const v of ["true", " TRUE ", "1", "yes", "on"]) {
    assert.equal(panelEnabled({ ENABLE_WEB_PANEL: v }), true, v);
  }
  for (const v of [undefined, "", "false", "0", "nope"]) {
    assert.equal(panelEnabled({ ENABLE_WEB_PANEL: v }), false, String(v));
  }
});

test("a missing, short or space-padded short password is refused", () => {
  for (const env of [{}, { PANEL_PASSWORD: "short" }, { PANEL_PASSWORD: "   123456789   " }]) {
    const { problems } = checkPanelEnv(env);
    assert.equal(problems.length, 1, JSON.stringify(env));
    assert.match(problems[0], /PANEL_PASSWORD/);
  }
});

test("a good password alone is enough: no FLICKSYNC_ADMIN_TOKEN needed", () => {
  assert.deepEqual(checkPanelEnv({ PANEL_PASSWORD: PW }), { problems: [], notes: [] });
  assert.deepEqual(checkPanelEnv({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: "  " }), { problems: [], notes: [] });
});

test("a legacy token is accepted with a deprecation note; a short one is refused", () => {
  const ok = checkPanelEnv({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: "legacy-token-0123456789" });
  assert.deepEqual(ok.problems, []);
  assert.equal(ok.notes.length, 1);
  assert.match(ok.notes[0], /deprecated/);

  const short = checkPanelEnv({ PANEL_PASSWORD: PW, FLICKSYNC_ADMIN_TOKEN: "short" });
  assert.equal(short.problems.length, 1);
  assert.match(short.problems[0], /FLICKSYNC_ADMIN_TOKEN/);
});
```

In `panel/test/lib.test.ts`, replace the line `import { adminError } from "../src/lib/admin-errors.ts";` with:

```ts
import { ADMIN_ERROR_TEXT, adminError } from "../src/lib/admin-errors.ts";
```

and append at the end of the file:

```ts
test("the panel's own texts point to PANEL_PASSWORD, not to the old admin token", () => {
  for (const code of ["ADMIN_TOKEN_MISSING", "ADMIN_API_DISABLED", "ADMIN_TOKEN_REJECTED"] as const) {
    assert.match(ADMIN_ERROR_TEXT[code], /PANEL_PASSWORD/, code);
  }
  assert.doesNotMatch(ADMIN_ERROR_TEXT.ADMIN_TOKEN_MISSING, /FLICKSYNC_ADMIN_TOKEN/);
  assert.doesNotMatch(ADMIN_ERROR_TEXT.ADMIN_API_DISABLED, /FLICKSYNC_ADMIN_TOKEN/);
});

test("the panel's route refusals have their own text", () => {
  assert.match(adminError({ error: { code: "FORBIDDEN" } }).text, /this panel/);
  assert.match(adminError({ error: { code: "NOT_FOUND" } }).text, /Reload the page/);
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd panel; npm test`
Expected: FAIL. `admin-token.test.ts` and `env-check.test.ts` fail with `ERR_MODULE_NOT_FOUND`; in `lib.test.ts` the two new tests fail (`ADMIN_TOKEN_MISSING` text does not mention `PANEL_PASSWORD`, `FORBIDDEN` falls back to the generic text).

- [ ] **Step 3: Implement the token derivation**

Create `panel/src/lib/admin-token.ts`:

```ts
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
```

Replace the whole content of `panel/src/lib/config.ts` with:

```ts
import "server-only";

import { resolveAdminToken } from "./admin-token";

const admin = resolveAdminToken(process.env);

export const panelConfig = {
  /** Where Flick Server listens, as seen from this process. */
  flicksyncUrl: (process.env.FLICKSYNC_URL || "http://localhost:8787").replace(/\/+$/, ""),
  /** Derived from PANEL_PASSWORD, or the deprecated FLICKSYNC_ADMIN_TOKEN while it is set. */
  adminToken: admin.token,
  adminTokenSource: admin.source,
  password: process.env.PANEL_PASSWORD ?? "",
};
```

- [ ] **Step 4: Implement the start-up checks**

Create `panel/env-check.mjs`:

```js
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
```

Create `panel/env-check.d.mts`:

```ts
export function panelEnabled(env: Record<string, string | undefined>): boolean;
export function checkPanelEnv(env: Record<string, string | undefined>): {
  problems: string[];
  notes: string[];
};
```

In `panel/start.mjs`, replace lines 1 to 27 (from `// Entry point of the panel` through the closing `}` of `if (problems.length) {...}`) with:

```js
// Entry point of the panel (`npm start`, Docker CMD).
//
//   ENABLE_WEB_PANEL=false (default)  -> log a line and exit 0: nothing listens on the port.
//   ENABLE_WEB_PANEL=true             -> start the Next.js server on PORT (default 3000).
//
// Refuses to start without a real password: the panel signs in with it, derives the server's
// admin token from it, and can then read the invitation (signing key) and stop modules.
import { checkPanelEnv, panelEnabled } from "./env-check.mjs";

if (!panelEnabled(process.env)) {
  console.log("Flick Panel is disabled (set ENABLE_WEB_PANEL=true to enable it).");
  process.exit(0);
}

const { problems, notes } = checkPanelEnv(process.env);
for (const note of notes) console.warn(`Flick Panel: ${note}`);
if (problems.length) {
  console.error("Flick Panel cannot start:\n - " + problems.join("\n - "));
  process.exit(2);
}
```

The rest of `start.mjs` (from `process.env.PORT ||= "3000";` on) is unchanged.

In `Dockerfile`, after the line `COPY --from=panel-build /app/start.mjs ./start.mjs` add:

```dockerfile
COPY --from=panel-build /app/env-check.mjs ./env-check.mjs
```

- [ ] **Step 5: Reword the panel's error texts**

Replace the whole content of `panel/src/lib/admin-errors.ts` with:

```ts
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
```

- [ ] **Step 6: Run the tests and the type check**

Run: `cd panel; npm test; npm run typecheck`
Expected: every test passes (`ℹ fail 0`), `tsc --noEmit` prints nothing.

- [ ] **Step 7: Check `start.mjs` by hand**

Run (Git Bash, from `panel/`):

```bash
ENABLE_WEB_PANEL=true PANEL_PASSWORD=short node start.mjs; echo "exit $?"
ENABLE_WEB_PANEL=true PANEL_PASSWORD='correct horse battery staple' FLICKSYNC_ADMIN_TOKEN=short node start.mjs; echo "exit $?"
ENABLE_WEB_PANEL=false node start.mjs; echo "exit $?"
```

Expected: the first two print `Flick Panel cannot start:` with the `PANEL_PASSWORD` and the `FLICKSYNC_ADMIN_TOKEN` problem and `exit 2`; the third prints `Flick Panel is disabled ...` and `exit 0`.

- [ ] **Step 8: Update the docs that say the panel needs `FLICKSYNC_ADMIN_TOKEN`**

`panel/README.md`, replace the block from `Both settings live in the repository's` down to the bullet ending `It refuses to start without a real password and admin token.` with:

````markdown
Both settings live in the repository's `.env` (see `.env.example`):

```
ENABLE_WEB_PANEL=true
PANEL_PASSWORD=<10+ characters>          # sign-in to the panel, and the source of the admin token
# FLICKSYNC_PUBLIC_URL=https://sync.example.com   # so the invitation carries your real address
```

Generate the password with `openssl rand -base64 18`.

The panel talks to Flick Server's [admin API](../docs/admin-api.md) with a token derived from the password:
`hex(HMAC-SHA256(PANEL_PASSWORD, "flick-admin-api-v1"))`, computed on the trimmed password. The server computes the
same token from the same `.env`, so there is no second secret to share. `FLICKSYNC_ADMIN_TOKEN` is deprecated: while it
is set the panel sends it instead (the server still accepts it) and logs a note at start; remove it to use the derived
token.

* `ENABLE_WEB_PANEL=false` (the default): the process logs one line and exits `0`. Nothing listens.
* `true`: the panel serves on port **3000** (`PORT` changes it; compose only exposes it to the other containers and the reverse proxy). It refuses to start without a password of at least 10 characters (leading and trailing spaces do not count).
````

In the same file, replace `ENABLE_WEB_PANEL=true PANEL_PASSWORD=... FLICKSYNC_ADMIN_TOKEN=... FLICKSYNC_URL=http://localhost:8787 npm start` with `ENABLE_WEB_PANEL=true PANEL_PASSWORD=... FLICKSYNC_URL=http://localhost:8787 npm start`, and replace the two configuration rows

```
| `PANEL_PASSWORD` | none | Password of the single panel account |
| `FLICKSYNC_ADMIN_TOKEN` | none | Bearer token of FlickSync's [admin API](../docs/admin-api.md); must equal the value set on FlickSync |
```

with

```
| `PANEL_PASSWORD` | none | Password of the single panel account; the admin token is derived from it |
| `FLICKSYNC_ADMIN_TOKEN` | none | Deprecated. When set, sent instead of the derived token; it must then equal the server's value (16+ characters) |
```

`README.md`, replace

````markdown
Want the panel? Add three lines to `.env`:

```sh
ENABLE_WEB_PANEL=true
PANEL_PASSWORD=<10+ characters>
FLICKSYNC_ADMIN_TOKEN=<16+ characters>
```
````

with

````markdown
Want the panel? Add two lines to `.env`:

```sh
ENABLE_WEB_PANEL=true
PANEL_PASSWORD=<10+ characters>
```
````

`TECHNICAL.md`, replace

````markdown
```sh
ENABLE_WEB_PANEL=true
PANEL_PASSWORD=<10+ characters>        # openssl rand -base64 18
FLICKSYNC_ADMIN_TOKEN=<16+ characters> # openssl rand -base64 32
```

`FLICKSYNC_ADMIN_TOKEN` is a shared secret between the panel and FlickSync, so
it must be the same on both. Without it FlickSync keeps its admin API switched
off. The panel refuses to start with a weak password or token.
````

with

````markdown
```sh
ENABLE_WEB_PANEL=true
PANEL_PASSWORD=<10+ characters>        # openssl rand -base64 18
```

`PANEL_PASSWORD` is the only secret: the panel and FlickSync both derive the
admin API token from it, so both containers must read the same `.env` (the
compose file does). Without it FlickSync keeps its admin API switched off. The
panel refuses to start with a password under 10 characters.
`FLICKSYNC_ADMIN_TOKEN` is deprecated and can be removed.
````

`docs/deployment.md`, replace

```
`ENABLE_WEB_PANEL=true` in `.env`, plus `PANEL_PASSWORD` and `FLICKSYNC_ADMIN_TOKEN` (still needed by the current
panel), starts the `panel` service on
```

with

```
`ENABLE_WEB_PANEL=true` in `.env`, plus `PANEL_PASSWORD` (the panel and FlickSync derive the admin token from it),
starts the `panel` service on
```

and replace

```
* FlickSync now derives its admin token from `PANEL_PASSWORD` (see [admin-api.md](admin-api.md)) and treats
  `FLICKSYNC_ADMIN_TOKEN` as deprecated. **Keep `FLICKSYNC_ADMIN_TOKEN` set while you run the panel**: the current
  panel still sends it, until the panel is updated.
```

with

```
* FlickSync and the panel derive the admin token from `PANEL_PASSWORD` (see [admin-api.md](admin-api.md)).
  `FLICKSYNC_ADMIN_TOKEN` is deprecated: remove it from `.env`. While it is still set, the panel sends it and
  FlickSync accepts it, so an older `.env` keeps working.
```

`.env.example`, replace

```
# Secret shared by the panel and FlickSync's admin API. Still required when the panel is enabled:
# the current panel sends it. FlickSync itself now derives its admin token from PANEL_PASSWORD
# (docs/admin-api.md) and treats this one as deprecated; it can go once the panel is updated.
# 16+ characters: openssl rand -base64 32
#FLICKSYNC_ADMIN_TOKEN=
```

with

```
# The panel and FlickSync derive the admin API token from PANEL_PASSWORD (docs/admin-api.md).
# Deprecated, leave unset: a separate shared token. While set (16+ characters) the panel sends it
# instead and FlickSync still accepts it.
#FLICKSYNC_ADMIN_TOKEN=
```

Check: `git grep -n "FLICKSYNC_ADMIN_TOKEN" -- README.md TECHNICAL.md docs/deployment.md panel/README.md panel/src panel/start.mjs .env.example`
Expected: only the deprecation mentions written above (README.md has none).

- [ ] **Step 9: Commit**

```bash
git add panel/src/lib/admin-token.ts panel/src/lib/config.ts panel/src/lib/admin-errors.ts panel/env-check.mjs panel/env-check.d.mts panel/start.mjs panel/test Dockerfile panel/README.md README.md TECHNICAL.md docs/deployment.md .env.example
git commit -m "feat(panel): derive the admin token from PANEL_PASSWORD

The panel computes hex(HMAC-SHA256(PANEL_PASSWORD, \"flick-admin-api-v1\"))
on the trimmed password, like the server, so FLICKSYNC_ADMIN_TOKEN is no
longer needed. A legacy token that is still set is sent instead, with a
deprecation note at start. Start-up checks move to env-check.mjs so they
can be tested; error texts point to PANEL_PASSWORD.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Settings form logic

**Files:**
- Create: `panel/src/lib/settings-form.ts`, `panel/test/settings-form.test.ts`
- Modify: `panel/src/lib/types.ts`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (`src/lib/types.ts`): `ModuleId`, `ModuleState`, `ModuleStatus`, `ModulesResponse`, `SettingsScope`, `FieldKind`, `FieldSource`, `SettingField`, `SettingsView`; `Overview.running: boolean`.
- Produces (`src/lib/settings-form.ts`):
  - `SETTINGS_SCOPES: readonly SettingsScope[]`, `SCOPE_LABEL`, `SCOPE_LEAD: Record<SettingsScope, string>`, `isSettingsScope(s: string): s is SettingsScope`
  - `SWITCH_FIELDS`, `visibleFields(view: SettingsView): SettingField[]`
  - `type Edits = Readonly<Record<string, string | null>>`
  - `parseBool(v: string | null | undefined): boolean | null`, `fieldLabel(name: string): string`, `SOURCE_LABEL`
  - `setEdit(edits, field, value: string): Edits`, `resetEdit(edits, field): Edits`, `undoEdit(edits, name: string): Edits`
  - `buildPatch(view, edits): Record<string, string | null>`
  - `interface FieldDisplay { value: string; changed: boolean; removing: boolean }`, `fieldDisplay(field, edits): FieldDisplay`, `fieldHint(field, d: FieldDisplay): string`
  - `choiceOptions(field, current: string): string[]`, `namedFields(text: string, names: readonly string[]): string[]`
  - `MAX_PATCH_CHARS = 65536`, `type PatchParse`, `parsePatchBody(raw: string): PatchParse`

- [ ] **Step 1: Add the API types**

In `panel/src/lib/types.ts`, inside `interface Overview`, add after `uptime_secs: number;`:

```ts
  /** False while FlickSync is stopped; its counters are then zero. */
  running: boolean;
```

and append at the end of the file:

```ts
// Module lifecycle and settings (/admin/v1/modules, /admin/v1/settings).

export type ModuleId = "flicksync" | "flickdd";
export type ModuleState = "stopped" | "running" | "failed";

export interface ModuleStatus {
  id: ModuleId;
  state: ModuleState;
  /** Why the module failed to start; null otherwise. */
  message: string | null;
  /** The persisted on/off switch. */
  enabled: boolean;
  /** Running with older settings than the ones saved. */
  pending_reload: boolean;
  /** Start time (ms since the epoch) while running, else null. */
  since: number | null;
}

export interface ModulesResponse {
  modules: ModuleStatus[];
}

export type SettingsScope = "server" | "flicksync" | "flickdd";
export type FieldKind = "bool" | "int" | "float" | "text" | "choice" | "list" | "secret";
export type FieldSource = "panel" | "environment" | "default";

export interface SettingField {
  name: string;
  kind: FieldKind;
  secret: boolean;
  /** Current value; always null for a secret. */
  value: string | null;
  /** A value exists in the panel or the environment. */
  set: boolean;
  source: FieldSource;
  /** Code default; always null for a secret. */
  default: string | null;
  /** Present for `choice` fields only. */
  choices?: string[];
}

export interface SettingsView {
  scope: SettingsScope;
  revision: number;
  fields: SettingField[];
}
```

- [ ] **Step 2: Write the failing tests**

Create `panel/test/settings-form.test.ts`:

```ts
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  type Edits,
  MAX_PATCH_CHARS,
  buildPatch,
  choiceOptions,
  fieldDisplay,
  fieldHint,
  fieldLabel,
  isSettingsScope,
  namedFields,
  parseBool,
  parsePatchBody,
  resetEdit,
  setEdit,
  undoEdit,
  visibleFields,
} from "../src/lib/settings-form.ts";
import type { SettingField, SettingsView } from "../src/lib/types.ts";

function field(over: Partial<SettingField> & Pick<SettingField, "name" | "kind">): SettingField {
  const secret = over.kind === "secret";
  return {
    secret,
    value: secret ? null : "",
    set: false,
    source: "default",
    default: secret ? null : "",
    ...over,
  };
}

const VIEW: SettingsView = {
  scope: "flickdd",
  revision: 3,
  fields: [
    field({ name: "FLICKDD_ENABLED", kind: "bool", value: "true", default: "false", source: "panel", set: true }),
    field({ name: "FLICKDD_MAX_PARALLEL", kind: "int", value: "10", default: "10" }),
    field({ name: "FLICKDD_MAX_GLOBAL", kind: "int", value: "50", default: "100", source: "panel", set: true }),
    field({ name: "FLICKDD_JELLYFIN_URL", kind: "text", value: "http://jellyfin:8096", source: "environment", set: true }),
    field({ name: "FLICKDD_JELLYFIN_API_KEY", kind: "secret", source: "environment", set: true }),
    field({ name: "FLICKDD_PLEX_TOKEN", kind: "secret", source: "panel", set: true }),
  ],
};

const byName = (name: string): SettingField => {
  const f = VIEW.fields.find((x) => x.name === name);
  if (!f) throw new Error(`no field ${name}`);
  return f;
};

test("scopes are a fixed list", () => {
  for (const s of ["server", "flicksync", "flickdd"]) assert.equal(isSettingsScope(s), true, s);
  for (const s of ["", "modules", "Server", "../server"]) assert.equal(isSettingsScope(s), false, s);
});

test("labels come from the variable name", () => {
  assert.equal(fieldLabel("FLICKSYNC_PUBLIC_URL"), "Public URL");
  assert.equal(fieldLabel("FLICKSYNC_MAX_ROOM_SIZE"), "Max room size");
  assert.equal(fieldLabel("FLICKDD_JELLYFIN_API_KEY"), "Jellyfin API key");
  assert.equal(fieldLabel("FLICKSYNC_WS_PING_INTERVAL"), "WS ping interval");
  assert.equal(fieldLabel("FLICKSYNC_SYNC_SEEK_COOLDOWN_MS"), "Sync seek cooldown ms");
  assert.equal(fieldLabel("FLICKSYNC_CORS_ORIGINS"), "CORS origins");
});

test("the module switches are left to Start and Stop", () => {
  assert.deepEqual(
    visibleFields(VIEW).map((f) => f.name),
    ["FLICKDD_MAX_PARALLEL", "FLICKDD_MAX_GLOBAL", "FLICKDD_JELLYFIN_URL", "FLICKDD_JELLYFIN_API_KEY", "FLICKDD_PLEX_TOKEN"],
  );
});

test("switch values are read like the server reads them", () => {
  for (const v of ["1", "true", "YES", " on "]) assert.equal(parseBool(v), true, v);
  for (const v of ["0", "false", "No", "off"]) assert.equal(parseBool(v), false, v);
  for (const v of ["", "perhaps", null, undefined]) assert.equal(parseBool(v), null, String(v));
});

test("nothing is sent until something really changes", () => {
  assert.deepEqual(buildPatch(VIEW, {}), {});
  const parallel = byName("FLICKDD_MAX_PARALLEL");
  let e = setEdit({}, parallel, "12");
  assert.deepEqual(buildPatch(VIEW, e), { FLICKDD_MAX_PARALLEL: "12" });
  e = setEdit(e, parallel, "10");
  assert.deepEqual(e, {}, "typing the current value back cancels the change");
  assert.deepEqual(buildPatch(VIEW, e), {});
});

test("a switch written differently in the environment is not a change", () => {
  const chat = field({
    name: "FLICKSYNC_CHAT_ENABLED", kind: "bool", value: "yes", default: "true", source: "environment", set: true,
  });
  assert.deepEqual(setEdit({}, chat, "true"), {});
  assert.deepEqual(setEdit({}, chat, "false"), { FLICKSYNC_CHAT_ENABLED: "false" });
  const odd = { ...chat, value: "perhaps" };
  assert.deepEqual(setEdit({}, odd, "true"), { FLICKSYNC_CHAT_ENABLED: "true" });
  assert.deepEqual(setEdit({}, odd, "false"), { FLICKSYNC_CHAT_ENABLED: "false" });
});

test("a secret is sent only when a new value is typed", () => {
  const key = byName("FLICKDD_JELLYFIN_API_KEY");
  assert.deepEqual(setEdit({}, key, ""), {});
  const e = setEdit({}, key, "new-key");
  assert.deepEqual(buildPatch(VIEW, e), { FLICKDD_JELLYFIN_API_KEY: "new-key" });
  assert.deepEqual(setEdit(e, key, ""), {}, "emptying the input cancels");
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_JELLYFIN_API_KEY: "" }), {});
});

test("reset sends null only for a value saved in the panel", () => {
  let e = resetEdit({}, byName("FLICKDD_MAX_GLOBAL"));
  e = resetEdit(e, byName("FLICKDD_JELLYFIN_URL"));
  e = resetEdit(e, byName("FLICKDD_PLEX_TOKEN"));
  assert.deepEqual(e, { FLICKDD_MAX_GLOBAL: null, FLICKDD_PLEX_TOKEN: null });
  assert.deepEqual(buildPatch(VIEW, e), { FLICKDD_MAX_GLOBAL: null, FLICKDD_PLEX_TOKEN: null });
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_JELLYFIN_URL: null }), {}, "null for a non-panel value is dropped");
  assert.deepEqual(undoEdit(e, "FLICKDD_MAX_GLOBAL"), { FLICKDD_PLEX_TOKEN: null });
});

test("edits for fields the form does not show are never sent", () => {
  assert.deepEqual(buildPatch(VIEW, { FLICKDD_ENABLED: "false", FLICKSYNC_MAX_ROOMS: "5" }), {});
});

test("what a control shows", () => {
  const global = byName("FLICKDD_MAX_GLOBAL");
  assert.deepEqual(fieldDisplay(global, {}), { value: "50", changed: false, removing: false });
  assert.deepEqual(fieldDisplay(global, { FLICKDD_MAX_GLOBAL: "60" }), { value: "60", changed: true, removing: false });
  assert.deepEqual(fieldDisplay(global, { FLICKDD_MAX_GLOBAL: null }), { value: "100", changed: true, removing: true });
  assert.deepEqual(fieldDisplay(byName("FLICKDD_PLEX_TOKEN"), {}), { value: "", changed: false, removing: false });
});

test("the hint says where a value comes from", () => {
  const hint = (f: SettingField, e: Edits = {}) => fieldHint(f, fieldDisplay(f, e));
  assert.equal(hint(byName("FLICKDD_MAX_PARALLEL")), "Default");
  assert.equal(hint(byName("FLICKDD_MAX_GLOBAL")), "Saved in the panel · default 100");
  assert.equal(hint(byName("FLICKDD_JELLYFIN_URL")), "From the environment · default none");
  assert.equal(hint(byName("FLICKDD_JELLYFIN_API_KEY")), "From the environment · a value saved here overrides it");
  assert.equal(hint(byName("FLICKDD_PLEX_TOKEN")), "Saved in the panel");
  assert.equal(hint(field({ name: "FLICKDD_PLEX_TOKEN", kind: "secret" })), "Not set");
  assert.equal(
    hint(byName("FLICKDD_MAX_GLOBAL"), { FLICKDD_MAX_GLOBAL: null }),
    "Save removes the panel's value: the environment value applies, or the default (100).",
  );
  assert.equal(hint(byName("FLICKDD_PLEX_TOKEN"), { FLICKDD_PLEX_TOKEN: null }), "Save clears the value saved in the panel.");
  assert.equal(hint(field({ name: "FLICKSYNC_CORS_ORIGINS", kind: "list" })), "Default · comma-separated");
  assert.equal(
    hint(field({ name: "FLICKSYNC_CHAT_ENABLED", kind: "bool", value: "perhaps", default: "true", source: "environment", set: true })),
    "From the environment · default true · 'perhaps' is not an on/off value",
  );
});

test("a choice keeps a current value that is not one of the choices", () => {
  const mode = field({
    name: "FLICKSYNC_DEFAULT_CONTROL_MODE", kind: "choice", value: "everyone", default: "everyone",
    choices: ["everyone", "host_only"],
  });
  assert.deepEqual(choiceOptions(mode, "everyone"), ["everyone", "host_only"]);
  assert.deepEqual(choiceOptions(mode, "hosts"), ["everyone", "host_only", "hosts"]);
});

test("only the settings a message names are flagged", () => {
  const names = ["FLICKSYNC_RATE_MIN", "FLICKSYNC_RATE_MAX", "FLICKSYNC_SYNC_DRIFT_SOFT", "FLICKSYNC_MAX_ROOMS"];
  assert.deepEqual(
    namedFields("FLICKSYNC_RATE_MIN (0.5) must be below FLICKSYNC_RATE_MAX (0.25). Nothing was saved. (SETTINGS_INVALID)", names),
    ["FLICKSYNC_RATE_MIN", "FLICKSYNC_RATE_MAX"],
  );
  assert.deepEqual(namedFields("FLICKSYNC_SYNC_DRIFT_SOFTER is not a known setting. (UNKNOWN_SETTING)", names), []);
  assert.deepEqual(namedFields("XFLICKSYNC_MAX_ROOMS and FLICKSYNC_MAX_ROOMS_2", names), []);
  assert.deepEqual(namedFields("Something else went wrong.", names), []);
});

test("the PUT body is checked before it is forwarded", () => {
  assert.deepEqual(parsePatchBody('{"values":{"FLICKSYNC_MAX_ROOMS":"200","FLICKSYNC_DEFAULT_CONTROL_MODE":null}}'), {
    ok: true,
    values: { FLICKSYNC_MAX_ROOMS: "200", FLICKSYNC_DEFAULT_CONTROL_MODE: null },
  });
  for (const raw of [
    "not json",
    "[]",
    "null",
    '{"values":[]}',
    '{"other":{}}',
    '{"values":{"lower_case":"1"}}',
    '{"values":{"FLICKSYNC_MAX_ROOMS":200}}',
    '{"values":{"FLICKSYNC_MAX_ROOMS":{"x":1}}}',
    '{"values":{}}',
    "x".repeat(MAX_PATCH_CHARS + 1),
  ]) {
    const r = parsePatchBody(raw);
    assert.equal(r.ok, false, raw.slice(0, 40));
    if (!r.ok) assert.ok(r.message.length > 0);
  }
});
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd panel; node --test test/settings-form.test.ts`
Expected: FAIL with `ERR_MODULE_NOT_FOUND` for `../src/lib/settings-form.ts`.

- [ ] **Step 4: Implement the settings logic**

Create `panel/src/lib/settings-form.ts`:

```ts
// Pure logic of the settings pages: labels, what each control shows, and what Save sends.
// No React, Next or server import, so `node --test` loads it as is.
import type { FieldSource, SettingField, SettingsScope, SettingsView } from "./types.ts";

export const SETTINGS_SCOPES: readonly SettingsScope[] = ["server", "flicksync", "flickdd"];

export const SCOPE_LABEL: Record<SettingsScope, string> = {
  server: "Server",
  flicksync: "FlickSync",
  flickdd: "FlickDD",
};

export const SCOPE_LEAD: Record<SettingsScope, string> = {
  server:
    "Shared by every module: public address, signing keys, CORS, metrics and log level. Saved changes take effect when you apply them; modules keep running.",
  flicksync:
    "Watch Together: rooms, sync, chat and connection limits. Saved changes take effect when FlickSync is reloaded.",
  flickdd:
    "Offline downloads: Jellyfin and Plex backends, limits and timeouts. Saved changes take effect when FlickDD is reloaded.",
};

export function isSettingsScope(s: string): s is SettingsScope {
  return (SETTINGS_SCOPES as readonly string[]).includes(s);
}

/** Module on/off switches: changed by Start and Stop, never by the form. */
export const SWITCH_FIELDS: readonly string[] = ["FLICKSYNC_ENABLED", "FLICKDD_ENABLED"];

export function visibleFields(view: SettingsView): SettingField[] {
  return view.fields.filter((f) => !SWITCH_FIELDS.includes(f.name));
}

/** Pending changes by setting name: a string stores that value, null removes the panel's value. */
export type Edits = Readonly<Record<string, string | null>>;

/** The server's reading of a switch: 1/true/yes/on and 0/false/no/off, case and spaces ignored. */
export function parseBool(v: string | null | undefined): boolean | null {
  const s = (v ?? "").trim().toLowerCase();
  if (["1", "true", "yes", "on"].includes(s)) return true;
  if (["0", "false", "no", "off"].includes(s)) return false;
  return null;
}

const ACRONYMS = new Set(["API", "CORS", "JWT", "MB", "MBPS", "TTL", "URL", "WS"]);

/** "FLICKSYNC_MAX_ROOM_SIZE" -> "Max room size", "FLICKDD_JELLYFIN_API_KEY" -> "Jellyfin API key". */
export function fieldLabel(name: string): string {
  const words = name.replace(/^(FLICKSYNC|FLICKDD)_/, "").split("_").filter(Boolean);
  return words
    .map((w, i) => {
      if (ACRONYMS.has(w)) return w;
      const lower = w.toLowerCase();
      return i === 0 ? lower.charAt(0).toUpperCase() + lower.slice(1) : lower;
    })
    .join(" ");
}

export const SOURCE_LABEL: Record<FieldSource, string> = {
  panel: "Saved in the panel",
  environment: "From the environment",
  default: "Default",
};

/** `value` means the same as the field's current value (switches compared as switches). */
function sameValue(field: SettingField, value: string): boolean {
  if (field.kind === "bool") {
    const now = parseBool(field.value);
    return now !== null && now === parseBool(value);
  }
  return value === (field.value ?? "");
}

/** Record what a control now holds. The current value again, or an empty secret, is no change. */
export function setEdit(edits: Edits, field: SettingField, value: string): Edits {
  const next: Record<string, string | null> = { ...edits };
  const unchanged = field.secret ? value === "" : sameValue(field, value);
  if (unchanged) delete next[field.name];
  else next[field.name] = value;
  return next;
}

/** Reset (Clear for a secret): remove the panel's value on save. Only a panel value can be removed. */
export function resetEdit(edits: Edits, field: SettingField): Edits {
  if (field.source !== "panel") return undoEdit(edits, field.name);
  return { ...edits, [field.name]: null };
}

export function undoEdit(edits: Edits, name: string): Edits {
  const next: Record<string, string | null> = { ...edits };
  delete next[name];
  return next;
}

/** The `values` of PUT /settings/<scope>: real changes to fields the form shows, nothing else. */
export function buildPatch(view: SettingsView, edits: Edits): Record<string, string | null> {
  const values: Record<string, string | null> = {};
  for (const f of visibleFields(view)) {
    const e = edits[f.name];
    if (e === undefined) continue;
    if (e === null) {
      if (f.source === "panel") values[f.name] = null;
    } else if (f.secret ? e !== "" : !sameValue(f, e)) {
      values[f.name] = e;
    }
  }
  return values;
}

export interface FieldDisplay {
  /** What the control shows: the pending value, else the current one; "" for a secret. */
  value: string;
  /** Save will send something for this field. */
  changed: boolean;
  /** Save will remove the panel's value. */
  removing: boolean;
}

export function fieldDisplay(field: SettingField, edits: Edits): FieldDisplay {
  const e = edits[field.name];
  if (e === undefined) {
    return { value: field.secret ? "" : (field.value ?? ""), changed: false, removing: false };
  }
  if (e === null) {
    return { value: field.secret ? "" : (field.default ?? ""), changed: true, removing: true };
  }
  return { value: e, changed: true, removing: false };
}

/** The line under a setting's name: where its value comes from, and what Save will do. */
export function fieldHint(field: SettingField, d: FieldDisplay): string {
  if (d.removing) {
    return field.secret
      ? "Save clears the value saved in the panel."
      : `Save removes the panel's value: the environment value applies, or the default (${field.default || "none"}).`;
  }
  const parts: string[] = [];
  if (field.secret) {
    parts.push(field.source === "default" ? "Not set" : SOURCE_LABEL[field.source]);
    if (field.source === "environment") parts.push("a value saved here overrides it");
  } else {
    parts.push(SOURCE_LABEL[field.source]);
    if (field.source !== "default") parts.push(`default ${field.default || "none"}`);
    if (field.kind === "bool" && parseBool(field.value) === null) {
      parts.push(`'${field.value ?? ""}' is not an on/off value`);
    }
  }
  if (field.kind === "list") parts.push("comma-separated");
  return parts.join(" · ");
}

/** A choice field's options, plus the current value when it is not one of them. */
export function choiceOptions(field: SettingField, current: string): string[] {
  const list = field.choices ?? [];
  return current && !list.includes(current) ? [...list, current] : [...list];
}

/** The settings a server message names, so the form can point at them. */
export function namedFields(text: string, names: readonly string[]): string[] {
  return names.filter((n) => {
    const escaped = n.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(`(^|[^A-Za-z0-9_])${escaped}($|[^A-Za-z0-9_])`).test(text);
  });
}

const SETTING_NAME = /^[A-Z][A-Z0-9_]{0,63}$/;

/** Largest PUT body (in characters) the panel forwards. */
export const MAX_PATCH_CHARS = 64 * 1024;

export type PatchParse =
  | { ok: true; values: Record<string, string | null> }
  | { ok: false; message: string };

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Check the browser's PUT body before it is forwarded: {"values": {"NAME": "text" | null}}. */
export function parsePatchBody(raw: string): PatchParse {
  if (raw.length > MAX_PATCH_CHARS) {
    return { ok: false, message: "The request is too large. Save fewer changes at once." };
  }
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    return { ok: false, message: 'The request is not valid JSON. Expected {"values": {"NAME": value}}.' };
  }
  const values = isObject(body) ? body.values : undefined;
  if (!isObject(values)) {
    return { ok: false, message: 'The request must have the form {"values": {"NAME": value}}.' };
  }
  const out: Record<string, string | null> = {};
  for (const [name, v] of Object.entries(values)) {
    if (!SETTING_NAME.test(name)) {
      return { ok: false, message: `'${name.slice(0, 64)}' is not a setting name.` };
    }
    if (v !== null && typeof v !== "string") {
      return { ok: false, message: `${name} must be a string, or null to remove the panel's value.` };
    }
    out[name] = v;
  }
  if (Object.keys(out).length === 0) return { ok: false, message: "There is nothing to save." };
  return { ok: true, values: out };
}
```

- [ ] **Step 5: Run the tests and the type check**

Run: `cd panel; npm test; npm run typecheck`
Expected: every test passes (`ℹ fail 0`), `tsc --noEmit` prints nothing.

- [ ] **Step 6: Commit**

```bash
git add panel/src/lib/types.ts panel/src/lib/settings-form.ts panel/test/settings-form.test.ts
git commit -m "feat(panel): settings form logic (labels, edits, patch, body check)

Pure functions behind the settings pages: field labels from variable
names, where a value comes from, which edits are real changes, the PUT
patch (only changes; null only for panel values; secrets only when a new
value is typed), the settings a server message names, and the check of
the PUT body before the panel forwards it.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Module status logic

**Files:**
- Create: `panel/src/lib/module-status.ts`, `panel/test/module-status.test.ts`

**Interfaces:**
- Consumes: `ModuleId`, `ModuleStatus`, `SettingsScope` (Task 2, `src/lib/types.ts`); `formatDuration(secs: number): string` (`src/lib/format.ts`, existing).
- Produces (`src/lib/module-status.ts`):
  - `MODULE_IDS: readonly ModuleId[]`, `MODULE_ACTIONS = ["start", "stop", "reload"] as const`, `type ModuleAction`, `MODULE_LABEL: Record<ModuleId, string>`
  - `isModuleId(s: string): s is ModuleId`, `isModuleAction(s: string): s is ModuleAction`
  - `stateBadge(s: ModuleStatus): { label: string; tone: "plain" | "strong" | "warn" }`
  - `availableActions(s: ModuleStatus): ModuleAction[]`, `ACTION_LABEL: Record<ModuleAction, { idle: string; busy: string }>`
  - `interface Confirmation { title: string; description: string; confirm: string }`, `confirmation(s: ModuleStatus, action: ModuleAction): Confirmation | null`
  - `sinceText(s: ModuleStatus, nowMs: number): string | null`, `offText(s: ModuleStatus): string`
  - `mergeStatus(list: ModuleStatus[], next: ModuleStatus): ModuleStatus[]`
  - `type FollowUp = "apply-server" | "reload" | "start-again" | "applies-on-start" | "none"`, `followUp(scope: SettingsScope, s: ModuleStatus | null): FollowUp`, `followUpText(scope: SettingsScope, f: FollowUp, revision: number): string`

- [ ] **Step 1: Write the failing tests**

Create `panel/test/module-status.test.ts`:

```ts
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  availableActions,
  confirmation,
  followUp,
  followUpText,
  isModuleAction,
  isModuleId,
  mergeStatus,
  offText,
  sinceText,
  stateBadge,
} from "../src/lib/module-status.ts";
import type { ModuleStatus } from "../src/lib/types.ts";

const status = (over: Partial<ModuleStatus> = {}): ModuleStatus => ({
  id: "flicksync",
  state: "stopped",
  message: null,
  enabled: false,
  pending_reload: false,
  since: null,
  ...over,
});

test("module ids and actions are fixed lists", () => {
  assert.equal(isModuleId("flicksync"), true);
  assert.equal(isModuleId("flickdd"), true);
  assert.equal(isModuleId("server"), false);
  for (const a of ["start", "stop", "reload"]) assert.equal(isModuleAction(a), true, a);
  for (const a of ["restart", "", "START"]) assert.equal(isModuleAction(a), false, a);
});

test("each state has its badge and its actions", () => {
  assert.deepEqual(stateBadge(status({ state: "running", since: 1 })), { label: "Running", tone: "strong" });
  assert.deepEqual(stateBadge(status()), { label: "Stopped", tone: "plain" });
  assert.deepEqual(stateBadge(status({ state: "failed", message: "x" })), { label: "Failed to start", tone: "warn" });
  assert.deepEqual(availableActions(status({ state: "running", since: 1 })), ["reload", "stop"]);
  assert.deepEqual(availableActions(status()), ["start"]);
  assert.deepEqual(availableActions(status({ state: "failed", message: "x" })), ["start", "stop"]);
});

test("stop and reload of a running module ask first and say what they interrupt", () => {
  const running = status({ state: "running", since: 0 });
  assert.equal(confirmation(running, "start"), null);
  const stop = confirmation(running, "stop");
  assert.equal(stop?.title, "Stop FlickSync?");
  assert.equal(stop?.confirm, "Stop FlickSync");
  assert.match(stop?.description ?? "", /Every room closes/);
  assert.match(stop?.description ?? "", /also after a server restart/);
  const reload = confirmation(status({ id: "flickdd", state: "running", since: 0 }), "reload");
  assert.equal(reload?.title, "Reload FlickDD?");
  assert.equal(reload?.confirm, "Reload FlickDD");
  assert.match(reload?.description ?? "", /Downloads in progress are cut/);
  assert.equal(confirmation(status({ state: "failed", message: "x" }), "stop"), null, "nothing to interrupt");
});

test("running time", () => {
  assert.equal(sinceText(status({ state: "running", since: 1_000 }), 1_000 + 7_500_000), "Running for 2 h 05 min");
  assert.equal(sinceText(status(), 5_000), null);
});

test("what a module page says when the module is off", () => {
  assert.match(offText(status()), /nobody can watch together/);
  assert.match(offText(status({ id: "flickdd" })), /Jellyfin or Plex/);
  assert.match(offText(status({ id: "flickdd", state: "failed", message: "x" })), /FlickDD is switched on but could not start/);
});

test("an action's answer replaces that module's status only", () => {
  const list = [status(), status({ id: "flickdd" })];
  const failed = status({ state: "failed", message: "FlickSync could not start.", enabled: true });
  assert.deepEqual(mergeStatus(list, failed), [failed, list[1]]);
  assert.deepEqual(mergeStatus([], failed), [failed]);
  assert.deepEqual(stateBadge(mergeStatus(list, failed)[0]).label, "Failed to start");
});

test("what a save offers next", () => {
  assert.equal(followUp("server", null), "apply-server");
  assert.equal(followUp("flicksync", null), "none");
  assert.equal(followUp("flicksync", status({ state: "running", since: 1, pending_reload: true })), "reload");
  assert.equal(followUp("flicksync", status({ state: "running", since: 1 })), "none");
  assert.equal(followUp("flickdd", status({ id: "flickdd", state: "failed", message: "x" })), "start-again");
  assert.equal(followUp("flickdd", status({ id: "flickdd" })), "applies-on-start");
  assert.equal(
    followUpText("server", "apply-server", 8),
    "Saved as revision 8. Apply the server settings to put them in force; running modules keep running.",
  );
  assert.equal(followUpText("flickdd", "reload", 7), "Saved as revision 7. FlickDD keeps its previous settings until it is reloaded.");
  assert.equal(followUpText("flickdd", "start-again", 7), "Saved as revision 7. Start FlickDD again to try them.");
  assert.equal(followUpText("flicksync", "applies-on-start", 2), "Saved as revision 2. FlickSync is stopped and uses them when it starts.");
  assert.equal(followUpText("flicksync", "none", 9), "Saved as revision 9.");
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd panel; node --test test/module-status.test.ts`
Expected: FAIL with `ERR_MODULE_NOT_FOUND` for `../src/lib/module-status.ts`.

- [ ] **Step 3: Implement the module logic**

Create `panel/src/lib/module-status.ts`:

```ts
// Pure logic of the module controls: labels, which actions to offer, confirmations, follow-ups.
// No React, Next or server import, so `node --test` loads it as is.
import { formatDuration } from "./format.ts";
import type { ModuleId, ModuleStatus, SettingsScope } from "./types.ts";

export const MODULE_IDS: readonly ModuleId[] = ["flicksync", "flickdd"];
export const MODULE_ACTIONS = ["start", "stop", "reload"] as const;
export type ModuleAction = (typeof MODULE_ACTIONS)[number];

export const MODULE_LABEL: Record<ModuleId, string> = { flicksync: "FlickSync", flickdd: "FlickDD" };

export function isModuleId(s: string): s is ModuleId {
  return (MODULE_IDS as readonly string[]).includes(s);
}

export function isModuleAction(s: string): s is ModuleAction {
  return (MODULE_ACTIONS as readonly string[]).includes(s);
}

export function stateBadge(s: ModuleStatus): { label: string; tone: "plain" | "strong" | "warn" } {
  if (s.state === "running") return { label: "Running", tone: "strong" };
  if (s.state === "failed") return { label: "Failed to start", tone: "warn" };
  return { label: "Stopped", tone: "plain" };
}

/** Running: Reload, Stop. Stopped: Start. Failed: Start (try again), Stop (switch it off). */
export function availableActions(s: ModuleStatus): ModuleAction[] {
  if (s.state === "running") return ["reload", "stop"];
  if (s.state === "failed") return ["start", "stop"];
  return ["start"];
}

export const ACTION_LABEL: Record<ModuleAction, { idle: string; busy: string }> = {
  start: { idle: "Start", busy: "Starting" },
  stop: { idle: "Stop", busy: "Stopping" },
  reload: { idle: "Reload", busy: "Reloading" },
};

export interface Confirmation {
  title: string;
  description: string;
  /** Label of the confirming button. */
  confirm: string;
}

const INTERRUPTS: Record<ModuleId, string> = {
  flicksync: "Every room closes and everyone watching together is disconnected.",
  flickdd: "Downloads in progress are cut (clients resume them later) and the download statistics are reset.",
};

/** Stop and Reload interrupt the users of a running module: ask first. Null: run at once. */
export function confirmation(s: ModuleStatus, action: ModuleAction): Confirmation | null {
  if (action === "start" || s.state !== "running") return null;
  const label = MODULE_LABEL[s.id];
  if (action === "stop") {
    return {
      title: `Stop ${label}?`,
      description: `${INTERRUPTS[s.id]} ${label} stays stopped, also after a server restart, until you start it again.`,
      confirm: `Stop ${label}`,
    };
  }
  return {
    title: `Reload ${label}?`,
    description: `${label} restarts with the saved settings. ${INTERRUPTS[s.id]}`,
    confirm: `Reload ${label}`,
  };
}

/** "Running for 2 h 05 min", or null when the module is not running. */
export function sinceText(s: ModuleStatus, nowMs: number): string | null {
  if (s.state !== "running" || s.since === null) return null;
  return `Running for ${formatDuration((nowMs - s.since) / 1000)}`;
}

/** What a module page says instead of its content while the module is stopped or failed. */
export function offText(s: ModuleStatus): string {
  const label = MODULE_LABEL[s.id];
  if (s.state === "failed") {
    return `${label} is switched on but could not start, for the reason above. Change the setting in ${label}'s settings, then start it again.`;
  }
  return s.id === "flicksync"
    ? "FlickSync is stopped, so nobody can watch together on this server. Start it to accept rooms again. It stays stopped, also after a server restart, until you start it."
    : "FlickDD is stopped, so offline downloads are not available. Set up a Jellyfin or Plex backend in its settings, then start it. It stays stopped, also after a server restart, until you start it.";
}

/** Replace one module's status in a /modules answer with the one an action returned. */
export function mergeStatus(list: ModuleStatus[], next: ModuleStatus): ModuleStatus[] {
  return list.some((m) => m.id === next.id)
    ? list.map((m) => (m.id === next.id ? next : m))
    : [...list, next];
}

export type FollowUp = "apply-server" | "reload" | "start-again" | "applies-on-start" | "none";

/** What the settings page offers once a save went through. */
export function followUp(scope: SettingsScope, s: ModuleStatus | null): FollowUp {
  if (scope === "server") return "apply-server";
  if (!s) return "none";
  if (s.state === "failed") return "start-again";
  if (s.state === "stopped") return "applies-on-start";
  return s.pending_reload ? "reload" : "none";
}

export function followUpText(scope: SettingsScope, f: FollowUp, revision: number): string {
  const saved = `Saved as revision ${revision}.`;
  const label = scope === "server" ? "The server" : MODULE_LABEL[scope];
  switch (f) {
    case "apply-server":
      return `${saved} Apply the server settings to put them in force; running modules keep running.`;
    case "reload":
      return `${saved} ${label} keeps its previous settings until it is reloaded.`;
    case "start-again":
      return `${saved} Start ${label} again to try them.`;
    case "applies-on-start":
      return `${saved} ${label} is stopped and uses them when it starts.`;
    default:
      return saved;
  }
}
```

- [ ] **Step 4: Run the tests and the type check**

Run: `cd panel; npm test; npm run typecheck`
Expected: every test passes (`ℹ fail 0`), `tsc --noEmit` prints nothing.

- [ ] **Step 5: Commit**

```bash
git add panel/src/lib/module-status.ts panel/test/module-status.test.ts
git commit -m "feat(panel): module status logic (badges, actions, confirmations)

Pure functions behind the module controls: state badges, which actions
each state offers, the confirmation for Stop and Reload of a running
module, running time, the text of a stopped or failed module page, and
what a settings save offers next.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Panel proxy routes for modules and settings

**Files:**
- Create: `panel/src/lib/proxy.ts`, `panel/src/app/api/modules/route.ts`, `panel/src/app/api/modules/[id]/[action]/route.ts`, `panel/src/app/api/settings/[scope]/route.ts`, `panel/src/app/api/settings/[scope]/reload/route.ts`
- Modify: `panel/src/lib/flicksync.ts`

**Interfaces:**
- Consumes: `isModuleId`, `isModuleAction` (Task 3); `isSettingsScope`, `parsePatchBody`, `MAX_PATCH_CHARS` (Task 2); `hasSession`, `sameOrigin` (`src/lib/guard.ts`, existing).
- Produces:
  - `adminFetch(path: string, method?: AdminMethod, body?: unknown): Promise<UpstreamResult>` with `type AdminMethod = "GET" | "DELETE" | "POST" | "PUT"`
  - `signedOut()`, `forbidden()`, `routeNotFound()`, `invalid(message: string)`, `relay(res: UpstreamResult)` (`src/lib/proxy.ts`, each returns a `NextResponse`)
  - Browser-facing routes: `GET /api/modules`, `POST /api/modules/{id}/{action}`, `GET|PUT /api/settings/{scope}`, `POST /api/settings/server/reload`. Success bodies are the server's (`ModulesResponse`, `ModuleStatus`, `SettingsView`, `{"reloaded": true}`); errors are `{"error": {"code", "message"?}}`.

- [ ] **Step 1: Extend `adminFetch`**

In `panel/src/lib/flicksync.ts`, replace the whole `adminFetch` function and its doc comment (from `/**\n * Call FlickSync's admin API.` through the closing `}` of the function) with:

```ts
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
```

`ddFetch` and the rest of the file are unchanged.

- [ ] **Step 2: Add the response helpers**

Create `panel/src/lib/proxy.ts`:

```ts
import "server-only";

import { NextResponse } from "next/server";

import type { UpstreamResult } from "./flicksync";

const NO_STORE = { "Cache-Control": "no-store" };

function error(status: number, code: string, message?: string) {
  return NextResponse.json({ error: message ? { code, message } : { code } }, { status, headers: NO_STORE });
}

export const signedOut = () => error(401, "SIGNED_OUT");
export const forbidden = () => error(403, "FORBIDDEN");
export const routeNotFound = () => error(404, "NOT_FOUND");
export const invalid = (message: string) => error(400, "INVALID_PAYLOAD", message);

/** Pass the server's answer on as is: its error messages explain the problem. */
export function relay(res: UpstreamResult) {
  if (res.status === 204) return new NextResponse(null, { status: 204, headers: NO_STORE });
  return NextResponse.json(res.body, { status: res.status, headers: NO_STORE });
}
```

- [ ] **Step 3: Add the module routes**

Create `panel/src/app/api/modules/route.ts`:

```ts
import { adminFetch } from "@/lib/flicksync";
import { hasSession } from "@/lib/guard";
import { relay, signedOut } from "@/lib/proxy";

export const dynamic = "force-dynamic";

/** State of every module. Session required. */
export async function GET() {
  if (!(await hasSession())) return signedOut();
  return relay(await adminFetch("modules"));
}
```

Create `panel/src/app/api/modules/[id]/[action]/route.ts`:

```ts
import { adminFetch } from "@/lib/flicksync";
import { hasSession, sameOrigin } from "@/lib/guard";
import { isModuleAction, isModuleId } from "@/lib/module-status";
import { forbidden, relay, routeNotFound, signedOut } from "@/lib/proxy";

export const dynamic = "force-dynamic";

/** Start, stop or reload a module. Session and same-origin required. */
export async function POST(_req: Request, ctx: { params: Promise<{ id: string; action: string }> }) {
  if (!(await hasSession())) return signedOut();
  if (!(await sameOrigin())) return forbidden();
  const { id, action } = await ctx.params;
  if (!isModuleId(id) || !isModuleAction(action)) return routeNotFound();
  return relay(await adminFetch(`modules/${id}/${action}`, "POST"));
}
```

- [ ] **Step 4: Add the settings routes**

Create `panel/src/app/api/settings/[scope]/route.ts`:

```ts
import { adminFetch } from "@/lib/flicksync";
import { hasSession, sameOrigin } from "@/lib/guard";
import { forbidden, invalid, relay, routeNotFound, signedOut } from "@/lib/proxy";
import { MAX_PATCH_CHARS, isSettingsScope, parsePatchBody } from "@/lib/settings-form";

export const dynamic = "force-dynamic";

type Ctx = { params: Promise<{ scope: string }> };

/** The settings of one scope; secrets only as set or not set. Session required. */
export async function GET(_req: Request, ctx: Ctx) {
  if (!(await hasSession())) return signedOut();
  const { scope } = await ctx.params;
  if (!isSettingsScope(scope)) return routeNotFound();
  return relay(await adminFetch(`settings/${scope}`));
}

/** Save changed settings. Session and same-origin required; the body is checked first. */
export async function PUT(req: Request, ctx: Ctx) {
  if (!(await hasSession())) return signedOut();
  if (!(await sameOrigin())) return forbidden();
  const { scope } = await ctx.params;
  if (!isSettingsScope(scope)) return routeNotFound();
  if (Number(req.headers.get("content-length") ?? 0) > MAX_PATCH_CHARS * 4) {
    return invalid("The request is too large. Save fewer changes at once.");
  }
  const parsed = parsePatchBody(await req.text());
  if (!parsed.ok) return invalid(parsed.message);
  return relay(await adminFetch(`settings/${scope}`, "PUT", { values: parsed.values }));
}
```

(`MAX_PATCH_CHARS * 4` bounds the bytes of a body that `parsePatchBody` then bounds in characters.)

Create `panel/src/app/api/settings/[scope]/reload/route.ts`:

```ts
import { adminFetch } from "@/lib/flicksync";
import { hasSession, sameOrigin } from "@/lib/guard";
import { forbidden, relay, routeNotFound, signedOut } from "@/lib/proxy";

export const dynamic = "force-dynamic";

/** Apply the stored server settings. Only the server scope can be applied this way. */
export async function POST(_req: Request, ctx: { params: Promise<{ scope: string }> }) {
  if (!(await hasSession())) return signedOut();
  if (!(await sameOrigin())) return forbidden();
  const { scope } = await ctx.params;
  if (scope !== "server") return routeNotFound();
  return relay(await adminFetch("settings/server/reload", "POST"));
}
```

- [ ] **Step 5: Type check, test and build**

Run: `cd panel; npm run typecheck; npm test; npm run build`
Expected: no type errors, `ℹ fail 0`, and the build lists `ƒ /api/modules`, `ƒ /api/modules/[id]/[action]`, `ƒ /api/settings/[scope]`, `ƒ /api/settings/[scope]/reload`.

- [ ] **Step 6: Check that the routes refuse a request without a session**

Run in one terminal: `cd panel; PANEL_PASSWORD='correct horse battery staple' npm run dev`
In another (Git Bash):

```bash
curl -s localhost:3000/api/modules; echo
curl -s -X POST localhost:3000/api/modules/flicksync/start; echo
curl -s -X PUT -H 'content-type: application/json' -d '{"values":{"FLICKSYNC_MAX_ROOMS":"5"}}' localhost:3000/api/settings/flicksync; echo
```

Expected: `{"error":{"code":"SIGNED_OUT"}}` three times. Stop the dev server.

- [ ] **Step 7: Commit**

```bash
git add panel/src/lib/flicksync.ts panel/src/lib/proxy.ts panel/src/app/api/modules panel/src/app/api/settings
git commit -m "feat(panel): proxy routes for modules and settings

/api/modules, /api/modules/{id}/{action}, /api/settings/{scope} (GET,
PUT) and /api/settings/server/reload relay to the admin API with the
session, plus the same-origin check on every change. Ids, actions and
scopes are checked against fixed lists and the PUT body is validated
before it is forwarded. adminFetch learns POST and PUT with a JSON body.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Modules view, module pages and navigation

**Files:**
- Create: `panel/src/lib/use-modules.ts`, `panel/src/components/ModuleCard.tsx`, `panel/src/components/ModuleGate.tsx`, `panel/src/app/(panel)/ModulesOverview.tsx`
- Modify: `panel/src/lib/use-admin.ts`, `panel/src/lib/modules.ts`, `panel/src/components/flick/icons.tsx`, `panel/src/components/PanelSidebar.tsx`, `panel/src/app/(panel)/page.tsx`, `panel/src/app/(panel)/flicksync/page.tsx`, `panel/src/app/(panel)/flickdd/page.tsx`, `panel/src/styles/panel.css`
- Delete: `panel/src/app/(panel)/flickdd/DdGate.tsx`

**Interfaces:**
- Consumes: Task 3 (`ACTION_LABEL`, `availableActions`, `confirmation`, `mergeStatus`, `offText`, `sinceText`, `stateBadge`, `ModuleAction`); Task 4 routes; `ModuleId`, `ModuleStatus`, `ModulesResponse`, `Overview.running` (Task 2).
- Produces:
  - `usePoll<T>(url: string, intervalMs: number | null): AdminState<T>`; `AdminState<T>` gains `refresh: () => Promise<void>` and `mutate: Dispatch<SetStateAction<T | null>>`; `useAdmin` keeps its signature
  - `interface ModuleControl { busy; actionError; act(id, action): Promise<boolean> }`, `interface ModulesState extends AdminState<ModulesResponse>, ModuleControl { status(id: ModuleId): ModuleStatus | null }`, `useModules(intervalMs?: number): ModulesState`
  - `ModuleActionButton({ status, action, control, onDone? })`, `ModuleCard({ mod, status, control, showOpen?, showSettings?, onChanged?, children? })`, `ModuleGate({ id, children })`
  - `PanelModule.id: ModuleId`, `PanelModule.settingsHref: string`, `SETTINGS` nav entry, icons `play`, `square`, `settings`, `server`
  - CSS classes `.pill-row`, `.muted`

- [ ] **Step 1: Generalise the polling hook**

Replace the whole content of `panel/src/lib/use-admin.ts` with:

```ts
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
```

- [ ] **Step 2: Add the modules hook**

Create `panel/src/lib/use-modules.ts`:

```ts
"use client";

import { useCallback, useState } from "react";

import { ADMIN_ERROR_TEXT, adminError } from "./admin-errors";
import { type ModuleAction, mergeStatus } from "./module-status";
import type { ModuleId, ModuleStatus, ModulesResponse } from "./types";
import { type AdminState, usePoll } from "./use-admin";

export interface ModuleControl {
  /** The action in flight, if any (one at a time across the page). */
  busy: { id: ModuleId; action: ModuleAction } | null;
  /** The last action that failed over HTTP, with the text to show. */
  actionError: { id: ModuleId; text: string } | null;
  /** Run an action. True when the server carried it out (the module may still have failed to start). */
  act: (id: ModuleId, action: ModuleAction) => Promise<boolean>;
}

export interface ModulesState extends AdminState<ModulesResponse>, ModuleControl {
  status: (id: ModuleId) => ModuleStatus | null;
}

/** Poll /api/modules and run Start / Stop / Reload; an action's answer updates the state at once. */
export function useModules(intervalMs = 5_000): ModulesState {
  const poll = usePoll<ModulesResponse>("/api/modules", intervalMs);
  const { mutate, data } = poll;
  const [busy, setBusy] = useState<ModuleControl["busy"]>(null);
  const [actionError, setActionError] = useState<ModuleControl["actionError"]>(null);

  const act = useCallback(
    async (id: ModuleId, action: ModuleAction) => {
      setBusy({ id, action });
      setActionError(null);
      try {
        const res = await fetch(`/api/modules/${id}/${action}`, { method: "POST" });
        if (res.status === 401) {
          window.location.assign("/login");
          return false;
        }
        const body = await res.json().catch(() => null);
        if (!res.ok) {
          setActionError({ id, text: adminError(body).text });
          return false;
        }
        const next = body as ModuleStatus;
        mutate((prev) => ({ modules: mergeStatus(prev?.modules ?? [], next) }));
        return true;
      } catch {
        setActionError({ id, text: ADMIN_ERROR_TEXT.UNREACHABLE });
        return false;
      } finally {
        setBusy(null);
      }
    },
    [mutate],
  );

  const status = useCallback(
    (id: ModuleId) => data?.modules.find((m) => m.id === id) ?? null,
    [data],
  );

  return { ...poll, busy, actionError, act, status };
}
```

- [ ] **Step 3: Add the icons, the nav entry and the module links**

In `panel/src/components/flick/icons.tsx`, add these entries to `ICONS` (after `"qr-code": [...]`, before the closing `} satisfies`):

```ts
  play: [["path", { d: "M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z" }]],
  square: [["rect", { width: "18", height: "18", x: "3", y: "3", rx: "2" }]],
  settings: [
    [
      "path",
      {
        d: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z",
      },
    ],
    ["circle", { cx: "12", cy: "12", r: "3" }],
  ],
  server: [
    ["rect", { width: "20", height: "8", x: "2", y: "2", rx: "2", ry: "2" }],
    ["rect", { width: "20", height: "8", x: "2", y: "14", rx: "2", ry: "2" }],
    ["line", { x1: "6", x2: "6.01", y1: "6", y2: "6" }],
    ["line", { x1: "6", x2: "6.01", y1: "18", y2: "18" }],
  ],
```

Replace the whole content of `panel/src/lib/modules.ts` with:

```ts
import type { IconName } from "@/components/flick/icons";

import type { ModuleId } from "./types";

/**
 * The panel is a base for Flick Server: one entry per module. To add one, the server must know
 * the module id (start / stop / reload) and its settings scope; then create
 * `src/app/(panel)/<id>/page.tsx` and list it here. The sidebar, the Overview and the settings
 * pages pick it up.
 */
export interface PanelModule {
  id: ModuleId;
  label: string;
  href: string;
  /** Its settings page. */
  settingsHref: string;
  icon: IconName;
  /** One sentence for the Overview card. */
  summary: string;
}

export const MODULES: PanelModule[] = [
  {
    id: "flicksync",
    label: "FlickSync",
    href: "/flicksync",
    settingsHref: "/settings/flicksync",
    icon: "monitor-play",
    summary: "Watch Together: invitation link, live rooms and sync statistics.",
  },
  {
    id: "flickdd",
    label: "FlickDD",
    href: "/flickdd",
    settingsHref: "/settings/flickdd",
    icon: "download",
    summary: "Offline downloads: live transfers, limits and download statistics.",
  },
];

export const HOME = { id: "overview", label: "Overview", href: "/", icon: "house" as IconName };
export const SETTINGS = { id: "settings", label: "Settings", href: "/settings", icon: "settings" as IconName };
```

In `panel/src/components/PanelSidebar.tsx`, replace `import { HOME, MODULES } from "@/lib/modules";` with `import { HOME, MODULES, SETTINGS } from "@/lib/modules";` and `const items = [HOME, ...MODULES];` with `const items = [HOME, ...MODULES, SETTINGS];`.

- [ ] **Step 4: Add the module card and its action button**

Create `panel/src/components/ModuleCard.tsx`:

```tsx
"use client";

import { type ReactNode, useState } from "react";

import type { IconName } from "@/components/flick/icons";
import { Button, ButtonLink, Dialog, Notice, Panel, Pill } from "@/components/flick/ui";
import {
  ACTION_LABEL,
  type ModuleAction,
  availableActions,
  confirmation,
  sinceText,
  stateBadge,
} from "@/lib/module-status";
import type { PanelModule } from "@/lib/modules";
import type { ModuleStatus } from "@/lib/types";
import type { ModuleControl } from "@/lib/use-modules";

const ACTION_ICON: Record<ModuleAction, IconName> = { start: "play", stop: "square", reload: "refresh-cw" };
const ACTION_VARIANT = { start: "primary", stop: "danger", reload: "glass" } as const;

/** One module action. Stop and Reload of a running module ask first, in a dialog. */
export function ModuleActionButton({
  status,
  action,
  control,
  onDone,
}: {
  status: ModuleStatus;
  action: ModuleAction;
  control: ModuleControl;
  onDone?: () => void;
}) {
  const [asking, setAsking] = useState(false);
  const ask = confirmation(status, action);
  const running = control.busy?.id === status.id && control.busy.action === action;
  const error = control.actionError?.id === status.id ? control.actionError.text : null;

  async function run() {
    if (await control.act(status.id, action)) {
      setAsking(false);
      onDone?.();
    }
  }

  return (
    <>
      <Button
        variant={ACTION_VARIANT[action]}
        size="sm"
        icon={ACTION_ICON[action]}
        disabled={control.busy !== null}
        onClick={() => (ask ? setAsking(true) : void run())}
      >
        {running ? ACTION_LABEL[action].busy : ACTION_LABEL[action].idle}
      </Button>
      {asking && ask && (
        <Dialog title={ask.title} description={ask.description} onClose={() => !running && setAsking(false)}>
          {error && <Notice tone="error">{error}</Notice>}
          <div className="row-actions">
            <Button variant={ACTION_VARIANT[action]} icon={ACTION_ICON[action]} onClick={() => void run()} disabled={running}>
              {running ? ACTION_LABEL[action].busy : ask.confirm}
            </Button>
            <Button variant="ghost" onClick={() => setAsking(false)} disabled={running}>
              Cancel
            </Button>
          </div>
        </Dialog>
      )}
    </>
  );
}

/** A module's state and its actions: Overview cards, module pages, settings pages. */
export function ModuleCard({
  mod,
  status,
  control,
  showOpen = true,
  showSettings = true,
  onChanged,
  children,
}: {
  mod: PanelModule;
  status: ModuleStatus;
  control: ModuleControl;
  showOpen?: boolean;
  showSettings?: boolean;
  /** Called after a successful action (the Overview re-renders its server-side figures). */
  onChanged?: () => void;
  children?: ReactNode;
}) {
  const badge = stateBadge(status);
  const since = sinceText(status, Date.now());
  const error = control.actionError?.id === status.id ? control.actionError.text : null;
  const links = (showOpen || showSettings) && (
    <div className="row-actions">
      {showOpen && (
        <ButtonLink href={mod.href} size="sm" variant="primary">
          Open
        </ButtonLink>
      )}
      {showSettings && (
        <ButtonLink href={mod.settingsHref} size="sm" icon="settings">
          Settings
        </ButtonLink>
      )}
    </div>
  );

  return (
    <Panel title={mod.label} actions={links || undefined}>
      <div className="pill-row">
        <Pill tone={badge.tone}>{badge.label}</Pill>
        {status.pending_reload && <Pill tone="warn">Reload required</Pill>}
        {since && <span className="muted">{since}</span>}
      </div>
      {status.state === "failed" && status.message && <Notice tone="error">{status.message}</Notice>}
      {status.pending_reload && (
        <p className="muted">
          The saved settings changed since {mod.label} started. Reload it to put them in force.
        </p>
      )}
      {children}
      {error && <Notice tone="error">{error}</Notice>}
      <div className="row-actions">
        {availableActions(status).map((a) => (
          <ModuleActionButton key={a} status={status} action={a} control={control} onDone={onChanged} />
        ))}
      </div>
    </Panel>
  );
}
```

- [ ] **Step 5: Add the module gate and use it on the module pages**

Create `panel/src/components/ModuleGate.tsx`:

```tsx
"use client";

import type { ReactNode } from "react";

import { ModuleCard } from "@/components/ModuleCard";
import { Notice, Spinner } from "@/components/flick/ui";
import { ADMIN_ERROR_TEXT } from "@/lib/admin-errors";
import { offText } from "@/lib/module-status";
import { MODULES } from "@/lib/modules";
import type { ModuleId } from "@/lib/types";
import { useModules } from "@/lib/use-modules";

/**
 * A module page's content while the module runs. Stopped or failed: the module card with Start
 * and one sentence on what is off. Running with unapplied settings: the card (Reload) on top.
 */
export function ModuleGate({ id, children }: { id: ModuleId; children: ReactNode }) {
  const modules = useModules();
  const mod = MODULES.find((m) => m.id === id);
  const status = modules.status(id);
  if (!mod) return null;
  if (!status) {
    if (modules.loading) return <Spinner label="Loading" />;
    return <Notice tone="error">{modules.errorText ?? ADMIN_ERROR_TEXT.UNKNOWN}</Notice>;
  }
  if (status.state !== "running") {
    return (
      <ModuleCard mod={mod} status={status} control={modules} showOpen={false}>
        <p className="muted">{offText(status)}</p>
      </ModuleCard>
    );
  }
  return (
    <>
      {modules.error && <Notice tone="warn">{modules.errorText}</Notice>}
      {status.pending_reload && <ModuleCard mod={mod} status={status} control={modules} showOpen={false} />}
      {children}
    </>
  );
}
```

Replace the whole content of `panel/src/app/(panel)/flicksync/page.tsx` with:

```tsx
import { ModuleGate } from "@/components/ModuleGate";
import { ButtonLink, PageHeader } from "@/components/flick/ui";

import { RoomsPanel } from "./RoomsPanel";
import { StatsPanel } from "./StatsPanel";

export const metadata = { title: "FlickSync" };

export default function FlickSyncPage() {
  return (
    <div className="panel-page">
      <PageHeader
        title="FlickSync"
        lead="Watch Together. Keep an eye on live rooms and see how well everyone stays in sync."
        actions={
          <ButtonLink href="/settings/flicksync" icon="settings">
            Settings
          </ButtonLink>
        }
      />
      <ModuleGate id="flicksync">
        <RoomsPanel />
        <StatsPanel />
      </ModuleGate>
    </div>
  );
}
```

Replace the whole content of `panel/src/app/(panel)/flickdd/page.tsx` with:

```tsx
import { ModuleGate } from "@/components/ModuleGate";
import { ButtonLink, PageHeader } from "@/components/flick/ui";

import { ActivePanel } from "./ActivePanel";
import { HistoryPanel } from "./HistoryPanel";
import { StatsPanel } from "./StatsPanel";

export const metadata = { title: "FlickDD" };

export default function FlickDDPage() {
  return (
    <div className="panel-page">
      <PageHeader
        title="FlickDD"
        lead="Offline downloads. See live transfers, cut one if needed and follow how much is downloaded."
        actions={
          <ButtonLink href="/settings/flickdd" icon="settings">
            Settings
          </ButtonLink>
        }
      />
      <ModuleGate id="flickdd">
        <ActivePanel />
        <HistoryPanel />
        <StatsPanel />
      </ModuleGate>
    </div>
  );
}
```

Delete `panel/src/app/(panel)/flickdd/DdGate.tsx`: `git rm "panel/src/app/(panel)/flickdd/DdGate.tsx"`.

- [ ] **Step 6: Rebuild the Overview around the module cards**

Create `panel/src/app/(panel)/ModulesOverview.tsx`:

```tsx
"use client";

import { useRouter } from "next/navigation";
import type { ReactNode } from "react";

import { ModuleCard } from "@/components/ModuleCard";
import { Notice, Spinner } from "@/components/flick/ui";
import { ADMIN_ERROR_TEXT } from "@/lib/admin-errors";
import { MODULES } from "@/lib/modules";
import type { ModuleId } from "@/lib/types";
import { useModules } from "@/lib/use-modules";

/** One card per module: state, Start / Stop / Reload, and the live figures the page passes in. */
export function ModulesOverview({ extras }: { extras: Partial<Record<ModuleId, ReactNode>> }) {
  const modules = useModules();
  const router = useRouter();
  if (!modules.data) {
    if (modules.loading) return <Spinner label="Loading modules" />;
    return <Notice tone="error">{modules.errorText ?? ADMIN_ERROR_TEXT.UNKNOWN}</Notice>;
  }
  return (
    <>
      {modules.error && <Notice tone="warn">{modules.errorText}</Notice>}
      {MODULES.map((m) => {
        const status = modules.status(m.id);
        if (!status) return null;
        return (
          <ModuleCard key={m.id} mod={m} status={status} control={modules} onChanged={() => router.refresh()}>
            <p className="muted">{m.summary}</p>
            {status.state === "running" && extras[m.id]}
          </ModuleCard>
        );
      })}
    </>
  );
}
```

Replace the whole content of `panel/src/app/(panel)/page.tsx` with:

```tsx
import type { ReactNode } from "react";

import { ButtonLink, Notice, PageHeader, Panel, Pill } from "@/components/flick/ui";
import { adminError } from "@/lib/admin-errors";
import { BACKEND_LABEL, formatBytes, formatCount, formatDuration } from "@/lib/format";
import { adminFetch, ddFetch } from "@/lib/flicksync";
import type { DdOverview, ModuleId, Overview } from "@/lib/types";

import { InvitePanel } from "./InvitePanel";
import { ModulesOverview } from "./ModulesOverview";

export const metadata = { title: "Overview" };

const counted = (n: number, one: string, many: string) => `${formatCount(n)} ${n === 1 ? one : many}`;

export default async function OverviewPage() {
  const [sync, dd] = await Promise.all([adminFetch("overview"), ddFetch("overview")]);
  const o = sync.status === 200 ? (sync.body as Overview) : null;
  const d = dd.status === 200 ? (dd.body as DdOverview) : null;

  // Live figures for the module cards, shown only while the module runs.
  const extras: Partial<Record<ModuleId, ReactNode>> = {
    flicksync: o?.running ? (
      <div className="pill-row">
        <Pill>{counted(o.rooms, "room", "rooms")}</Pill>
        <Pill>{counted(o.participants, "participant", "participants")}</Pill>
        <Pill>
          {formatCount(o.connections)} of {formatCount(o.max_connections)} connections
        </Pill>
      </div>
    ) : null,
    flickdd: d ? (
      <div className="pill-row">
        {(["jellyfin", "plex"] as const)
          .filter((b) => d.backends[b])
          .map((b) => (
            <Pill key={b}>{BACKEND_LABEL[b]}</Pill>
          ))}
        <Pill>
          {formatCount(d.active)} / {formatCount(d.limits.max_global)} active
        </Pill>
        <Pill>{counted(d.totals.downloads, "download", "downloads")}</Pill>
        <Pill>{formatBytes(d.totals.bytes_served)} served</Pill>
      </div>
    ) : null,
  };

  return (
    <div className="panel-page">
      <PageHeader title="Overview" lead="Your Flick Server and its modules at a glance." />
      <Panel
        title="Server"
        actions={
          <ButtonLink href="/settings/server" size="sm" icon="settings">
            Server Settings
          </ButtonLink>
        }
      >
        {o ? (
          <div className="pill-row">
            <Pill tone={o.ready ? "strong" : "warn"}>{o.ready ? "Online" : "Not ready"}</Pill>
            <Pill>v{o.version}</Pill>
            <Pill>Up {formatDuration(o.uptime_secs)}</Pill>
          </div>
        ) : (
          <Notice tone="error">{adminError(sync.body).text}</Notice>
        )}
      </Panel>
      <InvitePanel />
      <ModulesOverview extras={extras} />
    </div>
  );
}
```

- [ ] **Step 7: Add the shared CSS**

Append to `panel/src/styles/panel.css`:

```css
/* ---------- Modules ---------- */
.pill-row { display: flex; flex-wrap: wrap; align-items: center; gap: 0.5rem; }
.muted { margin: 0; color: var(--muted-foreground); font-size: 0.875rem; line-height: 1.6; }
```

- [ ] **Step 8: Type check, test and build**

Run: `cd panel; npm run typecheck; npm test; npm run build`
Expected: no type errors, `ℹ fail 0`, build succeeds. `git grep -n DdGate -- panel/src` prints nothing.

- [ ] **Step 9: Check by hand against a local server**

Terminal 1 (repository root, Git Bash):

```bash
mkdir -p "$TMP/flick-smoke" && PANEL_PASSWORD='correct horse battery staple' FLICKSYNC_DATA_DIR="$TMP/flick-smoke" cargo run
```

Terminal 2: `cd panel; PANEL_PASSWORD='correct horse battery staple' FLICKSYNC_URL=http://localhost:8787 npm run dev`, open `http://localhost:3000`, sign in with the password.

Expected:
- Sidebar: Overview, FlickSync, FlickDD, Settings (the Settings link leads to a 404 until Task 6).
- Overview: a Server card (Online, version, uptime), the invitation, and two module cards, both **Stopped** with **Start**.
- FlickSync **Start**: the card turns **Running**, "Running for 0 s", buttons Reload and Stop, rooms / participants / connections pills appear.
- **Stop**: a dialog "Stop FlickSync?" with the consequence; Cancel keeps it running; Stop FlickSync stops it.
- FlickDD **Start** with no backend: **Failed to start** with the server's message (naming `FLICKDD_JELLYFIN_URL`), buttons Start and Stop; Stop returns it to Stopped without a dialog.
- `/flicksync` while stopped: the card with Start and "FlickSync is stopped, so nobody can watch together..."; after Start the rooms and statistics panels appear.
- Stop the server process: the Overview cards keep their last state and show the "Flick Server is unreachable ... (UNREACHABLE)" notice.

- [ ] **Step 10: Commit**

```bash
git add panel/src
git commit -m "feat(panel): Modules view with start, stop and reload

The Overview shows a Server card and one card per module with its state,
the failure message, a Reload required badge, the running time and
Start / Stop / Reload (Stop and Reload of a running module ask first).
Module pages show that card and an explanation instead of their panels
while the module is not running. usePoll generalises useAdmin with a
forced refresh and local updates; the sidebar gets a Settings entry.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Settings pages

**Files:**
- Create: `panel/src/app/(panel)/settings/page.tsx`, `panel/src/app/(panel)/settings/[scope]/page.tsx`, `panel/src/app/(panel)/settings/ScopeTabs.tsx`, `panel/src/app/(panel)/settings/SettingsForm.tsx`, `panel/src/app/(panel)/settings/FieldRow.tsx`
- Modify: `panel/src/components/flick/ui.tsx`, `panel/src/styles/panel.css`

**Interfaces:**
- Consumes: Task 2 (`Edits`, `SCOPE_LABEL`, `SCOPE_LEAD`, `SETTINGS_SCOPES`, `isSettingsScope`, `buildPatch`, `namedFields`, `visibleFields`, `choiceOptions`, `fieldDisplay`, `fieldHint`, `fieldLabel`, `parseBool`, `resetEdit`, `setEdit`, `undoEdit`); Task 3 (`followUp`, `followUpText`); Task 5 (`usePoll`, `useModules`, `ModuleCard`, `ModuleActionButton`, `MODULES`); Task 4 routes.
- Produces: pages `/settings` (redirect) and `/settings/{server|flicksync|flickdd}`; `Switch({ id?, checked, onChange, label, disabled? })` in `ui.tsx`.

- [ ] **Step 1: Add the switch to the design-system components**

In `panel/src/components/flick/ui.tsx`, insert before the line `// ---------------------------------------------------------------- TextField`:

```tsx
// ---------------------------------------------------------------- Switch

/** On / off switch (the design system's `.fk-switch`). */
export function Switch({
  id,
  checked,
  onChange,
  label,
  disabled,
}: {
  id?: string;
  checked: boolean;
  onChange: (on: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      className="fk-switch"
      onClick={() => onChange(!checked)}
    >
      <span className="fk-switch__thumb" />
    </button>
  );
}

```

- [ ] **Step 2: Add the field row**

Create `panel/src/app/(panel)/settings/FieldRow.tsx`:

```tsx
"use client";

import { type ReactNode, useId, useState } from "react";

import { Button, Pill, Switch } from "@/components/flick/ui";
import {
  type Edits,
  choiceOptions,
  fieldDisplay,
  fieldHint,
  fieldLabel,
  parseBool,
  resetEdit,
  setEdit,
  undoEdit,
} from "@/lib/settings-form";
import type { SettingField } from "@/lib/types";

const INPUT = "fk-field__input fk-glass setting__input";

/** One setting: label, variable name, where its value comes from, its control, Undo / Reset. */
export function FieldRow({
  field,
  edits,
  flagged,
  onEdit,
}: {
  field: SettingField;
  edits: Edits;
  /** Named in the last error message. */
  flagged: boolean;
  onEdit: (change: (edits: Edits) => Edits) => void;
}) {
  const id = useId();
  const [replacing, setReplacing] = useState(false);
  const d = fieldDisplay(field, edits);
  const label = fieldLabel(field.name);
  const set = (value: string) => onEdit((e) => setEdit(e, field, value));

  let control: ReactNode;
  if (field.kind === "bool") {
    control = (
      <Switch
        id={id}
        label={label}
        checked={parseBool(d.value) ?? false}
        disabled={d.removing}
        onChange={(on) => set(on ? "true" : "false")}
      />
    );
  } else if (field.kind === "choice") {
    control = (
      <select
        id={id}
        className={INPUT}
        value={d.value}
        disabled={d.removing}
        onChange={(e) => set(e.currentTarget.value)}
      >
        {choiceOptions(field, d.value).map((c) => (
          <option key={c} value={c}>
            {c}
          </option>
        ))}
      </select>
    );
  } else if (field.secret) {
    control =
      replacing || (d.changed && !d.removing) ? (
        <input
          id={id}
          className={INPUT}
          type="password"
          autoComplete="new-password"
          placeholder="New value"
          value={d.value}
          onChange={(e) => set(e.currentTarget.value)}
        />
      ) : (
        <>
          <Pill tone={field.set && !d.removing ? "strong" : "plain"}>
            {d.removing ? "Will be cleared" : field.set ? "Set" : "Not set"}
          </Pill>
          {!d.removing && (
            <Button size="sm" onClick={() => setReplacing(true)}>
              {field.set ? "Replace" : "Set Value"}
            </Button>
          )}
        </>
      );
  } else {
    control = (
      <input
        id={id}
        className={INPUT}
        type="text"
        inputMode={field.kind === "int" ? "numeric" : field.kind === "float" ? "decimal" : "text"}
        spellCheck={false}
        autoComplete="off"
        placeholder={field.kind === "list" ? "first, second" : undefined}
        value={d.value}
        disabled={d.removing}
        onChange={(e) => set(e.currentTarget.value)}
      />
    );
  }

  let trailing: ReactNode = null;
  if (d.changed) {
    trailing = (
      <Button
        variant="ghost"
        size="sm"
        onClick={() => {
          setReplacing(false);
          onEdit((e) => undoEdit(e, field.name));
        }}
      >
        Undo
      </Button>
    );
  } else if (replacing) {
    trailing = (
      <Button variant="ghost" size="sm" onClick={() => setReplacing(false)}>
        Cancel
      </Button>
    );
  } else if (field.source === "panel") {
    trailing = (
      <Button variant="ghost" size="sm" onClick={() => onEdit((e) => resetEdit(e, field))}>
        {field.secret ? "Clear" : "Reset"}
      </Button>
    );
  }

  return (
    <div className="setting" data-changed={d.changed ? "" : undefined} data-flagged={flagged ? "" : undefined}>
      <div className="setting__text">
        <label className="setting__label" htmlFor={id}>
          {label}
        </label>
        <span className="setting__name">{field.name}</span>
        <span className="setting__hint">{fieldHint(field, d)}</span>
      </div>
      <div className="setting__control">
        {control}
        {trailing}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Add the form**

Create `panel/src/app/(panel)/settings/SettingsForm.tsx`:

```tsx
"use client";

import { useState } from "react";

import { ModuleActionButton, ModuleCard } from "@/components/ModuleCard";
import { Button, Notice, Panel, Pill, Spinner } from "@/components/flick/ui";
import { ADMIN_ERROR_TEXT, type AdminError, adminError } from "@/lib/admin-errors";
import { followUp, followUpText } from "@/lib/module-status";
import { MODULES } from "@/lib/modules";
import { type Edits, SCOPE_LABEL, buildPatch, namedFields, visibleFields } from "@/lib/settings-form";
import type { SettingsScope, SettingsView } from "@/lib/types";
import { usePoll } from "@/lib/use-admin";
import { useModules } from "@/lib/use-modules";

import { FieldRow } from "./FieldRow";

/** The settings of one scope: edit, save only what changed, then apply or reload. */
export function SettingsForm({ scope }: { scope: SettingsScope }) {
  const settings = usePoll<SettingsView>(`/api/settings/${scope}`, null);
  const modules = useModules();
  const mod = MODULES.find((m) => m.id === scope) ?? null;
  const status = mod ? modules.status(mod.id) : null;
  const [edits, setEdits] = useState<Edits>({});
  const [busy, setBusy] = useState<"save" | "apply" | null>(null);
  const [failure, setFailure] = useState<AdminError | null>(null);
  const [savedRevision, setSavedRevision] = useState<number | null>(null);
  const [applied, setApplied] = useState(false);

  const view = settings.data;
  if (!view) {
    if (settings.loading) return <Spinner label="Loading settings" />;
    return <Notice tone="error">{settings.errorText ?? ADMIN_ERROR_TEXT.UNKNOWN}</Notice>;
  }

  const patch = buildPatch(view, edits);
  const changes = Object.keys(patch).length;
  const flagged = failure ? namedFields(failure.text, view.fields.map((f) => f.name)) : [];
  const next = savedRevision === null ? null : followUp(scope, status);

  /** The answer's body on success; on failure the error is shown and null returned. */
  async function send(url: string, init: RequestInit): Promise<{ body: unknown } | null> {
    try {
      const res = await fetch(url, init);
      if (res.status === 401) {
        window.location.assign("/login");
        return null;
      }
      const body = await res.json().catch(() => null);
      if (!res.ok) {
        setFailure(adminError(body));
        return null;
      }
      return { body };
    } catch {
      setFailure({ code: "UNREACHABLE", text: ADMIN_ERROR_TEXT.UNREACHABLE });
      return null;
    }
  }

  async function save() {
    setBusy("save");
    setFailure(null);
    setSavedRevision(null);
    setApplied(false);
    const ok = await send(`/api/settings/${scope}`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ values: patch }),
    });
    if (ok) {
      const fresh = ok.body as SettingsView;
      settings.mutate(fresh);
      setEdits({});
      await modules.refresh(); // pending_reload changes with the save
      setSavedRevision(fresh.revision);
    }
    setBusy(null);
  }

  async function applyServer() {
    setBusy("apply");
    setFailure(null);
    setApplied(false);
    if (await send("/api/settings/server/reload", { method: "POST" })) {
      setApplied(true);
      setSavedRevision(null);
    }
    setBusy(null);
  }

  return (
    <>
      {mod && status && (
        <ModuleCard mod={mod} status={status} control={modules} showSettings={false} />
      )}
      {mod && !status && modules.errorText && <Notice tone="warn">{modules.errorText}</Notice>}

      <Panel
        title={`${SCOPE_LABEL[scope]} settings`}
        actions={
          <div className="row-actions">
            <Pill>Revision {view.revision}</Pill>
            {scope === "server" && (
              <Button size="sm" icon="refresh-cw" onClick={applyServer} disabled={busy !== null}>
                {busy === "apply" ? "Applying" : "Apply Server Settings"}
              </Button>
            )}
          </div>
        }
      >
        <p className="muted">
          A value saved here wins over the environment. Reset removes it, so the environment value or the default
          applies again. Secret values are never shown: replace or clear them.
        </p>
        <div className="settings-list">
          {visibleFields(view).map((f) => (
            <FieldRow
              key={`${f.name}:${view.revision}`}
              field={f}
              edits={edits}
              flagged={flagged.includes(f.name)}
              onEdit={setEdits}
            />
          ))}
        </div>
      </Panel>

      {applied && <Notice tone="info">The server settings are in force.</Notice>}
      {next && savedRevision !== null && (
        <Notice tone="info">
          <p className="notice-text">{followUpText(scope, next, savedRevision)}</p>
          {next === "apply-server" && (
            <div className="row-actions">
              <Button size="sm" variant="primary" icon="refresh-cw" onClick={applyServer} disabled={busy !== null}>
                {busy === "apply" ? "Applying" : "Apply Now"}
              </Button>
            </div>
          )}
          {(next === "reload" || next === "start-again") && status && (
            <div className="row-actions">
              <ModuleActionButton status={status} action={next === "reload" ? "reload" : "start"} control={modules} />
            </div>
          )}
        </Notice>
      )}

      <div className="save-bar fk-glass-strong">
        {failure && <Notice tone="error">{failure.text}</Notice>}
        <div className="save-bar__row">
          <span className="muted">
            {changes === 0 ? "No unsaved changes" : `${changes} unsaved ${changes === 1 ? "change" : "changes"}`}
          </span>
          <div className="row-actions">
            <Button
              variant="ghost"
              onClick={() => {
                setEdits({});
                setFailure(null);
              }}
              disabled={changes === 0 || busy !== null}
            >
              Discard Changes
            </Button>
            <Button variant="primary" icon="check" onClick={save} disabled={changes === 0 || busy !== null}>
              {busy === "save" ? "Saving" : "Save Changes"}
            </Button>
          </div>
        </div>
      </div>
    </>
  );
}
```

- [ ] **Step 4: Add the pages and the scope tabs**

Create `panel/src/app/(panel)/settings/ScopeTabs.tsx`:

```tsx
import Link from "next/link";

import { SCOPE_LABEL, SETTINGS_SCOPES } from "@/lib/settings-form";
import type { SettingsScope } from "@/lib/types";

/** Server / FlickSync / FlickDD, styled as the design system's segmented control. */
export function ScopeTabs({ current }: { current: SettingsScope }) {
  return (
    <nav aria-label="Settings scope" className="fk-seg fk-glass fk-seg--sm">
      {SETTINGS_SCOPES.map((s) => (
        <Link
          key={s}
          href={`/settings/${s}`}
          className="fk-seg__item fk-lift"
          aria-current={s === current ? "page" : undefined}
        >
          {SCOPE_LABEL[s]}
        </Link>
      ))}
    </nav>
  );
}
```

Create `panel/src/app/(panel)/settings/page.tsx`:

```tsx
import { redirect } from "next/navigation";

export default function SettingsIndex() {
  redirect("/settings/server");
}
```

Create `panel/src/app/(panel)/settings/[scope]/page.tsx`:

```tsx
import { notFound } from "next/navigation";

import { PageHeader } from "@/components/flick/ui";
import { SCOPE_LEAD, isSettingsScope } from "@/lib/settings-form";

import { ScopeTabs } from "../ScopeTabs";
import { SettingsForm } from "../SettingsForm";

export const metadata = { title: "Settings" };

export default async function SettingsPage({ params }: { params: Promise<{ scope: string }> }) {
  const { scope } = await params;
  if (!isSettingsScope(scope)) notFound();
  return (
    <div className="panel-page">
      <PageHeader title="Settings" lead={SCOPE_LEAD[scope]} />
      <ScopeTabs current={scope} />
      {/* A new form per scope: edits never carry over from one scope to another. */}
      <SettingsForm key={scope} scope={scope} />
    </div>
  );
}
```

- [ ] **Step 5: Add the settings CSS**

Append to `panel/src/styles/panel.css`:

```css
/* ---------- Settings ---------- */
a.fk-seg__item { display: inline-flex; align-items: center; text-decoration: none; }
.fk-seg__item[aria-current="page"] { color: #fff; background: rgb(255 255 255 / 0.18); }
.settings-list { display: flex; flex-direction: column; }
.setting { display: grid; grid-template-columns: minmax(0, 1fr) minmax(14rem, 24rem); align-items: center; gap: 0.5rem 1.5rem; border-top: 1px solid var(--hairline); border-radius: var(--radius-xl); padding: 0.875rem 1rem; }
.setting:first-child { border-top: 0; }
.setting[data-changed] { background: var(--hover); }
.setting[data-flagged] { box-shadow: inset 3px 0 0 var(--error); }
.setting__text { display: flex; min-width: 0; flex-direction: column; gap: 0.125rem; }
.setting__label { font-size: 0.9375rem; font-weight: 500; }
.setting__name { font-family: var(--font-mono); font-size: 0.75rem; color: var(--subtle); overflow-wrap: anywhere; }
.setting__hint { font-size: 0.8125rem; line-height: 1.4; color: var(--muted-foreground); }
.setting__control { display: flex; flex-wrap: wrap; align-items: center; justify-content: flex-end; gap: 0.5rem; }
.setting__input { flex: 1; min-width: 0; height: 2.5rem; }
select.setting__input option { background: var(--background); color: #fff; }
.save-bar { position: sticky; bottom: 1rem; z-index: 4; display: flex; flex-direction: column; gap: 0.75rem; border-radius: var(--radius-2xl); padding: 0.75rem 1rem; }
.save-bar__row { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 0.75rem; }
.notice-text { margin: 0 0 0.75rem; }

@media (max-width: 60rem) {
  .setting { grid-template-columns: minmax(0, 1fr); }
  .setting__control { justify-content: flex-start; }
}
```

- [ ] **Step 6: Type check, test and build**

Run: `cd panel; npm run typecheck; npm test; npm run build`
Expected: no type errors, `ℹ fail 0`, the build lists `ƒ /settings/[scope]` and `/settings`.

- [ ] **Step 7: Check by hand against a local server**

Start the server and the dev panel as in Task 5 Step 9, sign in, then:

- Sidebar **Settings** opens `/settings/server` with the tabs Server / FlickSync / FlickDD; `/settings/other` is a 404.
- Server tab: fields in the server's order, `FLICKSYNC_METRICS_TOKEN` and `FLICKSYNC_AUTH_KEYS` as **Not set** with **Set Value**; **Apply Server Settings** answers "The server settings are in force."
- FlickSync tab: no `FLICKSYNC_ENABLED` row; the FlickSync card on top. Set Max rooms to `ten`, Save: the save bar shows the server's message ending in `(SETTINGS_INVALID)`, the Max rooms row is marked, the edit is still there. Set it to `200`, Save: "Saved as revision N"; with FlickSync running the card shows **Reload required** and the notice offers **Reload** (with the confirmation dialog).
- Type `200` back over a field already at `200`: "No unsaved changes".
- Max rooms now shows **Saved in the panel · default 10000** and a **Reset** button; Reset, Save: back to **Default**.
- FlickDD tab: set Jellyfin URL `http://jellyfin:8096`, Jellyfin API key (Set Value, type a key), Save; then **Start** on the card. The browser does not offer to fill the panel password into the key field. Reload the page: the key shows **Set**, never its value; **Clear**, Save: **Not set**.
- Narrow the window under 60rem: rows stack, no horizontal scroll.

- [ ] **Step 8: Commit**

```bash
git add panel/src
git commit -m "feat(panel): settings pages for the server, FlickSync and FlickDD

One page per scope, built from the field list the admin API returns:
switches, numbers, text, lists, choices and write-only secrets, each with
where its value comes from. Save sends only real changes, keeps the edits
and marks the named field when the server refuses them, shows the new
revision and offers to apply the server settings or reload the module.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Panel README and final verification

**Files:**
- Modify: `panel/README.md`

**Interfaces:**
- Consumes: everything above.
- Produces: documentation only.

- [ ] **Step 1: Describe the new pages in the panel README**

In `panel/README.md`, replace the paragraph starting `FlickDD page (once FlickDD is enabled on the server, see` (one line) with:

```markdown
Overview: the server's state, the invitation link, and one card per module (FlickSync, FlickDD) with its state
(running, stopped, or failed to start with the reason), a "Reload required" badge when saved settings are not yet in
force, and Start / Stop / Reload. Stop and Reload ask first: they close rooms or cut downloads. A module stays as you
leave it, also after a server restart.

Settings (sidebar, or the Settings button of a module): one page per scope (Server, FlickSync, FlickDD), built from the
list of settings the server returns. Each setting shows where its value comes from (saved in the panel, the
environment, or the default); a value saved here wins over the environment and Reset removes it. Secrets are never
shown: replace or clear them. Save sends only what changed, shows the server's explanation when a value is refused,
then offers to apply the server settings or reload the module.

FlickDD page (once FlickDD runs with a Jellyfin or Plex backend set in its settings): downloads in progress with a stop button, recent downloads, bytes per day, top titles and the limits in force.
```

Replace the security bullet

```
* The admin token never reaches the browser. The browser calls the panel's own `/api/flicksync/*` routes, which need
  the session; those call FlickSync server-side. Closing a room also checks that the request is same-origin.
```

with

```
* The admin token and secret settings never reach the browser. The browser calls the panel's own `/api/*` routes,
  which need the session; those call the server's admin API. Every change (closing a room, cutting a download,
  starting or stopping a module, saving or applying settings) also checks that the request is same-origin.
```

In "Adding a component", replace step 2

```
2. Add an entry to `src/lib/modules.ts`; the sidebar and Overview pick it up.
```

with

```
2. Add an entry to `src/lib/modules.ts` (with its `settingsHref`); the sidebar, the Overview card and the settings
   tabs pick it up. The server must know the module id and its settings scope (see `docs/admin-api.md`); add the id
   to `MODULE_IDS` in `src/lib/module-status.ts` and the scope to `SETTINGS_SCOPES` in `src/lib/settings-form.ts`.
```

- [ ] **Step 2: Full verification**

```bash
cd panel
npm test 2>&1 | grep -E "^ℹ (tests|pass|fail)"
npm run typecheck && echo TYPECHECK_OK
npm run build 2>&1 | tail -25
git grep -n "FLICKSYNC_ADMIN_TOKEN" -- src start.mjs
git diff --stat main -- package.json package-lock.json
```

Expected: `ℹ fail 0`; `TYPECHECK_OK`; the build ends without error and lists `/settings/[scope]`, `/api/modules`, `/api/settings/[scope]`; the `git grep` shows only the deprecation mentions in `src/lib/admin-errors.ts`, `src/lib/admin-token.ts` and `src/lib/config.ts`; no change to `package.json` or `package-lock.json`.

- [ ] **Step 3: Production start with the derived token**

From the repository root, start the server as in Task 5 Step 9. Then, from `panel/` after `npm run build`:

```bash
ENABLE_WEB_PANEL=true PANEL_PASSWORD='correct horse battery staple' FLICKSYNC_URL=http://localhost:8787 npm start
```

Expected: `Flick Panel listening on http://0.0.0.0:3000`, no `FLICKSYNC_ADMIN_TOKEN` note; signing in shows the Overview with the Server card Online (the derived token is accepted). Restart with `FLICKSYNC_ADMIN_TOKEN=legacy-token-0123456789` added: the start prints the deprecation note and the Overview shows `The server rejected the panel's admin token ...` because this server was started without that legacy token, which proves the legacy value is the one sent.

- [ ] **Step 4: Commit**

```bash
git add panel/README.md
git commit -m "docs(panel): Modules view and settings pages

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-review

**Spec coverage**

| Spec section | Task |
|---|---|
| 2 Admin token, legacy precedence, `start.mjs` rule | 1 |
| 2 Modules view on the Overview, module cards, actions per state, confirmations | 3 (logic), 5 (UI) |
| 2 Navigation: one Settings entry, `/settings` redirect, scope tabs, Settings links | 5 (sidebar, links), 6 (pages, tabs) |
| 2 Module pages while stopped / failed / pending reload | 3 (`offText`), 5 (`ModuleGate`) |
| 2 Switch fields hidden, generic labels, server order, no client validation | 2 |
| 2 What Save sends; after Save follow-ups; server scope Apply button | 2 (`buildPatch`), 3 (`followUp`), 6 |
| 2 Polling (modules 5 s, settings once) | 5 (`useModules`), 6 (`usePoll(..., null)`) |
| 3.3 Controls per kind, hint line, trailing button, save bar, revision, flagged fields | 2 (hints, display), 6 (UI) |
| 4 Panel routes, body check, `adminFetch` POST / PUT, bare 404 for every method, timeouts | 2 (`parsePatchBody`), 4 |
| 5 Errors as `<message> (<CODE>)`, reworded texts, `FORBIDDEN`, `NOT_FOUND` | 1, 4, 5, 6 |
| 6 Code structure (pure modules, hooks, components, `Switch`, icons, CSS) | 1 to 6 |
| 7 Security (secrets write-only, `new-password`, same-origin) | 4, 6 |
| 8 Testing | 1, 2, 3 unit tests; 4 to 7 typecheck, build, manual |
| 9 Rollout, docs | one commit per task; 1 and 7 docs |

**Type consistency.** `ModuleId`, `ModuleStatus`, `ModulesResponse`, `SettingsScope`, `SettingField`, `SettingsView` (Task 2) are used unchanged in Tasks 3 to 6. `ModuleAction`, `confirmation`, `followUp` / `FollowUp` values `"apply-server" | "reload" | "start-again" | "applies-on-start" | "none"` match between Task 3 and `SettingsForm` (Task 6). `ModuleControl` (`busy`, `actionError`, `act`) is defined in Task 5 and consumed by `ModuleActionButton`, `ModuleCard`, `ModuleGate` and `SettingsForm`. `usePoll` returns `refresh: () => Promise<void>` and `mutate`; existing callers pass `refresh` as an `onClick`, which stays valid.

**Known judgment calls** (implementer: do not "fix" silently)
- A set `FLICKSYNC_ADMIN_TOKEN` wins over the derived token, so an old `.env` keeps working against any server version; removing the variable switches to the derived token.
- `FLICKSYNC_ENABLED` / `FLICKDD_ENABLED` are hidden from the form so that Start / Stop are the only switch.
- Typing a value equal to the current one (for example the default) does not pin it in the panel.
- After a save the panel offers Reload but never reloads by itself.

# Flick Panel manages the server: design

Sub-project 2 of 4 (see [2026-10-07-module-lifecycle-settings-design.md](2026-10-07-module-lifecycle-settings-design.md)):
the panel gets a Modules view (start / stop / reload), one Settings page per scope, and stops needing
`FLICKSYNC_ADMIN_TOKEN`. Everything it calls already exists on the server ([docs/admin-api.md](../../admin-api.md)).

## 1. Scope

In scope: panel UI and panel server routes for `/admin/v1/modules*` and `/admin/v1/settings*`, the admin token
derived from `PANEL_PASSWORD`, `panel/start.mjs`, `panel/src/lib/config.ts`, the panel README and the lines of the
other docs that still say the panel needs `FLICKSYNC_ADMIN_TOKEN`.

Out of scope: any server change (the server keeps accepting the legacy token), the log view (sub-project 3), the
`.env.example` / compose rewrite (sub-project 4), French UI, a new runtime dependency.

## 2. Decisions

| Question | Decision |
|---|---|
| Admin token | `hex(HMAC-SHA256(key = PANEL_PASSWORD.trim(), msg = "flick-admin-api-v1"))` with Node `crypto`, empty when the trimmed password has fewer than 10 bytes (the server's rule). Must give `602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011` for `correct horse battery staple`. |
| Legacy `FLICKSYNC_ADMIN_TOKEN` | Optional. When set (non-blank after trim) the panel sends it instead of the derived token, so an old `.env` keeps working with any server; `start.mjs` prints a deprecation note. Set but shorter than 16 characters: refused at start (same rule as the server). |
| `start.mjs` password rule | `PANEL_PASSWORD.trim()` must have at least 10 characters (so the server's 10-byte rule always holds). The login still compares the untrimmed value, as today. |
| Where the Modules view lives | On the Overview: one card per module (replacing today's per-module panels), plus a Server card. The same card component appears on module pages and module settings pages. |
| Navigation | Sidebar: Overview, FlickSync, FlickDD (unchanged), then one **Settings** entry. `/settings` redirects to `/settings/server`; the settings pages carry tabs Server / FlickSync / FlickDD. Every module card and module page header has a Settings link. |
| Module page while stopped or failed | The page content is replaced by the module card (state, failure message, Start) and one sentence explaining what is off. Running with `pending_reload`: the card (with Reload) is shown above the content. This replaces `DdGate` and the 503 errors FlickSync's panels show today. |
| Disruptive actions | Stop and Reload of a **running** module ask for confirmation in a dialog that says what is interrupted. Start, and Stop of a failed module, run at once. |
| Module switch fields | `FLICKSYNC_ENABLED` and `FLICKDD_ENABLED` are not shown in the settings form: Start / Stop own them. |
| Field labels | Generic, from the variable name: prefix dropped, sentence case, known acronyms kept (`FLICKSYNC_PUBLIC_URL` -> "Public URL"). The variable name is shown under the label. No per-field help table in the panel. |
| Order and grouping | The server's field order, in one list per scope. |
| Client-side validation | None. The server validates the merged scope and its message is shown; the panel only enforces the PUT body shape. |
| What Save sends | Only real changes: a value equal to the current one (booleans compared as booleans) is not sent; an untouched or emptied secret is not sent; `null` only for a field whose source is `panel`. |
| After Save | Never reloads by itself (a reload closes rooms and cuts downloads). It shows the new revision and offers the next step: server scope -> **Apply Now** (`POST /settings/server/reload`); running module with `pending_reload` -> **Reload** (with confirmation); failed module -> **Start**; stopped module -> a note that the settings apply when it starts. |
| Server scope pending state | The API has no `pending_reload` for the server scope, so the server settings page always has an **Apply Server Settings** button (applying is idempotent and does not touch modules). |
| Polling | Modules: every 5 s (paused while the tab is hidden). Settings: loaded once, then replaced by the PUT answer; no polling while editing. |
| Language and copy | English, the design system's content rules (Title Case buttons, sentence case text, no emoji, no exclamation marks). |

## 3. Pages

### 3.1 Overview

1. **Server** card: Online / Not ready (from `ready`), version, uptime, link **Server Settings**. On error: the error text.
2. Invitation link (unchanged).
3. One **module card** per module (FlickSync, FlickDD), in `MODULES` order.

Module card: title, state pill (**Running** / **Stopped** / **Failed to start**), **Reload required** pill when
`pending_reload`, "Running for 2 h 05 min" from `since`, the failure `message` as an error notice, the module summary,
the live figures while running (FlickSync: rooms, participants, connections; FlickDD: backends, active, downloads,
bytes served), the action buttons, and the links **Open** and **Settings**. Actions by state: running -> Reload, Stop;
stopped -> Start; failed -> Start, Stop. After an action the Overview re-renders its server-side figures.

### 3.2 Module pages

`/flicksync` and `/flickdd` keep their panels, wrapped in a module gate (decision table). Their header gets a
**Settings** link. Stopped texts: FlickSync "nobody can watch together on this server"; FlickDD "offline downloads are
not available; set up a Jellyfin or Plex backend in its settings, then start it". Failed: the server's message, then
"change the setting in its settings page, then start it again".

### 3.3 Settings pages

`/settings/{server|flicksync|flickdd}` (anything else: 404). Header "Settings" with a one-line lead per scope, the scope
tabs, then for a module scope its module card (without the Settings link), then the form, then a sticky save bar.

Each field row: label, variable name, a hint line, the control, and one trailing button.

| Kind | Control |
|---|---|
| `bool` | The design system switch (`.fk-switch`); sends `"true"` / `"false"` |
| `int`, `float` | Text input, `inputMode` numeric / decimal (the exact string is sent) |
| `text`, `list` | Text input; `list` hint says "comma-separated" |
| `choice` | Select of `choices` (plus the current value if it is not one of them) |
| `secret` | Pill **Set** / **Not set**; **Replace** (or **Set Value**) opens a password input (`autocomplete="new-password"`) |

Hint line: the source (**Saved in the panel**, **From the environment**, **Default**; a secret with no value reads
**Not set**), the default when the source is not `default`, "comma-separated" for lists, and for a boolean whose current
value is not an on/off word, that fact. A secret from the environment adds "a value saved here overrides it".

Trailing button: **Undo** when the field has a pending change; **Cancel** while a secret's input is open and empty;
otherwise **Reset** (non-secret) or **Clear** (secret) when the source is `panel`, which sends `null` on save and
says what will apply instead. Nothing for environment or default values (sending `null` would change nothing).

Save bar: "No unsaved changes" / "N unsaved changes", **Discard Changes**, **Save Changes**. The panel title shows
**Revision N**. A failed save keeps every edit, shows `<message> (<CODE>)` in the save bar, and marks each field
whose variable name appears in the message. A successful save replaces the view with the server's answer, clears the
edits and shows the follow-up of section 2.

## 4. Panel server routes

All new routes need the session; every mutation also needs `sameOrigin()`. Answers are `no-store`. The server's
answer is passed on as is (status and body); ids, actions and scopes are checked against fixed lists first (else
`404 NOT_FOUND`).

| Panel route | Calls |
|---|---|
| `GET /api/modules` | `GET /admin/v1/modules` |
| `POST /api/modules/{id}/{start\|stop\|reload}` | `POST /admin/v1/modules/{id}/{action}` |
| `GET /api/settings/{scope}` | `GET /admin/v1/settings/{scope}` |
| `PUT /api/settings/{scope}` | `PUT /admin/v1/settings/{scope}` with `{"values": ...}` after the panel checks the body: JSON, at most 64 KiB, `values` an object of `NAME` (`^[A-Z][A-Z0-9_]{0,63}$`) to string or `null`, at least one entry; else `400 INVALID_PAYLOAD` with a message |
| `POST /api/settings/server/reload` | `POST /admin/v1/settings/server/reload` |

`adminFetch` gains `POST` and `PUT` with a JSON body (`content-type: application/json`), a 15 s timeout for
mutations (5 s for reads), and treats a bare `404` (no error body) as `ADMIN_API_DISABLED` for every method.

## 5. Errors

Every error is shown as `<message> (<CODE>)` through `adminError` in `panel/src/lib/admin-errors.ts`; panel-side
codes without a message use its text table. That table is reworded for the derived token (`ADMIN_TOKEN_MISSING`,
`ADMIN_API_DISABLED`, `ADMIN_TOKEN_REJECTED` name `PANEL_PASSWORD`; `ADMIN_TOKEN_REJECTED` also mentions the legacy
token) and gains `FORBIDDEN` and `NOT_FOUND`. A module that fails to start is not an error: the action succeeds and
the card shows **Failed to start** with the server's message.

## 6. Code structure

Logic that decides what to show or send lives in pure TypeScript modules with no React or Next import, loaded as is
by `node --test`:

| Module | Content |
|---|---|
| `src/lib/admin-token.ts` | `deriveAdminToken`, `resolveAdminToken` |
| `panel/env-check.mjs` (+ `.d.mts`) | `panelEnabled`, `checkPanelEnv`, used by `start.mjs` (copied into the Docker image) |
| `src/lib/settings-form.ts` | scopes, labels, sources, `Edits`, `setEdit` / `resetEdit` / `undoEdit`, `buildPatch`, `fieldDisplay`, `fieldHint`, `choiceOptions`, `namedFields`, `parsePatchBody` |
| `src/lib/module-status.ts` | ids, actions, badges, actions per state, confirmations, `sinceText`, `offText`, `mergeStatus`, `followUp`, `followUpText` |

React side: `usePoll` (generalised from `useAdmin`, which stays as a wrapper), `useModules`, `ModuleCard` /
`ModuleActionButton`, `ModuleGate`, `SettingsForm`, `FieldRow`, `ScopeTabs`, a `Switch` in `components/flick/ui.tsx`,
four icons (`play`, `square`, `settings`, `server`), and panel CSS built from existing tokens only.

## 7. Security

- The admin token and every secret value stay server-side; the browser only ever sends a new secret (PUT) and never
  receives one (the API returns only `set`).
- Secret inputs exist only after **Replace** and use `autocomplete="new-password"`, so the browser does not fill the
  panel password into them; an empty secret input sends nothing.
- Mutations: session cookie (`SameSite=Strict`) plus the same-origin check, as for closing a room. No change to the
  session, the sign-in limiter or the CSP.

## 8. Testing

`node --test` (no new dependency): token vector, trimming and byte length, legacy precedence; start-up checks; field
labels; edits and patch building (unchanged values, boolean spellings, secrets, `null` only for panel values, stale
names ignored); hints; `namedFields` word boundaries; PUT body checks; module badges, actions, confirmations,
follow-ups, `mergeStatus`; the reworded error texts. `tsc --noEmit` and `next build` clean. A manual pass against a
local server covers the pages.

## 9. Rollout

One commit per concern, each leaving `npm test`, `npm run typecheck` and `npm run build` green: token and start-up;
settings logic; module logic; proxy routes; Modules view and navigation; settings pages; docs and final check.

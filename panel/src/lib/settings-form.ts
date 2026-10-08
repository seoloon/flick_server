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

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
  const [discards, setDiscards] = useState(0); // remounts the rows, closing open secret inputs

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
              key={`${f.name}:${view.revision}:${discards}`}
              field={f}
              edits={edits}
              flagged={flagged.includes(f.name)}
              onEdit={(update) => {
                setApplied(false);
                setEdits(update);
              }}
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
                setDiscards((n) => n + 1);
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

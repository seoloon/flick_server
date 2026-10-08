"use client";

import { type ReactNode, useEffect, useState } from "react";

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

  // The module's state changed under an open dialog (a poll): the question no longer applies.
  useEffect(() => {
    if (!ask) setAsking(false);
  }, [ask]);

  async function run() {
    const ok = await control.act(status.id, action);
    setAsking(false); // a failure is shown once, on the card
    if (ok) onDone?.();
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
      {/* Always above the children: the module page's text refers to "the reason above". */}
      {status.state === "failed" && (
        <Notice tone="error">
          {status.message?.trim() || `${mod.label} could not start and the server gave no reason. Check the server log.`}
        </Notice>
      )}
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

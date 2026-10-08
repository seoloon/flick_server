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

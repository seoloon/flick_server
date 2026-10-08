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

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

"use client";

import { useState } from "react";

import { Button, Dialog, EmptyState, Notice, Panel, Pill, Spinner } from "@/components/flick/ui";
import { BACKEND_LABEL, formatBytes, formatSpeed } from "@/lib/format";
import type { DdActive, DdActiveDownload, DdOverview } from "@/lib/types";
import { ADMIN_ERROR_TEXT, useAdmin } from "@/lib/use-admin";

function percent(d: DdActiveDownload): number {
  return d.size > 0 ? Math.min(100, (d.covered / d.size) * 100) : 0;
}

export function ActivePanel() {
  const active = useAdmin<DdActive>("active", 2_000, "flickdd");
  const overview = useAdmin<DdOverview>("overview", 5_000, "flickdd");
  const [target, setTarget] = useState<DdActiveDownload | null>(null);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  async function cut() {
    if (!target) return;
    setBusy(true);
    setFailure(null);
    try {
      const res = await fetch(`/api/flickdd/downloads/${encodeURIComponent(target.download_id)}`, {
        method: "DELETE",
      });
      // 404 means it already ended: the goal is reached either way.
      if (res.status === 204 || res.status === 404) {
        setTarget(null);
        active.refresh();
        overview.refresh();
      } else if (res.status === 401) {
        window.location.assign("/login");
      } else {
        setFailure("The download could not be cut. Try again.");
      }
    } catch {
      setFailure(ADMIN_ERROR_TEXT.UNREACHABLE);
    } finally {
      setBusy(false);
    }
  }

  const list = active.data?.downloads ?? [];
  const slots = overview.data
    ? `${list.length} of ${overview.data.limits.max_global} slots in use`
    : undefined;
  const err = active.error;

  return (
    <Panel
      title={`Active downloads${active.data ? ` · ${list.length}` : ""}`}
      actions={
        <Button variant="ghost" size="icon-sm" icon="refresh-cw" label="Refresh downloads" onClick={active.refresh} />
      }
    >
      {active.loading && !active.data && <Spinner label="Loading downloads" />}
      {err && <Notice tone={active.data ? "warn" : "error"}>{ADMIN_ERROR_TEXT[err]}</Notice>}
      {slots && (
        <p className="dl__meta" style={{ margin: 0 }}>
          {slots}
        </p>
      )}
      {active.data && list.length === 0 && (
        <EmptyState title="No downloads right now">
          Transfers appear here while someone downloads a title for offline viewing.
        </EmptyState>
      )}
      {list.length > 0 && (
        <div role="list" aria-label="Active downloads">
          {list.map((d) => (
            <div key={d.download_id} className="dl" role="listitem">
              <span className="dl__title">
                {d.title ?? d.item_id}
                <small> · {d.user_id}</small>
              </span>
              <span style={{ display: "flex", gap: "0.5rem", alignItems: "center" }}>
                <Pill tone="plain">{BACKEND_LABEL[d.backend] ?? d.backend}</Pill>
                <Button
                  variant="danger"
                  size="sm"
                  icon="x"
                  onClick={() => {
                    setFailure(null);
                    setTarget(d);
                  }}
                >
                  Cut
                </Button>
              </span>
              <div className="dl__progress">
                <span className="bar__track" aria-hidden="true">
                  <span style={{ width: `${percent(d)}%` }} />
                </span>
                <span className="bar__num">{formatSpeed(d.speed_bps)}</span>
              </div>
              <span className="dl__meta">
                <span>
                  {formatBytes(d.covered)} / {formatBytes(d.size)}
                </span>
                <span>
                  {d.resumes} {d.resumes === 1 ? "resume" : "resumes"}
                </span>
                <span>{d.user_name}</span>
              </span>
            </div>
          ))}
        </div>
      )}

      {target && (
        <Dialog
          title="Cut this download?"
          description={`The transfer of ${target.title ?? target.item_id} for ${target.user_id} stops and its slot is freed. The user can start it again.`}
          onClose={() => !busy && setTarget(null)}
        >
          {failure && <Notice tone="error">{failure}</Notice>}
          <div className="row-actions">
            <Button variant="danger" icon="x" onClick={cut} disabled={busy}>
              {busy ? "Cutting" : "Cut Download"}
            </Button>
            <Button variant="ghost" onClick={() => setTarget(null)} disabled={busy}>
              Cancel
            </Button>
          </div>
        </Dialog>
      )}
    </Panel>
  );
}

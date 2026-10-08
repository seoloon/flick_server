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

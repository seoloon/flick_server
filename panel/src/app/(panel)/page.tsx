import { ButtonLink, Notice, Pill } from "@/components/flick/ui";
import { PageHeader, Panel } from "@/components/flick/ui";
import { BACKEND_LABEL, formatBytes, formatCount, formatDuration } from "@/lib/format";
import { adminFetch, ddFetch, type UpstreamResult } from "@/lib/flicksync";
import { MODULES } from "@/lib/modules";
import type { DdOverview, Overview } from "@/lib/types";

import { InvitePanel } from "./InvitePanel";

export const metadata = { title: "Overview" };

function status<T>(res: UpstreamResult) {
  if (res.status === 200) return { data: res.body as T };
  const code = (res.body as { error?: { code?: string } } | null)?.error?.code ?? "UNREACHABLE";
  return { code };
}

export default async function OverviewPage() {
  const [sync, dd] = await Promise.all([adminFetch("overview"), ddFetch("overview")]);
  const syncStatus = status<Overview>(sync);
  const ddStatus = status<DdOverview>(dd);
  const o = "data" in syncStatus ? syncStatus.data : null;
  const d = "data" in ddStatus ? ddStatus.data : null;
  const ddOff = "code" in ddStatus && ddStatus.code === "DD_DISABLED";

  return (
    <div className="panel-page">
      <PageHeader title="Overview" lead="Your Flick Server components at a glance." />
      <InvitePanel />
      {MODULES.map((m) => (
        <Panel
          key={m.id}
          title={m.label}
          actions={
            <ButtonLink href={m.href} variant="primary" size="sm">
              Open
            </ButtonLink>
          }
        >
          <p style={{ margin: 0, color: "var(--muted-foreground)" }}>{m.summary}</p>
          {m.id === "flicksync" &&
            (o ? (
              <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem", alignItems: "center" }}>
                <Pill tone={o.ready ? "strong" : "warn"}>{o.ready ? "Online" : "Not ready"}</Pill>
                <Pill>v{o.version}</Pill>
                <Pill>
                  {formatCount(o.rooms)} {o.rooms === 1 ? "room" : "rooms"}
                </Pill>
                <Pill>
                  {formatCount(o.participants)} {o.participants === 1 ? "participant" : "participants"}
                </Pill>
                <Pill>Up {formatDuration(o.uptime_secs)}</Pill>
              </div>
            ) : (
              <Notice tone="warn">
                FlickSync is not reachable from the panel. Open the FlickSync page for details.
              </Notice>
            ))}
          {m.id === "flickdd" &&
            (d ? (
              <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem", alignItems: "center" }}>
                <Pill tone="strong">Online</Pill>
                {o && <Pill>v{o.version}</Pill>}
                {(["jellyfin", "plex"] as const)
                  .filter((b) => d.backends[b])
                  .map((b) => (
                    <Pill key={b}>{BACKEND_LABEL[b]}</Pill>
                  ))}
                <Pill>
                  {formatCount(d.active)} / {formatCount(d.limits.max_global)} active
                </Pill>
                <Pill>
                  {formatCount(d.totals.downloads)} {d.totals.downloads === 1 ? "download" : "downloads"}
                </Pill>
                <Pill>{formatBytes(d.totals.bytes_served)} served</Pill>
              </div>
            ) : ddOff ? (
              <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem", alignItems: "center" }}>
                <Pill tone="warn">Disabled</Pill>
                {o && <Pill>v{o.version}</Pill>}
              </div>
            ) : (
              <Notice tone="warn">
                FlickDD is not reachable from the panel. Open the FlickDD page for details.
              </Notice>
            ))}
        </Panel>
      ))}
    </div>
  );
}

import { ButtonLink, Notice, Pill } from "@/components/flick/ui";
import { PageHeader, Panel } from "@/components/flick/ui";
import { formatCount, formatDuration } from "@/lib/format";
import { adminFetch } from "@/lib/flicksync";
import { MODULES } from "@/lib/modules";
import type { Overview } from "@/lib/types";

export const metadata = { title: "Overview" };

async function flicksyncStatus() {
  const res = await adminFetch("overview");
  if (res.status === 200) return { overview: res.body as Overview };
  const code = (res.body as { error?: { code?: string } } | null)?.error?.code ?? "UNREACHABLE";
  return { code };
}

export default async function OverviewPage() {
  const status = await flicksyncStatus();
  const o = "overview" in status ? status.overview : null;

  return (
    <div className="panel-page">
      <PageHeader title="Overview" lead="Your Flick Server components at a glance." />
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
        </Panel>
      ))}
    </div>
  );
}

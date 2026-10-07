"use client";

import { niceMax } from "@/components/charts";
import { Notice, Panel, Spinner } from "@/components/flick/ui";
import { BACKEND_LABEL, OUTCOME_LABEL, formatBytes, formatCount, formatUtcDate } from "@/lib/format";
import type { DdStats } from "@/lib/types";
import { ADMIN_ERROR_TEXT, useAdmin } from "@/lib/use-admin";

function Tile({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="tile fk-glass">
      <span className="tile__label">{label}</span>
      <span className="tile__value">{value}</span>
      {sub && <span className="tile__sub">{sub}</span>}
    </div>
  );
}

const W = 520;
const H = 150;
const PAD = { l: 8, r: 8, t: 8, b: 18 };

function DayBars({ days }: { days: DdStats["days"] }) {
  const peak = Math.max(...days.map((d) => d.bytes), 0);
  const total = days.reduce((a, d) => a + d.bytes, 0);
  if (total === 0) {
    return <div className="chart__empty">No bytes served in this period yet.</div>;
  }
  const max = niceMax(peak);
  const slot = (W - PAD.l - PAD.r) / Math.max(1, days.length);
  return (
    <svg
      className="daybars"
      viewBox={`0 0 ${W} ${H}`}
      role="img"
      aria-label={`Bytes served per day over ${days.length} days, ${formatBytes(total)} in total, peak ${formatBytes(peak)}`}
    >
      {days.map((d, i) => {
        const h = (d.bytes / max) * (H - PAD.t - PAD.b);
        return (
          <rect
            key={d.day}
            x={PAD.l + i * slot + 1}
            y={H - PAD.b - Math.max(d.bytes > 0 ? 2 : 0, h)}
            width={Math.max(1, slot - 2)}
            height={Math.max(d.bytes > 0 ? 2 : 0, h)}
            rx={1.5}
          >
            <title>{`${formatUtcDate(d.date_ms)}: ${formatBytes(d.bytes)}`}</title>
          </rect>
        );
      })}
    </svg>
  );
}

export function StatsPanel() {
  const { data: s, error, loading } = useAdmin<DdStats>("stats", 10_000, "flickdd");

  return (
    <>
      {error && <Notice tone={s ? "warn" : "error"}>{ADMIN_ERROR_TEXT[error]}</Notice>}
      {loading && !s && <Spinner label="Loading statistics" />}
      {s && (
        <>
          <div className="tiles">
            <Tile label="Downloads" value={formatCount(s.totals.downloads)} sub="finished since startup" />
            <Tile label="Completed" value={formatCount(s.totals.completed)} />
            <Tile label="Bytes served" value={formatBytes(s.totals.bytes_served)} />
            <Tile label="Resumes" value={formatCount(s.totals.resumes)} />
            <Tile label="Upstream errors" value={formatCount(s.totals.upstream_errors)} />
          </div>

          <Panel title="Bytes served per day">
            <DayBars days={s.days} />
          </Panel>

          <Panel title="Most downloaded titles">
            {s.top_titles.length === 0 ? (
              <div className="chart__empty">Completed downloads rank here.</div>
            ) : (
              <ol className="people" aria-label="Top titles">
                {s.top_titles.map((t) => (
                  <li key={`${t.backend}-${t.item_id}`}>
                    <span className="who">{t.title ?? t.item_id}</span>
                    <span className="num">{formatBytes(t.bytes)}</span>
                    <span className="num">{formatCount(t.count)}×</span>
                  </li>
                ))}
              </ol>
            )}
          </Panel>

          <Panel title="By source and outcome">
            <dl className="fk-facts">
              {Object.entries(s.by_backend).map(([name, b]) => (
                <div key={name} style={{ display: "contents" }}>
                  <dt>{BACKEND_LABEL[name] ?? name}</dt>
                  <dd>
                    {formatBytes(b.bytes)} · {formatCount(b.downloads)} downloads · {formatCount(b.completed)} completed
                  </dd>
                </div>
              ))}
              {Object.entries(s.by_outcome).map(([name, n]) => (
                <div key={name} style={{ display: "contents" }}>
                  <dt>{OUTCOME_LABEL[name] ?? name.replaceAll("_", " ")}</dt>
                  <dd>{formatCount(n)}</dd>
                </div>
              ))}
            </dl>
          </Panel>
        </>
      )}
    </>
  );
}

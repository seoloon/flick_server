"use client";

import { useMemo, useState } from "react";

import { LineChart, type Series } from "@/components/charts";
import { Notice, Panel, Segmented, Spinner } from "@/components/flick/ui";
import { formatCount, formatDuration, formatMs, formatPercent } from "@/lib/format";
import type { Overview, Sample, Stats } from "@/lib/types";
import { useAdmin } from "@/lib/use-admin";

type Range = "10" | "30" | "60";

const RANGES: { value: Range; label: string }[] = [
  { value: "10", label: "10 min" },
  { value: "30", label: "30 min" },
  { value: "60", label: "1 hour" },
];

/** Per-minute rate of a cumulative counter between consecutive samples. */
function perMinute(samples: Sample[], pick: (s: Sample) => number): (number | null)[] {
  return samples.map((s, i) => {
    if (i === 0) return null;
    const dt = (s.t - samples[i - 1].t) / 60000;
    const dv = pick(s) - pick(samples[i - 1]);
    return dt > 0 && dv >= 0 ? dv / dt : null;
  });
}

function Tile({
  label,
  value,
  unit,
  sub,
}: {
  label: string;
  value: string;
  unit?: string;
  sub?: string;
}) {
  return (
    <div className="tile fk-glass">
      <span className="tile__label">{label}</span>
      <span className="tile__value">
        {value}
        {unit && <span className="tile__unit">{unit}</span>}
      </span>
      {sub && <span className="tile__sub">{sub}</span>}
    </div>
  );
}

function Distribution({ stats }: { stats: Stats }) {
  const { buckets, thresholds_ms: t } = stats.drift;
  const total = buckets.reduce((a, b) => a + b, 0);
  const rows = [
    { label: "In sync", hint: `under ${t.ignore} ms`, n: buckets[0] },
    { label: "Slightly off", hint: `${t.ignore} to ${t.soft} ms, eased with speed`, n: buckets[1] },
    { label: "Noticeably off", hint: `${t.soft} to ${t.hard} ms, stronger speed change`, n: buckets[2] },
    { label: "Out of sync", hint: `${t.hard} ms and over, jump to the right place`, n: buckets[3], hard: true },
  ];
  if (total === 0) {
    return (
      <div className="chart__empty">
        No sync reports yet. They arrive while people watch together.
      </div>
    );
  }
  return (
    <div className="bars" role="list" aria-label="Distribution of measured drift">
      {rows.map((r) => (
        <div key={r.label} className="bar" role="listitem" data-hard={r.hard && r.n > 0 ? "" : undefined}>
          <span className="bar__label">
            {r.label}
            <small>{r.hint}</small>
          </span>
          <span className="bar__track" aria-hidden="true">
            <span style={{ width: `${(r.n / total) * 100}%` }} />
          </span>
          <span className="bar__num">
            {formatCount(r.n)} · {formatPercent(r.n, total)}
          </span>
        </div>
      ))}
    </div>
  );
}

export function StatsPanel() {
  const stats = useAdmin<Stats>("stats", 10_000);
  const overview = useAdmin<Overview>("overview", 5_000);
  const [range, setRange] = useState<Range>("30");

  const s = stats.data;
  const o = overview.data;

  const view = useMemo(() => {
    if (!s) return null;
    const keep = Math.ceil((Number(range) * 60) / s.history_interval_secs) + 1;
    const h = s.history.slice(-keep);
    const times = h.map((x) => x.t);
    const activity: Series[] = [
      { name: "Participants", values: h.map((x) => x.participants) },
      { name: "Rooms", values: h.map((x) => x.rooms), dash: "5 4", opacity: 0.7 },
    ];
    const latency: Series[] = [
      {
        name: "Average round trip",
        values: h.map((x) => (x.rtt_ms > 0 ? x.rtt_ms : null)),
        format: (v) => String(Math.round(v)),
      },
    ];
    const rate = (v: number) => (v < 10 ? v.toFixed(1) : String(Math.round(v)));
    const corrections: Series[] = [
      { name: "Speed adjustments", values: perMinute(h, (x) => x.corrections - x.seeks), format: rate },
      { name: "Jumps", values: perMinute(h, (x) => x.seeks), dash: "5 4", opacity: 0.7, format: rate },
    ];
    const traffic: Series[] = [
      { name: "Messages received", values: perMinute(h, (x) => x.messages_in), format: (v) => String(Math.round(v)) },
      { name: "Sync reports", values: perMinute(h, (x) => x.reports), dash: "5 4", opacity: 0.7, format: (v) => String(Math.round(v)) },
    ];
    return { times, activity, latency, corrections, traffic };
  }, [s, range]);

  const err = stats.error ? stats.errorText : overview.errorText;

  return (
    <>
      {err && !s && <Notice tone="error">{err}</Notice>}
      {err && s && <Notice tone="warn">{err}</Notice>}
      {stats.loading && !s && <Spinner label="Loading statistics" />}

      <div className="tiles">
        <Tile
          label="Rooms"
          value={o ? formatCount(o.rooms) : "n/a"}
          sub={o ? `of ${formatCount(o.max_rooms)} allowed` : undefined}
        />
        <Tile label="Participants" value={o ? formatCount(o.participants) : "n/a"} sub="across all rooms" />
        <Tile
          label="Connections"
          value={o ? formatCount(o.connections) : "n/a"}
          sub={o ? `of ${formatCount(o.max_connections)} allowed` : undefined}
        />
        <Tile
          label="Latency"
          value={o && o.rtt_avg_ms > 0 ? String(Math.round(o.rtt_avg_ms)) : "n/a"}
          unit={o && o.rtt_avg_ms > 0 ? "ms" : undefined}
          sub="average round trip"
        />
        <Tile
          label="Uptime"
          value={o ? formatDuration(o.uptime_secs) : "n/a"}
          sub={o ? `FlickSync ${o.version}` : undefined}
        />
      </div>

      {s && view && (
        <>
          <Panel
            title="Activity"
            actions={<Segmented label="Time range" options={RANGES} value={range} onChange={setRange} />}
          >
            <div className="charts">
              <div>
                <h3 className="tile__label" style={{ margin: "0 0 0.5rem" }}>People and rooms</h3>
                <LineChart title="Participants and rooms" times={view.times} series={view.activity} />
              </div>
              <div>
                <h3 className="tile__label" style={{ margin: "0 0 0.5rem" }}>Latency</h3>
                <LineChart title="Average latency" times={view.times} series={view.latency} unit=" ms" minMax={50} />
              </div>
              <div>
                <h3 className="tile__label" style={{ margin: "0 0 0.5rem" }}>Traffic per minute</h3>
                <LineChart title="Traffic per minute" times={view.times} series={view.traffic} minMax={10} />
              </div>
              <div>
                <h3 className="tile__label" style={{ margin: "0 0 0.5rem" }}>Sync corrections per minute</h3>
                <LineChart title="Sync corrections per minute" times={view.times} series={view.corrections} minMax={2} />
              </div>
            </div>
          </Panel>

          <Panel title="Sync quality">
            <p style={{ margin: 0, color: "var(--muted-foreground)", lineHeight: 1.6 }}>
              How far each viewer was from the shared playback position when they reported it, since FlickSync started
              {s.totals.sync_reports > 0 ? ` · ${formatCount(s.totals.sync_reports)} reports` : ""}.
            </p>
            <Distribution stats={s} />
            <dl className="fk-facts">
              <dt>Speed adjustments</dt>
              <dd>{formatCount(s.totals.sync_corrections - s.totals.sync_seeks)}</dd>
              <dt>Jumps</dt>
              <dd>
                {formatCount(s.totals.sync_seeks)} · {formatPercent(s.totals.sync_seeks, s.totals.sync_reports)} of
                reports
              </dd>
              <dt>Average latency</dt>
              <dd>{formatMs(s.rtt_avg_ms > 0 ? s.rtt_avg_ms : null)}</dd>
            </dl>
          </Panel>

          <Panel title="Since startup">
            <dl className="fk-facts">
              <dt>Rooms created</dt>
              <dd>{formatCount(s.totals.rooms_created)}</dd>
              <dt>Rooms ended</dt>
              <dd>{formatCount(s.totals.rooms_destroyed)}</dd>
              <dt>Messages received</dt>
              <dd>{formatCount(s.totals.messages_in)}</dd>
              <dt>Malformed messages</dt>
              <dd>{formatCount(s.totals.malformed_messages)}</dd>
              <dt>Rate limited</dt>
              <dd>{formatCount(s.totals.rate_limited)}</dd>
              <dt>Failed sign-ins</dt>
              <dd>{formatCount(s.totals.auth_failures)}</dd>
            </dl>
          </Panel>
        </>
      )}
    </>
  );
}

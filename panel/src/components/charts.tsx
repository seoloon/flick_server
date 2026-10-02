"use client";

// Dependency-free SVG charts in Flick's monochrome language: white strokes told apart by
// weight and dash pattern (never by hue alone), hairline grid, tabular numbers.
import { useId, useState } from "react";

export interface Series {
  name: string;
  values: (number | null)[];
  /** SVG dash array; solid when omitted. */
  dash?: string;
  /** Stroke opacity 0 to 1 (default 1). */
  opacity?: number;
  format?: (v: number) => string;
}

const W = 520;
const H = 170;
const PAD = { l: 38, r: 8, t: 8, b: 20 };

/** Round the axis maximum up to 1, 2, 5 times a power of ten. */
export function niceMax(v: number): number {
  if (!(v > 0)) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  const n = v / p;
  return (n <= 1 ? 1 : n <= 2 ? 2 : n <= 5 ? 5 : 10) * p;
}

export function LineChart({
  title,
  times,
  series,
  unit = "",
  minMax = 1,
}: {
  title: string;
  /** Wall-clock ms for each point. */
  times: number[];
  series: Series[];
  unit?: string;
  /** Smallest value the y axis may top out at. */
  minMax?: number;
}) {
  const [hover, setHover] = useState<number | null>(null);
  const clip = useId();
  const n = times.length;

  if (n < 2) {
    return (
      <div className="chart__empty" role="img" aria-label={`${title}: collecting data`}>
        Collecting data. A point is added every few seconds.
      </div>
    );
  }

  const all = series.flatMap((s) => s.values).filter((v): v is number => v != null);
  const max = niceMax(Math.max(minMax, ...all, 0));
  const x = (i: number) => PAD.l + (i / (n - 1)) * (W - PAD.l - PAD.r);
  const y = (v: number) => PAD.t + (1 - v / max) * (H - PAD.t - PAD.b);
  const ticks = [0, max / 2, max];
  const span = times[n - 1] - times[0];
  const label = (ms: number) => {
    const ago = Math.round((times[n - 1] - ms) / 60000);
    return ago <= 0 ? "now" : `-${ago} min`;
  };

  const path = (vals: (number | null)[]) => {
    let d = "";
    let pen = false;
    vals.forEach((v, i) => {
      if (v == null) {
        pen = false;
        return;
      }
      d += `${pen ? "L" : "M"}${x(i).toFixed(1)} ${y(v).toFixed(1)}`;
      pen = true;
    });
    return d;
  };

  const onMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const px = ((e.clientX - r.left) / r.width) * W;
    const i = Math.round(((px - PAD.l) / (W - PAD.l - PAD.r)) * (n - 1));
    setHover(Math.max(0, Math.min(n - 1, i)));
  };

  const summary = series
    .map((s) => {
      const last = [...s.values].reverse().find((v) => v != null);
      return `${s.name} ${last == null ? "no data" : (s.format ?? String)(last)}${unit}`;
    })
    .join(", ");

  return (
    <div className="chart">
      <svg
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label={`${title}. Latest: ${summary}`}
        onPointerMove={onMove}
        onPointerLeave={() => setHover(null)}
      >
        <defs>
          <clipPath id={clip}>
            <rect x={PAD.l} y={PAD.t - 2} width={W - PAD.l - PAD.r} height={H - PAD.t - PAD.b + 4} />
          </clipPath>
        </defs>
        <g className="chart__grid">
          {ticks.map((t) => (
            <line key={t} x1={PAD.l} x2={W - PAD.r} y1={y(t)} y2={y(t)} />
          ))}
        </g>
        <g className="chart__axis">
          {ticks.map((t) => (
            <text key={t} x={PAD.l - 6} y={y(t) + 3.5} textAnchor="end">
              {Number.isInteger(t) ? t : t.toFixed(1)}
            </text>
          ))}
          <text x={PAD.l} y={H - 5} textAnchor="start">
            {span >= 60000 ? label(times[0]) : ""}
          </text>
          <text x={W - PAD.r} y={H - 5} textAnchor="end">
            now
          </text>
        </g>
        <g clipPath={`url(#${clip})`} fill="none" strokeLinecap="round" strokeLinejoin="round">
          {series.map((s) => (
            <path
              key={s.name}
              d={path(s.values)}
              stroke="#fff"
              strokeOpacity={s.opacity ?? 1}
              strokeWidth={s.dash ? 1.6 : 2.2}
              strokeDasharray={s.dash}
            />
          ))}
        </g>
        {hover != null && (
          <g>
            <line
              x1={x(hover)}
              x2={x(hover)}
              y1={PAD.t}
              y2={H - PAD.b}
              stroke="#fff"
              strokeOpacity={0.35}
              strokeWidth={1}
            />
            {series.map((s) => {
              const v = s.values[hover];
              return v == null ? null : (
                <circle key={s.name} cx={x(hover)} cy={y(v)} r={3.5} fill="#fff" fillOpacity={s.opacity ?? 1} />
              );
            })}
          </g>
        )}
      </svg>
      {hover != null && (
        <div
          className="chart__tip fk-glass-strong"
          style={{ left: `clamp(0%, calc(${(x(hover) / W) * 100}% - 4.5rem), calc(100% - 9.5rem))` }}
        >
          <b>{label(times[hover])}</b>
          {series.map((s) => {
            const v = s.values[hover];
            return (
              <div key={s.name}>
                {s.name}: {v == null ? "n/a" : `${(s.format ?? String)(v)}${unit}`}
              </div>
            );
          })}
        </div>
      )}
      <Legend series={series} />
    </div>
  );
}

export function Legend({ series }: { series: Series[] }) {
  return (
    <div className="chart__legend">
      {series.map((s) => (
        <span key={s.name}>
          <svg viewBox="0 0 24 8" aria-hidden="true">
            <line
              x1="1"
              x2="23"
              y1="4"
              y2="4"
              stroke="#fff"
              strokeOpacity={s.opacity ?? 1}
              strokeWidth={s.dash ? 1.6 : 2.2}
              strokeDasharray={s.dash}
              strokeLinecap="round"
            />
          </svg>
          {s.name}
        </span>
      ))}
    </div>
  );
}

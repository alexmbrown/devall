import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { fmtCount, fmtSecs } from './format';

/** Colours are CSS custom properties (tokens), never literals. */
export interface ChartSeries {
  key: string;
  label: string;
  color: string;
  values: (number | null)[];
}

const M = { left: 44, right: 14, top: 10, bottom: 22 };

function useWidth<T extends HTMLElement>(): [React.RefObject<T | null>, number] {
  const ref = useRef<T>(null);
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setW(el.clientWidth);
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, w];
}

/** 1, 2, 2.5, 5 × 10^n at or above `v`. */
function niceStep(v: number): number {
  if (!(v > 0)) return 1;
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

const TIME_STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200];

interface LineChartProps {
  title: string;
  /** Seconds since start, one per value. */
  x: number[];
  series: ChartSeries[];
  /** Value format for the tooltip. */
  format(v: number): string;
  /** Y tick format (compact). */
  axisFormat?(v: number): string;
  height?: number;
  /** A 10% wash under single-series charts. */
  area?: boolean;
  /** Fix the x domain (e.g. the planned duration during a live run). */
  xMax?: number;
  /** Width of one bucket in seconds (for the tooltip's time range). */
  bucket?: number;
  empty?: string;
}

/**
 * Multi-series line chart: hairline grid, 2px lines, end dots, a crosshair that snaps to the
 * nearest bucket with one tooltip for every series, and arrow-key navigation when focused.
 */
export function LineChart({ title, x, series, format, axisFormat = fmtCount, height = 132, area, xMax, bucket = 1, empty = 'Waiting for data…' }: LineChartProps) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  const n = x.length;
  const plotW = Math.max(10, width - M.left - M.right);
  const plotH = height;

  const { yMax, ticks } = useMemo(() => {
    let max = 0;
    for (const s of series) for (const v of s.values) if (v != null && v > max) max = v;
    const step = niceStep(max / 3 || 1);
    const top = Math.max(step, Math.ceil(max / step) * step);
    const t: number[] = [];
    for (let v = 0; v <= top + step / 2; v += step) t.push(v);
    return { yMax: top, ticks: t };
  }, [series]);

  const x0 = 0;
  const x1 = Math.max(xMax ?? 0, n ? x[n - 1] + bucket : bucket, 1);
  const sx = (t: number) => M.left + ((t - x0) / (x1 - x0)) * plotW;
  const sy = (v: number) => M.top + plotH - (v / yMax) * plotH;
  const xTicks = useMemo(() => {
    const span = x1 - x0;
    const target = Math.max(2, Math.floor(plotW / 90));
    const step = TIME_STEPS.find((s) => span / s <= target) ?? 7200;
    const out: number[] = [];
    for (let t = 0; t <= span + 1e-9; t += step) out.push(t);
    return out;
  }, [x1, plotW]);

  const paths = useMemo(
    () =>
      series.map((s) => {
        let d = '';
        let a = '';
        let started = false;
        let firstX = 0;
        let lastX = 0;
        s.values.forEach((v, i) => {
          if (v == null) {
            if (started && area) a += `L${lastX},${sy(0)}Z`;
            started = false;
            return;
          }
          const px = sx(x[i]);
          const py = sy(v);
          if (!started) {
            d += `M${px},${py}`;
            if (area) a += `M${px},${sy(0)}L${px},${py}`;
            firstX = px;
            started = true;
          } else {
            d += `L${px},${py}`;
            if (area) a += `L${px},${py}`;
          }
          lastX = px;
        });
        if (started && area) a += `L${lastX},${sy(0)}L${firstX},${sy(0)}Z`;
        return { key: s.key, d, a };
      }),
    [series, x, width, yMax, x1, area],
  );

  // Keep the keyboard cursor in range as points stream in.
  useEffect(() => {
    if (hover != null && hover >= n) setHover(n ? n - 1 : null);
  }, [n, hover]);

  const pick = (e: PointerEvent<SVGRectElement>) => {
    if (!n) return;
    const rect = (e.currentTarget.ownerSVGElement ?? e.currentTarget).getBoundingClientRect();
    const t = ((e.clientX - rect.left - M.left) / plotW) * (x1 - x0) + x0;
    let best = 0;
    for (let i = 1; i < n; i++) if (Math.abs(x[i] - t) < Math.abs(x[best] - t)) best = i;
    setHover(best);
  };

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!n) return;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault();
      const cur = hover ?? n - 1;
      setHover(Math.min(n - 1, Math.max(0, cur + (e.key === 'ArrowLeft' ? -1 : 1))));
    } else if (e.key === 'Home') setHover(0);
    else if (e.key === 'End') setHover(n - 1);
    else if (e.key === 'Escape') setHover(null);
  };

  const hx = hover != null && hover < n ? sx(x[hover]) : null;
  const tipLeft = hx != null ? (hx + 170 > width ? Math.max(0, hx - 166) : hx + 10) : 0;

  return (
    <div className="load-chart">
      <div className="load-chart-head">
        <h4>{title}</h4>
        {series.length > 1 && (
          <div className="legend">
            {series.map((s) => (
              <span key={s.key} className="key">
                <span className="line-key" style={{ background: s.color }} />
                {s.label}
              </span>
            ))}
          </div>
        )}
      </div>
      <div
        ref={ref}
        className="chart"
        tabIndex={0}
        role="group"
        aria-label={`${title} chart. Use the arrow keys to read values.`}
        onKeyDown={onKey}
        onFocus={() => n && hover == null && setHover(n - 1)}
        onBlur={() => setHover(null)}
      >
        {width > 0 && (
          <svg width={width} height={plotH + M.top + M.bottom} aria-hidden>
            {ticks.map((t) => (
              <g key={t}>
                <line x1={M.left} x2={M.left + plotW} y1={sy(t)} y2={sy(t)} style={{ stroke: 'var(--chart-grid)' }} strokeWidth={1} shapeRendering="crispEdges" />
                <text className="tick" x={M.left - 6} y={sy(t)} dy="0.32em" textAnchor="end">
                  {axisFormat(t)}
                </text>
              </g>
            ))}
            {xTicks.map((t) => (
              <text key={t} className="tick" x={sx(t)} y={M.top + plotH + 15} textAnchor={t === 0 ? 'start' : 'middle'}>
                {fmtSecs(t)}
              </text>
            ))}
            {paths.map((p, i) => (
              <g key={p.key}>
                {area && p.a && <path d={p.a} style={{ fill: series[i].color }} fillOpacity={0.1} />}
                <path d={p.d} fill="none" style={{ stroke: series[i].color }} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
              </g>
            ))}
            {hover == null &&
              n > 0 &&
              series.map((s) => {
                const v = s.values[n - 1];
                return v == null ? null : <circle key={s.key} cx={sx(x[n - 1])} cy={sy(v)} r={4} style={{ fill: s.color, stroke: 'var(--bg-elevated)' }} strokeWidth={2} />;
              })}
            {hx != null && (
              <g>
                <line x1={hx} x2={hx} y1={M.top} y2={M.top + plotH} style={{ stroke: 'var(--border-strong)' }} strokeWidth={1} />
                {series.map((s) => {
                  const v = s.values[hover!];
                  return v == null ? null : <circle key={s.key} cx={hx} cy={sy(v)} r={4} style={{ fill: s.color, stroke: 'var(--bg-elevated)' }} strokeWidth={2} />;
                })}
              </g>
            )}
            <rect x={M.left} y={M.top} width={plotW} height={plotH} fill="transparent" onPointerMove={pick} onPointerLeave={() => setHover(null)} />
          </svg>
        )}
        {!n && <div className="load-chart-empty">{empty}</div>}
        {hx != null && hover != null && (
          <div className="chart-tooltip" style={{ left: tipLeft, top: M.top }}>
            <div className="t-head">
              {fmtSecs(x[hover])}
              {bucket > 1 ? `–${fmtSecs(x[hover] + bucket)}` : ''}
            </div>
            {series.map((s) => (
              <div key={s.key} className="t-row">
                <span className="line-key" style={{ background: s.color }} />
                <b>{s.values[hover] == null ? '—' : format(s.values[hover]!)}</b>
                <span>{s.label}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** The planned load profile (VUs or iterations/s over time) for the editor. */
export function ProfilePreview({ points, unit }: { points: [number, number][]; unit: string }) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const h = 56;
  const total = points.length ? points[points.length - 1][0] : 0;
  const max = Math.max(...points.map((p) => p[1]), 0);
  const valid = total > 0 && max > 0 && points.every((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]));
  const left = 36;
  const w = Math.max(10, width - left - 8);
  const sx = (t: number) => left + (t / total) * w;
  const sy = (v: number) => 6 + h - (v / max) * h;
  const d = valid ? points.map((p, i) => `${i ? 'L' : 'M'}${sx(p[0])},${sy(p[1])}`).join('') : '';
  const label = valid ? `Planned profile: peaks at ${fmtCount(max)} ${unit} over ${fmtSecs(total)}` : 'Planned profile unavailable until the durations are valid';
  return (
    <div ref={ref} className="load-profile" role="img" aria-label={label}>
      {width > 0 && valid && (
        <svg width={width} height={h + 24} aria-hidden>
          <line x1={left} x2={left + w} y1={sy(0)} y2={sy(0)} style={{ stroke: 'var(--chart-grid)' }} strokeWidth={1} shapeRendering="crispEdges" />
          <line x1={left} x2={left + w} y1={sy(max)} y2={sy(max)} style={{ stroke: 'var(--chart-grid)' }} strokeWidth={1} shapeRendering="crispEdges" />
          <text className="tick" x={left - 6} y={sy(max)} dy="0.32em" textAnchor="end">
            {fmtCount(max)}
          </text>
          <text className="tick" x={left - 6} y={sy(0)} dy="0.32em" textAnchor="end">
            0
          </text>
          <path d={`${d}L${sx(total)},${sy(0)}L${sx(0)},${sy(0)}Z`} style={{ fill: 'var(--chart-matched)' }} fillOpacity={0.1} />
          <path d={d} fill="none" style={{ stroke: 'var(--chart-matched)' }} strokeWidth={2} strokeLinejoin="round" />
          <text className="tick" x={left} y={h + 21}>
            0s
          </text>
          <text className="tick" x={left + w} y={h + 21} textAnchor="end">
            {fmtSecs(total)} · {unit}
          </text>
        </svg>
      )}
      {!valid && <div className="subtle xs">{label}</div>}
    </div>
  );
}

const STATUS_CLASSES: { key: string; label: string; color: string; match(code: number): boolean }[] = [
  { key: '2xx', label: '2xx', color: 'var(--chart-2xx)', match: (c) => c >= 200 && c < 300 },
  { key: '3xx', label: '3xx', color: 'var(--chart-3xx)', match: (c) => c >= 300 && c < 400 },
  { key: '4xx', label: '4xx', color: 'var(--chart-4xx)', match: (c) => c >= 400 && c < 500 },
  { key: '5xx', label: '5xx', color: 'var(--chart-5xx)', match: (c) => c >= 500 },
  { key: 'other', label: 'Other', color: 'var(--chart-other)', match: (c) => c < 200 },
];

/** Status-class mix (status tokens: the colours mean status here) plus transport errors. */
export function StatusMix({ codes, transportErrors }: { codes: Record<string, number>; transportErrors: number }) {
  const rows = STATUS_CLASSES.map((c) => ({
    ...c,
    n: Object.entries(codes)
      .filter(([k]) => c.match(Number(k)))
      .reduce((a, [, v]) => a + v, 0),
  }));
  if (transportErrors > 0) rows.push({ key: 'err', label: 'No response', color: 'var(--danger)', match: () => false, n: transportErrors });
  const total = rows.reduce((a, r) => a + r.n, 0);
  const shown = rows.filter((r) => r.n > 0);
  if (!total) return <p className="subtle small">No responses yet.</p>;
  return (
    <div className="col-sm">
      <div className="mixbar" role="img" aria-label={shown.map((r) => `${r.label}: ${r.n}`).join(', ')}>
        {shown.map((r) => (
          <div key={r.key} style={{ flexGrow: r.n, background: r.color }} title={`${r.label}: ${r.n}`} />
        ))}
      </div>
      <div className="mix-legend">
        {shown.map((r) => (
          <MixRow key={r.key} color={r.color} label={r.label} n={r.n} total={total} codes={r.key === 'err' ? [] : Object.keys(codes).filter((k) => r.match(Number(k)))} />
        ))}
      </div>
    </div>
  );
}

function MixRow({ color, label, n, total, codes }: { color: string; label: string; n: number; total: number; codes: string[] }) {
  return (
    <>
      <span className="sw" style={{ background: color, width: 10, height: 10, borderRadius: 3 }} />
      <span className="muted">
        {label}
        {codes.length > 0 && <span className="subtle"> · {codes.join(', ')}</span>}
      </span>
      <span className="n">{n.toLocaleString()}</span>
      <span className="p">{((n / total) * 100).toFixed(1)}%</span>
    </>
  );
}

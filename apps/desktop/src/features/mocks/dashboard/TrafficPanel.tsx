import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, Table2, BarChart3 } from 'lucide-react';
import clsx from 'clsx';
import type { ServeEvent } from '@/features/mocks/lib/types';
import { useNav } from '@/features/mocks/store/nav';
import { compact, formatClock, kpisFor, niceMax, statusMix, STATUS_CLASSES, windowFor, type Bucket, type RangeKey, type Window } from './activity';

function useWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setW(Math.floor(e.contentRect.width)));
    ro.observe(el);
    setW(Math.floor(el.getBoundingClientRect().width));
    return () => ro.disconnect();
  }, []);
  return [ref, w] as const;
}

/** Keeps a moving "now" so the time window slides even between polls. */
export function useNow(ms = 10_000) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

const fmtMs = (v?: number) => (v === undefined ? '—' : v >= 1000 ? `${(v / 1000).toFixed(v >= 10_000 ? 0 : 1)} s` : `${Math.round(v)} ms`);

export function TrafficPanel({ events, range, onShowAll, truncated, limit }: { events: ServeEvent[]; range: RangeKey; onShowAll(): void; truncated: boolean; limit: number }) {
  const now = useNow();
  const win = useMemo(() => windowFor(events, range, now), [events, range, now]);
  const kpi = useMemo(() => kpisFor(win, range, now), [win, range, now]);
  const [view, setView] = useState<'chart' | 'table'>('chart');

  return (
    <div className="panel">
      <div className="panel-header">
        <h3>Traffic</h3>
        <div className="legend" aria-label="Legend">
          <span className="key">
            <span className="sw" style={{ background: 'var(--chart-matched)' }} /> Matched
          </span>
          <span className="key">
            <span className="sw" style={{ background: 'var(--chart-unmatched)' }} /> Unmatched
          </span>
        </div>
        <span className="spacer" />
        <div className="segmented" role="tablist" aria-label="View">
          <button className={clsx(view === 'chart' && 'on')} onClick={() => setView('chart')} title="Chart">
            <BarChart3 /> Chart
          </button>
          <button className={clsx(view === 'table' && 'on')} onClick={() => setView('table')} title="Table view">
            <Table2 /> Table
          </button>
        </div>
      </div>
      <div className="panel-body col-lg">
        <div className="kpis">
          <div className="kpi">
            <span className="k">Requests</span>
            <span className="v">{compact(kpi.total)}</span>
          </div>
          <div className="kpi">
            <span className="k">Rate</span>
            <span className="v">
              {kpi.perMinute >= 10 ? Math.round(kpi.perMinute) : kpi.perMinute.toFixed(1)}
              <small>/ min</small>
            </span>
          </div>
          <div className="kpi">
            <span className="k">Unmatched</span>
            <span className="v">
              {compact(kpi.unmatched)}
              <small>{kpi.total ? `${kpi.unmatchedPct < 1 && kpi.unmatched ? '<1' : Math.round(kpi.unmatchedPct)}%` : ''}</small>
            </span>
            {kpi.unmatched > 0 && (
              <span className="d text-warning">
                <AlertTriangle /> no stub answered
              </span>
            )}
          </div>
          <div className="kpi">
            <span className="k">Latency p50 · p95</span>
            <span className="v">
              {fmtMs(kpi.p50)}
              <small>· {fmtMs(kpi.p95)}</small>
            </span>
          </div>
        </div>

        {win.events.length === 0 ? (
          <div className="empty" style={{ padding: 'var(--space-7) 0', minHeight: 180 }}>
            <BarChart3 />
            <h3>{events.length ? 'No requests in this time range' : 'No requests yet'}</h3>
            {events.length ? (
              <button className="btn btn-sm" onClick={onShowAll}>
                Show the latest requests
              </button>
            ) : (
              <p>Point your app or tests at the WireMock server and traffic shows up here live.</p>
            )}
          </div>
        ) : view === 'chart' ? (
          <TrafficChart win={win} />
        ) : (
          <TrafficTable win={win} />
        )}

        <StatusMixBar events={win.events} />
        {truncated && (
          <div className="subtle xs">
            Showing the latest {limit} requests in this range — older ones in the window aren't included. Pick a shorter range for exact numbers.
          </div>
        )}
      </div>
    </div>
  );
}

const LABEL_STEPS = [1, 2, 5, 10, 15, 30].map((s) => s * 1000).concat([1, 2, 5, 10, 15, 30].map((m) => m * 60_000), [1, 2, 3, 6, 12, 24].map((h) => h * 3_600_000));
const PLOT_H = 150;
const AXIS_H = 20;
const Y_AXIS_W = 30;

function TrafficChart({ win }: { win: Window }) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  const [focusIdx, setFocusIdx] = useState<number | null>(null);
  // Drag-to-select a range of columns; a plain click selects one column.
  const [drag, setDrag] = useState<{ a: number; b: number } | null>(null);
  const dragRef = useRef(drag);
  dragRef.current = drag;
  const go = useNav((s) => s.go);
  const active = hover ?? focusIdx;
  const { buckets, interval } = win;
  const openRange = (i: number, j: number) => {
    const lo = Math.min(i, j);
    const hi = Math.max(i, j);
    go('requests', undefined, { from: String(buckets[lo].start), to: String(buckets[hi].end) });
  };
  useEffect(() => {
    const up = () => {
      const d = dragRef.current;
      if (!d) return;
      setDrag(null);
      openRange(d.a, d.b);
    };
    window.addEventListener('pointerup', up);
    return () => window.removeEventListener('pointerup', up);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [buckets]);
  const max = niceMax(Math.max(1, ...buckets.map((b) => b.matched + b.unmatched)));
  const ticks = max <= 4 ? Array.from({ length: max + 1 }, (_, i) => i) : [0, max / 2, max];
  const plotW = Math.max(0, width - Y_AXIS_W);
  const slot = buckets.length ? plotW / buckets.length : 0;
  const barW = Math.max(2, Math.min(24, slot - 2));
  const y = (v: number) => PLOT_H - (v / max) * PLOT_H;
  // Label round times (14:35, 14:40…): the smallest nice step, a multiple of the bucket size, giving ≤ 6 labels.
  const span = win.end - win.start;
  const labelStep = LABEL_STEPS.find((s) => s >= interval && s % interval === 0 && span / s <= 6) ?? interval * Math.ceil(buckets.length / 6);

  const onKey = (e: React.KeyboardEvent) => {
    if (!buckets.length) return;
    if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
      e.preventDefault();
      const cur = focusIdx ?? buckets.length - 1;
      setFocusIdx(Math.max(0, Math.min(buckets.length - 1, cur + (e.key === 'ArrowRight' ? 1 : -1))));
    } else if (e.key === 'Home') setFocusIdx(0);
    else if (e.key === 'End') setFocusIdx(buckets.length - 1);
    else if (e.key === 'Escape') setFocusIdx(null);
    else if (e.key === 'Enter' && focusIdx !== null) openRange(focusIdx, focusIdx);
  };

  const lo = drag ? Math.min(drag.a, drag.b) : active;
  const hi = drag ? Math.max(drag.a, drag.b) : active;
  const sel = lo !== null && hi !== null ? buckets.slice(lo, hi + 1) : [];
  const b: Bucket | undefined = sel.length
    ? { start: sel[0].start, end: sel[sel.length - 1].end, matched: sel.reduce((n, x) => n + x.matched, 0), unmatched: sel.reduce((n, x) => n + x.unmatched, 0) }
    : undefined;
  const tipLeft = lo !== null && hi !== null ? Y_AXIS_W + ((lo + hi + 1) / 2) * slot : 0;

  return (
    <div
      ref={ref}
      className="chart"
      tabIndex={0}
      role="img"
      aria-label={`Requests over time: ${win.events.length} requests, ${buckets.reduce((a, x) => a + x.unmatched, 0)} unmatched. Use the arrow keys to read each interval and Enter to open it in the request journal.`}
      onKeyDown={onKey}
      onFocus={() => setFocusIdx((i) => i ?? buckets.length - 1)}
      onBlur={() => setFocusIdx(null)}
      onMouseLeave={() => setHover(null)}
    >
      {width > 0 && (
        <svg width={width} height={PLOT_H + AXIS_H}>
          {ticks.map((t) => (
            <g key={t}>
              <line x1={Y_AXIS_W} x2={width} y1={y(t) + 0.5} y2={y(t) + 0.5} stroke={t === 0 ? 'var(--border-strong)' : 'var(--chart-grid)'} strokeWidth={1} />
              <text className="tick" x={Y_AXIS_W - 6} y={y(t) + 3} textAnchor="end">
                {compact(t)}
              </text>
            </g>
          ))}
          {drag && lo !== null && hi !== null && (
            <rect x={Y_AXIS_W + lo * slot} y={0} width={(hi - lo + 1) * slot} height={PLOT_H} fill="var(--accent-subtle)" stroke="var(--accent-border)" />
          )}
          {buckets.map((bk, i) => {
            const x = Y_AXIS_W + i * slot + (slot - barW) / 2;
            return (
              <g key={bk.start}>
                {!drag && active === i && <rect x={Y_AXIS_W + i * slot} y={0} width={slot} height={PLOT_H} fill="var(--bg-hover)" />}
                <Column x={x} w={barW} bucket={bk} y={y} />
                {bk.start % labelStep === 0 && (
                  <text className="tick" x={Y_AXIS_W + i * slot} y={PLOT_H + 14} textAnchor={i === 0 ? 'start' : 'middle'}>
                    {formatClock(bk.start, labelStep)}
                  </text>
                )}
                {/* Hit target: the whole column slot, not just the painted bar. */}
                <rect
                  x={Y_AXIS_W + i * slot}
                  y={0}
                  width={slot}
                  height={PLOT_H}
                  fill="transparent"
                  style={{ cursor: drag ? 'col-resize' : 'pointer' }}
                  onPointerEnter={() => {
                    setHover(i);
                    if (dragRef.current) setDrag({ a: dragRef.current.a, b: i });
                  }}
                  onPointerMove={() => hover !== i && setHover(i)}
                  onPointerDown={(e) => {
                    e.preventDefault();
                    setDrag({ a: i, b: i });
                  }}
                  aria-hidden
                />
              </g>
            );
          })}
        </svg>
      )}
      {b && (
        <div
          className="chart-tooltip"
          style={{
            top: 4,
            left: Math.min(Math.max(tipLeft - 110, Y_AXIS_W), Math.max(Y_AXIS_W, width - 250)),
          }}
          role="status"
        >
          <div className="t-head">
            {formatClock(b.start, interval)} – {formatClock(b.end, interval)}
          </div>
          <div className="t-row">
            <span className="line-key" style={{ background: 'var(--chart-unmatched)' }} />
            <b>{b.unmatched}</b>
            <span>Unmatched</span>
          </div>
          <div className="t-row">
            <span className="line-key" style={{ background: 'var(--chart-matched)' }} />
            <b>{b.matched}</b>
            <span>Matched</span>
          </div>
          <div className="t-row">
            <span className="line-key" style={{ background: 'transparent' }} />
            <b>{b.matched + b.unmatched}</b>
            <span>Total</span>
          </div>
          <div className="t-hint">{drag ? 'Release to open this range in Requests' : focusIdx !== null && hover === null ? 'Enter: open in Requests' : 'Click to open in Requests · drag to select a range'}</div>
        </div>
      )}
    </div>
  );
}

/** Stacked column: matched at the baseline, unmatched on top; 2px surface gap; 4px rounded top only. */
function Column({ x, w, bucket, y }: { x: number; w: number; bucket: Bucket; y(v: number): number }) {
  const total = bucket.matched + bucket.unmatched;
  if (!total) return null;
  const gap = bucket.matched && bucket.unmatched ? 2 : 0;
  const base = y(0);
  const mTop = y(bucket.matched);
  const tTop = y(total);
  const r = Math.min(4, w / 2);
  const roundedTop = (x0: number, top: number, bottom: number) => {
    const h = bottom - top;
    const rr = Math.min(r, h);
    return `M${x0},${bottom} V${top + rr} Q${x0},${top} ${x0 + rr},${top} H${x0 + w - rr} Q${x0 + w},${top} ${x0 + w},${top + rr} V${bottom} Z`;
  };
  const segs: React.ReactNode[] = [];
  if (bucket.matched) {
    const top = mTop;
    segs.push(
      bucket.unmatched ? (
        <rect key="m" x={x} y={top} width={w} height={Math.max(1, base - top)} fill="var(--chart-matched)" />
      ) : (
        <path key="m" d={roundedTop(x, top, base)} fill="var(--chart-matched)" />
      ),
    );
  }
  if (bucket.unmatched) {
    const bottom = bucket.matched ? mTop - gap : base;
    segs.push(<path key="u" d={roundedTop(x, Math.min(tTop, bottom - 1), bottom)} fill="var(--chart-unmatched)" />);
  }
  return <>{segs}</>;
}

function TrafficTable({ win }: { win: Window }) {
  const rows = win.buckets.filter((b) => b.matched + b.unmatched > 0).reverse();
  return (
    <div style={{ maxHeight: 220, overflow: 'auto', border: '1px solid var(--border)', borderRadius: 'var(--radius)' }}>
      <table className="table">
        <thead>
          <tr>
            <th>Interval</th>
            <th style={{ textAlign: 'right' }}>Matched</th>
            <th style={{ textAlign: 'right' }}>Unmatched</th>
            <th style={{ textAlign: 'right' }}>Total</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((b) => (
            <tr key={b.start}>
              <td className="mono xs">
                {formatClock(b.start, win.interval)} – {formatClock(b.end, win.interval)}
              </td>
              <td className="xs" style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                {b.matched}
              </td>
              <td className="xs" style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                {b.unmatched}
              </td>
              <td className="xs strong" style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                {b.matched + b.unmatched}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Part-to-whole of response status classes; legend carries labels, counts and shares. */
export function StatusMixBar({ events }: { events: ServeEvent[] }) {
  const mix = statusMix(events);
  const total = events.length;
  if (!total) return null;
  const parts = STATUS_CLASSES.filter((c) => mix[c.key] > 0);
  return (
    <div className="col-sm">
      <span className="label">Responses by status</span>
      <div className="mixbar" role="img" aria-label={parts.map((p) => `${p.label}: ${mix[p.key]}`).join(', ')}>
        {parts.map((p) => (
          <div key={p.key} style={{ flex: mix[p.key], background: `var(${p.token})` }} title={`${p.label}: ${mix[p.key]}`} />
        ))}
      </div>
      <div className="mix-legend">
        {parts.map((p) => (
          <div key={p.key} style={{ display: 'contents' }}>
            <span className="sw" style={{ width: 10, height: 10, borderRadius: 3, background: `var(${p.token})` }} />
            <span className="muted">{p.label}</span>
            <span className="n">{compact(mix[p.key])}</span>
            <span className="p">{Math.round((mix[p.key] / total) * 100)}%</span>
          </div>
        ))}
      </div>
    </div>
  );
}

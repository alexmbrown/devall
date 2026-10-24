import { useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import clsx from 'clsx';
import { LEVELS, type Histogram, type Level } from '@/lib/api/logs';
import { LEVEL_LABEL, STACK_ORDER, fmtCount, fmtSpan, fmtTs, levelVar } from './format';

const M = { left: 36, right: 8, top: 6, bottom: 16 };
const PLOT_H = 56;

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

const TICK_STEPS = [1, 5, 10, 30, 60, 300, 600, 1800, 3600, 3 * 3600, 6 * 3600, 12 * 3600, 86400, 7 * 86400, 30 * 86400].map((s) => s * 1000);

function niceMax(v: number): number {
  if (v <= 4) return Math.max(1, v);
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

interface Props {
  hist: Histogram | null;
  loading?: boolean;
  /** Current zoom (custom range), shown with a reset control. */
  zoomed: boolean;
  onZoom(from: number, to: number): void;
  onReset(): void;
  /** Click on a bucket: jump the table to that time. */
  onLocate(ts: number): void;
}

/**
 * Stacked count-per-bucket timeline (severity levels, severe ones on the baseline). Hover for
 * counts, click to jump the table, drag to zoom into a range; ←/→ + Enter from the keyboard.
 */
export function Timeline({ hist, loading, zoomed, onZoom, onReset, onLocate }: Props) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  const [drag, setDrag] = useState<{ a: number; b: number } | null>(null);
  const buckets = hist?.buckets ?? [];
  const n = buckets.length;
  const plotW = Math.max(10, width - M.left - M.right);
  const bw = n ? plotW / n : plotW;
  const yMax = niceMax(hist?.max ?? 0);
  const sy = (v: number) => (v / yMax) * PLOT_H;

  const bars = useMemo(() => {
    const gap = bw >= 5 ? 1.5 : bw >= 3 ? 1 : 0;
    return buckets.map((b, i) => {
      const x = M.left + i * bw + gap / 2;
      const w = Math.max(0.5, bw - gap);
      let y = M.top + PLOT_H;
      const segs: { level: Level; y: number; h: number }[] = [];
      for (const l of STACK_ORDER) {
        const c = b.counts[LEVELS.indexOf(l)] ?? 0;
        if (!c) continue;
        const h = Math.max(1, sy(c));
        y -= h;
        // 1px surface gap between stacked segments when they're tall enough to keep it.
        segs.push({ level: l, y: y + (segs.length && h > 2 ? 1 : 0), h: segs.length && h > 2 ? h - 1 : h });
      }
      return { x, w, segs };
    });
  }, [buckets, bw, yMax]);

  const ticks = useMemo(() => {
    if (!hist || !n) return [];
    const span = hist.end - hist.start;
    const target = Math.max(2, Math.floor(plotW / 110));
    const step = TICK_STEPS.find((s) => span / s <= target) ?? 365 * 86_400_000;
    const multiDay = span > 86_400_000;
    const out: { x: number; label: string }[] = [];
    // Align to the step in local time.
    const offset = new Date(hist.start).getTimezoneOffset() * 60_000;
    let t = Math.ceil((hist.start - offset) / step) * step + offset;
    for (; t <= hist.end; t += step) {
      const x = M.left + ((t - hist.start) / span) * plotW;
      const d = new Date(t);
      const label = multiDay && step >= 86_400_000 ? `${d.getMonth() + 1}/${d.getDate()}` : fmtTs(t).slice(0, step < 60_000 ? 8 : 5);
      out.push({ x, label });
    }
    return out;
  }, [hist, n, plotW]);

  const idxAt = (clientX: number, el: Element) => {
    const rect = el.getBoundingClientRect();
    return Math.min(n - 1, Math.max(0, Math.floor((clientX - rect.left - M.left) / bw)));
  };

  const onDown = (e: PointerEvent<SVGRectElement>) => {
    if (!n) return;
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    const i = idxAt(e.clientX, e.currentTarget.ownerSVGElement!);
    setDrag({ a: i, b: i });
  };
  const onMove = (e: PointerEvent<SVGRectElement>) => {
    if (!n) return;
    const i = idxAt(e.clientX, e.currentTarget.ownerSVGElement!);
    setHover(i);
    if (drag) setDrag({ ...drag, b: i });
  };
  const onUp = () => {
    if (!drag || !hist) return setDrag(null);
    const [a, b] = drag.a <= drag.b ? [drag.a, drag.b] : [drag.b, drag.a];
    setDrag(null);
    if (a === b) onLocate(buckets[a].t);
    else onZoom(buckets[a].t, buckets[b].t + hist.bucketMs);
  };

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!n) return;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault();
      setHover((h) => Math.min(n - 1, Math.max(0, (h ?? (e.key === 'ArrowLeft' ? n : -1)) + (e.key === 'ArrowLeft' ? -1 : 1))));
    } else if (e.key === 'Enter' && hover != null && hist) {
      e.preventDefault();
      if (e.shiftKey) onZoom(buckets[hover].t, buckets[hover].t + hist.bucketMs);
      else onLocate(buckets[hover].t);
    } else if (e.key === 'Escape' && zoomed) {
      e.preventDefault();
      onReset();
    }
  };

  const hb = hover != null ? buckets[hover] : null;
  const tipLeft = hover != null ? Math.min(Math.max(M.left + hover * bw + bw / 2 - 80, 0), Math.max(0, width - 180)) : 0;
  const sel = drag && drag.a !== drag.b ? [Math.min(drag.a, drag.b), Math.max(drag.a, drag.b)] : null;

  return (
    <div
      ref={ref}
      className={clsx('chart logs-timeline', loading && 'loading')}
      tabIndex={0}
      role="img"
      aria-label={hist ? `Timeline: ${hist.total} entries over ${n} buckets` : 'Timeline'}
      onKeyDown={onKey}
      onBlur={() => setHover(null)}
    >
      {width > 0 && (
        <svg width={width} height={M.top + PLOT_H + M.bottom}>
          <line x1={M.left} x2={M.left + plotW} y1={M.top + PLOT_H + 0.5} y2={M.top + PLOT_H + 0.5} className="axis" />
          <line x1={M.left} x2={M.left + plotW} y1={M.top + 0.5} y2={M.top + 0.5} className="grid" />
          <text x={M.left - 6} y={M.top + 8} className="tick" textAnchor="end">
            {fmtCount(yMax)}
          </text>
          <text x={M.left - 6} y={M.top + PLOT_H} className="tick" textAnchor="end">
            0
          </text>
          {bars.map((b, i) => (
            <g key={i} opacity={hover != null && hover !== i && !drag ? 0.55 : 1}>
              {b.segs.map((s) => (
                <rect key={s.level} x={b.x} y={s.y} width={b.w} height={s.h} fill={levelVar(s.level)} rx={b.w >= 6 ? 1 : 0} />
              ))}
            </g>
          ))}
          {sel && <rect className="brush" x={M.left + sel[0] * bw} y={M.top} width={(sel[1] - sel[0] + 1) * bw} height={PLOT_H} />}
          {ticks.map((t, i) => (
            <text key={i} x={t.x} y={M.top + PLOT_H + 12} className="tick" textAnchor="middle">
              {t.label}
            </text>
          ))}
          <rect
            x={M.left}
            y={M.top}
            width={plotW}
            height={PLOT_H}
            fill="transparent"
            style={{ cursor: n ? 'crosshair' : 'default' }}
            onPointerDown={onDown}
            onPointerMove={onMove}
            onPointerUp={onUp}
            onPointerLeave={() => !drag && setHover(null)}
          />
        </svg>
      )}
      {hist && n === 0 && (
        <div className="logs-timeline-empty">{hist.withoutTs ? `${fmtCount(hist.withoutTs)} entries without timestamps` : loading ? 'Loading…' : 'Nothing to chart'}</div>
      )}
      {hb && hist && !drag && (
        <div className="chart-tooltip" style={{ left: tipLeft, top: M.top + PLOT_H + M.bottom + 2 }}>
          <div className="t-head">{fmtSpan(hb.t, hb.t + hist.bucketMs)}</div>
          {STACK_ORDER.map((l) => {
            const c = hb.counts[LEVELS.indexOf(l)] ?? 0;
            return c ? (
              <div className="t-row" key={l}>
                <span className="sw" style={{ background: levelVar(l) }} />
                <b>{fmtCount(c)}</b>
                <span>{LEVEL_LABEL[l]}</span>
              </div>
            ) : null;
          })}
          <div className="t-hint">Click to jump · drag to zoom</div>
        </div>
      )}
    </div>
  );
}

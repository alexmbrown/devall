import type { ServeEvent } from '@/features/mocks/lib/types';

export type RangeKey = '5m' | '15m' | '1h' | '6h' | 'all';

export const RANGES: { key: RangeKey; label: string; ms?: number }[] = [
  { key: '5m', label: '5 min', ms: 5 * 60_000 },
  { key: '15m', label: '15 min', ms: 15 * 60_000 },
  { key: '1h', label: '1 hour', ms: 60 * 60_000 },
  { key: '6h', label: '6 hours', ms: 6 * 60 * 60_000 },
  { key: 'all', label: 'All loaded' },
];

const NICE = [1, 2, 5, 10, 15, 30].map((s) => s * 1000).concat([1, 2, 5, 10, 15, 30].map((m) => m * 60_000), [1, 2, 3, 6, 12, 24].map((h) => h * 3_600_000));

/** Smallest "nice" interval that keeps the chart at or under `maxBuckets` columns. */
export function niceInterval(spanMs: number, maxBuckets = 40): number {
  return NICE.find((i) => spanMs / i <= maxBuckets) ?? NICE[NICE.length - 1];
}

export interface Bucket {
  start: number;
  end: number;
  matched: number;
  unmatched: number;
}

export interface Window {
  start: number;
  end: number;
  interval: number;
  buckets: Bucket[];
  events: ServeEvent[];
}

const timeOf = (e: ServeEvent) => e.request.loggedDate ?? 0;

/** Events inside the range, bucketed on interval boundaries ending at `now`. */
export function windowFor(events: ServeEvent[], range: RangeKey, now = Date.now()): Window {
  const spec = RANGES.find((r) => r.key === range)!;
  const times = events.map(timeOf).filter(Boolean);
  const oldest = times.length ? Math.min(...times) : now - 5 * 60_000;
  const rawStart = spec.ms ? now - spec.ms : Math.min(oldest, now - 60_000);
  const interval = niceInterval(now - rawStart);
  const end = Math.ceil(now / interval) * interval;
  const start = Math.floor(rawStart / interval) * interval;
  const n = Math.max(1, Math.round((end - start) / interval));
  const buckets: Bucket[] = Array.from({ length: n }, (_, i) => ({ start: start + i * interval, end: start + (i + 1) * interval, matched: 0, unmatched: 0 }));
  const inRange = events.filter((e) => timeOf(e) >= (spec.ms ? now - spec.ms : start) && timeOf(e) <= end);
  for (const e of inRange) {
    const i = Math.min(n - 1, Math.floor((timeOf(e) - start) / interval));
    if (i < 0) continue;
    if (e.wasMatched) buckets[i].matched++;
    else buckets[i].unmatched++;
  }
  return { start, end, interval, buckets, events: inRange };
}

export function percentile(sorted: number[], p: number): number | undefined {
  if (!sorted.length) return undefined;
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[idx];
}

export interface Kpis {
  total: number;
  perMinute: number;
  unmatched: number;
  unmatchedPct: number;
  p50?: number;
  p95?: number;
}

export function kpisFor(w: Window, range: RangeKey, now = Date.now()): Kpis {
  const total = w.events.length;
  const unmatched = w.events.filter((e) => !e.wasMatched).length;
  const spec = RANGES.find((r) => r.key === range)!;
  const spanMin = Math.max(1 / 60, ((spec.ms ?? now - w.start) || 60_000) / 60_000);
  const lat = w.events
    .map((e) => e.timing?.totalTime)
    .filter((x): x is number => typeof x === 'number' && x >= 0)
    .sort((a, b) => a - b);
  return { total, perMinute: total / spanMin, unmatched, unmatchedPct: total ? (unmatched / total) * 100 : 0, p50: percentile(lat, 50), p95: percentile(lat, 95) };
}

export type StatusClass = '2xx' | '3xx' | '4xx' | '5xx' | 'other';

export const STATUS_CLASSES: { key: StatusClass; label: string; token: string }[] = [
  { key: '2xx', label: '2xx Success', token: '--chart-2xx' },
  { key: '3xx', label: '3xx Redirect', token: '--chart-3xx' },
  { key: '4xx', label: '4xx Client error', token: '--chart-4xx' },
  { key: '5xx', label: '5xx Server error', token: '--chart-5xx' },
  { key: 'other', label: 'Fault / no response', token: '--chart-other' },
];

export function statusClassOf(e: ServeEvent): StatusClass {
  if (e.response?.fault || e.responseDefinition?.fault) return 'other';
  const s = e.response?.status ?? e.responseDefinition?.status;
  if (!s) return 'other';
  const c = `${String(s)[0]}xx`;
  return (['2xx', '3xx', '4xx', '5xx'] as string[]).includes(c) ? (c as StatusClass) : 'other';
}

export function statusMix(events: ServeEvent[]): Record<StatusClass, number> {
  const m: Record<StatusClass, number> = { '2xx': 0, '3xx': 0, '4xx': 0, '5xx': 0, other: 0 };
  for (const e of events) m[statusClassOf(e)]++;
  return m;
}

/** Round an axis max up to 1/2/5 × 10^n so ticks are clean numbers. */
export function niceMax(v: number): number {
  if (v <= 4) return Math.max(1, v);
  const p = 10 ** Math.floor(Math.log10(v));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

export function compact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}K`;
  return String(Math.round(n));
}

export function formatClock(t: number, interval: number): string {
  const d = new Date(t);
  const opts: Intl.DateTimeFormatOptions = { hour: '2-digit', minute: '2-digit', hour12: false };
  if (interval < 60_000) opts.second = '2-digit';
  if (interval >= 3_600_000 * 3) Object.assign(opts, { month: 'short', day: 'numeric' });
  return d.toLocaleTimeString([], opts);
}

import type { Executor, ExecutorType, Stage } from '@/lib/api/load';

/** `0.42 ms`, `8.3 ms`, `123 ms`, `1.24 s`. */
export function fmtMs(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return '—';
  if (v >= 1000) return `${(v / 1000).toFixed(v >= 10_000 ? 1 : 2)} s`;
  if (v >= 100) return `${Math.round(v)} ms`;
  if (v >= 10) return `${v.toFixed(1)} ms`;
  return `${v.toFixed(2)} ms`;
}

/** Compact number: 950, 1.2K, 12K, 3.4M. */
export function fmtCount(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return '—';
  const a = Math.abs(v);
  if (a >= 1e6) return `${trim(v / 1e6, a >= 1e7 ? 0 : 1)}M`;
  if (a >= 1e3) return `${trim(v / 1e3, a >= 1e4 ? 0 : 1)}K`;
  return trim(v, Number.isInteger(v) ? 0 : 1);
}

export function fmtRate(v: number | null | undefined): string {
  return v == null ? '—' : `${fmtCount(v)}/s`;
}

/** Fraction → percent. */
export function fmtPct(fraction: number | null | undefined, digits = 2): string {
  if (fraction == null || !Number.isFinite(fraction)) return '—';
  const p = fraction * 100;
  if (p === 0) return '0%';
  if (p < 0.01) return '<0.01%';
  return `${trim(p, p >= 10 ? 1 : digits)}%`;
}

/** `45s`, `2m 30s`, `1h 5m`. */
export function fmtDuration(ms: number): string {
  if (!Number.isFinite(ms)) return '—';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  let s = Math.round(ms / 1000);
  const h = Math.floor(s / 3600);
  s -= h * 3600;
  const m = Math.floor(s / 60);
  s -= m * 60;
  if (h) return `${h}h${m ? ` ${m}m` : ''}`;
  if (m) return `${m}m${s ? ` ${s}s` : ''}`;
  return `${s}s`;
}

/** Axis label for seconds since start: `0s`, `30s`, `1m`, `1m30s`. */
export function fmtSecs(t: number): string {
  if (t < 60) return `${Math.round(t)}s`;
  const m = Math.floor(t / 60);
  const s = Math.round(t - m * 60);
  if (m >= 60) return `${Math.floor(m / 60)}h${m % 60 ? `${m % 60}m` : ''}`;
  return s ? `${m}m${s}s` : `${m}m`;
}

function trim(v: number, digits: number): string {
  return v.toFixed(digits).replace(/\.0+$/, '').replace(/(\.\d*?)0+$/, '$1');
}

/** Same grammar as the Rust parser: `1m30s`, `500ms`, `1.5s`; a bare number is seconds. */
export function parseDuration(text: string | undefined): number | null {
  const s = (text ?? '').trim();
  if (!s) return null;
  if (/^\d+(\.\d+)?$/.test(s)) return parseFloat(s) * 1000;
  // Longer units first so `ms` is not read as `m` + `s`.
  const re = /(\d+(?:\.\d+)?)\s*(ms|min|h|m|sec|s|us|µs)/gy;
  let total = 0;
  let pos = 0;
  let m: RegExpExecArray | null;
  const unit: Record<string, number> = { h: 3_600_000, min: 60_000, m: 60_000, sec: 1000, s: 1000, ms: 1, us: 0.001, µs: 0.001 };
  while (pos < s.length) {
    re.lastIndex = pos;
    m = re.exec(s);
    if (!m) return null;
    total += parseFloat(m[1]) * unit[m[2]];
    pos = re.lastIndex;
    while (s[pos] === ' ') pos++;
  }
  return total;
}

export const EXECUTOR_INFO: Record<ExecutorType, { label: string; short: string; open: boolean; help: string }> = {
  constantArrivalRate: {
    label: 'Constant arrival rate',
    short: 'Rate',
    open: true,
    help: 'Starts iterations on a fixed schedule whatever the response times. Latency is measured from each intended start, so a slow server cannot hide (no coordinated omission).',
  },
  rampingArrivalRate: {
    label: 'Ramping arrival rate',
    short: 'Ramp rate',
    open: true,
    help: 'Like constant arrival rate, with the rate ramping linearly between stages — spikes, step tests, soak ramps.',
  },
  constantVus: {
    label: 'Constant VUs',
    short: 'VUs',
    open: false,
    help: 'A fixed number of virtual users loop back-to-back. Closed model: when the server slows down, load drops with it — good for "N concurrent users", not for latency SLOs.',
  },
  rampingVus: {
    label: 'Ramping VUs',
    short: 'Ramp VUs',
    open: false,
    help: 'Virtual users ramp linearly between stage targets. Closed model.',
  },
};

/** Planned profile as (seconds, value) points: VUs for VU executors, iterations/s for rates. */
export function profilePoints(ex: Executor): { points: [number, number][]; unit: 'VUs' | 'it/s' } {
  const secs = (d: string) => (parseDuration(d) ?? 0) / 1000;
  const perSec = (rate: number, unit?: string) => {
    const u = secs(unit ?? '1s');
    return u > 0 ? rate / u : 0;
  };
  const ramp = (start: number, stages: Stage[], map: (v: number) => number): [number, number][] => {
    const pts: [number, number][] = [[0, map(start)]];
    let t = 0;
    for (const s of stages) {
      t += secs(s.duration);
      pts.push([t, map(s.target)]);
    }
    return pts;
  };
  switch (ex.type) {
    case 'constantVus':
      return { points: [[0, ex.vus], [secs(ex.duration), ex.vus]], unit: 'VUs' };
    case 'rampingVus':
      return { points: ramp(ex.startVus ?? 0, ex.stages, (v) => v), unit: 'VUs' };
    case 'constantArrivalRate': {
      const r = perSec(ex.rate, ex.timeUnit);
      return { points: [[0, r], [secs(ex.duration), r]], unit: 'it/s' };
    }
    case 'rampingArrivalRate':
      return { points: ramp(ex.startRate ?? 0, ex.stages, (v) => perSec(v, ex.timeUnit)), unit: 'it/s' };
  }
}

/** One-line description of an executor, e.g. `50 it/s for 30s`. */
export function describeExecutor(ex: Executor): string {
  const unit = (u?: string) => (!u || u === '1s' ? '/s' : `/${u}`);
  switch (ex.type) {
    case 'constantVus':
      return `${ex.vus} VUs for ${ex.duration}`;
    case 'rampingVus':
      return `${ex.startVus ?? 0} → ${ex.stages.map((s) => s.target).join(' → ')} VUs`;
    case 'constantArrivalRate':
      return `${ex.rate}${unit(ex.timeUnit)} for ${ex.duration}`;
    case 'rampingArrivalRate':
      return `${ex.startRate ?? 0} → ${ex.stages.map((s) => s.target).join(' → ')}${unit(ex.timeUnit)}`;
  }
}

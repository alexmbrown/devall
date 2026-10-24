import { describe, expect, it } from 'vitest';
import { kpisFor, niceInterval, niceMax, percentile, statusMix, windowFor } from './activity';
import type { ServeEvent } from '@/features/mocks/lib/types';

const now = Date.UTC(2026, 9, 20, 12, 0, 0);
const ev = (agoMs: number, o: Partial<ServeEvent> & { status?: number; ms?: number } = {}): ServeEvent => ({
  id: String(Math.random()),
  wasMatched: o.wasMatched ?? true,
  request: { method: 'GET', url: '/x', loggedDate: now - agoMs },
  response: { status: o.status ?? 200 },
  timing: o.ms !== undefined ? { totalTime: o.ms } : undefined,
});

describe('activity window', () => {
  it('picks nice intervals', () => {
    expect(niceInterval(5 * 60_000)).toBe(10_000);
    expect(niceInterval(60 * 60_000)).toBe(120_000);
    expect(niceInterval(6 * 3_600_000)).toBe(600_000);
  });
  it('buckets only events inside the range, ending at now', () => {
    const w = windowFor([ev(10_000), ev(20_000, { wasMatched: false }), ev(20 * 60_000)], '15m', now);
    expect(w.events).toHaveLength(2);
    expect(w.end).toBeGreaterThanOrEqual(now);
    expect(w.buckets.reduce((a, b) => a + b.matched + b.unmatched, 0)).toBe(2);
    expect(w.buckets.length).toBeLessThanOrEqual(40);
  });
  it('"all" starts at the oldest loaded event', () => {
    const w = windowFor([ev(3 * 3_600_000), ev(1000)], 'all', now);
    expect(w.events).toHaveLength(2);
    expect(w.start).toBeLessThanOrEqual(now - 3 * 3_600_000);
  });
  it('computes KPIs', () => {
    const w = windowFor([ev(1000, { ms: 10 }), ev(2000, { ms: 30 }), ev(3000, { ms: 20, wasMatched: false })], '5m', now);
    const k = kpisFor(w, '5m', now);
    expect(k.total).toBe(3);
    expect(k.unmatched).toBe(1);
    expect(Math.round(k.unmatchedPct)).toBe(33);
    expect(k.perMinute).toBeCloseTo(0.6);
    expect(k.p50).toBe(20);
    expect(k.p95).toBe(30);
  });
  it('status mix and helpers', () => {
    expect(statusMix([ev(1, { status: 201 }), ev(1, { status: 404 }), ev(1, { status: 503 }), ev(1, { status: 302 })])).toEqual({ '2xx': 1, '3xx': 1, '4xx': 1, '5xx': 1, other: 0 });
    expect(niceMax(7)).toBe(10);
    expect(niceMax(3)).toBe(3);
    expect(niceMax(130)).toBe(200);
    expect(percentile([1, 2, 3, 4], 50)).toBe(2);
  });
});

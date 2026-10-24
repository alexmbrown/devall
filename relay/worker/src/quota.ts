// Pure quota helpers (usage periods, per-IP rate limiting). No runtime APIs, so they unit-test anywhere.

export interface Period {
  key: string;
  start: number;
  end: number;
}

/** Calendar month (UTC) that `now` falls in; bandwidth quotas reset on the 1st. */
export function monthPeriod(now = Date.now()): Period {
  const d = new Date(now);
  const start = Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1);
  const end = Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1);
  const key = `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, "0")}`;
  return { key, start, end };
}

/** UTC day key, e.g. `2026-10-21` (inbox daily event limits). */
export function dayKey(now = Date.now()): string {
  return new Date(now).toISOString().slice(0, 10);
}

export function nextUtcMidnight(now = Date.now()): number {
  const d = new Date(now);
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate() + 1);
}

export interface UsageSnapshot {
  period: string;
  /** Bytes / requests already recorded in the account's ledger. */
  bytes: number;
  requests: number;
  /** Not yet flushed to the ledger. */
  pendingBytes: number;
  pendingRequests: number;
  lastFlush: number;
}

export function emptyUsage(now = Date.now()): UsageSnapshot {
  return { period: monthPeriod(now).key, bytes: 0, requests: 0, pendingBytes: 0, pendingRequests: 0, lastFlush: now };
}

/** Rolls a snapshot into the current month (a new month starts from zero). */
export function rollUsage(u: UsageSnapshot, now = Date.now()): UsageSnapshot {
  const key = monthPeriod(now).key;
  if (u.period === key) return u;
  return { period: key, bytes: 0, requests: 0, pendingBytes: 0, pendingRequests: 0, lastFlush: u.lastFlush };
}

export function totalBytes(u: UsageSnapshot): number {
  return u.bytes + u.pendingBytes;
}

export const FLUSH_BYTES = 1024 * 1024;
export const FLUSH_INTERVAL_MS = 30_000;

/** Flush the pending delta to the ledger after 1 MiB or 30 s (whichever first). */
export function shouldFlush(u: UsageSnapshot, now = Date.now()): boolean {
  if (u.pendingBytes === 0 && u.pendingRequests === 0) return false;
  return u.pendingBytes >= FLUSH_BYTES || now - u.lastFlush >= FLUSH_INTERVAL_MS;
}

/** Monthly ledger entry kept by the AccountDO. */
export interface Ledger {
  period: string;
  bytes: number;
  requests: number;
}

export function addToLedger(l: Ledger | undefined, bytes: number, requests: number, now = Date.now()): Ledger {
  const key = monthPeriod(now).key;
  const base = l && l.period === key ? l : { period: key, bytes: 0, requests: 0 };
  return { period: key, bytes: base.bytes + Math.max(0, bytes), requests: base.requests + Math.max(0, requests) };
}

/**
 * Token bucket per key (client IP). `perMinute` tokens refill continuously; bursts up to the
 * same amount. The key map is bounded so a flood of distinct IPs can't grow memory unbounded.
 */
export class RateLimiter {
  private buckets = new Map<string, { tokens: number; at: number }>();
  constructor(
    private readonly perMinute: number,
    private readonly maxKeys = 5000,
  ) {}

  /** Consumes one token for `key`; `false` means rate limited. */
  allow(key: string, now = Date.now()): boolean {
    const rate = this.perMinute / 60_000;
    let b = this.buckets.get(key);
    if (!b) {
      if (this.buckets.size >= this.maxKeys) {
        const oldest = this.buckets.keys().next().value;
        if (oldest !== undefined) this.buckets.delete(oldest);
      }
      b = { tokens: this.perMinute, at: now };
    } else {
      this.buckets.delete(key); // re-insert to keep LRU order
      b.tokens = Math.min(this.perMinute, b.tokens + (now - b.at) * rate);
      b.at = now;
    }
    this.buckets.set(key, b);
    if (b.tokens < 1) return false;
    b.tokens -= 1;
    return true;
  }

  /** Seconds until one token is available again. */
  retryAfter(key: string, now = Date.now()): number {
    const b = this.buckets.get(key);
    if (!b || b.tokens >= 1) return 0;
    const rate = this.perMinute / 60_000;
    const tokens = Math.min(this.perMinute, b.tokens + (now - b.at) * rate);
    return Math.max(1, Math.ceil((1 - tokens) / rate / 1000));
  }

  get size(): number {
    return this.buckets.size;
  }
}

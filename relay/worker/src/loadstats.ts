// Pure pieces of cloud load runs (no runtime APIs, so they unit-test anywhere): the arrival
// schedule of an open-model profile, a mergeable log-linear latency histogram, the recorder each
// region fills, and the merge into a devall-load–shaped summary.

/** Microseconds → milliseconds rounded to 0.01 (the devall-load summary convention). */
function ms(us: number): number {
  return Math.round(us / 10) / 100;
}

function round2(v: number): number {
  return Math.round(v * 100) / 100;
}

// ---------- arrival schedule ----------

export interface ProfileStage {
  durationMs: number;
  /** Iterations per second reached at the end of the stage. */
  target: number;
}

interface Seg {
  t0: number;
  t1: number;
  a: number;
  b: number;
  c0: number;
}

/**
 * A piecewise-linear iteration rate turned into the intended start offset of every iteration:
 * iteration `k` starts when the integral of the rate reaches `k` (same as devall-load's
 * `ArrivalSchedule`, so cloud and local runs schedule identically).
 */
export class ArrivalSchedule {
  private segs: Seg[] = [];
  private idx = 0;
  private k = 0;

  constructor(startRate: number, stages: ProfileStage[]) {
    let t = 0;
    let rate = Math.max(0, startRate);
    let c = 0;
    for (const st of stages) {
      const target = Math.max(0, st.target);
      const seg: Seg = { t0: t, t1: t + st.durationMs / 1000, a: rate, b: target, c0: c };
      c += ((seg.a + seg.b) / 2) * (seg.t1 - seg.t0);
      t = seg.t1;
      rate = target;
      this.segs.push(seg);
    }
  }

  /** Total iterations the profile asks for. */
  expected(): number {
    const s = this.segs[this.segs.length - 1];
    return s ? s.c0 + ((s.a + s.b) / 2) * (s.t1 - s.t0) : 0;
  }

  /** Intended start of the next iteration in ms from the start of the run, or null when done. */
  next(): number | null {
    const k = this.k;
    while (this.idx < this.segs.length) {
      const seg = this.segs[this.idx]!;
      const c1 = seg.c0 + ((seg.a + seg.b) / 2) * (seg.t1 - seg.t0);
      if (seg.t1 > seg.t0 && k < c1 - 1e-9) {
        const delta = Math.max(0, k - seg.c0);
        let tau = 0;
        if (delta > 0) {
          const s = (seg.b - seg.a) / (seg.t1 - seg.t0);
          const disc = Math.max(0, seg.a * seg.a + 2 * s * delta);
          const denom = seg.a + Math.sqrt(disc);
          tau = denom <= 0 ? 0 : (2 * delta) / denom;
        }
        this.k += 1;
        return Math.min(seg.t0 + tau, seg.t1) * 1000;
      }
      this.idx += 1;
    }
    return null;
  }
}

/** Peak iteration rate (per second) of a profile. */
export function peakRate(startRate: number, stages: ProfileStage[]): number {
  return stages.reduce((m, s) => Math.max(m, s.target), Math.max(0, startRate));
}

// ---------- log-linear histogram ----------

/**
 * Sparse log-linear histogram of non-negative integers (microseconds): exact below 128, then 64
 * linear sub-buckets per power of two (≤ 1.6 % relative error, the HDR idea with 2 significant
 * digits). Merging is adding counts, so regions merge exactly like devall-load's HDR histograms.
 */
export class LogHistogram {
  counts = new Map<number, number>();
  count = 0;
  min = Number.POSITIVE_INFINITY;
  max = 0;
  sum = 0;

  static index(v: number): number {
    if (v < 128) return v;
    const m = Math.floor(Math.log2(v)) - 6;
    return m * 64 + Math.floor(v / 2 ** m);
  }

  /** Highest value that lands in bucket `idx`. */
  static highest(idx: number): number {
    if (idx < 128) return idx;
    const m = Math.floor(idx / 64) - 1;
    const sub = idx - m * 64;
    return (sub + 1) * 2 ** m - 1;
  }

  record(value: number): void {
    const v = Math.max(0, Math.round(value));
    const i = LogHistogram.index(v);
    this.counts.set(i, (this.counts.get(i) ?? 0) + 1);
    this.count += 1;
    this.sum += v;
    if (v < this.min) this.min = v;
    if (v > this.max) this.max = v;
  }

  merge(o: LogHistogram | HistogramJson): void {
    const h = o instanceof LogHistogram ? o.toJSON() : o;
    for (const [i, c] of h.buckets) this.counts.set(i, (this.counts.get(i) ?? 0) + c);
    this.count += h.count;
    this.sum += h.sum;
    if (h.count > 0) {
      this.min = Math.min(this.min, h.min);
      this.max = Math.max(this.max, h.max);
    }
  }

  /** Value at quantile `q` (0–1), clamped to the observed min/max. */
  quantile(q: number): number {
    if (this.count === 0) return 0;
    const rank = Math.max(1, Math.ceil(q * this.count));
    let seen = 0;
    for (const i of [...this.counts.keys()].sort((a, b) => a - b)) {
      seen += this.counts.get(i)!;
      if (seen >= rank) return Math.min(this.max, Math.max(this.min, LogHistogram.highest(i)));
    }
    return this.max;
  }

  /** devall-load `Percentiles` (milliseconds). */
  percentiles(): Percentiles {
    if (this.count === 0) return { p50: 0, p75: 0, p90: 0, p95: 0, p99: 0, p999: 0, min: 0, max: 0, mean: 0 };
    return {
      p50: ms(this.quantile(0.5)),
      p75: ms(this.quantile(0.75)),
      p90: ms(this.quantile(0.9)),
      p95: ms(this.quantile(0.95)),
      p99: ms(this.quantile(0.99)),
      p999: ms(this.quantile(0.999)),
      min: ms(this.min),
      max: ms(this.max),
      mean: ms(this.sum / this.count),
    };
  }

  toJSON(): HistogramJson {
    return { buckets: [...this.counts.entries()].sort((a, b) => a[0] - b[0]), count: this.count, min: this.count ? this.min : 0, max: this.max, sum: this.sum };
  }

  static from(j: HistogramJson): LogHistogram {
    const h = new LogHistogram();
    h.merge(j);
    return h;
  }
}

export interface HistogramJson {
  /** `[bucket index, count]`, ascending. */
  buckets: [number, number][];
  count: number;
  min: number;
  max: number;
  sum: number;
}

export interface Percentiles {
  p50: number;
  p75: number;
  p90: number;
  p95: number;
  p99: number;
  p999: number;
  min: number;
  max: number;
  mean: number;
}

// ---------- recorder ----------

export type ErrorKind = "connect" | "timeout" | "other";

export interface Sample {
  req: number;
  /** From the intended start (µs); null when there was no response and no timeout. */
  latencyUs: number | null;
  /** From the actual send (µs). */
  serviceUs: number | null;
  status: number | null;
  error: { kind: ErrorKind; message: string } | null;
  bytesIn: number;
  bytesOut: number;
  checksPassed: number;
  checksFailed: number;
  /** ms since the start of the run, for the timeline. */
  at: number;
}

interface ReqAcc {
  hist: LogHistogram;
  requests: number;
  failed: number;
  statuses: Record<string, number>;
}

interface Bucket {
  hist: LogHistogram;
  requests: number;
  failed: number;
  dropped: number;
  vus: number;
}

export interface RegionTotals {
  requests: number;
  failed: number;
  iterations: number;
  droppedIterations: number;
  bytesIn: number;
  bytesOut: number;
  checksPassed: number;
  checksFailed: number;
  maxInFlight: number;
}

/** What one region reports (`LoadDO.finish`). */
export interface RegionResult {
  region: string;
  colo: string | null;
  ok: boolean;
  error: string | null;
  /** ms since the run's t0 when the region's last request finished. */
  elapsedMs: number;
  totals: RegionTotals;
  latency: HistogramJson;
  service: HistogramJson;
  requests: { hist: HistogramJson; requests: number; failed: number; statuses: Record<string, number> }[];
  statuses: Record<string, number>;
  errors: { connect: number; timeout: number; other: number };
  errorSamples: string[];
  buckets: [number, { hist: HistogramJson; requests: number; failed: number; dropped: number; vus: number }][];
}

const MAX_ERROR_SAMPLES = 5;

export class Recorder {
  latency = new LogHistogram();
  service = new LogHistogram();
  reqs: ReqAcc[];
  statuses: Record<string, number> = {};
  errors = { connect: 0, timeout: 0, other: 0 };
  errorSamples: string[] = [];
  totals: RegionTotals = { requests: 0, failed: 0, iterations: 0, droppedIterations: 0, bytesIn: 0, bytesOut: 0, checksPassed: 0, checksFailed: 0, maxInFlight: 0 };
  buckets = new Map<number, Bucket>();
  lastAt = 0;

  constructor(
    requests: number,
    readonly bucketMs: number,
  ) {
    this.reqs = Array.from({ length: requests }, () => ({ hist: new LogHistogram(), requests: 0, failed: 0, statuses: {} }));
  }

  private bucket(at: number): Bucket {
    const i = Math.max(0, Math.floor(at / this.bucketMs));
    let b = this.buckets.get(i);
    if (!b) {
      b = { hist: new LogHistogram(), requests: 0, failed: 0, dropped: 0, vus: 0 };
      this.buckets.set(i, b);
    }
    return b;
  }

  record(s: Sample): void {
    const failed = s.error !== null || (s.status !== null && s.status >= 400);
    const t = this.totals;
    t.requests += 1;
    if (failed) t.failed += 1;
    t.bytesIn += s.bytesIn;
    t.bytesOut += s.bytesOut;
    t.checksPassed += s.checksPassed;
    t.checksFailed += s.checksFailed;
    this.lastAt = Math.max(this.lastAt, s.at);
    const b = this.bucket(s.at);
    b.requests += 1;
    if (failed) b.failed += 1;
    const r = this.reqs[s.req];
    if (s.latencyUs !== null) {
      this.latency.record(s.latencyUs);
      b.hist.record(s.latencyUs);
      r?.hist.record(s.latencyUs);
    }
    if (s.serviceUs !== null) this.service.record(s.serviceUs);
    if (r) {
      r.requests += 1;
      if (failed) r.failed += 1;
      if (s.status !== null) r.statuses[String(s.status)] = (r.statuses[String(s.status)] ?? 0) + 1;
    }
    if (s.status !== null) this.statuses[String(s.status)] = (this.statuses[String(s.status)] ?? 0) + 1;
    if (s.error) {
      this.errors[s.error.kind] += 1;
      if (this.errorSamples.length < MAX_ERROR_SAMPLES && !this.errorSamples.includes(s.error.message)) this.errorSamples.push(s.error.message);
    }
  }

  iterationDone(at: number): void {
    this.totals.iterations += 1;
    this.lastAt = Math.max(this.lastAt, at);
  }

  dropped(at: number): void {
    this.totals.droppedIterations += 1;
    this.bucket(at).dropped += 1;
  }

  inFlight(n: number, at: number): void {
    this.totals.maxInFlight = Math.max(this.totals.maxInFlight, n);
    const b = this.bucket(at);
    b.vus = Math.max(b.vus, n);
  }

  result(region: string, colo: string | null, error: string | null): RegionResult {
    return {
      region,
      colo,
      ok: error === null,
      error,
      elapsedMs: this.lastAt,
      totals: { ...this.totals },
      latency: this.latency.toJSON(),
      service: this.service.toJSON(),
      requests: this.reqs.map((r) => ({ hist: r.hist.toJSON(), requests: r.requests, failed: r.failed, statuses: { ...r.statuses } })),
      statuses: { ...this.statuses },
      errors: { ...this.errors },
      errorSamples: [...this.errorSamples],
      buckets: [...this.buckets.entries()].sort((a, b) => a[0] - b[0]).map(([i, b]) => [i, { hist: b.hist.toJSON(), requests: b.requests, failed: b.failed, dropped: b.dropped, vus: b.vus }]),
    };
  }
}

// ---------- merge into a summary ----------

export interface RequestMeta {
  path: string;
  name: string;
  method: string;
}

export interface TimelinePoint {
  t: number;
  requests: number;
  failed: number;
  rps: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
  vus: number;
  dropped: number;
}

export interface RegionSummary {
  region: string;
  name: string;
  /** Data center the region's Durable Object actually ran in, when detected (IATA code). */
  colo: string | null;
  share: number;
  ok: boolean;
  error: string | null;
  requests: number;
  failed: number;
  droppedIterations: number;
  p50: number;
  p95: number;
}

/** The answer of `POST /v1/load/runs`: the devall-load `LoadSummary` fields the edge can fill. */
export interface CloudLoadResult {
  version: 1;
  runId: string;
  test: string;
  startedAt: string;
  durationMs: number;
  plannedDurationMs: number;
  aborted: boolean;
  abortReason: string | null;
  openModel: true;
  totals: {
    requests: number;
    failed: number;
    errorRate: number;
    iterations: number;
    droppedIterations: number;
    rps: number;
    iterationsPerSec: number;
    bytesIn: number;
    bytesOut: number;
    checksPassed: number;
    checksFailed: number;
    checksRate: number;
    maxVus: number;
  };
  latency: Percentiles;
  serviceTime: Percentiles;
  statusCodes: Record<string, number>;
  errorKinds: { connect: number; timeout: number; other: number };
  errorSamples: string[];
  requests: { path: string; name: string; method: string; requests: number; failed: number; errorRate: number; rps: number; latency: Percentiles; statusCodes: Record<string, number> }[];
  bucketSeconds: number;
  timeline: TimelinePoint[];
  warnings: string[];
  regions: RegionSummary[];
  /** Merged latency histogram (µs, log-linear) for clients that want other quantiles. */
  histogram: HistogramJson;
}

/** Timeline bucket width: ≤ 600 points per run (devall-load `bucket_seconds`). */
export function bucketSecondsFor(plannedMs: number): number {
  const secs = Math.floor(plannedMs / 1000) + 30;
  return Math.max(1, Math.ceil(secs / 600));
}

export function mergeRegions(a: {
  runId: string;
  test: string;
  startedAt: number;
  plannedMs: number;
  bucketSeconds: number;
  meta: RequestMeta[];
  regions: { region: string; name: string; share: number; result: RegionResult | null; error: string | null }[];
  aborted: boolean;
  abortReason: string | null;
  warnings: string[];
}): CloudLoadResult {
  const latency = new LogHistogram();
  const service = new LogHistogram();
  const reqs = a.meta.map(() => ({ hist: new LogHistogram(), requests: 0, failed: 0, statuses: {} as Record<string, number> }));
  const statuses: Record<string, number> = {};
  const errors = { connect: 0, timeout: 0, other: 0 };
  const samples: string[] = [];
  const totals = { requests: 0, failed: 0, iterations: 0, droppedIterations: 0, bytesIn: 0, bytesOut: 0, checksPassed: 0, checksFailed: 0, maxInFlight: 0 };
  const buckets = new Map<number, { hist: LogHistogram; requests: number; failed: number; dropped: number; vus: number }>();
  let elapsed = 0;
  const add = (into: Record<string, number>, from: Record<string, number>) => {
    for (const [k, v] of Object.entries(from)) into[k] = (into[k] ?? 0) + v;
  };
  for (const r of a.regions) {
    const res = r.result;
    if (!res) continue;
    latency.merge(res.latency);
    service.merge(res.service);
    res.requests.forEach((x, i) => {
      const acc = reqs[i];
      if (!acc) return;
      acc.hist.merge(x.hist);
      acc.requests += x.requests;
      acc.failed += x.failed;
      add(acc.statuses, x.statuses);
    });
    add(statuses, res.statuses);
    errors.connect += res.errors.connect;
    errors.timeout += res.errors.timeout;
    errors.other += res.errors.other;
    for (const s of res.errorSamples) if (samples.length < MAX_ERROR_SAMPLES && !samples.includes(s)) samples.push(s);
    for (const k of Object.keys(totals) as (keyof typeof totals)[]) totals[k] += res.totals[k];
    for (const [i, b] of res.buckets) {
      let m = buckets.get(i);
      if (!m) {
        m = { hist: new LogHistogram(), requests: 0, failed: 0, dropped: 0, vus: 0 };
        buckets.set(i, m);
      }
      m.hist.merge(b.hist);
      m.requests += b.requests;
      m.failed += b.failed;
      m.dropped += b.dropped;
      m.vus += b.vus;
    }
    elapsed = Math.max(elapsed, res.elapsedMs);
  }
  const durationMs = Math.max(1, Math.round(a.aborted ? elapsed : Math.max(elapsed, a.plannedMs)));
  const secs = durationMs / 1000;
  const bucketMs = a.bucketSeconds * 1000;
  const lastIdx = Math.max(-1, ...buckets.keys());
  const timeline: TimelinePoint[] = [];
  for (let i = 0; i <= lastIdx; i++) {
    const b = buckets.get(i);
    const covered = Math.min(bucketMs, Math.max(1, durationMs - i * bucketMs)) / 1000;
    timeline.push({
      t: i * a.bucketSeconds,
      requests: b?.requests ?? 0,
      failed: b?.failed ?? 0,
      rps: round2((b?.requests ?? 0) / covered),
      p50: b ? ms(b.hist.quantile(0.5)) : 0,
      p95: b ? ms(b.hist.quantile(0.95)) : 0,
      p99: b ? ms(b.hist.quantile(0.99)) : 0,
      max: b && b.hist.count ? ms(b.hist.max) : 0,
      vus: b?.vus ?? 0,
      dropped: b?.dropped ?? 0,
    });
  }
  const checked = totals.checksPassed + totals.checksFailed;
  const regions: RegionSummary[] = a.regions.map((r) => {
    const lat = r.result ? LogHistogram.from(r.result.latency) : null;
    return {
      region: r.region,
      name: r.name,
      colo: r.result?.colo ?? null,
      share: round2(r.share * 1000) / 1000,
      ok: !r.error && !!r.result?.ok,
      error: r.error ?? r.result?.error ?? null,
      requests: r.result?.totals.requests ?? 0,
      failed: r.result?.totals.failed ?? 0,
      droppedIterations: r.result?.totals.droppedIterations ?? 0,
      p50: lat ? ms(lat.quantile(0.5)) : 0,
      p95: lat ? ms(lat.quantile(0.95)) : 0,
    };
  });
  return {
    version: 1,
    runId: a.runId,
    test: a.test,
    startedAt: new Date(a.startedAt).toISOString().replace(/\.\d{3}Z$/, "Z"),
    durationMs,
    plannedDurationMs: a.plannedMs,
    aborted: a.aborted,
    abortReason: a.abortReason,
    openModel: true,
    totals: {
      requests: totals.requests,
      failed: totals.failed,
      errorRate: totals.requests ? totals.failed / totals.requests : 0,
      iterations: totals.iterations,
      droppedIterations: totals.droppedIterations,
      rps: round2(totals.requests / secs),
      iterationsPerSec: round2(totals.iterations / secs),
      bytesIn: totals.bytesIn,
      bytesOut: totals.bytesOut,
      checksPassed: totals.checksPassed,
      checksFailed: totals.checksFailed,
      checksRate: checked ? totals.checksPassed / checked : 1,
      maxVus: totals.maxInFlight,
    },
    latency: latency.percentiles(),
    serviceTime: service.percentiles(),
    statusCodes: statuses,
    errorKinds: errors,
    errorSamples: samples,
    requests: a.meta.map((m, i) => {
      const r = reqs[i]!;
      return {
        path: m.path,
        name: m.name,
        method: m.method,
        requests: r.requests,
        failed: r.failed,
        errorRate: r.requests ? r.failed / r.requests : 0,
        rps: round2(r.requests / secs),
        latency: r.hist.percentiles(),
        statusCodes: r.statuses,
      };
    }),
    bucketSeconds: a.bucketSeconds,
    timeline,
    warnings: a.warnings,
    regions,
    histogram: latency.toJSON(),
  };
}

// ---------- target policy ----------

function ipv4(h: string): number[] | null {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(h);
  if (!m) return null;
  const o = m.slice(1).map(Number);
  return o.every((x) => x <= 255) ? o : null;
}

/** Why a target host is private (loopback, private, link-local, local-only names…), or null. */
export function privateReason(hostname: string): string | null {
  const h = hostname.toLowerCase().replace(/\.$/, "");
  if (h === "localhost" || /\.(localhost|local|internal|lan|home\.arpa)$/.test(h)) return "a local-only host name";
  if (!h.includes(".") && !h.includes(":") && !h.startsWith("[")) return "a single-label (local) host name";
  const v4 = ipv4(h);
  if (v4) {
    const [a, b] = v4 as [number, number, number, number];
    const priv =
      a === 0 ||
      a === 10 ||
      a === 127 ||
      (a === 100 && b >= 64 && b <= 127) ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168) ||
      (a === 192 && b === 0 && v4[2] === 0) ||
      (a === 198 && (b === 18 || b === 19)) ||
      a >= 224;
    return priv ? "a private, loopback, link-local or reserved address" : null;
  }
  if (h.startsWith("[") || h.includes(":")) {
    const v6 = h.replace(/^\[|\]$/g, "");
    const mapped = /^::ffff:(\d+\.\d+\.\d+\.\d+)$/.exec(v6);
    if (mapped) return privateReason(mapped[1]!);
    if (v6 === "::" || v6 === "::1" || /^f[cd]/.test(v6) || /^fe[89ab]/.test(v6) || /^ff/.test(v6)) return "a private, loopback, link-local or multicast address";
    return null;
  }
  return null;
}

/** Whether `h` is an IP literal (`1.2.3.4`, `[::1]`). */
export function isIpLiteral(h: string): boolean {
  return ipv4(h) !== null || h.startsWith("[");
}

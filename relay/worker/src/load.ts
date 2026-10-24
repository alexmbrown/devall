// Cloud load-generator regions (Supporter / self-host): short, bounded open-model load tests run
// from Durable Objects created with a `locationHint`, only against hosts the account proved it
// controls.
//
// Abuse model (relay/README.md → Abuse policy):
//  * Target ownership is mandatory (like loader.io / k6 cloud): a token served at
//    `<origin>/.well-known/devall-load-verification.txt`, or a DNS TXT record
//    `_devall-load.<host>`, before any load is sent. Verifications expire after 30 days.
//  * Hosted plans never reach private, loopback or link-local addresses (nor the relay's own
//    domains); self-hosted relays may (still verified).
//  * Bounded runs: duration, total request rate, regions, runs per day, one run at a time per
//    account; a kill switch (`CLOUD_LOAD = "off"` or `POST /admin/load/disable`) is checked
//    before every segment. Every request carries `x-devall-load: <run id>`.
//
// Placement is best-effort: `locationHint` only suggests where a Durable Object is created (and it
// never moves). The result records the data center each region really ran in when it can be
// detected (a `/cdn-cgi/trace` probe), and null otherwise.
//
// A run is split into segments (≤ 10 s and ≤ SEGMENT_MAX_REQUESTS requests per region): each
// segment is one RPC to every region, so no single invocation comes near the subrequest limit,
// and the kill switch / stop button act between segments.

import { DurableObject } from "cloudflare:workers";
import type { Principal } from "./auth";
import { domainOf, mockDomain, type Env } from "./env";
import { json, jsonError, readBodyCapped } from "./http";
import { ArrivalSchedule, bucketSecondsFor, isIpLiteral, mergeRegions, peakRate, privateReason, Recorder, type CloudLoadResult, type ErrorKind, type ProfileStage, type RegionResult, type RequestMeta } from "./loadstats";
import { isValidHostname, randomId, ulid } from "./names";
import { limitsFor, type Limits } from "./plans";

export const LOAD_REGIONS = ["wnam", "enam", "sam", "weur", "eeur", "apac", "oc", "afr", "me"] as const;
export type LoadRegion = (typeof LOAD_REGIONS)[number];
export const REGION_NAMES: Record<LoadRegion, string> = {
  wnam: "Western North America",
  enam: "Eastern North America",
  sam: "South America",
  weur: "Western Europe",
  eeur: "Eastern Europe",
  apac: "Asia-Pacific",
  oc: "Oceania",
  afr: "Africa",
  me: "Middle East",
};

export const VERIFY_PATH = "/.well-known/devall-load-verification.txt";
export const DNS_PREFIX = "_devall-load.";
export const VERIFY_TTL_MS = 30 * 24 * 3600_000;
export const MAX_TARGETS = 20;
const VERIFY_TIMEOUT_MS = 10_000;
const SEGMENT_MAX_MS = 10_000;
/** Requests per region per segment (one RPC invocation): far below the platform's subrequest limit. */
export const SEGMENT_MAX_REQUESTS = 800;
const START_DELAY_MS = 300;
const MAX_REQUEST_TIMEOUT_MS = 30_000;
const MAX_BODY = 64 * 1024;
const MAX_REQUESTS = 20;
const UA = "devall-load-cloud/1 (+verified target; see /.well-known/devall-load-verification.txt)";

/** Outbound network for verification and load (tests replace it). */
export const loadNet: { fetch: (r: Request) => Promise<Response> } = { fetch: (r) => fetch(r) };

// ---------- targets ----------

export interface TargetAddress {
  /** `https://api.example.com`, `http://127.0.0.1:3000` */
  origin: string;
  scheme: "http" | "https";
  hostname: string;
  port: number;
  /** `hostname:port` — what a verification covers (DNS verifications cover every port). */
  key: string;
}

export interface LoadTarget extends TargetAddress {
  token: string;
  createdAt: number;
  verifiedAt: number | null;
  expiresAt: number | null;
  method: "http" | "dns" | null;
  lastCheckAt: number | null;
  lastError: string | null;
}

export interface TargetView {
  key: string;
  origin: string;
  hostname: string;
  port: number;
  status: "pending" | "verified" | "expired";
  token: string;
  /** Where to serve the token (the file contains just the token). */
  fileUrl: string;
  dnsName: string;
  verifiedAt: string | null;
  expiresAt: string | null;
  method: "http" | "dns" | null;
  lastError: string | null;
}

/** Normalizes what a user typed (`api.example.com`, `http://127.0.0.1:3000`, a full URL). */
export function parseTarget(input: unknown): TargetAddress | string {
  if (typeof input !== "string" || !input.trim() || input.length > 300) return "target must be a host name or URL, e.g. api.example.com";
  const raw = input.trim();
  let u: URL;
  try {
    u = new URL(raw.includes("://") ? raw : `https://${raw}`);
  } catch {
    return `"${raw}" isn't a host name or URL`;
  }
  if (u.protocol !== "https:" && u.protocol !== "http:") return "only http:// and https:// targets can be load tested";
  if (u.username || u.password) return "no credentials in the target";
  const hostname = u.hostname.toLowerCase().replace(/\.$/, "");
  if (!hostname || !(isIpLiteral(hostname) || hostname === "localhost" || isValidHostname(hostname))) return `"${hostname}" isn't a valid host name`;
  const scheme = u.protocol === "http:" ? "http" : "https";
  const defaultPort = scheme === "https" ? 443 : 80;
  const port = u.port ? Number(u.port) : defaultPort;
  return { origin: `${scheme}://${hostname}${port === defaultPort ? "" : `:${port}`}`, scheme, hostname, port, key: `${hostname}:${port}` };
}

/** Why `hostname` can't be a target on this plan / relay, or null. */
export function targetPolicy(env: Env, limits: Limits, hostname: string): string | null {
  const h = hostname.toLowerCase();
  const own = [domainOf(env), mockDomain(env)];
  if (own.some((d) => h === d || h.endsWith(`.${d}`))) return `${hostname} is served by this relay; load tests can't target the relay itself`;
  if (!limits.loadPrivateTargets) {
    const why = privateReason(h);
    if (why) return `${hostname} is ${why}; the hosted relay only load tests public hosts (self-host the relay to test private ones)`;
  }
  return null;
}

export function targetView(t: LoadTarget, now = Date.now()): TargetView {
  const status = t.verifiedAt === null ? "pending" : t.expiresAt !== null && t.expiresAt <= now ? "expired" : "verified";
  return {
    key: t.key,
    origin: t.origin,
    hostname: t.hostname,
    port: t.port,
    status,
    token: t.token,
    fileUrl: `${t.origin}${VERIFY_PATH}`,
    dnsName: `${DNS_PREFIX}${t.hostname}`,
    verifiedAt: t.verifiedAt ? new Date(t.verifiedAt).toISOString() : null,
    expiresAt: t.expiresAt ? new Date(t.expiresAt).toISOString() : null,
    method: t.method,
    lastError: t.lastError,
  };
}

/** Whether `targets` cover `url` with a live verification (same host:port, or DNS for the host). */
export function coveredBy(targets: LoadTarget[], url: URL, now = Date.now()): boolean {
  const hostname = url.hostname.toLowerCase().replace(/\.$/, "");
  const port = url.port ? Number(url.port) : url.protocol === "https:" ? 443 : 80;
  return targets.some(
    (t) => t.verifiedAt !== null && t.expiresAt !== null && t.expiresAt > now && t.hostname === hostname && (t.port === port || t.method === "dns"),
  );
}

export function newToken(): string {
  return `devall-load-${randomId(26)}`;
}

async function checkFile(t: TargetAddress, token: string): Promise<string | null> {
  const url = `${t.origin}${VERIFY_PATH}`;
  try {
    const res = await loadNet.fetch(new Request(url, { headers: { "user-agent": UA, accept: "text/plain" }, redirect: "manual", signal: AbortSignal.timeout(VERIFY_TIMEOUT_MS) }));
    if (res.status !== 200) {
      await res.body?.cancel().catch(() => {});
      return `${url} answered HTTP ${res.status}${res.status >= 300 && res.status < 400 ? " (redirects aren't followed)" : ""}`;
    }
    const body = await readBodyCapped(res.body, 4096);
    if (body === null) return `${url} is larger than 4 KB`;
    const words = new TextDecoder().decode(body).split(/\s+/);
    return words.includes(token) ? null : `${url} doesn't contain the token`;
  } catch (e) {
    return `${url}: ${e instanceof Error ? (e.name === "TimeoutError" ? "timed out" : e.message) : String(e)}`;
  }
}

async function checkDns(hostname: string, token: string): Promise<string | null> {
  const name = `${DNS_PREFIX}${hostname}`;
  try {
    const res = await loadNet.fetch(
      new Request(`https://cloudflare-dns.com/dns-query?name=${encodeURIComponent(name)}&type=TXT`, { headers: { accept: "application/dns-json" }, signal: AbortSignal.timeout(VERIFY_TIMEOUT_MS) }),
    );
    if (!res.ok) return `DNS lookup of ${name} failed (HTTP ${res.status})`;
    const body = (await res.json()) as { Answer?: { type?: number; data?: string }[] };
    const values = (body.Answer ?? []).filter((a) => a.type === 16 && typeof a.data === "string").map((a) => a.data!.replace(/"\s*"/g, "").replace(/^"|"$/g, "").trim());
    return values.includes(token) ? null : `no TXT record ${name} with the token`;
  } catch (e) {
    return `DNS lookup of ${name}: ${e instanceof Error ? e.message : String(e)}`;
  }
}

/** Checks the HTTP file, then (for DNS names) the TXT record. */
export async function verifyOwnership(env: Env, t: LoadTarget): Promise<{ ok: true; method: "http" | "dns" } | { ok: false; details: string[] }> {
  const details: string[] = [];
  const file = await checkFile(t, t.token);
  if (file === null) return { ok: true, method: "http" };
  details.push(file);
  if (!isIpLiteral(t.hostname) && t.hostname.includes(".") && (env.LOAD_VERIFY_DNS ?? "on").toLowerCase() !== "off") {
    const dns = await checkDns(t.hostname, t.token);
    if (dns === null) return { ok: true, method: "dns" };
    details.push(dns);
  }
  return { ok: false, details };
}

// ---------- run plans ----------

export type CloudCheck = { type: "status"; expect: string[] } | { type: "maxDuration"; ms: number } | { type: "bodyContains"; text: string };

export interface CloudRequestSpec {
  name: string;
  /** Request file relative to `http/` (for per-request stats). */
  path: string;
  method: string;
  url: string;
  headers: [string, string][];
  bodyBase64?: string | null;
  timeoutMs?: number;
  followRedirects?: boolean;
}

/** `POST /v1/load/runs` body (built by devall-load's `cloud` module). */
export interface CloudLoadPlan {
  runId?: string;
  name: string;
  regions: string[];
  /** Iterations per second, piecewise linear (constant rate = one stage at the same rate). */
  profile: { startRate: number; stages: ProfileStage[] };
  /** Iterations in flight at once across the run (like k6's maxVUs); split across regions. */
  maxInFlight?: number;
  requests: CloudRequestSpec[];
  /** Weighted mix: one entry per request. */
  weights?: number[] | null;
  /** Ordered flow: request indexes run in order every iteration. */
  flow?: number[] | null;
  thinkTimeMs?: { min: number; max: number } | null;
  checks?: CloudCheck[];
}

/** One region's share, as sent to its LoadDO. */
export interface RegionPlan {
  runId: string;
  region: string;
  t0: number;
  share: number;
  profile: { startRate: number; stages: ProfileStage[] };
  maxInFlight: number;
  requests: CloudRequestSpec[];
  weights: number[] | null;
  flow: number[] | null;
  thinkTimeMs: { min: number; max: number } | null;
  checks: CloudCheck[];
  bucketMs: number;
  coloProbe: string | null;
}

function isRegion(r: unknown): r is LoadRegion {
  return typeof r === "string" && (LOAD_REGIONS as readonly string[]).includes(r);
}

const HEADER_NAME = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;

/** Validates a plan against the plan limits. Returns an error message or null. */
export function validatePlan(p: Partial<CloudLoadPlan>, limits: Limits): string | null {
  if (typeof p.name !== "string" || !p.name.trim() || p.name.length > 200) return "name must be 1–200 characters";
  if (p.runId !== undefined && (typeof p.runId !== "string" || !/^[A-Za-z0-9_-]{8,40}$/.test(p.runId))) return "runId must be 8–40 characters of A-Z a-z 0-9 _ -";
  if (!Array.isArray(p.regions) || p.regions.length === 0) return "pick at least one region";
  if (!p.regions.every(isRegion)) return `regions must be among ${LOAD_REGIONS.join(", ")}`;
  if (new Set(p.regions).size !== p.regions.length) return "regions must be distinct";
  if (p.regions.length > limits.loadRegions) return `Your plan runs cloud load from at most ${limits.loadRegions} region${limits.loadRegions === 1 ? "" : "s"} at once.`;
  const prof = p.profile;
  if (!prof || typeof prof !== "object" || !Array.isArray(prof.stages) || prof.stages.length === 0 || prof.stages.length > 20) return "profile needs 1–20 stages";
  if (typeof prof.startRate !== "number" || !Number.isFinite(prof.startRate) || prof.startRate < 0) return "profile.startRate must be ≥ 0";
  for (const s of prof.stages) {
    if (!s || typeof s.durationMs !== "number" || !Number.isFinite(s.durationMs) || s.durationMs <= 0) return "every stage needs a positive durationMs";
    if (typeof s.target !== "number" || !Number.isFinite(s.target) || s.target < 0) return "every stage needs a target rate ≥ 0";
  }
  const totalMs = prof.stages.reduce((n, s) => n + s.durationMs, 0);
  if (totalMs < 1000) return "the run must last at least 1 s";
  if (totalMs > limits.loadMaxDurationSeconds * 1000) return `Your plan allows cloud runs of up to ${limits.loadMaxDurationSeconds} s (this one is ${Math.ceil(totalMs / 1000)} s).`;
  if (!Array.isArray(p.requests) || p.requests.length === 0 || p.requests.length > MAX_REQUESTS) return `requests must hold 1–${MAX_REQUESTS} entries`;
  for (const [i, r] of p.requests.entries()) {
    const where = `requests[${i}]`;
    if (!r || typeof r !== "object") return `${where} must be an object`;
    if (typeof r.name !== "string" || typeof r.path !== "string") return `${where} needs name and path`;
    if (typeof r.method !== "string" || !/^[A-Z]{3,10}$/.test(r.method)) return `${where}.method must be an HTTP method`;
    let u: URL;
    try {
      u = new URL(String(r.url));
    } catch {
      return `${where}.url must be an absolute http(s) URL`;
    }
    if (u.protocol !== "http:" && u.protocol !== "https:") return `${where}.url must be http(s)`;
    if (!Array.isArray(r.headers) || r.headers.length > 50 || !r.headers.every((h) => Array.isArray(h) && h.length === 2 && typeof h[0] === "string" && HEADER_NAME.test(h[0]) && typeof h[1] === "string")) {
      return `${where}.headers must be up to 50 [name, value] pairs`;
    }
    if (r.bodyBase64 !== undefined && r.bodyBase64 !== null && (typeof r.bodyBase64 !== "string" || r.bodyBase64.length > Math.ceil((MAX_BODY * 4) / 3) + 4)) return `${where}.bodyBase64 must be ≤ 64 KiB`;
    if (r.timeoutMs !== undefined && (typeof r.timeoutMs !== "number" || r.timeoutMs < 100 || r.timeoutMs > MAX_REQUEST_TIMEOUT_MS)) return `${where}.timeoutMs must be 100–${MAX_REQUEST_TIMEOUT_MS}`;
  }
  const weighted = Array.isArray(p.weights) && p.weights.length > 0;
  const flow = Array.isArray(p.flow) && p.flow.length > 0;
  if (weighted === flow) return "set exactly one of weights (a weighted mix) or flow (an ordered flow)";
  if (weighted && (p.weights!.length !== p.requests.length || !p.weights!.every((w) => Number.isInteger(w) && w >= 1 && w <= 1000))) return "weights needs one whole number 1–1000 per request";
  if (flow && (p.flow!.length > 20 || !p.flow!.every((i) => Number.isInteger(i) && i >= 0 && i < p.requests!.length))) return "flow must be up to 20 request indexes";
  if (p.maxInFlight !== undefined && (!Number.isInteger(p.maxInFlight) || p.maxInFlight < 1 || p.maxInFlight > 2000)) return "maxInFlight must be 1–2000";
  if (p.thinkTimeMs) {
    const t = p.thinkTimeMs;
    if (typeof t.min !== "number" || typeof t.max !== "number" || t.min < 0 || t.max < t.min || t.max > 5000) return "thinkTimeMs must be {min, max} within 0–5000";
  }
  if (p.checks !== undefined) {
    if (!Array.isArray(p.checks) || p.checks.length > 20) return "checks must be up to 20 entries";
    for (const c of p.checks) {
      const ok =
        c &&
        ((c.type === "status" && Array.isArray(c.expect) && c.expect.every((x) => typeof x === "string" && /^[1-5][0-9x]{2}$/i.test(x))) ||
          (c.type === "maxDuration" && typeof c.ms === "number" && c.ms > 0) ||
          (c.type === "bodyContains" && typeof c.text === "string" && c.text.length > 0 && c.text.length <= 1000));
      if (!ok) return `unsupported check ${JSON.stringify(c)}`;
    }
  }
  const perIteration = flow ? p.flow!.length : 1;
  const peak = peakRate(prof.startRate, prof.stages) * perIteration;
  if (limits.loadMaxRps !== null && peak > limits.loadMaxRps + 1e-9) return `Your plan allows up to ${limits.loadMaxRps} requests/s in total (this test peaks at ${Math.round(peak * 100) / 100}).`;
  return null;
}

// ---------- the region worker ----------

interface RegionState {
  plan: RegionPlan;
  schedule: ArrivalSchedule;
  next: number | null;
  rec: Recorder;
  inFlight: Set<Promise<void>>;
  colo: string | null;
  error: string | null;
  totalWeight: number;
  bodies: (Uint8Array | null)[];
}

function sleep(ms: number): Promise<void> {
  return ms > 0 ? new Promise((r) => setTimeout(r, ms)) : Promise.resolve();
}

function statusMatches(status: number, pattern: string): boolean {
  const p = pattern.toLowerCase();
  const s = String(status);
  return p.length === 3 && [...p].every((c, i) => c === "x" || c === s[i]);
}

function classify(e: unknown): { kind: ErrorKind; message: string } {
  if (e instanceof Error) {
    if (e.name === "TimeoutError" || e.name === "AbortError") return { kind: "timeout", message: "timed out" };
    const m = e.message || e.name;
    if (/connect|network|dns|refused|reset|lookup|resolve|unreachable|tls|certificate|ssl/i.test(m)) return { kind: "connect", message: m.slice(0, 200) };
    return { kind: "other", message: m.slice(0, 200) };
  }
  return { kind: "other", message: String(e).slice(0, 200) };
}

function headerBytes(h: Headers): number {
  let n = 0;
  for (const [k, v] of h) n += k.length + v.length + 4;
  return n;
}

/** One region of one run (`newUniqueId()` + `locationHint` per run, memory only). */
export class LoadDO extends DurableObject<Env> {
  private state: RegionState | null = null;

  /** Prepares the region; returns the data center it runs in when it can tell. */
  async start(plan: RegionPlan): Promise<{ colo: string | null }> {
    const bodies = plan.requests.map((r) => {
      if (!r.bodyBase64) return null;
      const bin = atob(r.bodyBase64);
      return Uint8Array.from(bin, (c) => c.charCodeAt(0));
    });
    const colo = plan.coloProbe ? await this.probeColo(plan.coloProbe) : null;
    this.state = {
      plan,
      schedule: new ArrivalSchedule(plan.profile.startRate, plan.profile.stages),
      next: null,
      rec: new Recorder(plan.requests.length, plan.bucketMs),
      inFlight: new Set(),
      colo,
      error: null,
      totalWeight: (plan.weights ?? []).reduce((a, b) => a + b, 0),
      bodies,
    };
    this.state.next = this.state.schedule.next();
    return { colo };
  }

  private async probeColo(url: string): Promise<string | null> {
    try {
      const res = await loadNet.fetch(new Request(url, { signal: AbortSignal.timeout(3000) }));
      const text = await res.text();
      return /^colo=([A-Z]{3})$/m.exec(text)?.[1] ?? null;
    } catch {
      return null;
    }
  }

  /** Runs every arrival scheduled before `untilMs` (ms after t0), then waits for them to finish. */
  async segment(untilMs: number): Promise<{ done: boolean; requests: number; error: string | null }> {
    const st = this.state;
    if (!st) return { done: true, requests: 0, error: "region state lost (the object restarted)" };
    const { plan } = st;
    while (st.next !== null && st.next < untilMs) {
      const due = plan.t0 + st.next;
      const wait = due - Date.now();
      if (wait > 1) await sleep(wait);
      if (st.inFlight.size >= plan.maxInFlight) {
        st.rec.dropped(st.next);
      } else {
        const p = this.iteration(st, due).catch(() => {});
        st.inFlight.add(p);
        void p.then(() => st.inFlight.delete(p));
        st.rec.inFlight(st.inFlight.size, st.next);
      }
      st.next = st.schedule.next();
    }
    await Promise.all([...st.inFlight]);
    return { done: st.next === null, requests: st.rec.totals.requests, error: st.error };
  }

  /** The region's measurements; forgets the run. */
  async finish(error?: string | null): Promise<RegionResult> {
    const st = this.state;
    if (!st) throw new Error("region state lost (the object restarted)");
    await Promise.all([...st.inFlight]);
    this.state = null;
    return st.rec.result(st.plan.region, st.colo, error ?? st.error);
  }

  private pick(st: RegionState): number[] {
    if (st.plan.flow) return st.plan.flow;
    let r = Math.random() * st.totalWeight;
    const w = st.plan.weights ?? [];
    for (let i = 0; i < w.length; i++) {
      r -= w[i]!;
      if (r < 0) return [i];
    }
    return [w.length - 1];
  }

  private async iteration(st: RegionState, intended: number): Promise<void> {
    const steps = this.pick(st);
    const think = st.plan.thinkTimeMs;
    for (const [n, i] of steps.entries()) {
      // The first request is measured from its intended start (coordinated-omission free); later
      // flow steps from their own send, like devall-load.
      await this.exec(st, i, n === 0 ? intended : Date.now());
      if (think && n < steps.length - 1) await sleep(think.min + Math.random() * (think.max - think.min));
    }
    st.rec.iterationDone(Date.now() - st.plan.t0);
  }

  private async exec(st: RegionState, i: number, from: number): Promise<void> {
    const r = st.plan.requests[i]!;
    const headers = new Headers();
    for (const [k, v] of r.headers) {
      try {
        headers.append(k, v);
      } catch {
        // invalid header value: skip
      }
    }
    if (!headers.has("user-agent")) headers.set("user-agent", UA);
    headers.set("x-devall-load", st.plan.runId);
    const body = r.method === "GET" || r.method === "HEAD" ? null : st.bodies[i];
    const timeout = Math.min(r.timeoutMs ?? 10_000, MAX_REQUEST_TIMEOUT_MS);
    const wantBody = st.plan.checks.some((c) => c.type === "bodyContains");
    const sent = Date.now();
    const bytesOut = (body?.byteLength ?? 0) + headerBytes(headers) + r.method.length + r.url.length + 12;
    let status: number | null = null;
    let error: { kind: ErrorKind; message: string } | null = null;
    let bytesIn = 0;
    let text = "";
    try {
      const res = await loadNet.fetch(new Request(r.url, { method: r.method, headers, body, redirect: r.followRedirects === false ? "manual" : "follow", signal: AbortSignal.timeout(timeout) }));
      status = res.status;
      bytesIn += headerBytes(res.headers) + 17;
      if (res.body) {
        const reader = res.body.getReader();
        const dec = new TextDecoder();
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          bytesIn += value.byteLength;
          if (wantBody && text.length < MAX_BODY) text += dec.decode(value, { stream: true });
        }
      }
    } catch (e) {
      error = classify(e);
    }
    const end = Date.now();
    let passed = 0;
    let failedChecks = 0;
    const latencyMs = end - from;
    if (st.plan.checks.length === 0) {
      if (status !== null && status < 400 && !error) passed += 1;
      else failedChecks += 1;
    } else {
      for (const c of st.plan.checks) {
        const ok =
          !error &&
          status !== null &&
          (c.type === "status" ? c.expect.some((p) => statusMatches(status!, p)) : c.type === "maxDuration" ? latencyMs <= c.ms : text.includes(c.text));
        if (ok) passed += 1;
        else failedChecks += 1;
      }
    }
    const timedOut = error?.kind === "timeout";
    st.rec.record({
      req: i,
      latencyUs: status !== null || timedOut ? latencyMs * 1000 : null,
      serviceUs: status !== null || timedOut ? (end - sent) * 1000 : null,
      status,
      error,
      bytesIn,
      bytesOut,
      checksPassed: passed,
      checksFailed: failedChecks,
      at: end - st.plan.t0,
    });
  }
}

// ---------- coordinator ----------

export async function loadEnabled(env: Env): Promise<boolean> {
  if ((env.CLOUD_LOAD ?? "on").toLowerCase() === "off") return false;
  try {
    return !(await env.RELAY_KV.get("load:disabled"));
  } catch {
    return true;
  }
}

function coloProbe(env: Env): string | null {
  const v = (env.LOAD_COLO_PROBE ?? "").trim();
  if (v.toLowerCase() === "off") return null;
  return v || "https://www.cloudflare.com/cdn-cgi/trace";
}

/** Segment boundaries (ms after t0): ≤ 10 s and ≤ SEGMENT_MAX_REQUESTS requests per region. */
export function segments(totalMs: number, regionPeakRps: number): number[] {
  const byCount = regionPeakRps > 0 ? Math.floor((SEGMENT_MAX_REQUESTS / regionPeakRps) * 1000) : SEGMENT_MAX_MS;
  const len = Math.max(1000, Math.min(SEGMENT_MAX_MS, byCount));
  const out: number[] = [];
  for (let t = len; t < totalMs; t += len) out.push(t);
  out.push(totalMs + 1);
  return out;
}

function account(env: Env, uid: string) {
  return env.ACCOUNT.get(env.ACCOUNT.idFromName(uid));
}

export async function runCloudLoad(env: Env, p: Principal, plan: CloudLoadPlan, limits: Limits): Promise<Response> {
  const acct = account(env, p.uid);
  const targets = await acct.loadTargets();
  for (const r of plan.requests) {
    const u = new URL(r.url);
    const why = targetPolicy(env, limits, u.hostname);
    if (why) return jsonError(403, "target_not_allowed", why);
    if (!coveredBy(targets, u)) {
      const t = parseTarget(u.origin);
      const origin = typeof t === "string" ? u.origin : t.origin;
      return jsonError(403, "target_not_verified", `${origin} isn't verified for cloud load. Run \`devall load cloud verify ${origin}\` (or "Run from cloud regions…" in the app) and serve the token first.`, { target: origin });
    }
  }
  const runId = plan.runId ?? ulid();
  const totalMs = plan.profile.stages.reduce((n, s) => n + s.durationMs, 0);
  const begin = await acct.loadRunBegin(runId, limits.loadRunsPerDay, totalMs + 120_000);
  if (!begin.ok) return jsonError(begin.code === "load_busy" ? 409 : 429, begin.code, begin.message);

  const regions = plan.regions as LoadRegion[];
  const n = regions.length;
  const share = 1 / n;
  const perIteration = plan.flow?.length ? plan.flow.length : 1;
  const peak = peakRate(plan.profile.startRate, plan.profile.stages);
  const maxInFlight = plan.maxInFlight ?? 200;
  const bucketSeconds = bucketSecondsFor(totalMs);
  const meta: RequestMeta[] = plan.requests.map((r) => ({ path: r.path, name: r.name, method: r.method }));
  const warnings: string[] = [];
  const startedAt = Date.now();
  const t0 = startedAt + START_DELAY_MS;
  const probe = coloProbe(env);
  const parts = regions.map((region) => ({
    region,
    name: REGION_NAMES[region],
    share,
    stub: env.LOAD.get(env.LOAD.newUniqueId(), { locationHint: region }),
    started: false,
    result: null as RegionResult | null,
    error: null as string | null,
  }));
  let aborted = false;
  let abortReason: string | null = null;
  try {
    const regionPlan = (region: string): RegionPlan => ({
      runId,
      region,
      t0,
      share,
      profile: { startRate: plan.profile.startRate * share, stages: plan.profile.stages.map((s) => ({ durationMs: s.durationMs, target: s.target * share })) },
      maxInFlight: Math.max(1, Math.ceil(maxInFlight * share)),
      requests: plan.requests,
      weights: plan.weights?.length ? plan.weights : null,
      flow: plan.flow?.length ? plan.flow : null,
      thinkTimeMs: plan.thinkTimeMs ?? null,
      checks: plan.checks ?? [],
      bucketMs: bucketSeconds * 1000,
      coloProbe: probe,
    });
    const started = await Promise.allSettled(parts.map((x) => x.stub.start(regionPlan(x.region))));
    started.forEach((s, i) => {
      if (s.status === "fulfilled") parts[i]!.started = true;
      else parts[i]!.error = `couldn't start: ${s.reason instanceof Error ? s.reason.message : String(s.reason)}`;
    });
    for (const until of segments(totalMs, peak * share * perIteration)) {
      if (!(await loadEnabled(env))) {
        aborted = true;
        abortReason = "cloud load was switched off by the relay operator";
        break;
      }
      if (await acct.loadRunStopRequested(runId)) {
        aborted = true;
        abortReason = "stopped";
        break;
      }
      const live = parts.filter((x) => !x.error);
      if (!live.length) break;
      const res = await Promise.allSettled(live.map((x) => x.stub.segment(until)));
      res.forEach((r, i) => {
        if (r.status === "rejected") live[i]!.error = `region failed: ${r.reason instanceof Error ? r.reason.message : String(r.reason)}`;
        else if (r.value.error) live[i]!.error = r.value.error;
      });
    }
    // Partial results of a region that failed mid-run still count (marked failed).
    const started_ = parts.filter((x) => x.started);
    const finished = await Promise.allSettled(started_.map((x) => x.stub.finish(x.error)));
    finished.forEach((f, i) => {
      if (f.status === "fulfilled") started_[i]!.result = f.value as unknown as RegionResult; // RPC typing widens tuples
      else started_[i]!.error ??= f.reason instanceof Error ? f.reason.message : String(f.reason);
    });
  } finally {
    await acct.loadRunEnd(runId).catch(() => {});
  }
  const failed = parts.filter((x) => x.error);
  if (failed.length && !aborted) {
    aborted = true;
    abortReason = `region failure — ${failed.map((x) => `${x.region}: ${x.error}`).join("; ")}`;
  }
  if (parts.every((x) => !x.result)) return jsonError(502, "load_failed", abortReason ?? "no region returned results");
  const result = mergeRegions({
    runId,
    test: plan.name,
    startedAt,
    plannedMs: totalMs,
    bucketSeconds,
    meta,
    regions: parts.map((x) => ({ region: x.region, name: x.name, share: x.share, result: x.result, error: x.error })),
    aborted,
    abortReason,
    warnings,
  });
  const expected = new ArrivalSchedule(plan.profile.startRate, plan.profile.stages).expected();
  if (result.totals.droppedIterations > 0) warnings.push(`${result.totals.droppedIterations} iterations were dropped because ${maxInFlight} were already in flight (raise maxVus).`);
  if (!aborted && expected >= 10 && result.totals.iterations < expected * 0.9) {
    warnings.push(`The regions completed ${result.totals.iterations} of ${Math.round(expected)} planned iterations; the edge couldn't keep up (Workers allow 6 concurrent outgoing connections per invocation).`);
  }
  if (probe && parts.some((x) => x.result && !x.result.colo)) warnings.push("Some regions' data center couldn't be detected; placement is best-effort (locationHint).");
  return json(result satisfies CloudLoadResult);
}

// ---------- API: /v1/load… ----------

async function readJson(request: Request, max: number): Promise<unknown | Response> {
  const body = await readBodyCapped(request.body, max);
  if (body === null) return jsonError(413, "too_large", `Request body is larger than ${max} bytes.`);
  try {
    return JSON.parse(new TextDecoder().decode(body)) as unknown;
  } catch {
    return jsonError(400, "bad_request", "Body must be JSON.");
  }
}

function limitsView(l: Limits) {
  return { loadRegions: l.loadRegions, loadMaxDurationSeconds: l.loadMaxDurationSeconds, loadMaxRps: l.loadMaxRps, loadRunsPerDay: l.loadRunsPerDay, loadPrivateTargets: l.loadPrivateTargets };
}

export async function loadRoutes(request: Request, url: URL, rest: string[], p: Principal, env: Env): Promise<Response> {
  const limits = limitsFor(p.plan, env);
  const acct = account(env, p.uid);
  const enabled = await loadEnabled(env);
  if (rest.length === 0 && request.method === "GET") {
    const now = Date.now();
    return json({
      enabled,
      regions: LOAD_REGIONS.map((id) => ({ id, name: REGION_NAMES[id] })),
      limits: limitsView(limits),
      runsToday: await acct.loadRunsToday(),
      targets: (await acct.loadTargets()).map((t) => targetView(t, now)),
      verifyPath: VERIFY_PATH,
    });
  }
  if (limits.loadRegions === 0) return jsonError(403, "plan_required", "Cloud load regions are part of the Supporter plan (or self-host the relay for free).");
  if (rest[0] === "targets") {
    if (rest.length === 1 && request.method === "DELETE") {
      const t = parseTarget(url.searchParams.get("target"));
      if (typeof t === "string") return jsonError(400, "invalid_target", t);
      return (await acct.removeLoadTarget(t.key)) ? json({ ok: true, removed: t.key }) : jsonError(404, "not_found", "No such target.");
    }
    if (request.method !== "POST" || rest.length > 2 || (rest.length === 2 && rest[1] !== "verify")) return jsonError(404, "not_found", "Not found");
    const body = await readJson(request, 16 * 1024);
    if (body instanceof Response) return body;
    const t = parseTarget((body as { target?: unknown })?.target);
    if (typeof t === "string") return jsonError(400, "invalid_target", t);
    const why = targetPolicy(env, limits, t.hostname);
    if (why) return jsonError(403, "target_not_allowed", why);
    const existing = (await acct.loadTargets()).find((x) => x.key === t.key);
    if (rest.length === 1) {
      if (existing) return json(targetView(existing));
      const created: LoadTarget = { ...t, token: newToken(), createdAt: Date.now(), verifiedAt: null, expiresAt: null, method: null, lastCheckAt: null, lastError: null };
      const r = await acct.putLoadTarget(created, MAX_TARGETS);
      if (!r.ok) return jsonError(429, "too_many_targets", r.message);
      return json(targetView(created), 201);
    }
    // verify
    if (!existing) return jsonError(404, "not_found", `Add ${t.origin} first (POST /v1/load/targets) to get its token.`);
    if (existing.lastCheckAt && Date.now() - existing.lastCheckAt < 2000) return jsonError(429, "rate_limited", "Checked a moment ago; try again in a few seconds.", {}, { "retry-after": "2" });
    // Verify against the origin as typed this time (scheme/port), keeping the stored token.
    const target: LoadTarget = { ...existing, origin: t.origin, scheme: t.scheme };
    const v = await verifyOwnership(env, target);
    const now = Date.now();
    const next: LoadTarget = v.ok
      ? { ...target, verifiedAt: now, expiresAt: now + VERIFY_TTL_MS, method: v.method, lastCheckAt: now, lastError: null }
      : { ...target, lastCheckAt: now, lastError: v.details.join("; ") };
    await acct.putLoadTarget(next, MAX_TARGETS);
    if (!v.ok) return jsonError(422, "verification_failed", `Couldn't verify ${t.origin}: ${v.details.join("; ")}`, { details: v.details, target: targetView(next, now) });
    return json(targetView(next, now));
  }
  if (rest[0] === "runs") {
    if (rest.length === 3 && rest[2] === "stop" && request.method === "POST") {
      return (await acct.loadRunStop(rest[1] ?? "")) ? json({ ok: true, stopping: rest[1] }) : jsonError(404, "not_found", "No such run in progress.");
    }
    if (rest.length !== 1 || request.method !== "POST") return jsonError(404, "not_found", "Not found");
    if (!enabled) return jsonError(503, "load_disabled", "Cloud load is switched off on this relay right now.", {}, { "retry-after": "3600" });
    const body = await readJson(request, 2 * 1024 * 1024);
    if (body instanceof Response) return body;
    const err = validatePlan(body as Partial<CloudLoadPlan>, limits);
    if (err) return jsonError(400, "invalid_plan", err);
    return runCloudLoad(env, p, body as CloudLoadPlan, limits);
  }
  return jsonError(404, "not_found", "Not found");
}

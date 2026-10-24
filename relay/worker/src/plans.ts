import type { Env } from "./env";
import type { LimitsPayload, PlanName } from "./protocol";

export type Plan = PlanName;
export type Limits = LimitsPayload;

const KiB = 1024;
const MiB = 1024 * KiB;
const GiB = 1024 * MiB;
const HOUR = 3600;
const DAY = 24 * HOUR;

/**
 * Tier limits. Free must stay genuinely useful; Supporter is priced near cost and mostly lifts
 * abuse guards; self-hosters get everything (null = unlimited).
 */
export const DEFAULT_LIMITS: Record<Plan, Limits> = {
  free: {
    sessionSeconds: 8 * HOUR,
    bandwidthBytesPerMonth: 1 * GiB,
    maxRequestBodyBytes: 10 * MiB,
    requestTimeoutMs: 60_000,
    maxTunnels: 1,
    interstitial: true,
    inboxRetentionSeconds: 1 * DAY,
    inboxEventsPerDay: 500,
    inboxMaxBuffered: 500,
    inboxMaxBodyBytes: 1 * MiB,
    maxInboxes: 1,
    reservedSubdomains: 0,
    ipRequestsPerMinute: 300,
    customDomains: 0,
    tcpTunnels: false,
    tcpStreamsPerTunnel: 0,
    // One small always-on mock: a few DO rows and requests, effectively free to run.
    hostedMocks: 1,
    mockMaxStubs: 50,
    mockMaxBytes: 1 * MiB,
    mockRequestsPerDay: 5_000,
    mockJournalSize: 50,
    shares: 20,
    shareMaxBytes: 256 * KiB,
    shareTtlSeconds: 7 * DAY,
    // Monitors are a Supporter feature (docs/FEATURES.md); LIMITS_JSON can grant some to free.
    monitors: 0,
    monitorMinIntervalMinutes: 60,
    monitorMaxRequests: 5,
    monitorHistory: 168,
    // Cloud load-generator regions: Supporter / self-host only.
    loadRegions: 0,
    loadMaxDurationSeconds: 0,
    loadMaxRps: 0,
    loadRunsPerDay: 0,
    loadPrivateTargets: false,
  },
  supporter: {
    // Sessions still end daily so long-lived sockets pick up fresh tokens and plan changes;
    // the client reconnects to the same reserved subdomain transparently.
    sessionSeconds: 24 * HOUR,
    bandwidthBytesPerMonth: 50 * GiB,
    maxRequestBodyBytes: 100 * MiB,
    requestTimeoutMs: 100_000,
    maxTunnels: 5,
    interstitial: false,
    inboxRetentionSeconds: 30 * DAY,
    inboxEventsPerDay: 10_000,
    inboxMaxBuffered: 10_000,
    inboxMaxBodyBytes: 1 * MiB,
    maxInboxes: 20,
    reservedSubdomains: 5,
    ipRequestsPerMinute: 1200,
    customDomains: 2,
    tcpTunnels: true,
    tcpStreamsPerTunnel: 20,
    hostedMocks: 10,
    mockMaxStubs: 1000,
    mockMaxBytes: 20 * MiB,
    mockRequestsPerDay: 100_000,
    mockJournalSize: 500,
    shares: 500,
    shareMaxBytes: 5 * MiB,
    shareTtlSeconds: 90 * DAY,
    monitors: 20,
    monitorMinIntervalMinutes: 5,
    monitorMaxRequests: 20,
    monitorHistory: 2016,
    // Bounded bursts against verified hosts only: ≤ 3 regions, 60 s, 100 requests/s in total.
    loadRegions: 3,
    loadMaxDurationSeconds: 60,
    loadMaxRps: 100,
    loadRunsPerDay: 20,
    loadPrivateTargets: false,
  },
  selfhost: {
    sessionSeconds: null,
    bandwidthBytesPerMonth: null,
    maxRequestBodyBytes: null,
    requestTimeoutMs: 300_000,
    maxTunnels: null,
    interstitial: false,
    inboxRetentionSeconds: 90 * DAY,
    inboxEventsPerDay: null,
    inboxMaxBuffered: null,
    inboxMaxBodyBytes: 1.5 * MiB,
    maxInboxes: null,
    reservedSubdomains: null,
    ipRequestsPerMinute: null,
    customDomains: null,
    tcpTunnels: true,
    tcpStreamsPerTunnel: null,
    hostedMocks: null,
    mockMaxStubs: null,
    mockMaxBytes: 100 * MiB,
    mockRequestsPerDay: null,
    mockJournalSize: 1000,
    shares: null,
    shareMaxBytes: 20 * MiB,
    shareTtlSeconds: 365 * DAY,
    monitors: null,
    // The cron trigger fires every 5 minutes; self-hosters can change both (wrangler.toml + LIMITS_JSON).
    monitorMinIntervalMinutes: 5,
    monitorMaxRequests: 50,
    monitorHistory: 2016,
    // Self-hosters own the egress: every region, private/loopback targets allowed (still verified).
    // A Durable Object call can't run forever, so runs stay bounded; raise it with LIMITS_JSON.
    loadRegions: 9,
    loadMaxDurationSeconds: 600,
    loadMaxRps: null,
    loadRunsPerDay: null,
    loadPrivateTargets: true,
  },
};

export function isPlan(v: unknown): v is Plan {
  return v === "free" || v === "supporter" || v === "selfhost";
}

/** Plan from a Firebase custom claim; anything unknown is `free`. */
export function planFromClaim(v: unknown): Plan {
  return v === "supporter" ? "supporter" : "free";
}

let cachedOverrides: { raw: string; parsed: Partial<Record<Plan, Partial<Limits>>> } | null = null;

function overrides(env: Env | undefined): Partial<Record<Plan, Partial<Limits>>> {
  const raw = env?.LIMITS_JSON;
  if (!raw) return {};
  if (cachedOverrides?.raw === raw) return cachedOverrides.parsed;
  let parsed: Partial<Record<Plan, Partial<Limits>>> = {};
  try {
    const v = JSON.parse(raw) as unknown;
    if (v && typeof v === "object") parsed = v as typeof parsed;
  } catch {
    parsed = {};
  }
  cachedOverrides = { raw, parsed };
  return parsed;
}

export function limitsFor(plan: Plan, env?: Env): Limits {
  const base = DEFAULT_LIMITS[plan] ?? DEFAULT_LIMITS.free;
  const o = overrides(env)[plan];
  const merged: Limits = { ...base, ...(o ?? {}) };
  // The interstitial can be switched off globally (self-hosters), never on for paid plans.
  if ((env?.INTERSTITIAL ?? "auto").toLowerCase() === "off") merged.interstitial = false;
  return merged;
}

/** `true` when `used` is at or past a (nullable) limit. */
export function overLimit(used: number, limit: number | null): boolean {
  return limit !== null && used >= limit;
}

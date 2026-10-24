// Custom domains for tunnels: a customer's hostname (CNAME → the relay) served by a tunnel.
//
// Hosted: Cloudflare for SaaS custom hostnames, managed by the Firebase functions
// (addCustomDomain / removeCustomDomain), which write `host:<hostname>` → {sub, uid} to KV once the
// customer owns a reserved subdomain. Self-hosted: the static CUSTOM_HOSTS map (plus a Worker route
// for each hostname in the operator's Cloudflare account).

import type { Env } from "./env";
import { checkSubdomainSyntax, isValidHostname, normalizeHost } from "./names";

export interface CustomHostTarget {
  sub: string;
  /** Account that mapped the hostname (hosted); null for the operator's static map. */
  uid: string | null;
  source: "static" | "hosted";
}

/** What `/v1/account` shows for a custom domain (hosted entries come from KV `domains:<uid>`). */
export interface CustomDomainView {
  hostname: string;
  subdomain: string;
  /** `pending` (waiting for DNS / certificate), `active`, `error`, or `static` (self-host map). */
  status: "pending" | "active" | "error" | "static" | string;
  sslStatus: string | null;
  /** DNS records the customer adds at their DNS provider. */
  records: { type: string; name: string; value: string; purpose: string }[];
  errors: string[];
  source: "static" | "hosted";
  updatedAt: string | null;
}

let staticCache: { raw: string; map: Map<string, string> } | null = null;

/** Parses CUSTOM_HOSTS (`{"api.example.com":"box"}` or `api.example.com=box, …`). Invalid entries are dropped. */
export function parseCustomHosts(raw: string | undefined): Map<string, string> {
  const out = new Map<string, string>();
  if (!raw || !raw.trim()) return out;
  let pairs: [string, unknown][] = [];
  const trimmed = raw.trim();
  if (trimmed.startsWith("{")) {
    try {
      const v = JSON.parse(trimmed) as unknown;
      if (v && typeof v === "object" && !Array.isArray(v)) pairs = Object.entries(v as Record<string, unknown>);
    } catch {
      pairs = [];
    }
  } else {
    pairs = trimmed
      .split(/[,\n]/)
      .map((e) => e.trim())
      .filter(Boolean)
      .map((e) => {
        const i = e.indexOf("=");
        return i > 0 ? ([e.slice(0, i).trim(), e.slice(i + 1).trim()] as [string, unknown]) : (["", ""] as [string, unknown]);
      });
  }
  for (const [host, sub] of pairs) {
    if (typeof sub !== "string") continue;
    const h = normalizeHost(host);
    const s = sub.trim().toLowerCase();
    if (isValidHostname(h) && checkSubdomainSyntax(s).ok) out.set(h, s);
  }
  return out;
}

export function staticCustomHosts(env: Env): Map<string, string> {
  const raw = env.CUSTOM_HOSTS ?? "";
  if (staticCache?.raw === raw) return staticCache.map;
  const map = parseCustomHosts(raw);
  staticCache = { raw, map };
  return map;
}

// Small per-isolate cache so every request to a custom domain doesn't read KV (positive and
// negative answers, 60 s; KV itself is eventually consistent for about as long).
const CACHE_TTL_MS = 60_000;
const CACHE_MAX = 2000;
const kvCache = new Map<string, { at: number; value: CustomHostTarget | null }>();

export function clearCustomHostCache(): void {
  kvCache.clear();
}

/** Resolves a custom hostname to the tunnel subdomain it serves (null = not mapped). */
export async function resolveCustomHost(env: Env, host: string, now = Date.now()): Promise<CustomHostTarget | null> {
  const h = normalizeHost(host);
  const sub = staticCustomHosts(env).get(h);
  if (sub) return { sub, uid: null, source: "static" };
  const hit = kvCache.get(h);
  if (hit && now - hit.at < CACHE_TTL_MS) return hit.value;
  let value: CustomHostTarget | null = null;
  try {
    const v = await env.RELAY_KV.get<{ sub?: unknown; uid?: unknown }>(`host:${h}`, "json");
    if (v && typeof v.sub === "string" && checkSubdomainSyntax(v.sub).ok && typeof v.uid === "string" && v.uid) {
      value = { sub: v.sub, uid: v.uid, source: "hosted" };
    }
  } catch {
    value = null;
  }
  if (kvCache.size >= CACHE_MAX) {
    const oldest = kvCache.keys().next().value;
    if (oldest !== undefined) kvCache.delete(oldest);
  }
  kvCache.set(h, { at: now, value });
  return value;
}

/** Hostnames that serve `sub` (shown in HELLO's publicUrls): the static map plus KV `subhosts:<sub>`. */
export async function hostsForSubdomain(env: Env, sub: string): Promise<string[]> {
  const out: string[] = [];
  for (const [h, s] of staticCustomHosts(env)) if (s === sub) out.push(h);
  try {
    const list = await env.RELAY_KV.get<unknown>(`subhosts:${sub}`, "json");
    if (Array.isArray(list)) for (const h of list) if (typeof h === "string" && isValidHostname(h) && !out.includes(h)) out.push(h);
  } catch {
    // KV hiccup: the primary URL still works
  }
  return out;
}

/** Custom domains shown on `/v1/account`. */
export async function customDomainsFor(env: Env, uid: string, selfHosted: boolean): Promise<CustomDomainView[]> {
  const out: CustomDomainView[] = [];
  if (selfHosted) {
    for (const [hostname, subdomain] of staticCustomHosts(env)) {
      out.push({ hostname, subdomain, status: "static", sslStatus: null, records: [], errors: [], source: "static", updatedAt: null });
    }
  }
  try {
    const list = await env.RELAY_KV.get<unknown>(`domains:${uid}`, "json");
    if (Array.isArray(list)) {
      for (const d of list) {
        if (!d || typeof d !== "object") continue;
        const v = d as Partial<CustomDomainView>;
        if (typeof v.hostname !== "string" || typeof v.subdomain !== "string") continue;
        out.push({
          hostname: v.hostname,
          subdomain: v.subdomain,
          status: typeof v.status === "string" ? v.status : "pending",
          sslStatus: typeof v.sslStatus === "string" ? v.sslStatus : null,
          records: Array.isArray(v.records) ? v.records.filter((r) => r && typeof r.name === "string" && typeof r.value === "string") : [],
          errors: Array.isArray(v.errors) ? v.errors.filter((e): e is string => typeof e === "string") : [],
          source: "hosted",
          updatedAt: typeof v.updatedAt === "string" ? v.updatedAt : null,
        });
      }
    }
  } catch {
    // ignore
  }
  return out;
}

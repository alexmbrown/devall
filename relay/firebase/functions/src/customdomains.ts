// Custom domains for tunnels (Supporter): a customer's hostname, CNAMEd to the relay, serves one of
// their reserved subdomains. Cloudflare for SaaS issues and renews the certificate; this module
// decides what's allowed, talks to the Custom Hostnames API through `CustomDomainDeps`, and pushes
// what the edge needs to Workers KV:
//
//   host:<hostname>   → {sub, uid}      routing (read by the Worker for unknown hosts)
//   subhosts:<sub>    → [hostname, …]   extra public URLs in the tunnel's HELLO
//   domains:<uid>     → [view, …]       what the desktop shows (status + DNS records)
//
// Firestore `customDomains/{hostname}` is the source of truth; the user's entitlement document
// lists their hostnames.

import type { CfCustomHostname } from "./cloudflare.js";

/** Supporter plan: custom domains per account (the Worker's plans.ts says the same). */
export const MAX_CUSTOM_DOMAINS = 2;

export interface DnsRecord {
  type: "CNAME" | "TXT";
  name: string;
  value: string;
  /** `routing` (required), `ownership` / `certificate` (only needed when the CNAME can't be added first). */
  purpose: "routing" | "ownership" | "certificate";
}

export interface CustomDomain {
  hostname: string;
  uid: string;
  subdomain: string;
  cfId: string | null;
  /** `pending` (waiting for DNS / certificate), `active`, `error`. */
  status: "pending" | "active" | "error";
  sslStatus: string | null;
  records: DnsRecord[];
  errors: string[];
  createdAt: number;
  updatedAt: number;
}

/** The shape pushed to KV `domains:<uid>` (read by the Worker's `/v1/account`). */
export interface CustomDomainView {
  hostname: string;
  subdomain: string;
  status: string;
  sslStatus: string | null;
  records: DnsRecord[];
  errors: string[];
  updatedAt: string;
}

export interface CustomDomainDeps {
  plan(uid: string): Promise<string>;
  reservedSubdomains(uid: string): Promise<string[]>;
  getDomain(hostname: string): Promise<CustomDomain | null>;
  listDomains(uid: string): Promise<CustomDomain[]>;
  /** Creates `customDomains/{hostname}` only if absent (transaction); false when someone else holds it. */
  claim(d: CustomDomain): Promise<boolean>;
  saveDomain(d: CustomDomain): Promise<void>;
  deleteDomain(hostname: string): Promise<void>;
  cfCreate(hostname: string): Promise<CfCustomHostname>;
  cfFind(hostname: string): Promise<CfCustomHostname | null>;
  cfGet(id: string): Promise<CfCustomHostname>;
  cfDelete(id: string): Promise<void>;
  kvPut(key: string, value: string): Promise<void>;
  kvDelete(key: string): Promise<void>;
  /** The relay's own domain (tunnels live under it; customers can't add it). */
  publicDomain: string;
  /** What customers CNAME to (the Cloudflare for SaaS fallback origin). */
  cnameTarget: string;
  now?(): number;
}

export class DomainError extends Error {
  override name = "DomainError";
  constructor(
    readonly code: "permission-denied" | "invalid-argument" | "already-exists" | "resource-exhausted" | "not-found" | "failed-precondition" | "unavailable",
    message: string,
  ) {
    super(message);
  }
}

const LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

/** A fully qualified host name the customer owns: ≥ 2 labels, LDH (punycode OK), not an IP, not ours. */
export function validateHostname(input: unknown, publicDomain: string): { ok: true; hostname: string } | { ok: false; reason: string } {
  if (typeof input !== "string") return { ok: false, reason: "hostname must be a string" };
  const h = input.trim().toLowerCase().replace(/\.$/, "");
  if (h.length < 4 || h.length > 253) return { ok: false, reason: "hostname must be 4–253 characters" };
  if (h.includes("://") || h.includes("/") || h.includes(":") || h.includes("*")) return { ok: false, reason: "enter a bare hostname like api.example.com (no scheme, path, port or wildcard)" };
  const labels = h.split(".");
  if (labels.length < 2 || !labels.every((l) => LABEL.test(l))) return { ok: false, reason: "not a valid hostname" };
  if (/^\d+$/.test(labels[labels.length - 1]!)) return { ok: false, reason: "IP addresses can't be custom domains" };
  const d = publicDomain.toLowerCase();
  if (h === d || h.endsWith(`.${d}`)) return { ok: false, reason: `${d} addresses are already yours through reserved subdomains` };
  return { ok: true, hostname: h };
}

/** Maps Cloudflare's custom hostname to our status + the DNS records the customer adds. */
export function describe(cf: CfCustomHostname, cnameTarget: string): Pick<CustomDomain, "status" | "sslStatus" | "records" | "errors"> {
  const records: DnsRecord[] = [{ type: "CNAME", name: cf.hostname, value: cnameTarget, purpose: "routing" }];
  const ov = cf.ownership_verification;
  if (ov?.name && ov.value && (ov.type ?? "txt") === "txt") records.push({ type: "TXT", name: ov.name, value: ov.value, purpose: "ownership" });
  for (const r of cf.ssl?.validation_records ?? []) {
    if (r.txt_name && r.txt_value) records.push({ type: "TXT", name: r.txt_name, value: r.txt_value, purpose: "certificate" });
  }
  const errors = [...(cf.verification_errors ?? []), ...(cf.ssl?.validation_errors ?? []).map((e) => e.message ?? "").filter(Boolean)];
  const ssl = cf.ssl?.status ?? null;
  const failed = ["blocked", "moved", "deleted"].includes(cf.status ?? "") || ["validation_timed_out", "issuance_timed_out", "deleted", "expired"].includes(ssl ?? "");
  const status: CustomDomain["status"] = failed ? "error" : cf.status === "active" && ssl === "active" ? "active" : "pending";
  return { status, sslStatus: ssl, records, errors };
}

export function view(d: CustomDomain): CustomDomainView {
  return { hostname: d.hostname, subdomain: d.subdomain, status: d.status, sslStatus: d.sslStatus, records: d.records, errors: d.errors, updatedAt: new Date(d.updatedAt).toISOString() };
}

/** Rewrites `domains:<uid>` and `subhosts:<sub>` for every subdomain the user's domains touch. */
async function pushIndex(deps: CustomDomainDeps, uid: string, touchedSubs: string[]): Promise<CustomDomainView[]> {
  const all = await deps.listDomains(uid);
  const views = all.map(view).sort((a, b) => a.hostname.localeCompare(b.hostname));
  await deps.kvPut(`domains:${uid}`, JSON.stringify(views));
  for (const sub of new Set([...touchedSubs, ...all.map((d) => d.subdomain)])) {
    const hosts = all.filter((d) => d.subdomain === sub).map((d) => d.hostname).sort();
    if (hosts.length) await deps.kvPut(`subhosts:${sub}`, JSON.stringify(hosts));
    else await deps.kvDelete(`subhosts:${sub}`);
  }
  return views;
}

export async function addCustomDomain(deps: CustomDomainDeps, uid: string, input: { hostname?: unknown; subdomain?: unknown }): Promise<CustomDomainView> {
  const plan = await deps.plan(uid);
  if (plan !== "supporter") throw new DomainError("permission-denied", "Custom domains are part of the Supporter plan (or self-host the relay for free with CUSTOM_HOSTS).");
  const v = validateHostname(input.hostname, deps.publicDomain);
  if (!v.ok) throw new DomainError("invalid-argument", v.reason);
  const sub = typeof input.subdomain === "string" ? input.subdomain.trim().toLowerCase() : "";
  const reserved = await deps.reservedSubdomains(uid);
  if (!reserved.includes(sub)) throw new DomainError("failed-precondition", `Point the domain at one of your reserved subdomains (${reserved.join(", ") || "reserve one first"}).`);
  const existing = await deps.getDomain(v.hostname);
  if (existing && existing.uid !== uid) throw new DomainError("already-exists", `${v.hostname} is connected to another account.`);
  if (existing) {
    // Re-adding the same hostname (e.g. pointing it at another subdomain) is an update.
    const prevSub = existing.subdomain;
    const next = { ...existing, subdomain: sub, updatedAt: deps.now?.() ?? Date.now() };
    await deps.saveDomain(next);
    await deps.kvPut(`host:${v.hostname}`, JSON.stringify({ sub, uid }));
    await pushIndex(deps, uid, [prevSub, sub]);
    return view(next);
  }
  const mine = await deps.listDomains(uid);
  if (mine.length >= MAX_CUSTOM_DOMAINS) throw new DomainError("resource-exhausted", `You can connect up to ${MAX_CUSTOM_DOMAINS} custom domains; remove one first.`);

  const now = deps.now?.() ?? Date.now();
  const draft: CustomDomain = { hostname: v.hostname, uid, subdomain: sub, cfId: null, status: "pending", sslStatus: null, records: [], errors: [], createdAt: now, updatedAt: now };
  if (!(await deps.claim(draft))) throw new DomainError("already-exists", `${v.hostname} is connected to another account.`);
  let cf: CfCustomHostname;
  try {
    try {
      cf = await deps.cfCreate(v.hostname);
    } catch (e) {
      // Already registered on the zone (a previous attempt that failed half-way): adopt it.
      const found = await deps.cfFind(v.hostname).catch(() => null);
      if (!found) throw e;
      cf = found;
    }
  } catch (e) {
    await deps.deleteDomain(v.hostname);
    throw new DomainError("unavailable", `Cloudflare refused ${v.hostname}: ${e instanceof Error ? e.message : String(e)}`);
  }
  const d: CustomDomain = { ...draft, cfId: cf.id, ...describe(cf, deps.cnameTarget), updatedAt: deps.now?.() ?? Date.now() };
  await deps.saveDomain(d);
  await deps.kvPut(`host:${d.hostname}`, JSON.stringify({ sub, uid }));
  await pushIndex(deps, uid, [sub]);
  return view(d);
}

export async function removeCustomDomain(deps: CustomDomainDeps, uid: string, input: { hostname?: unknown }): Promise<{ removed: string }> {
  const h = typeof input.hostname === "string" ? input.hostname.trim().toLowerCase().replace(/\.$/, "") : "";
  const d = h ? await deps.getDomain(h) : null;
  if (!d || d.uid !== uid) throw new DomainError("not-found", `${h || "That hostname"} isn't one of your custom domains.`);
  if (d.cfId) await deps.cfDelete(d.cfId);
  await deps.kvDelete(`host:${d.hostname}`);
  await deps.deleteDomain(d.hostname);
  await pushIndex(deps, uid, [d.subdomain]);
  return { removed: d.hostname };
}

/** Polls Cloudflare for validation / certificate progress. */
export async function refreshCustomDomain(deps: CustomDomainDeps, uid: string | null, input: { hostname?: unknown }): Promise<CustomDomainView> {
  const h = typeof input.hostname === "string" ? input.hostname.trim().toLowerCase().replace(/\.$/, "") : "";
  const d = h ? await deps.getDomain(h) : null;
  if (!d || (uid !== null && d.uid !== uid)) throw new DomainError("not-found", `${h || "That hostname"} isn't one of your custom domains.`);
  if (!d.cfId) return view(d);
  const cf = await deps.cfGet(d.cfId);
  const next: CustomDomain = { ...d, ...describe(cf, deps.cnameTarget), updatedAt: deps.now?.() ?? Date.now() };
  await deps.saveDomain(next);
  await pushIndex(deps, d.uid, [d.subdomain]);
  return view(next);
}

/** Subdomains with custom domains can't be released until the domains are removed. */
export async function subdomainInUse(deps: Pick<CustomDomainDeps, "listDomains">, uid: string, sub: string): Promise<string[]> {
  return (await deps.listDomains(uid)).filter((d) => d.subdomain === sub).map((d) => d.hostname);
}

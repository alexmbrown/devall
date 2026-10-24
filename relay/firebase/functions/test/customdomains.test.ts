import { describe, expect, it } from "vitest";
import { CfApiError, createCustomHostname, deleteCustomHostname, findCustomHostname, getCustomHostname, type CfCustomHostname } from "../src/cloudflare.js";
import {
  addCustomDomain,
  describe as describeCf,
  DomainError,
  MAX_CUSTOM_DOMAINS,
  refreshCustomDomain,
  removeCustomDomain,
  subdomainInUse,
  validateHostname,
  type CustomDomain,
  type CustomDomainDeps,
} from "../src/customdomains.js";

const pendingCf = (hostname: string, id = `cf-${hostname}`): CfCustomHostname => ({
  id,
  hostname,
  status: "pending",
  ssl: { status: "pending_validation", method: "http", validation_records: [{ http_url: `http://${hostname}/.well-known/acme`, http_body: "x" }] },
  verification_errors: ["custom hostname does not CNAME to this zone."],
  ownership_verification: { type: "txt", name: `_cf-custom-hostname.${hostname}`, value: "0d89c70d-ad9f-4843-b99f-6cc0252067e9" },
});

class Fake implements CustomDomainDeps {
  plans = new Map<string, string>();
  subs = new Map<string, string[]>();
  domains = new Map<string, CustomDomain>();
  kv = new Map<string, string>();
  cf = new Map<string, CfCustomHostname>();
  cfFail = false;
  publicDomain = "devall.test";
  cnameTarget = "custom.devall.test";
  async plan(uid: string) {
    return this.plans.get(uid) ?? "free";
  }
  async reservedSubdomains(uid: string) {
    return this.subs.get(uid) ?? [];
  }
  async getDomain(h: string) {
    return this.domains.get(h) ?? null;
  }
  async listDomains(uid: string) {
    return [...this.domains.values()].filter((d) => d.uid === uid);
  }
  async claim(d: CustomDomain) {
    const cur = this.domains.get(d.hostname);
    if (cur && cur.uid !== d.uid) return false;
    this.domains.set(d.hostname, d);
    return true;
  }
  async saveDomain(d: CustomDomain) {
    this.domains.set(d.hostname, d);
  }
  async deleteDomain(h: string) {
    this.domains.delete(h);
  }
  async cfCreate(h: string) {
    if (this.cfFail) throw new Error("Duplicate custom hostname found.");
    const r = pendingCf(h);
    this.cf.set(r.id, r);
    return r;
  }
  async cfFind(h: string) {
    return [...this.cf.values()].find((c) => c.hostname === h) ?? null;
  }
  async cfGet(id: string) {
    const r = this.cf.get(id);
    if (!r) throw new Error("not found");
    return r;
  }
  async cfDelete(id: string) {
    this.cf.delete(id);
  }
  async kvPut(k: string, v: string) {
    this.kv.set(k, v);
  }
  async kvDelete(k: string) {
    this.kv.delete(k);
  }
  now() {
    return 1_800_000_000_000;
  }
}

function supporter(): Fake {
  const f = new Fake();
  f.plans.set("u1", "supporter");
  f.subs.set("u1", ["my-api", "shop"]);
  return f;
}

describe("custom domain validation", () => {
  it("accepts customer hostnames and refuses ours, IPs, wildcards and URLs", () => {
    expect(validateHostname(" API.Example.com. ", "devall.test")).toEqual({ ok: true, hostname: "api.example.com" });
    expect(validateHostname("xn--bcher-kva.example", "devall.test").ok).toBe(true);
    for (const bad of ["devall.test", "x.devall.test", "1.2.3.4", "*.example.com", "https://a.example.com", "a.example.com/x", "localhost", "a_b.example.com", "", 5]) {
      expect(validateHostname(bad, "devall.test").ok).toBe(false);
    }
  });

  it("maps Cloudflare's state to status and DNS records", () => {
    const p = describeCf(pendingCf("api.example.com"), "custom.devall.test");
    expect(p.status).toBe("pending");
    expect(p.records).toEqual([
      { type: "CNAME", name: "api.example.com", value: "custom.devall.test", purpose: "routing" },
      { type: "TXT", name: "_cf-custom-hostname.api.example.com", value: "0d89c70d-ad9f-4843-b99f-6cc0252067e9", purpose: "ownership" },
    ]);
    expect(p.errors).toEqual(["custom hostname does not CNAME to this zone."]);
    expect(describeCf({ id: "1", hostname: "a.example.com", status: "active", ssl: { status: "active" } }, "t").status).toBe("active");
    expect(describeCf({ id: "1", hostname: "a.example.com", status: "pending", ssl: { status: "validation_timed_out" } }, "t").status).toBe("error");
    const txt = describeCf({ id: "1", hostname: "a.example.com", ssl: { status: "pending_validation", validation_records: [{ txt_name: "_acme-challenge.a.example.com", txt_value: "abc" }] } }, "t");
    expect(txt.records).toContainEqual({ type: "TXT", name: "_acme-challenge.a.example.com", value: "abc", purpose: "certificate" });
  });
});

describe("custom domain lifecycle", () => {
  it("adds a domain: Cloudflare hostname, Firestore doc and the edge's KV keys", async () => {
    const f = supporter();
    const v = await addCustomDomain(f, "u1", { hostname: "API.example.com", subdomain: "my-api" });
    expect(v).toMatchObject({ hostname: "api.example.com", subdomain: "my-api", status: "pending" });
    expect(f.domains.get("api.example.com")!.cfId).toBe("cf-api.example.com");
    expect(JSON.parse(f.kv.get("host:api.example.com")!)).toEqual({ sub: "my-api", uid: "u1" });
    expect(JSON.parse(f.kv.get("subhosts:my-api")!)).toEqual(["api.example.com"]);
    expect((JSON.parse(f.kv.get("domains:u1")!) as { hostname: string }[]).map((d) => d.hostname)).toEqual(["api.example.com"]);

    // Re-pointing it at another reserved subdomain is an update.
    await addCustomDomain(f, "u1", { hostname: "api.example.com", subdomain: "shop" });
    expect(JSON.parse(f.kv.get("host:api.example.com")!)).toEqual({ sub: "shop", uid: "u1" });
    expect(f.kv.has("subhosts:my-api")).toBe(false);
    expect(JSON.parse(f.kv.get("subhosts:shop")!)).toEqual(["api.example.com"]);
    expect(await subdomainInUse(f, "u1", "shop")).toEqual(["api.example.com"]);
  });

  it("refreshes to active and removes everything again", async () => {
    const f = supporter();
    await addCustomDomain(f, "u1", { hostname: "api.example.com", subdomain: "my-api" });
    f.cf.set("cf-api.example.com", { id: "cf-api.example.com", hostname: "api.example.com", status: "active", ssl: { status: "active" } });
    const v = await refreshCustomDomain(f, "u1", { hostname: "api.example.com" });
    expect(v.status).toBe("active");
    expect((JSON.parse(f.kv.get("domains:u1")!) as { status: string }[])[0]!.status).toBe("active");
    await expect(refreshCustomDomain(f, "u2", { hostname: "api.example.com" })).rejects.toMatchObject({ code: "not-found" });

    await expect(removeCustomDomain(f, "u2", { hostname: "api.example.com" })).rejects.toBeInstanceOf(DomainError);
    expect(await removeCustomDomain(f, "u1", { hostname: "api.example.com" })).toEqual({ removed: "api.example.com" });
    expect(f.cf.size).toBe(0);
    expect(f.kv.has("host:api.example.com")).toBe(false);
    expect(f.kv.has("subhosts:my-api")).toBe(false);
    expect(JSON.parse(f.kv.get("domains:u1")!)).toEqual([]);
  });

  it("enforces plan, ownership of the subdomain, uniqueness and the limit", async () => {
    const f = supporter();
    f.plans.set("free", "free");
    await expect(addCustomDomain(f, "free", { hostname: "a.example.com", subdomain: "x" })).rejects.toMatchObject({ code: "permission-denied" });
    await expect(addCustomDomain(f, "u1", { hostname: "a.example.com", subdomain: "not-mine" })).rejects.toMatchObject({ code: "failed-precondition" });
    await expect(addCustomDomain(f, "u1", { hostname: "x.devall.test", subdomain: "my-api" })).rejects.toMatchObject({ code: "invalid-argument" });
    for (let i = 0; i < MAX_CUSTOM_DOMAINS; i++) await addCustomDomain(f, "u1", { hostname: `d${i}.example.com`, subdomain: "my-api" });
    await expect(addCustomDomain(f, "u1", { hostname: "one-more.example.com", subdomain: "my-api" })).rejects.toMatchObject({ code: "resource-exhausted" });
    f.plans.set("u2", "supporter");
    f.subs.set("u2", ["other"]);
    await expect(addCustomDomain(f, "u2", { hostname: "d0.example.com", subdomain: "other" })).rejects.toMatchObject({ code: "already-exists" });
  });

  it("rolls back the claim when Cloudflare refuses, and adopts a half-created hostname", async () => {
    const f = supporter();
    f.cfFail = true;
    await expect(addCustomDomain(f, "u1", { hostname: "bad.example.com", subdomain: "my-api" })).rejects.toMatchObject({ code: "unavailable" });
    expect(f.domains.has("bad.example.com")).toBe(false);
    f.cf.set("old", pendingCf("again.example.com", "old"));
    const v = await addCustomDomain(f, "u1", { hostname: "again.example.com", subdomain: "my-api" });
    expect(v.status).toBe("pending");
    expect(f.domains.get("again.example.com")!.cfId).toBe("old");
  });
});

describe("Cloudflare custom hostnames client", () => {
  const cfg = { zoneId: "zone1", apiToken: "tok", apiBase: "https://cf.invalid/v4" };

  it("creates, reads, finds and deletes with the zone token", async () => {
    const calls: { url: string; init: RequestInit }[] = [];
    const fake = (async (url: string, init: RequestInit) => {
      calls.push({ url, init });
      const result = url.includes("?hostname=") ? [pendingCf("a.example.com")] : pendingCf("a.example.com");
      return new Response(JSON.stringify({ success: true, result }), { status: 200 });
    }) as unknown as typeof fetch;
    const created = await createCustomHostname(cfg, "a.example.com", fake);
    expect(created.id).toBe("cf-a.example.com");
    expect(calls[0]!.url).toBe("https://cf.invalid/v4/zones/zone1/custom_hostnames");
    expect(JSON.parse(String(calls[0]!.init.body))).toEqual({ hostname: "a.example.com", ssl: { method: "http", type: "dv", settings: { min_tls_version: "1.2" } } });
    expect((calls[0]!.init.headers as Record<string, string>).authorization).toBe("Bearer tok");
    await getCustomHostname(cfg, "cf-1", fake);
    expect(calls[1]!.url).toBe("https://cf.invalid/v4/zones/zone1/custom_hostnames/cf-1");
    expect((await findCustomHostname(cfg, "a.example.com", fake))?.hostname).toBe("a.example.com");
    await deleteCustomHostname(cfg, "cf-1", fake);
    expect(calls[3]!.init.method).toBe("DELETE");
  });

  it("surfaces API errors, tolerates deleting a missing hostname and needs config", async () => {
    const failing = (async () => new Response(JSON.stringify({ success: false, errors: [{ code: 1406, message: "Duplicate custom hostname found." }] }), { status: 409 })) as unknown as typeof fetch;
    await expect(createCustomHostname(cfg, "a.example.com", failing)).rejects.toMatchObject({ status: 409, codes: [1406] });
    const missing = (async () => new Response(JSON.stringify({ success: false, errors: [{ message: "not found" }] }), { status: 404 })) as unknown as typeof fetch;
    await expect(deleteCustomHostname(cfg, "gone", missing)).resolves.toBeUndefined();
    await expect(createCustomHostname({ zoneId: "", apiToken: "t" }, "a.example.com", failing)).rejects.toBeInstanceOf(CfApiError);
  });
});

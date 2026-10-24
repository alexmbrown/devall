import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { kvDelete, kvPut, KvError } from "../src/cloudflare.js";
import { applyStripeEvent, type Entitlement, type EntitlementDeps } from "../src/entitlements.js";
import { RESERVED_SUBDOMAINS } from "../src/reserved.js";
import { parseSignatureHeader, planChangeFromEvent, SignatureError, signPayload, verifyStripeWebhook, type StripeEvent } from "../src/stripe.js";
import { decideClaim, MAX_RESERVED, validateSubdomain } from "../src/subdomains.js";

const SECRET = "whsec_test_0123456789abcdef";
const NOW = 1_800_000_000;

function signed(body: string, t = NOW, secret = SECRET): string {
  return `t=${t},v1=${signPayload(body, secret, t)}`;
}

function event(type: string, object: Record<string, unknown>, created = NOW, id = `evt_${created}_${type}`): StripeEvent {
  return { id, type, created, data: { object } };
}

describe("Stripe signature verification", () => {
  const body = JSON.stringify(event("customer.subscription.updated", { id: "sub_1", status: "active" }));

  it("accepts a correctly signed payload (string or Buffer)", () => {
    expect(verifyStripeWebhook(body, signed(body), SECRET, { now: NOW }).type).toBe("customer.subscription.updated");
    expect(verifyStripeWebhook(Buffer.from(body), signed(body), SECRET, { now: NOW + 200 }).id).toBeTruthy();
  });

  it("accepts any of several v1 signatures (secret rotation) and ignores v0", () => {
    const header = `t=${NOW},v0=${signPayload(body, SECRET, NOW)},v1=${"0".repeat(64)},v1=${signPayload(body, SECRET, NOW)}`;
    expect(verifyStripeWebhook(body, header, SECRET, { now: NOW }).id).toBeTruthy();
    expect(() => verifyStripeWebhook(body, `t=${NOW},v0=${signPayload(body, SECRET, NOW)}`, SECRET, { now: NOW })).toThrow(SignatureError);
  });

  it("rejects tampering, wrong secrets, old timestamps and junk", () => {
    expect(() => verifyStripeWebhook(body.replace("active", "canceled"), signed(body), SECRET, { now: NOW })).toThrow(/no matching/);
    expect(() => verifyStripeWebhook(body, signed(body, NOW, "whsec_other"), SECRET, { now: NOW })).toThrow(/no matching/);
    expect(() => verifyStripeWebhook(body, signed(body, NOW - 301), SECRET, { now: NOW })).toThrow(/tolerance/);
    expect(() => verifyStripeWebhook(body, undefined, SECRET, { now: NOW })).toThrow(/missing/);
    expect(() => verifyStripeWebhook(body, "garbage", SECRET, { now: NOW })).toThrow(/malformed/);
    expect(() => verifyStripeWebhook(body, signed(body), "", { now: NOW })).toThrow(/not configured/);
    expect(parseSignatureHeader("t=5, v1=ab ,v1=cd")).toEqual({ timestamp: 5, signatures: ["ab", "cd"] });
  });
});

describe("Stripe events → plan", () => {
  it("maps paid subscription checkouts to supporter with the client reference", () => {
    const c = planChangeFromEvent(event("checkout.session.completed", { mode: "subscription", payment_status: "paid", client_reference_id: "uid-1", customer: "cus_1", subscription: "sub_1" }));
    expect(c).toMatchObject({ uid: "uid-1", customerId: "cus_1", subscriptionId: "sub_1", plan: "supporter" });
    expect(planChangeFromEvent(event("checkout.session.completed", { mode: "payment", payment_status: "paid" }))).toBeNull();
    expect(planChangeFromEvent(event("checkout.session.completed", { mode: "subscription", payment_status: "unpaid" }))).toBeNull();
  });

  it.each([
    ["active", "supporter"],
    ["trialing", "supporter"],
    ["past_due", "supporter"],
    ["unpaid", "free"],
    ["canceled", "free"],
    ["incomplete_expired", "free"],
    ["paused", "free"],
  ])("subscription status %s → %s", (status, plan) => {
    expect(planChangeFromEvent(event("customer.subscription.updated", { id: "sub_1", customer: { id: "cus_1" }, status }))?.plan).toBe(plan);
  });

  it("treats deletions as free and ignores unrelated events", () => {
    expect(planChangeFromEvent(event("customer.subscription.deleted", { id: "sub_1", customer: "cus_1", status: "active" }))?.plan).toBe("free");
    expect(planChangeFromEvent(event("invoice.paid", {}))).toBeNull();
  });
});

class Fake implements EntitlementDeps {
  ent = new Map<string, Entitlement>();
  customers = new Map<string, string>();
  claims = new Map<string, Record<string, unknown>>();
  kv = new Map<string, string>();
  async getEntitlement(uid: string) {
    return this.ent.get(uid) ?? null;
  }
  async setEntitlement(uid: string, e: Partial<Entitlement>) {
    this.ent.set(uid, { ...(this.ent.get(uid) ?? ({} as Entitlement)), ...e } as Entitlement);
  }
  async uidForCustomer(c: string) {
    return this.customers.get(c) ?? null;
  }
  async linkCustomer(c: string, uid: string) {
    this.customers.set(c, uid);
  }
  async getClaims(uid: string) {
    return this.claims.get(uid) ?? {};
  }
  async setClaims(uid: string, c: Record<string, unknown>) {
    this.claims.set(uid, c);
  }
  async kvPut(k: string, v: string) {
    this.kv.set(k, v);
  }
  now() {
    return 42;
  }
}

describe("applying entitlements", () => {
  it("upgrades on checkout, follows the customer on later events and downgrades on cancel", async () => {
    const f = new Fake();
    f.claims.set("uid-1", { admin: false });
    const r1 = await applyStripeEvent(event("checkout.session.completed", { mode: "subscription", payment_status: "paid", client_reference_id: "uid-1", customer: "cus_1", subscription: "sub_1" }, NOW), f);
    expect(r1).toEqual({ applied: true, uid: "uid-1", plan: "supporter" });
    expect(f.claims.get("uid-1")).toEqual({ admin: false, plan: "supporter" });
    expect(JSON.parse(f.kv.get("plan:uid-1")!)).toEqual({ plan: "supporter", updatedAt: 42 });
    expect(f.ent.get("uid-1")).toMatchObject({ plan: "supporter", customerId: "cus_1", subscriptionId: "sub_1" });

    const r2 = await applyStripeEvent(event("customer.subscription.deleted", { id: "sub_1", customer: "cus_1", status: "canceled" }, NOW + 100), f);
    expect(r2).toEqual({ applied: true, uid: "uid-1", plan: "free" });
    expect(f.claims.get("uid-1")!.plan).toBe("free");
    expect(JSON.parse(f.kv.get("plan:uid-1")!).plan).toBe("free");
  });

  it("ignores stale events delivered out of order", async () => {
    const f = new Fake();
    f.customers.set("cus_2", "uid-2");
    await applyStripeEvent(event("customer.subscription.deleted", { id: "sub_2", customer: "cus_2" }, NOW + 50), f);
    const stale = await applyStripeEvent(event("customer.subscription.updated", { id: "sub_2", customer: "cus_2", status: "active" }, NOW), f);
    expect(stale).toEqual({ applied: false, reason: "stale event" });
    expect(f.ent.get("uid-2")!.plan).toBe("free");
  });

  it("can't attribute payments without a linked account", async () => {
    const f = new Fake();
    const r = await applyStripeEvent(event("customer.subscription.updated", { id: "sub_3", customer: "cus_unknown", status: "active" }), f);
    expect(r.applied).toBe(false);
    expect(f.kv.size).toBe(0);
  });
});

describe("subdomain claims", () => {
  it("validates names like the relay edge", () => {
    expect(validateSubdomain("  My-App ")).toEqual({ ok: true, name: "my-app" });
    for (const bad of ["ab", "-app", "app-", "a--b", "xn--abc", "www", "admin", "happy-otter-1234", "a".repeat(41), 42, null]) {
      expect(validateSubdomain(bad).ok).toBe(false);
    }
  });

  it("only lets supporters claim free names, idempotently, up to the limit", () => {
    expect(decideClaim({ uid: "u", plan: "free", name: "my-app", currentOwner: null, userSubdomains: [] })).toMatchObject({ ok: false, code: "permission-denied" });
    expect(decideClaim({ uid: "u", plan: "supporter", name: "admin", currentOwner: null, userSubdomains: [] })).toMatchObject({ ok: false, code: "invalid-argument" });
    expect(decideClaim({ uid: "u", plan: "supporter", name: "my-app", currentOwner: "other", userSubdomains: [] })).toMatchObject({ ok: false, code: "already-exists" });
    expect(decideClaim({ uid: "u", plan: "supporter", name: "my-app", currentOwner: null, userSubdomains: ["a1x"] })).toEqual({ ok: true, name: "my-app", subdomains: ["a1x", "my-app"], changed: true });
    expect(decideClaim({ uid: "u", plan: "supporter", name: "my-app", currentOwner: "u", userSubdomains: ["my-app"] })).toEqual({ ok: true, name: "my-app", subdomains: ["my-app"], changed: false });
    const full = Array.from({ length: MAX_RESERVED }, (_, i) => `name-${i}x`);
    expect(decideClaim({ uid: "u", plan: "supporter", name: "one-more", currentOwner: null, userSubdomains: full })).toMatchObject({ ok: false, code: "resource-exhausted" });
  });

  it("keeps the reserved list identical to the worker's", () => {
    const worker = JSON.parse(readFileSync(fileURLToPath(new URL("../../../worker/src/reserved-subdomains.json", import.meta.url)), "utf8")) as string[];
    expect([...RESERVED_SUBDOMAINS]).toEqual(worker);
  });
});

describe("Cloudflare KV client", () => {
  const cfg = { accountId: "acc", namespaceId: "ns", apiToken: "tok", apiBase: "https://cf.invalid/v4" };

  it("PUTs values with the API token", async () => {
    const calls: { url: string; init: RequestInit }[] = [];
    const fake = (async (url: string, init: RequestInit) => {
      calls.push({ url, init });
      return new Response(JSON.stringify({ success: true }), { status: 200 });
    }) as unknown as typeof fetch;
    await kvPut(cfg, "sub:my-app", '{"uid":"u"}', fake);
    expect(calls[0]!.url).toBe("https://cf.invalid/v4/accounts/acc/storage/kv/namespaces/ns/values/sub%3Amy-app");
    expect(calls[0]!.init.method).toBe("PUT");
    expect((calls[0]!.init.headers as Record<string, string>).authorization).toBe("Bearer tok");
    expect(calls[0]!.init.body).toBe('{"uid":"u"}');
    await kvDelete(cfg, "sub:my-app", fake);
    expect(calls[1]!.init.method).toBe("DELETE");
  });

  it("surfaces API errors and missing config", async () => {
    const failing = (async () => new Response(JSON.stringify({ success: false, errors: [{ message: "Authentication error" }] }), { status: 403 })) as unknown as typeof fetch;
    await expect(kvPut(cfg, "k", "v", failing)).rejects.toThrow(/Authentication error/);
    await expect(kvPut({ ...cfg, apiToken: "" }, "k", "v", failing)).rejects.toBeInstanceOf(KvError);
    const missing = (async () => new Response("", { status: 404 })) as unknown as typeof fetch;
    await expect(kvDelete(cfg, "gone", missing)).resolves.toBeUndefined();
  });
});

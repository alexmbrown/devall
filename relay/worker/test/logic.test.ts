import { describe, expect, it } from "vitest";
import { continueResponse, hasContinueCookie, safeContinueTarget, shouldShowInterstitial } from "../src/interstitial";
import { checkClaimableSubdomain, checkSubdomainSyntax, classifyHost, isRandomStyle, isValidInboxId, randomInboxId, randomSubdomain, ulid } from "../src/names";
import { DEFAULT_LIMITS, limitsFor, overLimit, planFromClaim } from "../src/plans";
import { addToLedger, dayKey, emptyUsage, monthPeriod, nextUtcMidnight, RateLimiter, rollUsage, shouldFlush, totalBytes } from "../src/quota";
import { forwardRequestHeaders, responseHeaders, sendableCloseCode } from "../src/http";
import { sanitizeResponse } from "../src/inbox";
import type { Env } from "../src/env";

const BROWSER_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36";

function browserGet(over: Record<string, string> = {}, url = "https://brave-otter-1234.devall.test/app") {
  return new Request(url, {
    headers: { accept: "text/html,application/xhtml+xml,*/*;q=0.8", "user-agent": BROWSER_UA, "sec-fetch-dest": "document", "sec-fetch-mode": "navigate", ...over },
  });
}

describe("interstitial", () => {
  it("shows only for browser page navigations", () => {
    expect(shouldShowInterstitial(browserGet(), true)).toBe(true);
    expect(shouldShowInterstitial(browserGet(), false)).toBe(false);
    expect(shouldShowInterstitial(browserGet({ accept: "application/json" }), true)).toBe(false);
    expect(shouldShowInterstitial(browserGet({ "user-agent": "curl/8.9" }), true)).toBe(false);
    expect(shouldShowInterstitial(browserGet({ "user-agent": "Stripe/1.0 (+https://stripe.com/docs/webhooks)" }), true)).toBe(false);
    expect(shouldShowInterstitial(browserGet({ "sec-fetch-dest": "image" }), true)).toBe(false);
    expect(shouldShowInterstitial(browserGet({ "sec-fetch-mode": "cors" }), true)).toBe(false);
    expect(shouldShowInterstitial(browserGet({ "devall-skip-browser-warning": "1" }), true)).toBe(false);
    expect(shouldShowInterstitial(browserGet({ cookie: "a=b; devall_continue=1" }), true)).toBe(false);
    expect(shouldShowInterstitial(new Request("https://x.devall.test/", { method: "POST", headers: { accept: "text/html", "user-agent": BROWSER_UA } }), true)).toBe(false);
    expect(shouldShowInterstitial(browserGet({}, "https://x.devall.test/__devall/continue?to=/"), true)).toBe(false);
  });

  it("parses the continue cookie exactly", () => {
    expect(hasContinueCookie("devall_continue=1")).toBe(true);
    expect(hasContinueCookie("x=1;  devall_continue=1 ;y=2")).toBe(true);
    expect(hasContinueCookie("devall_continue=0")).toBe(false);
    expect(hasContinueCookie("notdevall_continue=1")).toBe(false);
    expect(hasContinueCookie(null)).toBe(false);
  });

  it("never redirects off-site from the continue link", () => {
    expect(safeContinueTarget("/a?b=1")).toBe("/a?b=1");
    expect(safeContinueTarget("//evil.example/")).toBe("/");
    expect(safeContinueTarget("/\\evil.example")).toBe("/");
    expect(safeContinueTarget("https://evil.example")).toBe("/");
    expect(safeContinueTarget("/x\r\nSet-Cookie: a=b")).toBe("/");
    const r = continueResponse(new URL("https://x.devall.test/__devall/continue?to=%2Fdocs%3Fq%3D1"));
    expect(r.status).toBe(303);
    expect(r.headers.get("location")).toBe("/docs?q=1");
    expect(r.headers.get("set-cookie")).toMatch(/^devall_continue=1; Path=\/; Max-Age=604800; Secure; HttpOnly; SameSite=Lax$/);
  });
});

describe("names", () => {
  it("generates random adjective-noun-NNNN names", () => {
    const seen = new Set<string>();
    for (let i = 0; i < 200; i++) {
      const n = randomSubdomain();
      expect(n).toMatch(/^[a-z]+-[a-z]+-[1-9]\d{3}$/);
      expect(isRandomStyle(n)).toBe(true);
      expect(checkSubdomainSyntax(n).ok).toBe(true);
      seen.add(n);
    }
    expect(seen.size).toBeGreaterThan(190);
  });

  it("validates claimable names", () => {
    expect(checkClaimableSubdomain("my-app").ok).toBe(true);
    expect(checkClaimableSubdomain("ab").ok).toBe(false);
    expect(checkClaimableSubdomain("a".repeat(41)).ok).toBe(false);
    expect(checkClaimableSubdomain("-app").ok).toBe(false);
    expect(checkClaimableSubdomain("app-").ok).toBe(false);
    expect(checkClaimableSubdomain("my--app").ok).toBe(false);
    expect(checkClaimableSubdomain("xn--80ak6aa92e").ok).toBe(false);
    expect(checkClaimableSubdomain("My-App").ok).toBe(false);
    expect(checkClaimableSubdomain("www").ok).toBe(false);
    expect(checkClaimableSubdomain("login").ok).toBe(false);
    expect(checkClaimableSubdomain("happy-otter-1234").ok).toBe(false);
  });

  it("classifies hosts", () => {
    expect(classifyHost("relay.devall.test", "devall.test")).toEqual({ kind: "relay" });
    expect(classifyHost("HOOKS.devall.test:443", "devall.test")).toEqual({ kind: "hooks" });
    expect(classifyHost("my-app.devall.test", "devall.test")).toEqual({ kind: "tunnel", sub: "my-app" });
    expect(classifyHost("devall.test", "devall.test")).toEqual({ kind: "apex" });
    expect(classifyHost("a.b.devall.test", "devall.test")).toEqual({ kind: "unknown" });
    // Hosts outside the domain are custom-domain candidates (routed only when mapped).
    expect(classifyHost("evil-devall.test", "devall.test")).toEqual({ kind: "custom", host: "evil-devall.test" });
    expect(classifyHost("X.Example.com.", "devall.test")).toEqual({ kind: "custom", host: "x.example.com" });
    // Local development: the bare loopback host serves the relay routes.
    expect(classifyHost("127.0.0.1:8787", "devall.test")).toEqual({ kind: "relay" });
    expect(classifyHost("localhost", "devall.test")).toEqual({ kind: "relay" });
    expect(classifyHost("[::1]:8787", "devall.test")).toEqual({ kind: "relay" });
    expect(classifyHost("10.0.0.1", "devall.test")).toEqual({ kind: "unknown" });
    expect(classifyHost("share.devall.test", "devall.test")).toEqual({ kind: "share" });
    expect(classifyHost("mock.devall.test", "devall.test")).toEqual({ kind: "mockPath" });
    expect(classifyHost("pets.mock.devall.test", "devall.test")).toEqual({ kind: "mock", name: "pets" });
    expect(classifyHost("pets.devall-mock.dev", "devall.test", "devall-mock.dev")).toEqual({ kind: "mock", name: "pets" });
    expect(classifyHost("a.b.mock.devall.test", "devall.test")).toEqual({ kind: "unknown" });
  });

  it("makes unguessable inbox ids and sortable ulids", () => {
    const id = randomInboxId();
    expect(isValidInboxId(id)).toBe(true);
    expect(id).toHaveLength(20);
    expect(isValidInboxId("short")).toBe(false);
    const a = ulid(1_700_000_000_000);
    const b = ulid(1_700_000_000_001);
    expect(a).toHaveLength(26);
    expect(a.slice(0, 10) < b.slice(0, 10)).toBe(true);
  });
});

describe("plans and quotas", () => {
  it("has sensible tiers", () => {
    expect(DEFAULT_LIMITS.free.interstitial).toBe(true);
    expect(DEFAULT_LIMITS.supporter.interstitial).toBe(false);
    expect(DEFAULT_LIMITS.free.inboxEventsPerDay).toBe(500);
    expect(DEFAULT_LIMITS.free.inboxRetentionSeconds).toBe(86400);
    expect(DEFAULT_LIMITS.supporter.inboxRetentionSeconds).toBe(30 * 86400);
    expect(DEFAULT_LIMITS.free.sessionSeconds).toBe(8 * 3600);
    expect(DEFAULT_LIMITS.free.bandwidthBytesPerMonth).toBe(1024 ** 3);
    expect(DEFAULT_LIMITS.selfhost.bandwidthBytesPerMonth).toBeNull();
    // Monitors and cloud load regions are Supporter features (free: none).
    expect(DEFAULT_LIMITS.free.monitors).toBe(0);
    expect(DEFAULT_LIMITS.supporter.monitors).toBe(20);
    expect(DEFAULT_LIMITS.free.loadRegions).toBe(0);
    expect(DEFAULT_LIMITS.supporter.loadRegions).toBe(3);
    expect(DEFAULT_LIMITS.supporter.loadMaxDurationSeconds).toBe(60);
    expect(DEFAULT_LIMITS.supporter.loadMaxRps).toBe(100);
    expect(DEFAULT_LIMITS.supporter.loadPrivateTargets).toBe(false);
    expect(DEFAULT_LIMITS.selfhost.loadPrivateTargets).toBe(true);
    expect(DEFAULT_LIMITS.selfhost.loadRegions).toBe(9);
    expect(planFromClaim("supporter")).toBe("supporter");
    expect(planFromClaim("selfhost")).toBe("free");
    expect(planFromClaim(undefined)).toBe("free");
  });

  it("applies env overrides and the global interstitial switch", () => {
    const env = { LIMITS_JSON: '{"free":{"bandwidthBytesPerMonth":5}}', INTERSTITIAL: "off" } as unknown as Env;
    expect(limitsFor("free", env).bandwidthBytesPerMonth).toBe(5);
    expect(limitsFor("free", env).interstitial).toBe(false);
    // A community relay can still hand free accounts a monitor.
    expect(limitsFor("free", { LIMITS_JSON: '{"free":{"monitors":1}}' } as unknown as Env).monitors).toBe(1);
    expect(limitsFor("free", { LIMITS_JSON: "{broken" } as unknown as Env).bandwidthBytesPerMonth).toBe(1024 ** 3);
    expect(overLimit(10, null)).toBe(false);
    expect(overLimit(10, 10)).toBe(true);
    expect(overLimit(9, 10)).toBe(false);
  });

  it("computes UTC periods", () => {
    const t = Date.UTC(2026, 9, 21, 16, 13, 24);
    expect(monthPeriod(t)).toEqual({ key: "2026-10", start: Date.UTC(2026, 9, 1), end: Date.UTC(2026, 10, 1) });
    expect(monthPeriod(Date.UTC(2026, 11, 31, 23)).end).toBe(Date.UTC(2027, 0, 1));
    expect(dayKey(t)).toBe("2026-10-21");
    expect(nextUtcMidnight(t)).toBe(Date.UTC(2026, 9, 22));
  });

  it("accounts usage per month and decides when to flush", () => {
    const t = Date.UTC(2026, 9, 21);
    const u = { ...emptyUsage(t), bytes: 100, pendingBytes: 50 };
    expect(totalBytes(u)).toBe(150);
    expect(shouldFlush(u, t)).toBe(false);
    expect(shouldFlush(u, t + 30_000)).toBe(true);
    expect(shouldFlush({ ...u, pendingBytes: 1024 * 1024 }, t)).toBe(true);
    expect(shouldFlush({ ...u, pendingBytes: 0, pendingRequests: 0 }, t + 60_000)).toBe(false);
    const next = rollUsage(u, Date.UTC(2026, 10, 2));
    expect(next.period).toBe("2026-11");
    expect(totalBytes(next)).toBe(0);
    expect(rollUsage(u, t)).toBe(u);
    let l = addToLedger(undefined, 10, 1, t);
    l = addToLedger(l, 5, 2, t);
    expect(l).toEqual({ period: "2026-10", bytes: 15, requests: 3 });
    expect(addToLedger(l, 1, 1, Date.UTC(2026, 10, 1))).toEqual({ period: "2026-11", bytes: 1, requests: 1 });
  });

  it("rate limits per key with refill", () => {
    const rl = new RateLimiter(60);
    const t = 1_000_000;
    for (let i = 0; i < 60; i++) expect(rl.allow("1.2.3.4", t)).toBe(true);
    expect(rl.allow("1.2.3.4", t)).toBe(false);
    expect(rl.retryAfter("1.2.3.4", t)).toBe(1);
    expect(rl.allow("5.6.7.8", t)).toBe(true);
    expect(rl.allow("1.2.3.4", t + 1000)).toBe(true);
    const small = new RateLimiter(10, 3);
    for (const k of ["a", "b", "c", "d"]) small.allow(k, t);
    expect(small.size).toBe(3);
  });
});

describe("headers", () => {
  it("drops edge, internal and hop-by-hop request headers", () => {
    const h = new Headers({
      "content-type": "application/json",
      "cf-connecting-ip": "1.2.3.4",
      "cf-ray": "abc",
      "x-devall-control": "connect",
      connection: "keep-alive",
      "x-forwarded-for": "9.9.9.9",
      "stripe-signature": "t=1,v1=abc",
      "sec-websocket-key": "k",
    });
    expect(forwardRequestHeaders(h)).toEqual([
      ["content-type", "application/json"],
      ["sec-websocket-key", "k"],
      ["stripe-signature", "t=1,v1=abc"],
    ]);
    expect(forwardRequestHeaders(h, { websocket: true }).map(([k]) => k)).not.toContain("sec-websocket-key");
  });

  it("builds response headers keeping repeated set-cookie and content-encoding", () => {
    const h = responseHeaders([
      ["Set-Cookie", "a=1"],
      ["set-cookie", "b=2"],
      ["content-encoding", "gzip"],
      ["content-length", "10"],
      ["transfer-encoding", "chunked"],
      ["bad header", "x"],
    ]);
    expect(h.getSetCookie()).toEqual(["a=1", "b=2"]);
    expect(h.get("content-encoding")).toBe("gzip");
    expect(h.has("content-length")).toBe(false);
    expect(h.has("transfer-encoding")).toBe(false);
  });

  it("maps reserved close codes", () => {
    expect(sendableCloseCode(1006)).toBe(1000);
    expect(sendableCloseCode(1005)).toBe(1000);
    expect(sendableCloseCode(4009)).toBe(4009);
    expect(sendableCloseCode(1001)).toBe(1001);
  });

  it("sanitizes configured inbox responses", () => {
    expect(sanitizeResponse(null)).toEqual({ status: 200, headers: [["content-type", "application/json"]], body: '{"ok":true}' });
    const r = sanitizeResponse({ status: 999, headers: [["Content-Type", "text/html"], ["set-cookie", "x=1"], ["x-ok", "1"], ["bad\nname", "v"]], body: "<b>hi</b>" });
    expect(r.status).toBe(200);
    expect(r.headers).toEqual([
      ["content-type", "text/plain; charset=utf-8"],
      ["x-ok", "1"],
    ]);
  });
});

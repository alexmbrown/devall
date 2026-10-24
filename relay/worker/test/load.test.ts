// Cloud load-generator regions: target verification, plan limits, the kill switch and real runs
// (LoadDO objects inside workerd) against a fake network.

import { SELF } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";
import { coveredBy, loadNet, parseTarget, segments, SEGMENT_MAX_REQUESTS, validatePlan, VERIFY_PATH, type CloudLoadPlan, type LoadTarget } from "../src/load";
import { ArrivalSchedule, LogHistogram, mergeRegions, privateReason, Recorder, type CloudLoadResult } from "../src/loadstats";
import { DEFAULT_LIMITS } from "../src/plans";
import { RELAY, TOKENS } from "./helpers";

const auth = (token: string) => ({ authorization: `Bearer ${token}`, "content-type": "application/json" });

async function api<T>(method: string, path: string, token: string | null, body?: unknown): Promise<{ status: number; body: T }> {
  const res = await SELF.fetch(`${RELAY}${path}`, { method, headers: token ? auth(token) : {}, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await res.text();
  return { status: res.status, body: (text ? JSON.parse(text) : null) as T };
}

const realFetch = loadNet.fetch;
afterEach(() => {
  loadNet.fetch = realFetch;
});

/** Fake internet: serves verification files from `files`, records load requests, a colo trace and DNS. */
function fakeNet(files: Map<string, string>, calls: Request[], dns: Map<string, string> = new Map()) {
  loadNet.fetch = async (r: Request) => {
    const u = new URL(r.url);
    if (u.hostname === "colo.example.test") return new Response("fl=1\nh=x\ncolo=AMS\n");
    if (u.hostname === "cloudflare-dns.com") {
      const name = u.searchParams.get("name") ?? "";
      const v = dns.get(name);
      return Response.json({ Status: 0, Answer: v ? [{ name, type: 16, TTL: 60, data: `"${v}"` }] : [] });
    }
    if (u.pathname === VERIFY_PATH) {
      const f = files.get(u.origin);
      return f ? new Response(f) : new Response("nope", { status: 404 });
    }
    calls.push(r);
    if (u.pathname === "/slow") await new Promise((res) => setTimeout(res, 50));
    if (u.pathname === "/fail") return new Response("bad", { status: 503 });
    return new Response(`hello from ${u.pathname}`, { headers: { "content-type": "text/plain" } });
  };
}

function plan(over: Partial<CloudLoadPlan> = {}): CloudLoadPlan {
  return {
    name: "edge smoke",
    regions: ["weur", "enam"],
    profile: { startRate: 5, stages: [{ durationMs: 2000, target: 5 }] },
    requests: [{ name: "home", path: "home.req.yaml", method: "GET", url: "https://api.load.test/", headers: [["accept", "text/plain"]] }],
    weights: [1],
    ...over,
  };
}

describe("cloud load (pure parts)", () => {
  it("schedules arrivals exactly like devall-load", () => {
    const s = new ArrivalSchedule(2, [{ durationMs: 5000, target: 2 }]);
    const ts: number[] = [];
    for (let t = s.next(); t !== null; t = s.next()) ts.push(t);
    expect(ts).toEqual([0, 500, 1000, 1500, 2000, 2500, 3000, 3500, 4000, 4500]);
    const ramp = new ArrivalSchedule(0, [{ durationMs: 10_000, target: 10 }]);
    expect(ramp.expected()).toBeCloseTo(50);
    let n = 0;
    let last = -1;
    for (let t = ramp.next(); t !== null; t = ramp.next()) {
      expect(t).toBeGreaterThanOrEqual(last);
      last = t;
      n++;
    }
    expect(n).toBe(50);
  });

  it("keeps a log-linear histogram within 1.6 % and merges exactly", () => {
    const h = new LogHistogram();
    for (const v of [1, 50, 127, 128, 1000, 12_345, 999_999, 30_000_000]) {
      const top = LogHistogram.highest(LogHistogram.index(v));
      expect(top).toBeGreaterThanOrEqual(v);
      expect((top - v) / Math.max(v, 1)).toBeLessThanOrEqual(1 / 64);
      h.record(v);
    }
    for (let i = 0; i < 1000; i++) expect(LogHistogram.index(i + 1)).toBeGreaterThanOrEqual(LogHistogram.index(i));
    const a = new LogHistogram();
    const b = new LogHistogram();
    for (let i = 1; i <= 100; i++) (i % 2 ? a : b).record(i * 1000);
    const m = LogHistogram.from(a.toJSON());
    m.merge(b.toJSON());
    expect(m.count).toBe(100);
    expect(m.percentiles().p50).toBeCloseTo(50, 0);
    expect(m.percentiles().max).toBe(100);
    expect(m.percentiles().min).toBe(1);
  });

  it("merges regions into a devall-load–shaped summary", () => {
    const r1 = new Recorder(1, 1000);
    const r2 = new Recorder(1, 1000);
    for (let i = 0; i < 10; i++) {
      r1.record({ req: 0, latencyUs: 10_000, serviceUs: 9_000, status: 200, error: null, bytesIn: 10, bytesOut: 5, checksPassed: 1, checksFailed: 0, at: i * 100 });
      r1.iterationDone(i * 100);
    }
    r2.record({ req: 0, latencyUs: null, serviceUs: null, status: null, error: { kind: "connect", message: "refused" }, bytesIn: 0, bytesOut: 5, checksPassed: 0, checksFailed: 1, at: 1500 });
    r2.dropped(1600);
    const res: CloudLoadResult = mergeRegions({
      runId: "r1",
      test: "t",
      startedAt: Date.UTC(2026, 9, 22),
      plannedMs: 2000,
      bucketSeconds: 1,
      meta: [{ path: "a.req.yaml", name: "a", method: "GET" }],
      regions: [
        { region: "weur", name: "Western Europe", share: 0.5, result: r1.result("weur", "AMS", null), error: null },
        { region: "enam", name: "Eastern North America", share: 0.5, result: r2.result("enam", null, null), error: null },
      ],
      aborted: false,
      abortReason: null,
      warnings: [],
    });
    expect(res.totals.requests).toBe(11);
    expect(res.totals.failed).toBe(1);
    expect(res.totals.iterations).toBe(10);
    expect(res.totals.droppedIterations).toBe(1);
    expect(res.statusCodes).toEqual({ "200": 10 });
    expect(res.errorKinds.connect).toBe(1);
    expect(res.latency.p95).toBe(10);
    expect(res.requests[0]!.requests).toBe(11);
    expect(res.timeline.map((p) => p.requests)).toEqual([10, 1]);
    expect(res.regions.map((r) => [r.region, r.colo, r.requests])).toEqual([
      ["weur", "AMS", 10],
      ["enam", null, 1],
    ]);
    expect(res.totals.checksRate).toBeCloseTo(10 / 11);
  });

  it("knows private targets and normalizes target addresses", () => {
    for (const h of ["127.0.0.1", "10.1.2.3", "192.168.0.10", "172.20.0.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "[::1]", "[fd00::1]", "[fe80::1]", "[::ffff:10.0.0.1]", "localhost", "db.internal", "printer.local", "intranet"]) {
      expect(privateReason(h), h).not.toBeNull();
    }
    for (const h of ["api.example.com", "8.8.8.8", "[2606:4700::1111]", "172.32.0.1"]) expect(privateReason(h), h).toBeNull();
    expect(parseTarget("API.Example.com")).toMatchObject({ origin: "https://api.example.com", key: "api.example.com:443" });
    expect(parseTarget("http://127.0.0.1:3000/x")).toMatchObject({ origin: "http://127.0.0.1:3000", key: "127.0.0.1:3000", scheme: "http" });
    expect(parseTarget("https://api.example.com:443")).toMatchObject({ origin: "https://api.example.com" });
    expect(typeof parseTarget("ftp://x.example.com")).toBe("string");
    expect(typeof parseTarget("https://user:pw@x.example.com")).toBe("string");
    const t = (over: Partial<LoadTarget>): LoadTarget => ({ origin: "", scheme: "https", hostname: "api.example.com", port: 443, key: "api.example.com:443", token: "x", createdAt: 0, verifiedAt: 1, expiresAt: Date.now() + 1000, method: "http", lastCheckAt: null, lastError: null, ...over });
    expect(coveredBy([t({})], new URL("https://api.example.com/a"))).toBe(true);
    expect(coveredBy([t({})], new URL("http://api.example.com/a"))).toBe(false);
    expect(coveredBy([t({ method: "dns" })], new URL("http://api.example.com:8080/a"))).toBe(true);
    expect(coveredBy([t({ expiresAt: Date.now() - 1 })], new URL("https://api.example.com/a"))).toBe(false);
    expect(coveredBy([t({ verifiedAt: null })], new URL("https://api.example.com/a"))).toBe(false);
  });

  it("validates plans against the tier and splits runs into bounded segments", () => {
    const sup = DEFAULT_LIMITS.supporter;
    expect(validatePlan(plan(), sup)).toBeNull();
    expect(validatePlan(plan({ regions: ["weur", "enam", "apac", "oc"] }), sup)).toMatch(/at most 3 regions/);
    expect(validatePlan(plan({ regions: ["mars"] }), sup)).toMatch(/regions must be among/);
    expect(validatePlan(plan({ profile: { startRate: 1, stages: [{ durationMs: 61_000, target: 1 }] } }), sup)).toMatch(/up to 60 s/);
    expect(validatePlan(plan({ profile: { startRate: 101, stages: [{ durationMs: 5000, target: 101 }] } }), sup)).toMatch(/100 requests\/s/);
    // A flow of 3 steps at 40 iterations/s is 120 requests/s.
    const flow = plan({ weights: null, flow: [0, 0, 0], profile: { startRate: 40, stages: [{ durationMs: 5000, target: 40 }] } });
    expect(validatePlan(flow, sup)).toMatch(/100 requests\/s/);
    expect(validatePlan(plan({ weights: [1], flow: [0] }), sup)).toMatch(/exactly one/);
    expect(validatePlan(plan(), DEFAULT_LIMITS.free)).toMatch(/at most 0 regions/);
    expect(validatePlan(plan({ profile: { startRate: 1, stages: [{ durationMs: 500_000, target: 1 }] } }), DEFAULT_LIMITS.selfhost)).toBeNull();
    expect(segments(5000, 2)).toEqual([5001]);
    expect(segments(25_000, 10)).toEqual([10_000, 20_000, 25_001]);
    const fast = segments(60_000, 100); // 800 requests per segment at 100/s → 8 s segments
    expect(fast[0]).toBe((SEGMENT_MAX_REQUESTS / 100) * 1000);
  });
});

describe("cloud load (API)", () => {
  it("is Supporter / self-host only", async () => {
    const info = await api<{ enabled: boolean; limits: { loadRegions: number }; regions: { id: string }[] }>("GET", "/v1/load", TOKENS.alice);
    expect(info.status).toBe(200);
    expect(info.body.limits.loadRegions).toBe(0);
    expect(info.body.regions.map((r) => r.id)).toContain("weur");
    const add = await api<{ error: { code: string } }>("POST", "/v1/load/targets", TOKENS.alice, { target: "api.load.test" });
    expect(add.status).toBe(403);
    expect(add.body.error.code).toBe("plan_required");
    expect((await api("GET", "/v1/load", null)).status).toBe(401);
  });

  it("refuses unverified, private and relay targets; verifies by file or DNS", async () => {
    const files = new Map<string, string>();
    const dns = new Map<string, string>();
    const calls: Request[] = [];
    fakeNet(files, calls, dns);
    const priv = await api<{ error: { code: string; message: string } }>("POST", "/v1/load/targets", TOKENS.sam, { target: "http://127.0.0.1:3000" });
    expect(priv.status).toBe(403);
    expect(priv.body.error.code).toBe("target_not_allowed");
    const relay = await api<{ error: { code: string } }>("POST", "/v1/load/targets", TOKENS.sam, { target: "my-app.devall.test" });
    expect(relay.body.error.code).toBe("target_not_allowed");

    const added = await api<{ token: string; status: string; fileUrl: string; dnsName: string }>("POST", "/v1/load/targets", TOKENS.sam, { target: "api.load.test" });
    expect(added.status).toBe(201);
    expect(added.body.status).toBe("pending");
    expect(added.body.fileUrl).toBe(`https://api.load.test${VERIFY_PATH}`);
    expect(added.body.dnsName).toBe("_devall-load.api.load.test");
    // Adding again returns the same token.
    expect((await api<{ token: string }>("POST", "/v1/load/targets", TOKENS.sam, { target: "https://api.load.test/" })).body.token).toBe(added.body.token);

    // No load before verification.
    const early = await api<{ error: { code: string } }>("POST", "/v1/load/runs", TOKENS.sam, plan());
    expect(early.status).toBe(403);
    expect(early.body.error.code).toBe("target_not_verified");
    expect(calls).toHaveLength(0);

    const fail = await api<{ error: { code: string; details: string[] } }>("POST", "/v1/load/targets/verify", TOKENS.sam, { target: "api.load.test" });
    expect(fail.status).toBe(422);
    expect(fail.body.error.code).toBe("verification_failed");
    expect(fail.body.error.details.join(" ")).toMatch(/HTTP 404/);
    expect(fail.body.error.details.join(" ")).toMatch(/no TXT record/);
    await new Promise((r) => setTimeout(r, 2100));
    files.set("https://api.load.test", `${added.body.token}\n`);
    const ok = await api<{ status: string; method: string; expiresAt: string }>("POST", "/v1/load/targets/verify", TOKENS.sam, { target: "api.load.test" });
    expect(ok.status).toBe(200);
    expect(ok.body.status).toBe("verified");
    expect(ok.body.method).toBe("http");
    expect(Date.parse(ok.body.expiresAt) - Date.now()).toBeGreaterThan(29 * 24 * 3600_000);

    // DNS TXT verification covers every port of the host.
    const other = await api<{ token: string }>("POST", "/v1/load/targets", TOKENS.sam, { target: "dns.load.test" });
    dns.set("_devall-load.dns.load.test", other.body.token);
    const byDns = await api<{ status: string; method: string }>("POST", "/v1/load/targets/verify", TOKENS.sam, { target: "dns.load.test" });
    expect(byDns.body).toMatchObject({ status: "verified", method: "dns" });

    const list = await api<{ targets: { key: string; status: string }[] }>("GET", "/v1/load", TOKENS.sam);
    expect(list.body.targets.map((t) => [t.key, t.status])).toEqual([
      ["api.load.test:443", "verified"],
      ["dns.load.test:443", "verified"],
    ]);
    // Bob can't use Sam's verification.
    expect((await api<{ error: { code: string } }>("POST", "/v1/load/runs", TOKENS.root, plan())).body.error.code).toBe("target_not_verified");
    expect((await api("DELETE", `/v1/load/targets?target=dns.load.test`, TOKENS.sam)).status).toBe(200);
  });

  it("runs from two regions, merges the results, enforces the daily quota and the kill switch", async () => {
    const files = new Map<string, string>();
    const calls: Request[] = [];
    fakeNet(files, calls);
    const t = await api<{ token: string }>("POST", "/v1/load/targets", TOKENS.sam, { target: "api.load.test" });
    files.set("https://api.load.test", t.body.token);
    await api("POST", "/v1/load/targets/verify", TOKENS.sam, { target: "api.load.test" });

    const run = await api<CloudLoadResult>("POST", "/v1/load/runs", TOKENS.sam, plan());
    expect(run.status).toBe(200);
    // 5 iterations/s for 2 s, split across 2 regions = 10 requests.
    expect(run.body.totals.requests).toBe(10);
    expect(run.body.totals.iterations).toBe(10);
    expect(run.body.statusCodes).toEqual({ "200": 10 });
    expect(run.body.aborted).toBe(false);
    expect(run.body.regions.map((r) => [r.region, r.requests, r.colo])).toEqual([
      ["weur", 5, "AMS"],
      ["enam", 5, "AMS"],
    ]);
    expect(run.body.requests[0]).toMatchObject({ path: "home.req.yaml", requests: 10, failed: 0 });
    expect(run.body.latency.p50).toBeGreaterThanOrEqual(0);
    expect(run.body.totals.checksRate).toBe(1);
    expect(calls).toHaveLength(10);
    expect(calls.every((c) => /^[0-9A-Z]{26}$/.test(c.headers.get("x-devall-load") ?? ""))).toBe(true);

    // Kill switch (admin): refused while disabled.
    const admin = { authorization: "Bearer admin-token-0123456789" };
    expect((await SELF.fetch(`${RELAY}/admin/load/disable`, { method: "POST", headers: admin })).status).toBe(200);
    const off = await api<{ error: { code: string } }>("POST", "/v1/load/runs", TOKENS.sam, plan());
    expect(off.status).toBe(503);
    expect(off.body.error.code).toBe("load_disabled");
    expect((await api<{ enabled: boolean }>("GET", "/v1/load", TOKENS.sam)).body.enabled).toBe(false);
    expect((await SELF.fetch(`${RELAY}/admin/load/enable`, { method: "POST", headers: admin })).status).toBe(200);

    // Daily quota: 3 runs (vitest.config.ts), one used above.
    expect((await api("POST", "/v1/load/runs", TOKENS.sam, plan({ profile: { startRate: 1, stages: [{ durationMs: 1000, target: 1 }] } }))).status).toBe(200);
    expect((await api("POST", "/v1/load/runs", TOKENS.sam, plan({ profile: { startRate: 1, stages: [{ durationMs: 1000, target: 1 }] } }))).status).toBe(200);
    const over = await api<{ error: { code: string } }>("POST", "/v1/load/runs", TOKENS.sam, plan());
    expect(over.status).toBe(429);
    expect(over.body.error.code).toBe("load_quota");
  });

  it("self-hosted: private targets, flows with checks, and stopping a run", async () => {
    const files = new Map<string, string>();
    const calls: Request[] = [];
    fakeNet(files, calls);
    const t = await api<{ token: string }>("POST", "/v1/load/targets", TOKENS.root, { target: "http://127.0.0.1:3000" });
    expect(t.status).toBe(201);
    files.set("http://127.0.0.1:3000", t.body.token);
    expect((await api<{ status: string }>("POST", "/v1/load/targets/verify", TOKENS.root, { target: "http://127.0.0.1:3000" })).body.status).toBe("verified");

    const flow = plan({
      regions: ["weur"],
      requests: [
        { name: "ok", path: "ok.req.yaml", method: "POST", url: "http://127.0.0.1:3000/slow", headers: [], bodyBase64: btoa("{}") },
        { name: "fail", path: "fail.req.yaml", method: "GET", url: "http://127.0.0.1:3000/fail", headers: [] },
      ],
      weights: null,
      flow: [0, 1],
      checks: [{ type: "bodyContains", text: "hello" }],
      profile: { startRate: 4, stages: [{ durationMs: 1000, target: 4 }] },
    });
    const res = await api<CloudLoadResult>("POST", "/v1/load/runs", TOKENS.root, flow);
    expect(res.status).toBe(200);
    expect(res.body.totals.iterations).toBe(4);
    expect(res.body.totals.requests).toBe(8);
    expect(res.body.totals.failed).toBe(4);
    expect(res.body.statusCodes).toEqual({ "200": 4, "503": 4 });
    expect(res.body.totals.checksPassed).toBe(4);
    expect(res.body.totals.checksFailed).toBe(4);
    expect(res.body.requests.map((r) => r.failed)).toEqual([0, 4]);
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(4);

    // Stop: the run ends at its next segment boundary and is marked aborted.
    const long = plan({ runId: "stoptest01", regions: ["weur"], requests: [{ name: "ok", path: "ok.req.yaml", method: "GET", url: "http://127.0.0.1:3000/", headers: [] }], profile: { startRate: 2, stages: [{ durationMs: 30_000, target: 2 }] } });
    const pending = api<CloudLoadResult>("POST", "/v1/load/runs", TOKENS.root, long);
    await new Promise((r) => setTimeout(r, 500));
    const busy = await api<{ error: { code: string } }>("POST", "/v1/load/runs", TOKENS.root, plan({ regions: ["weur"], requests: long.requests }));
    expect(busy.status).toBe(409);
    expect(busy.body.error.code).toBe("load_busy");
    expect((await api("POST", "/v1/load/runs/stoptest01/stop", TOKENS.root)).status).toBe(200);
    const stopped = await pending;
    expect(stopped.status).toBe(200);
    expect(stopped.body.aborted).toBe(true);
    expect(stopped.body.abortReason).toBe("stopped");
    expect(stopped.body.totals.requests).toBeLessThan(60);
  }, 40_000);
});

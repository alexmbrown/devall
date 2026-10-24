// Hosted mocks, shared snapshots and scheduled monitors, end to end inside workerd.

import { createExecutionContext, createScheduledController, env, runDurableObjectAlarm, runInDurableObject, SELF, waitOnExecutionContext } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import worker from "../src/index";
import type { MonitorDO, MonitorView, RunResult } from "../src/monitor";
import { DOMAIN, RELAY, TOKENS, waitFor } from "./helpers";

const auth = (token: string) => ({ authorization: `Bearer ${token}`, "content-type": "application/json" });

async function api<T>(method: string, path: string, token: string | null, body?: unknown): Promise<{ status: number; body: T }> {
  const res = await SELF.fetch(`${RELAY}${path}`, { method, headers: token ? auth(token) : {}, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await res.text();
  return { status: res.status, body: (text ? JSON.parse(text) : null) as T };
}

const PETS = [
  { name: "list pets", request: { method: "GET", urlPath: "/pets" }, response: { status: 200, headers: { "Content-Type": "application/json" }, jsonBody: [{ id: 1, name: "Rex" }] } },
  {
    name: "one pet",
    request: { method: "GET", urlPathTemplate: "/pets/{id}" },
    response: { status: 200, headers: { "Content-Type": "application/json", "X-Pet": "{{request.path.id}}" }, body: '{"id":{{request.path.id}},"q":"{{request.query.q}}"}', transformers: ["response-template"] },
  },
  { name: "create", request: { method: "POST", urlPath: "/pets", bodyPatterns: [{ matchesJsonPath: "$.name" }] }, response: { status: 201, bodyFileName: "created.json" } },
  { name: "html", request: { method: "GET", urlPath: "/page" }, response: { status: 200, headers: { "Content-Type": "text/html" }, body: "<script>alert(1)</script>" } },
  { name: "xml", request: { method: "POST", urlPath: "/xml", bodyPatterns: [{ equalToXml: "<a/>" }] }, response: { status: 200 } },
];

describe("hosted mocks", () => {
  it("publishes, serves (host and path style), journals and unpublishes", async () => {
    const files = { "created.json": btoa('{"created":true}') };
    const pub = await api<{ url: string; hostUrl: string; pathUrl: string; stubs: number; warnings: { stub: string; feature: string }[] }>("PUT", "/v1/mocks/pets-api", TOKENS.sam, {
      mappings: PETS,
      files,
      settings: { cors: true },
    });
    expect(pub.status).toBe(200);
    expect(pub.body.stubs).toBe(5);
    expect(pub.body.url).toBe(`https://pets-api.mock.${DOMAIN}`);
    expect(pub.body.pathUrl).toBe(`https://mock.${DOMAIN}/pets-api`);
    expect(pub.body.warnings.map((w) => `${w.stub}:${w.feature}`)).toEqual(["xml:request"]);

    const list = await SELF.fetch(`https://pets-api.mock.${DOMAIN}/pets`);
    expect(list.status).toBe(200);
    expect(await list.json()).toEqual([{ id: 1, name: "Rex" }]);
    expect(list.headers.get("x-robots-tag")).toContain("noindex");

    const one = await SELF.fetch(`https://mock.${DOMAIN}/pets-api/pets/42?q=hi`, { headers: { origin: "https://app.example" } });
    expect(await one.text()).toBe('{"id":42,"q":"hi"}');
    expect(one.headers.get("x-pet")).toBe("42");
    expect(one.headers.get("access-control-allow-origin")).toBe("https://app.example");

    const created = await SELF.fetch(`https://pets-api.mock.${DOMAIN}/pets`, { method: "POST", body: '{"name":"Tom"}' });
    expect(created.status).toBe(201);
    expect(await created.text()).toBe('{"created":true}');

    const page = await SELF.fetch(`https://pets-api.mock.${DOMAIN}/page`);
    expect(page.headers.get("content-type")).toBe("text/plain; charset=utf-8");
    expect(page.headers.get("content-security-policy")).toContain("sandbox");

    const preflight = await SELF.fetch(`https://pets-api.mock.${DOMAIN}/pets`, { method: "OPTIONS", headers: { origin: "https://app.example", "access-control-request-method": "POST" } });
    expect(preflight.status).toBe(204);

    const miss = await SELF.fetch(`https://pets-api.mock.${DOMAIN}/nope`);
    expect(miss.status).toBe(404);
    expect(await miss.text()).toMatch(/No stub matched GET \/nope/);

    const journal = await api<{ requests: { url: string; matched: boolean; stubName: string | null; status: number }[] }>("GET", "/v1/mocks/pets-api/requests?limit=3", TOKENS.sam);
    expect(journal.body.requests.map((r) => [r.url, r.matched, r.status])).toEqual([
      ["/nope", false, 404],
      ["/pets", false, 204],
      ["/page", true, 200],
    ]);

    const mine = await api<{ mocks: { name: string; stubs: number }[] }>("GET", "/v1/mocks", TOKENS.sam);
    expect(mine.body.mocks.map((m) => m.name)).toContain("pets-api");
    const acct = await api<{ hostedMocks: { name: string }[] }>("GET", "/v1/account", TOKENS.sam);
    expect(acct.body.hostedMocks.map((m) => m.name)).toContain("pets-api");

    // Someone else can't overwrite, read or delete it.
    expect((await api("PUT", "/v1/mocks/pets-api", TOKENS.root, { mappings: [] })).status).toBe(409);
    expect((await api("GET", "/v1/mocks/pets-api/requests", TOKENS.root)).status).toBe(404);
    expect((await api("DELETE", "/v1/mocks/pets-api", TOKENS.root)).status).toBe(404);

    expect((await api("DELETE", "/v1/mocks/pets-api", TOKENS.sam)).status).toBe(200);
    expect((await SELF.fetch(`https://pets-api.mock.${DOMAIN}/pets`)).status).toBe(404);
  });

  it("keeps scenario state and resets it", async () => {
    const mappings = [
      { scenarioName: "cart", requiredScenarioState: "Started", newScenarioState: "full", request: { method: "POST", urlPath: "/cart" }, response: { status: 201 } },
      { scenarioName: "cart", requiredScenarioState: "full", request: { method: "GET", urlPath: "/cart" }, response: { status: 200, body: "full" } },
      { scenarioName: "cart", requiredScenarioState: "Started", request: { method: "GET", urlPath: "/cart" }, response: { status: 200, body: "empty" } },
    ];
    expect((await api("PUT", "/v1/mocks/cart-flow", TOKENS.root, { mappings })).status).toBe(200);
    const get = () => SELF.fetch(`https://cart-flow.mock.${DOMAIN}/cart`).then((r) => r.text());
    expect(await get()).toBe("empty");
    await SELF.fetch(`https://cart-flow.mock.${DOMAIN}/cart`, { method: "POST" });
    expect(await get()).toBe("full");
    expect((await api("POST", "/v1/mocks/cart-flow/scenarios/reset", TOKENS.root)).status).toBe(200);
    expect(await get()).toBe("empty");
  });

  it("enforces plan limits and requires an account", async () => {
    expect((await api("PUT", "/v1/mocks/anon", null, { mappings: [] })).status).toBe(401);
    const tooMany = Array.from({ length: 6 }, (_, i) => ({ request: { urlPath: `/x${i}` }, response: { status: 200 } }));
    const big = await api<{ error: { code: string } }>("PUT", "/v1/mocks/alice-one", TOKENS.alice, { mappings: tooMany });
    expect(big.status).toBe(413);
    expect(big.body.error.code).toBe("too_many_stubs");
    expect((await api("PUT", "/v1/mocks/alice-one", TOKENS.alice, { mappings: tooMany.slice(0, 2) })).status).toBe(200);
    // Free: one hosted mock per account.
    const second = await api<{ error: { code: string } }>("PUT", "/v1/mocks/alice-two", TOKENS.alice, { mappings: [] });
    expect(second.status).toBe(429);
    expect(second.body.error.code).toBe("too_many_mocks");
    expect((await api("PUT", "/v1/mocks/admin", TOKENS.alice, { mappings: [] })).status).toBe(400);
    // Daily request cap (40 in tests), then 429.
    let last = 0;
    for (let i = 0; i < 41; i++) last = (await SELF.fetch(`https://alice-one.mock.${DOMAIN}/x0`)).status;
    expect(last).toBe(429);
    expect((await api("DELETE", "/v1/mocks/alice-one", TOKENS.alice)).status).toBe(200);
  });
});

interface ShareCreated {
  id: string;
  url: string;
  expiresAt: string;
}

const SNAPSHOT = {
  version: 1,
  title: "Get user <b>",
  request: { method: "GET", url: "https://api.example.test/users/1", headers: [["authorization", "••••••"], ["accept", "application/json"]] },
  response: { status: 200, headers: [["content-type", "application/json"]], body: '{"id":1,"name":"Ada"}', contentType: "application/json", size: 21 },
  timing: { totalMs: 87.4 },
  curl: "curl https://api.example.test/users/1 -H 'authorization: ••••••'",
  reqYaml: "version: 1\nname: Get user\nmethod: GET\nurl: https://api.example.test/users/1\n",
  redactions: ["secret values", "authorization headers"],
};

describe("shared snapshots", () => {
  it("creates an expiring read-only page with downloads, lists and revokes", async () => {
    const created = await api<ShareCreated>("POST", "/v1/shares", TOKENS.root, { snapshot: SNAPSHOT, ttlSeconds: 3600 });
    expect(created.status).toBe(201);
    expect(created.body.url).toBe(`https://share.${DOMAIN}/${created.body.id}`);
    expect(Date.parse(created.body.expiresAt)).toBeGreaterThan(Date.now() + 3500_000);

    const page = await SELF.fetch(created.body.url);
    expect(page.status).toBe(200);
    const html = await page.text();
    expect(html).toContain("Get user &lt;b&gt;");
    expect(html).not.toContain("<b>");
    expect(html).toContain("&quot;name&quot;: &quot;Ada&quot;");
    expect(html).toContain("Copy as cURL");
    const csp = page.headers.get("content-security-policy")!;
    const nonce = /'nonce-([0-9a-f]+)'/.exec(csp)?.[1];
    expect(nonce && html.includes(`<script nonce="${nonce}">`)).toBe(true);
    expect(page.headers.get("x-robots-tag")).toContain("noindex");

    const yaml = await SELF.fetch(`${created.body.url}/request.req.yaml`);
    expect(yaml.headers.get("content-disposition")).toBe('attachment; filename="get-user-b.req.yaml"');
    expect(await yaml.text()).toContain("method: GET");
    expect((await (await SELF.fetch(`${created.body.url}/raw.json`)).json()) as { title: string }).toMatchObject({ title: "Get user <b>" });

    const list = await api<{ shares: { id: string }[] }>("GET", "/v1/shares", TOKENS.root);
    expect(list.body.shares.map((s) => s.id)).toContain(created.body.id);

    expect((await api("DELETE", `/v1/shares/${created.body.id}`, TOKENS.sam)).status).toBe(404);
    expect((await api("DELETE", `/v1/shares/${created.body.id}`, TOKENS.root)).status).toBe(200);
    expect((await SELF.fetch(created.body.url)).status).toBe(410);
  });

  it("expires by alarm, enforces size/count limits and refuses anonymous uploads", async () => {
    expect((await api("POST", "/v1/shares", null, { snapshot: SNAPSHOT })).status).toBe(401);
    const tooBig = { ...SNAPSHOT, response: { ...SNAPSHOT.response, body: "x".repeat(5000) } };
    expect((await api("POST", "/v1/shares", TOKENS.bob, { snapshot: tooBig })).status).toBe(413);
    expect((await api("POST", "/v1/shares", TOKENS.bob, { snapshot: SNAPSHOT, ttlSeconds: 8 * 86400 })).status).toBe(400);
    const ids: string[] = [];
    for (let i = 0; i < 3; i++) ids.push((await api<ShareCreated>("POST", "/v1/shares", TOKENS.bob, { snapshot: SNAPSHOT })).body.id);
    const fourth = await api<{ error: { code: string } }>("POST", "/v1/shares", TOKENS.bob, { snapshot: SNAPSHOT });
    expect(fourth.status).toBe(429);
    expect(fourth.body.error.code).toBe("too_many_shares");

    // Expire the first one: move its expiry into the past and run the alarm.
    const stub = env.SHARE.get(env.SHARE.idFromName(ids[0]!));
    await runInDurableObject(stub, async (_obj, state) => {
      const meta = (await state.storage.get<{ expiresAt: number }>("meta"))!;
      meta.expiresAt = Date.now() - 1;
      await state.storage.put("meta", meta);
    });
    expect((await SELF.fetch(`https://share.${DOMAIN}/${ids[0]}`)).status).toBe(410);
    await runDurableObjectAlarm(stub);
    expect(await stub.info()).toBeNull();
    for (const id of ids.slice(1)) await api("DELETE", `/v1/shares/${id}`, TOKENS.bob);
  });
});

function monitorInput(over: Record<string, unknown> = {}) {
  return {
    name: "API health",
    intervalMinutes: 5,
    requests: [
      {
        name: "health",
        method: "GET",
        url: "https://target.example.test/health",
        headers: [["authorization", "Bearer {{secret.TOKEN}}"]],
        assertions: [
          { assert: "status", op: "eq", value: "200" },
          { assert: "jsonPath", path: "$.status", op: "eq", value: "ok" },
        ],
      },
    ],
    alerts: { webhookUrl: "https://alerts.example.test/hook", slackWebhookUrl: "https://hooks.slack.com/services/T/B/x" },
    secrets: { TOKEN: "tok-super-secret" },
    ...over,
  };
}

/** Fake targets: /health is up while `up.value` is true; alert endpoints are recorded. */
function fakeNet(up: { value: boolean }, calls: { url: string; auth: string | null; body: string }[]) {
  return async (r: Request): Promise<Response> => {
    calls.push({ url: r.url, auth: r.headers.get("authorization"), body: r.method === "POST" ? await r.text() : "" });
    if (r.url.startsWith("https://target.example.test/health")) {
      return up.value ? Response.json({ status: "ok" }) : new Response("boom", { status: 503 });
    }
    return new Response("ok");
  };
}

describe("scheduled monitors", () => {
  it("creates monitors with encrypted secrets, runs on the cron and alerts on state changes", async () => {
    const created = await api<MonitorView>("POST", "/v1/monitors", TOKENS.sam, monitorInput());
    expect(created.status).toBe(201);
    expect(created.body.secretNames).toEqual(["TOKEN"]);
    expect(JSON.stringify(created.body)).not.toContain("tok-super-secret");
    const id = created.body.id;
    const stub = env.MONITOR.get(env.MONITOR.idFromName(id));
    await runInDurableObject(stub, async (_obj, state) => {
      const raw = JSON.stringify(await state.storage.get("meta"));
      expect(raw).not.toContain("tok-super-secret");
    });

    const up = { value: false };
    const calls: { url: string; auth: string | null; body: string }[] = [];
    await runInDurableObject(stub, (obj: MonitorDO) => {
      obj.fetcher = fakeNet(up, calls);
    });

    // The cron arms the monitor (KV metadata says every 5 minutes); the alarm runs the checks.
    const slot = 5 * 5_000_000;
    const ctrl = createScheduledController({ scheduledTime: slot * 60_000, cron: "*/5 * * * *" });
    const ctx = createExecutionContext();
    await worker.scheduled!(ctrl, env, ctx);
    await waitOnExecutionContext(ctx);
    // The alarm is due immediately; workerd runs it on its own (or we run it here).
    await runDurableObjectAlarm(stub);
    await waitFor(() => calls.some((c) => c.url === "https://alerts.example.test/hook"), 5000, "the down alert");

    const health = calls.find((c) => c.url.startsWith("https://target.example.test/health"))!;
    expect(health.auth).toBe("Bearer tok-super-secret");
    const hook = calls.find((c) => c.url === "https://alerts.example.test/hook")!;
    expect(JSON.parse(hook.body)).toMatchObject({ monitor: { id, name: "API health" }, status: "down" });
    expect(hook.body).not.toContain("tok-super-secret");
    expect(calls.find((c) => c.url.startsWith("https://hooks.slack.com/"))!.body).toContain("is failing");

    // Recovery: manual run (also alerts once), then results + uptime.
    up.value = true;
    calls.length = 0;
    await runInDurableObject(stub, async (_obj, state) => {
      const meta = (await state.storage.get<{ lastRunAt: number }>("meta"))!;
      meta.lastRunAt = Date.now() - 60_000;
      await state.storage.put("meta", meta);
    });
    const run = await api<RunResult>("POST", `/v1/monitors/${id}/run`, TOKENS.sam);
    expect(run.status).toBe(200);
    expect(run.body.ok).toBe(true);
    expect(run.body.trigger).toBe("manual");
    expect(JSON.parse(calls.find((c) => c.url === "https://alerts.example.test/hook")!.body).status).toBe("up");
    expect((await api("POST", `/v1/monitors/${id}/run`, TOKENS.sam)).status).toBe(429);

    const view = await api<MonitorView>("GET", `/v1/monitors/${id}`, TOKENS.sam);
    expect(view.body.runs).toBe(2);
    expect(view.body.uptime).toBe(0.5);
    expect(view.body.recent.map((r) => r.ok)).toEqual([false, true]);
    const results = await api<{ results: RunResult[] }>("GET", `/v1/monitors/${id}/results`, TOKENS.sam);
    expect(results.body.results[1]!.checks[0]!.failures.join(" ")).toMatch(/status eq 200/);

    // Update: drop the secret; delete.
    const upd = await api<MonitorView>("PUT", `/v1/monitors/${id}`, TOKENS.sam, monitorInput({ secrets: { TOKEN: null }, intervalMinutes: 15 }));
    expect(upd.body.secretNames).toEqual([]);
    expect(upd.body.intervalMinutes).toBe(15);
    expect((await api("DELETE", `/v1/monitors/${id}`, TOKENS.root)).status).toBe(404);
    expect((await api("DELETE", `/v1/monitors/${id}`, TOKENS.sam)).status).toBe(200);
    expect(await env.RELAY_KV.get(`mon:${id}`)).toBeNull();
  });

  it("applies plan limits", async () => {
    // Free accounts have no monitors (Supporter feature); the list still answers with the limits.
    const free = await api<{ error: { code: string; message: string } }>("POST", "/v1/monitors", TOKENS.alice, monitorInput({ secrets: {}, intervalMinutes: 60 }));
    expect(free.status).toBe(403);
    expect(free.body.error.code).toBe("plan_required");
    const freeList = await api<{ monitors: MonitorView[]; limits: { monitors: number } }>("GET", "/v1/monitors", TOKENS.alice);
    expect(freeList.body.limits.monitors).toBe(0);
    // Supporter count limit (2 in vitest.config.ts).
    const ids: string[] = [];
    for (let i = 0; i < 2; i++) {
      const m = await api<MonitorView>("POST", "/v1/monitors", TOKENS.sam, monitorInput({ secrets: {}, intervalMinutes: 60 }));
      expect(m.status).toBe(201);
      ids.push(m.body.id);
    }
    const third = await api<{ error: { code: string } }>("POST", "/v1/monitors", TOKENS.sam, monitorInput({ secrets: {}, intervalMinutes: 60 }));
    expect(third.status).toBe(429);
    expect(third.body.error.code).toBe("too_many_monitors");
    const list = await api<{ monitors: MonitorView[]; limits: { monitors: number } }>("GET", "/v1/monitors", TOKENS.sam);
    expect(list.body.monitors).toHaveLength(2);
    expect(list.body.limits.monitors).toBe(2);
    for (const id of ids) await api("DELETE", `/v1/monitors/${id}`, TOKENS.sam);
    expect((await api("GET", "/v1/monitors", null)).status).toBe(401);
  });

  it("skips a retried cron slot but runs the next one", async () => {
    const created = await api<MonitorView>("POST", "/v1/monitors", TOKENS.root, monitorInput({ secrets: {}, alerts: null }));
    expect(created.status).toBe(201);
    const id = created.body.id;
    const stub = env.MONITOR.get(env.MONITOR.idFromName(id));
    const calls: { url: string; auth: string | null; body: string }[] = [];
    await runInDurableObject(stub, (obj: MonitorDO) => {
      obj.fetcher = fakeNet({ value: true }, calls);
    });
    const fire = async (slotMinute: number) => {
      const ctx = createExecutionContext();
      await worker.scheduled!(createScheduledController({ scheduledTime: slotMinute * 60_000, cron: "*/5 * * * *" }), env, ctx);
      await waitOnExecutionContext(ctx);
      await runDurableObjectAlarm(stub);
    };
    const slot = 5 * 5_100_000;
    await fire(slot);
    await fire(slot); // a retry of the same slot
    expect((await api<MonitorView>("GET", `/v1/monitors/${id}`, TOKENS.root)).body.runs).toBe(1);
    await fire(slot + 5); // the next slot runs right away (no wall-clock wait)
    expect((await api<MonitorView>("GET", `/v1/monitors/${id}`, TOKENS.root)).body.runs).toBe(2);
    await api("DELETE", `/v1/monitors/${id}`, TOKENS.root);
  });
});

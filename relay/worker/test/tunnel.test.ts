import { env, runDurableObjectAlarm, runInDurableObject, SELF } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT_WINDOW, F, frameU32, MAX_FRAME, type GoAway, type Hello, type QuotaPayload, type RequestHead, type WsOpen } from "../src/protocol";
import { concat, DOMAIN, MockClient, RELAY, text, TOKENS, uidOf, waitFor } from "./helpers";

const open: MockClient[] = [];

async function connect(token: string, path = "/connect"): Promise<{ c: MockClient; hello: Hello }> {
  const c = await MockClient.connect(token, path);
  open.push(c);
  const { body } = await c.nextJson<Hello>(F.HELLO);
  return { c, hello: body };
}

afterEach(async () => {
  // Wait for the close handshake: the relay has then seen the socket go away, so free accounts
  // (one tunnel at a time) don't trip over the previous test's tunnel.
  const closing = open.splice(0);
  for (const c of closing) c.close();
  await Promise.all(closing.map((c) => c.waitClosed(5000).catch(() => undefined)));
  await new Promise((r) => setTimeout(r, 20));
});

const BROWSER = {
  accept: "text/html,*/*",
  "user-agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 Safari/605.1.15",
  "sec-fetch-dest": "document",
  "sec-fetch-mode": "navigate",
};

describe("connect", () => {
  it("authenticates, negotiates the version and assigns a random subdomain (free)", async () => {
    const { c, hello } = await connect(TOKENS.alice);
    expect(c.response.headers.get("sec-websocket-protocol")).toBe("devall-relay.1");
    expect(hello.version).toBe(1);
    expect(hello.kind).toBe("tunnel");
    expect(hello.plan).toBe("free");
    expect(hello.subdomain).toMatch(/^[a-z]+-[a-z]+-\d{4}$/);
    expect(hello.publicUrl).toBe(`https://${hello.subdomain}.${DOMAIN}`);
    expect(hello.limits.interstitial).toBe(true);
    expect(hello.limits.sessionSeconds).toBe(8 * 3600);
    expect(hello.window).toBe(DEFAULT_WINDOW);
    expect(hello.maxFrame).toBe(MAX_FRAME);
    expect(Date.parse(hello.sessionExpiresAt!)).toBeGreaterThan(Date.now() + 7 * 3600 * 1000);
  });

  it("refuses bad credentials, versions and plain HTTP", async () => {
    const noAuth = (await MockClient.open(null, "/connect")) as Response;
    expect(noAuth.status).toBe(401);
    expect(((await noAuth.json()) as { error: { code: string } }).error.code).toBe("unauthorized");
    const bad = (await MockClient.open("not-a-valid-token-at-all", "/connect")) as Response;
    expect(bad.status).toBe(401);
    const ver = (await MockClient.open(TOKENS.alice, "/connect", "devall-relay.99")) as Response;
    expect(ver.status).toBe(426);
    expect(((await ver.json()) as { error: { supported: number[] } }).error.supported).toEqual([1]);
    const plain = await SELF.fetch(`${RELAY}/connect`);
    expect(plain.status).toBe(426);
  });

  it("answers the hibernation heartbeat", async () => {
    const { c } = await connect(TOKENS.alice);
    c.ws.send("devall-relay:ping");
    await waitFor(() => c.texts.includes("devall-relay:pong"), 3000, "pong");
  });

  it("lets free users resume their random subdomain after a drop", async () => {
    const first = await connect(TOKENS.alice);
    const sub = first.hello.subdomain!;
    first.c.close();
    await first.c.waitClosed();
    await new Promise((r) => setTimeout(r, 30));
    const again = await connect(TOKENS.alice, `/connect?name=${sub}`);
    expect(again.hello.subdomain).toBe(sub);
    expect(again.hello.sessionId).toBe(first.hello.sessionId);
    expect(again.hello.sessionExpiresAt).toBe(first.hello.sessionExpiresAt);
  });

  it("never hands another account's random subdomain over", async () => {
    const a = await connect(TOKENS.alice);
    const sub = a.hello.subdomain!;
    const b = await connect(TOKENS.bob, `/connect?name=${sub}`);
    expect(b.hello.subdomain).not.toBe(sub);
  });

  it("replaces an older connection of the same account", async () => {
    const a = await connect(TOKENS.alice);
    const sub = a.hello.subdomain!;
    const b = await connect(TOKENS.alice, `/connect?name=${sub}`);
    expect(b.hello.subdomain).toBe(sub);
    const { body } = await a.c.nextJson<GoAway>(F.GOAWAY);
    expect(body).toMatchObject({ reason: "replaced", reconnect: false });
    expect((await a.c.waitClosed()).code).toBe(4009);
  });

  it("limits free accounts to one tunnel at a time", async () => {
    await connect(TOKENS.bob);
    const second = (await MockClient.open(TOKENS.bob, "/connect")) as Response;
    expect(second.status).toBe(429);
    expect(((await second.json()) as { error: { code: string } }).error.code).toBe("too_many_tunnels");
  });

  it("only lets supporters use subdomains they reserved", async () => {
    const free = (await MockClient.open(TOKENS.alice, "/connect?name=my-api")) as Response;
    expect(free.status).toBe(403);
    expect(((await free.json()) as { error: { code: string } }).error.code).toBe("plan_required");

    const notMine = (await MockClient.open(TOKENS.sam, "/connect?name=my-api")) as Response;
    expect(notMine.status).toBe(403);
    expect(((await notMine.json()) as { error: { code: string } }).error.code).toBe("subdomain_not_reserved");

    await env.RELAY_KV.put("sub:my-api", JSON.stringify({ uid: uidOf("sam") }));
    const { hello } = await connect(TOKENS.sam, "/connect?name=my-api");
    expect(hello.subdomain).toBe("my-api");
    expect(hello.plan).toBe("supporter");
    expect(hello.limits.interstitial).toBe(false);
  });

  it("lets self-hosters pick any non-reserved name", async () => {
    const { hello } = await connect(TOKENS.root, "/connect?name=staging-box");
    expect(hello).toMatchObject({ subdomain: "staging-box", plan: "selfhost" });
    expect(hello.limits.bandwidthBytesPerMonth).toBeNull();
    const reserved = (await MockClient.open(TOKENS.root, "/connect?name=admin")) as Response;
    expect(reserved.status).toBe(400);
  });
});

describe("HTTP through the tunnel", () => {
  it("round-trips a request with body, headers and exact URL", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=roundtrip");
    const pending = SELF.fetch(`${hello.publicUrl}/webhooks/twilio?a=1&b=%20x`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded", "x-twilio-signature": "sig==", "cf-connecting-ip": "203.0.113.9", "x-devall-control": "connect" },
      body: "Body=hello&From=%2B1555",
    });
    const { frame, body: head } = await c.nextJson<RequestHead>(F.OPEN);
    expect(head.method).toBe("POST");
    expect(head.url).toBe(`https://roundtrip.${DOMAIN}/webhooks/twilio?a=1&b=%20x`);
    expect(head.path).toBe("/webhooks/twilio?a=1&b=%20x");
    expect(head.clientIp).toBe("203.0.113.9");
    expect(head.bodyLength).toBe(23);
    const names = head.headers.map(([k]) => k);
    expect(names).toContain("x-twilio-signature");
    expect(names).not.toContain("cf-connecting-ip");
    expect(names).not.toContain("x-devall-control");
    const chunks: Uint8Array[] = [];
    while (!c.has(F.END, frame.stream)) {
      if (c.has(F.DATA, frame.stream)) chunks.push((await c.next(F.DATA, frame.stream)).payload);
      else await new Promise((r) => setTimeout(r, 5));
    }
    while (c.has(F.DATA, frame.stream)) chunks.push((await c.next(F.DATA, frame.stream)).payload);
    expect(text(concat(chunks))).toBe("Body=hello&From=%2B1555");

    c.respond(frame.stream, 201, [["content-type", "text/plain"], ["set-cookie", "a=1"], ["set-cookie", "b=2"], ["connection", "close"]], "created");
    const res = await pending;
    expect(res.status).toBe(201);
    expect(await res.text()).toBe("created");
    expect(res.headers.getSetCookie()).toEqual(["a=1", "b=2"]);
    expect(res.headers.get("x-robots-tag")).toMatch(/noindex/);
  });

  it("streams large responses under flow control", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=streaming");
    const pending = SELF.fetch(`${hello.publicUrl}/big`);
    const { frame } = await c.nextJson<RequestHead>(F.OPEN);
    await c.next(F.END, frame.stream);
    c.sendJson(F.RESPONSE, frame.stream, { status: 200, headers: [["content-type", "application/octet-stream"]] });

    const total = 1024 * 1024; // 4× the window
    const payload = new Uint8Array(total);
    for (let i = 0; i < total; i++) payload[i] = i % 251;
    let credit = DEFAULT_WINDOW;
    let sent = 0;
    let windows = 0;
    const res = await pending;
    expect(res.status).toBe(200);
    const reader = res.body!.getReader();
    const got: Uint8Array[] = [];
    let received = 0;
    const read = (async () => {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        got.push(value);
        received += value.byteLength;
      }
    })();
    while (sent < total) {
      while (c.has(F.WINDOW, frame.stream)) {
        credit += frameU32(await c.next(F.WINDOW, frame.stream));
        windows++;
      }
      if (credit <= 0) {
        await new Promise((r) => setTimeout(r, 2));
        continue;
      }
      const n = Math.min(MAX_FRAME, credit, total - sent);
      c.send(F.RESPONSE_DATA, frame.stream, payload.subarray(sent, sent + n));
      credit -= n;
      sent += n;
    }
    c.send(F.RESPONSE_END, frame.stream);
    await read;
    expect(received).toBe(total);
    expect(concat(got)).toEqual(payload);
    expect(windows).toBeGreaterThanOrEqual(2);
  });

  it("returns a friendly 502 when the tunnel is offline", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=goes-offline");
    c.close();
    await c.waitClosed();
    await new Promise((r) => setTimeout(r, 30));
    const browser = await SELF.fetch(`${hello.publicUrl}/`, { headers: BROWSER });
    expect(browser.status).toBe(502);
    expect(browser.headers.get("content-type")).toMatch(/text\/html/);
    expect(await browser.text()).toMatch(/This tunnel is offline/);
    const api = await SELF.fetch(`${hello.publicUrl}/api`);
    expect(api.status).toBe(502);
    expect(await api.text()).toMatch(/offline/);
    const never = await SELF.fetch(`https://never-connected.${DOMAIN}/`);
    expect(never.status).toBe(502);
  });

  it("times out with 504 when the client never answers", async () => {
    const { c, hello } = await connect(TOKENS.alice);
    const res = await SELF.fetch(`${hello.publicUrl}/slow`);
    expect(res.status).toBe(504);
    const { body } = await c.nextJson<{ code: string }>(F.RESET);
    expect(body.code).toBe("timeout");
  });

  it("rejects bodies over the plan limit with 413", async () => {
    const { hello } = await connect(TOKENS.alice);
    const res = await SELF.fetch(`${hello.publicUrl}/upload`, { method: "POST", headers: { "content-length": String(11 * 1024 * 1024) }, body: new Uint8Array(11 * 1024 * 1024) });
    expect(res.status).toBe(413);
  });

  it("passes the client's error status through on RESET", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=resets");
    const pending = SELF.fetch(`${hello.publicUrl}/x`);
    const { frame } = await c.nextJson<RequestHead>(F.OPEN);
    c.sendJson(F.RESET, frame.stream, { code: "local_unreachable", message: "connection refused" });
    const res = await pending;
    expect(res.status).toBe(502);
    expect(await res.text()).toMatch(/connection refused/);
  });
});

describe("free-tier interstitial", () => {
  it("warns browsers once, then proxies after continue", async () => {
    const { c, hello } = await connect(TOKENS.alice);
    const first = await SELF.fetch(`${hello.publicUrl}/dashboard?x=1`, { headers: BROWSER });
    expect(first.status).toBe(200);
    const html = await first.text();
    expect(html).toMatch(/You are about to visit/);
    expect(html).toContain(`/__devall/continue?to=${encodeURIComponent("/dashboard?x=1")}`);
    expect(html).toContain(`https://relay.${DOMAIN}/report?host=`);
    expect(first.headers.get("x-robots-tag")).toMatch(/noindex/);
    expect(c.has(F.OPEN)).toBe(false);

    const cont = await SELF.fetch(`${hello.publicUrl}/__devall/continue?to=%2Fdashboard%3Fx%3D1`, { headers: BROWSER, redirect: "manual" });
    expect(cont.status).toBe(303);
    const cookie = cont.headers.get("set-cookie")!.split(";")[0]!;

    const pending = SELF.fetch(`${hello.publicUrl}/dashboard?x=1`, { headers: { ...BROWSER, cookie } });
    const { frame } = await c.nextJson<RequestHead>(F.OPEN);
    c.respond(frame.stream, 200, [["content-type", "text/html"]], "<h1>app</h1>");
    expect(await (await pending).text()).toBe("<h1>app</h1>");
  });

  it("never shows it to webhooks, APIs or supporters", async () => {
    const { c, hello } = await connect(TOKENS.alice);
    const pending = SELF.fetch(`${hello.publicUrl}/hook`, { method: "POST", headers: { "user-agent": "Stripe/1.0", accept: "*/*" }, body: "{}" });
    const { frame } = await c.nextJson<RequestHead>(F.OPEN);
    c.respond(frame.stream, 200, [], "ok");
    expect((await pending).status).toBe(200);

    await env.RELAY_KV.put("sub:paid-app", JSON.stringify({ uid: uidOf("sam") }));
    const sup = await connect(TOKENS.sam, "/connect?name=paid-app");
    const p2 = SELF.fetch(`${sup.hello.publicUrl}/`, { headers: BROWSER });
    const o2 = await sup.c.nextJson<RequestHead>(F.OPEN);
    sup.c.respond(o2.frame.stream, 200, [["content-type", "text/html"]], "paid");
    expect(await (await p2).text()).toBe("paid");
  });
});

describe("WebSocket pass-through", () => {
  it("relays upgrade, messages both ways and close", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=ws-app");
    const pending = SELF.fetch(`${hello.publicUrl}/socket?room=1`, { headers: { upgrade: "websocket", "sec-websocket-protocol": "chat, superchat" } });
    const { frame, body: wsOpen } = await c.nextJson<WsOpen>(F.WS_OPEN);
    expect(wsOpen.path).toBe("/socket?room=1");
    expect(wsOpen.protocols).toEqual(["chat", "superchat"]);
    expect(wsOpen.headers.map(([k]) => k)).not.toContain("sec-websocket-protocol");
    c.sendJson(F.WS_ACCEPT, frame.stream, { protocol: "chat" });
    c.send(F.WS_TEXT, frame.stream, "welcome"); // may arrive before the public side is attached
    const res = await pending;
    expect(res.status).toBe(101);
    expect(res.headers.get("sec-websocket-protocol")).toBe("chat");
    const pub = res.webSocket!;
    pub.binaryType = "arraybuffer";
    const got: (string | ArrayBuffer)[] = [];
    let pubClosed: number | null = null;
    pub.addEventListener("message", (e: MessageEvent) => {
      got.push(e.data as string | ArrayBuffer);
    });
    pub.addEventListener("close", (e: CloseEvent) => {
      pubClosed = e.code;
    });
    pub.accept();
    await waitFor(() => got.includes("welcome"), 3000, "welcome message");

    pub.send("hello from browser");
    pub.send(new Uint8Array([1, 2, 3]));
    expect(text((await c.next(F.WS_TEXT, frame.stream)).payload)).toBe("hello from browser");
    expect([...(await c.next(F.WS_BINARY, frame.stream)).payload]).toEqual([1, 2, 3]);

    c.send(F.WS_BINARY, frame.stream, new Uint8Array([9, 9]));
    await waitFor(() => got.some((m) => m instanceof ArrayBuffer), 3000, "binary message");

    c.sendJson(F.WS_CLOSE, frame.stream, { code: 4001, reason: "bye" });
    await waitFor(() => pubClosed !== null, 3000, "public close");
    expect(pubClosed).toBe(4001);
  });

  it("forwards the local server's refusal status", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=ws-refused");
    const pending = SELF.fetch(`${hello.publicUrl}/socket`, { headers: { upgrade: "websocket" } });
    const { frame } = await c.nextJson<WsOpen>(F.WS_OPEN);
    c.sendJson(F.WS_REJECT, frame.stream, { status: 403, message: "nope" });
    const res = await pending;
    expect(res.status).toBe(403);
  });

  it("tells the client when the public side closes", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=ws-close");
    const pending = SELF.fetch(`${hello.publicUrl}/`, { headers: { upgrade: "websocket" } });
    const { frame } = await c.nextJson<WsOpen>(F.WS_OPEN);
    c.sendJson(F.WS_ACCEPT, frame.stream, {});
    const pub = (await pending).webSocket!;
    pub.accept();
    pub.close(1000, "done");
    const { body } = await c.nextJson<{ code: number; reason: string }>(F.WS_CLOSE, frame.stream);
    expect(body.code).toBe(1000);
  });
});

describe("quotas and sessions", () => {
  it("reports bandwidth exhaustion and answers 429", async () => {
    await env.ACCOUNT.get(env.ACCOUNT.idFromName(uidOf("bob"))).addUsage(1024 ** 3, 1);
    const { c, hello } = await connect(TOKENS.bob);
    expect(hello.usage.bandwidthBytes).toBeGreaterThanOrEqual(1024 ** 3);
    const res = await SELF.fetch(`${hello.publicUrl}/api`);
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBeTruthy();
    const { body } = await c.nextJson<QuotaPayload>(F.QUOTA);
    expect(body.kind).toBe("bandwidth");
  });

  it("records usage in the account ledger", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=metered");
    const pending = SELF.fetch(`${hello.publicUrl}/x`, { method: "POST", body: "0123456789" });
    const { frame } = await c.nextJson<RequestHead>(F.OPEN);
    c.respond(frame.stream, 200, [], "abcdefghij");
    await (await pending).text();
    c.close();
    await c.waitClosed();
    const ledger = env.ACCOUNT.get(env.ACCOUNT.idFromName(uidOf("root")));
    let u = await ledger.getUsage();
    for (let i = 0; i < 100 && u.bandwidthBytes < 20; i++) {
      await new Promise((r) => setTimeout(r, 20));
      u = await ledger.getUsage();
    }
    expect(u.bandwidthBytes).toBeGreaterThanOrEqual(20);
    expect(u.requests).toBeGreaterThanOrEqual(1);
  });

  it("ends expired sessions with GOAWAY and close 4008", async () => {
    const { c, hello } = await connect(TOKENS.alice);
    const stub = env.TUNNEL.get(env.TUNNEL.idFromName(hello.subdomain!));
    // Pretend the session already ran out, then fire the alarm.
    const sockets = await runInDurableObject(stub, (_obj, state) => {
      for (const ws of state.getWebSockets("client")) {
        const att = ws.deserializeAttachment() as { sessionExpiresAt: number };
        att.sessionExpiresAt = Date.now() - 1;
        ws.serializeAttachment(att);
      }
      return state.getWebSockets("client").length;
    });
    expect(sockets).toBe(1);
    await runDurableObjectAlarm(stub);
    const { body } = await c.nextJson<GoAway>(F.GOAWAY);
    expect(body).toMatchObject({ reason: "session_expired", reconnect: true });
    expect((await c.nextJson<QuotaPayload>(F.QUOTA)).body.kind).toBe("session");
    expect((await c.waitClosed()).code).toBe(4008);
  });
});

describe("abuse handling", () => {
  it("blocks a subdomain through the admin API", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=phishy");
    const unauth = await SELF.fetch(`${RELAY}/admin/block?name=phishy`, { method: "POST" });
    expect(unauth.status).toBe(401);
    const ok = await SELF.fetch(`${RELAY}/admin/block?name=phishy`, { method: "POST", headers: { authorization: "Bearer admin-token-0123456789" } });
    expect(ok.status).toBe(200);
    expect((await c.nextJson<GoAway>(F.GOAWAY)).body.reason).toBe("blocked");
    expect((await SELF.fetch(`${hello.publicUrl}/`)).status).toBe(451);
    const again = (await MockClient.open(TOKENS.root, "/connect?name=phishy")) as Response;
    expect(again.status).toBe(451);
  });

  it("serves the report page and the account API", async () => {
    const report = await SELF.fetch(`${RELAY}/report?host=x.${DOMAIN}`);
    expect(await report.text()).toContain("mailto:abuse@devall.test");
    const unauth = await SELF.fetch(`${RELAY}/v1/account`);
    expect(unauth.status).toBe(401);
    await env.RELAY_KV.put(`subs:${uidOf("sam")}`, JSON.stringify(["my-api"]));
    const acct = await SELF.fetch(`${RELAY}/v1/account`, { headers: { authorization: `Bearer ${TOKENS.sam}` } });
    const body = (await acct.json()) as { plan: string; reservedSubdomains: string[]; limits: { maxTunnels: number }; domain: string; upgradeUrl: string };
    expect(body).toMatchObject({ plan: "supporter", reservedSubdomains: ["my-api"], domain: DOMAIN, upgradeUrl: "https://auth.devall.test/upgrade.html" });
    expect(body.limits.maxTunnels).toBe(5);
  });
});


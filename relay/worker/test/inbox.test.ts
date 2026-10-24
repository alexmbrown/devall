import { env, SELF } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";
import { base64ToBytes } from "../src/http";
import { F, type Hello, type InboxEvent, type InboxStatus, type QuotaPayload } from "../src/protocol";
import { DOMAIN, MockClient, TOKENS, uidOf, waitFor } from "./helpers";

const open: MockClient[] = [];

async function connectInbox(token: string, project = ""): Promise<{ c: MockClient; hello: Hello; status: InboxStatus }> {
  const c = await MockClient.connect(token, `/inbox${project ? `?project=${project}` : ""}`);
  open.push(c);
  const hello = (await c.nextJson<Hello>(F.HELLO)).body;
  const status = (await c.nextJson<InboxStatus>(F.INBOX_STATUS)).body;
  return { c, hello, status };
}

async function disconnect(c: MockClient): Promise<void> {
  c.close();
  await c.waitClosed();
  await new Promise((r) => setTimeout(r, 30));
}

afterEach(async () => {
  // Wait for the close handshake: the relay has then seen the socket go away, so free accounts
  // (one tunnel at a time) don't trip over the previous test's tunnel.
  const closing = open.splice(0);
  for (const c of closing) c.close();
  await Promise.all(closing.map((c) => c.waitClosed(5000).catch(() => undefined)));
  await new Promise((r) => setTimeout(r, 20));
});

async function events(c: MockClient, n: number): Promise<InboxEvent[]> {
  const out: InboxEvent[] = [];
  for (let i = 0; i < n; i++) out.push((await c.nextJson<InboxEvent>(F.INBOX_EVENT)).body);
  return out;
}

describe("hosted webhook inbox", () => {
  it("buffers while offline and delivers the backlog in order with acks", async () => {
    const first = await connectInbox(TOKENS.root, "proj-a");
    const url = first.hello.publicUrl;
    expect(first.hello.kind).toBe("inbox");
    expect(url).toMatch(new RegExp(`^https://hooks\\.${DOMAIN.replace(".", "\\.")}/[a-z0-9]{20}/$`));
    expect(first.hello.inboxId).toBe(url.split("/")[3]);
    expect(first.status.buffered).toBe(0);
    await disconnect(first.c);

    // Laptop is away: three webhooks arrive, raw bytes included.
    const raw = new Uint8Array([0, 1, 2, 254, 255, 10, 13]);
    const r1 = await SELF.fetch(`${url}stripe?source=cli`, { method: "POST", headers: { "content-type": "application/json", "stripe-signature": "t=1,v1=abc", "cf-connecting-ip": "54.187.174.169" }, body: '{"id":"evt_1"}' });
    expect(r1.status).toBe(200);
    expect(await r1.json()).toEqual({ ok: true });
    expect(r1.headers.get("x-devall-event-id")).toMatch(/^evt_/);
    expect(r1.headers.get("x-robots-tag")).toMatch(/noindex/);
    await SELF.fetch(`${url}raw`, { method: "PUT", body: raw });
    await SELF.fetch(`${url}`, { method: "GET" });

    const second = await connectInbox(TOKENS.root, "proj-a");
    expect(second.hello.publicUrl).toBe(url);
    expect(second.status.buffered).toBe(3);
    const evs = await events(second.c, 3);
    expect(evs.map((e) => e.method)).toEqual(["POST", "PUT", "GET"]);
    expect(evs.map((e) => e.seq)).toEqual([...evs.map((e) => e.seq)].sort((a, b) => a - b));
    const [stripe, put, get] = evs as [InboxEvent, InboxEvent, InboxEvent];
    expect(stripe.url).toBe(`${url}stripe?source=cli`);
    expect(stripe.path).toBe("/stripe?source=cli");
    expect(stripe.sourceIp).toBe("54.187.174.169");
    expect(stripe.headers).toContainEqual(["stripe-signature", "t=1,v1=abc"]);
    expect(stripe.headers.map(([k]) => k)).not.toContain("cf-connecting-ip");
    expect(new TextDecoder().decode(base64ToBytes(stripe.bodyBase64))).toBe('{"id":"evt_1"}');
    expect([...base64ToBytes(put.bodyBase64)]).toEqual([...raw]);
    expect(put.bodySize).toBe(raw.byteLength);
    expect(get.path).toBe("/");
    expect(Date.parse(stripe.receivedAt)).toBeLessThanOrEqual(Date.now());

    // Ack only the first: the other two must come back after a reconnect (at-least-once).
    second.c.sendJson(F.INBOX_ACK, 0, { ids: [stripe.id] });
    const st = await waitFor(() => second.c.frames.find((f) => f.type === F.INBOX_STATUS), 3000, "status after ack");
    expect(JSON.parse(new TextDecoder().decode(st.payload)).buffered).toBe(2);
    await disconnect(second.c);

    const third = await connectInbox(TOKENS.root, "proj-a");
    expect(third.status.buffered).toBe(2);
    const again = await events(third.c, 2);
    expect(again.map((e) => e.id)).toEqual([put.id, get.id]);
    third.c.sendJson(F.INBOX_ACK, 0, { ids: again.map((e) => e.id) });
    await waitFor(() => third.c.frames.some((f) => f.type === F.INBOX_STATUS && JSON.parse(new TextDecoder().decode(f.payload)).buffered === 0), 3000, "empty inbox");
  });

  it("delivers live events immediately while connected", async () => {
    const { c, hello } = await connectInbox(TOKENS.root, "live");
    const res = await SELF.fetch(`${hello.publicUrl}github`, { method: "POST", headers: { "x-hub-signature-256": "sha256=00" }, body: "{}" });
    expect(res.status).toBe(200);
    const [ev] = await events(c, 1);
    expect(ev!.headers).toContainEqual(["x-hub-signature-256", "sha256=00"]);
  });

  it("answers with the configured response, never as HTML", async () => {
    const { c, hello } = await connectInbox(TOKENS.root, "custom-response");
    c.sendJson(F.INBOX_CONFIG, 0, { response: { status: 202, headers: [["content-type", "text/html"], ["x-mode", "test"]], body: "<script>alert(1)</script>" } });
    const st = (await c.nextJson<InboxStatus>(F.INBOX_STATUS)).body;
    expect(st.response.status).toBe(202);
    const res = await SELF.fetch(`${hello.publicUrl}x`, { method: "POST", body: "a" });
    expect(res.status).toBe(202);
    expect(res.headers.get("content-type")).toMatch(/^text\/plain/);
    expect(res.headers.get("content-security-policy")).toMatch(/sandbox/);
    expect(res.headers.get("x-mode")).toBe("test");
  });

  it("gives free accounts one inbox and enforces the daily limit", async () => {
    const a = await connectInbox(TOKENS.alice, "one");
    const b = await connectInbox(TOKENS.alice, "two");
    expect(b.hello.publicUrl).toBe(a.hello.publicUrl);
    expect(b.hello.limits.inboxEventsPerDay).toBe(6);
    expect((await a.c.nextJson<{ reason: string }>(F.GOAWAY)).body.reason).toBe("replaced");
    const url = b.hello.publicUrl;
    for (let i = 0; i < 6; i++) expect((await SELF.fetch(`${url}e${i}`, { method: "POST", body: String(i) })).status).toBe(200);
    const over = await SELF.fetch(`${url}e6`, { method: "POST", body: "6" });
    expect(over.status).toBe(429);
    expect(Number(over.headers.get("retry-after"))).toBeGreaterThan(0);
    const q = (await b.c.nextJson<QuotaPayload>(F.QUOTA)).body;
    expect(q.kind).toBe("inbox_daily");
    expect(q.limit).toBe(6);
  });

  it("rejects oversized bodies and unknown inboxes", async () => {
    const { hello } = await connectInbox(TOKENS.bob);
    const big = await SELF.fetch(`${hello.publicUrl}big`, { method: "POST", body: new Uint8Array(4096) });
    expect(big.status).toBe(413);
    const unknown = await SELF.fetch(`https://hooks.${DOMAIN}/abcdefghijklmnopqrst/x`, { method: "POST", body: "x" });
    expect(unknown.status).toBe(404);
    const junk = await SELF.fetch(`https://hooks.${DOMAIN}/NOT_AN_ID/x`, { method: "POST", body: "x" });
    expect(junk.status).toBe(404);
  });

  it("keeps inboxes private to their owner", async () => {
    const mine = await connectInbox(TOKENS.root, "private");
    const inboxId = mine.hello.inboxId!;
    // Another account can't attach to it, even by forging the internal connect path.
    const res = await env.INBOX.get(env.INBOX.idFromName(inboxId)).fetch(
      new Request(`https://relay.${DOMAIN}/inbox`, {
        headers: { upgrade: "websocket", "x-devall-control": "connect", "x-devall-uid": uidOf("bob"), "x-devall-plan": "free", "x-devall-inbox": inboxId },
      }),
    );
    expect(res.status).toBe(403);
    // Public requests can't smuggle internal control headers either.
    const smuggled = await SELF.fetch(`${mine.hello.publicUrl}x`, { method: "POST", headers: { "x-devall-control": "connect", "x-devall-uid": uidOf("bob") }, body: "x" });
    expect(smuggled.status).toBe(200);
  });

  it("buffers a tunnel's webhook routes into the inbox while the tunnel is offline", async () => {
    const inbox = await connectInbox(TOKENS.root, "tunnel-fallback");
    const inboxId = inbox.hello.inboxId!;
    const t = await MockClient.connect(TOKENS.root, "/connect?name=fallback-app");
    open.push(t);
    const hello = (await t.nextJson<Hello>(F.HELLO)).body;
    t.sendJson(F.TUNNEL_CONFIG, 0, { offlineInbox: { inboxId, pathPrefixes: ["/webhooks/"] } });
    await new Promise((r) => setTimeout(r, 50));
    t.close();
    await t.waitClosed();
    await new Promise((r) => setTimeout(r, 30));

    const hook = await SELF.fetch(`${hello.publicUrl}/webhooks/stripe`, { method: "POST", body: '{"a":1}' });
    expect(hook.status).toBe(200);
    const page = await SELF.fetch(`${hello.publicUrl}/other`);
    expect(page.status).toBe(502);
    const [ev] = await events(inbox.c, 1);
    expect(ev!.url).toBe(`https://fallback-app.${DOMAIN}/webhooks/stripe`);
    expect(ev!.path).toBe("/webhooks/stripe");
  });
});

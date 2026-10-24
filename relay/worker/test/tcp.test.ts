// Raw TCP tunnels (TCP over WebSocket) and custom domains, end to end inside workerd.

import { env, SELF } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";
import { F, frameJson, frameU32, type Hello, type RequestHead, type TcpClose, type TcpOpen } from "../src/protocol";
import { DOMAIN, MockClient, RELAY, text, TOKENS, uidOf } from "./helpers";

const open: MockClient[] = [];

async function connect(token: string, path: string): Promise<{ c: MockClient; hello: Hello }> {
  const c = await MockClient.connect(token, path);
  open.push(c);
  const { body } = await c.nextJson<Hello>(F.HELLO);
  return { c, hello: body };
}

afterEach(async () => {
  const closing = open.splice(0);
  for (const c of closing) c.close();
  await Promise.all(closing.map((c) => c.waitClosed(5000).catch(() => undefined)));
  await new Promise((r) => setTimeout(r, 20));
});

async function sha256Hex(s: string): Promise<string> {
  const d = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)));
  return [...d].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** Opens a consumer socket; the publisher must answer the TCP_OPEN for the upgrade to finish. */
async function consumer(token: string, name: string, publisher: MockClient, accept = true): Promise<{ c: MockClient | Response; stream: number; open: TcpOpen }> {
  const pending = MockClient.open(token, `/tcp?name=${name}`);
  const { frame, body } = await publisher.nextJson<TcpOpen>(F.TCP_OPEN);
  if (accept) publisher.send(F.TCP_ACCEPT, frame.stream);
  else publisher.sendJson(F.TCP_CLOSE, frame.stream, { reason: "local_unreachable", message: "connection refused" } satisfies TcpClose);
  const c = await pending;
  if (!(c instanceof Response)) open.push(c);
  return { c, stream: frame.stream, open: body };
}

describe("raw TCP tunnels", () => {
  it("needs the Supporter plan (or self-hosting)", async () => {
    const free = (await MockClient.open(TOKENS.alice, "/connect?kind=tcp")) as Response;
    expect(free.status).toBe(403);
    expect(((await free.json()) as { error: { code: string } }).error.code).toBe("plan_required");
  });

  it("is free on a self-hosted relay, with any name", async () => {
    const { hello } = await connect(TOKENS.root, "/connect?kind=tcp&name=db-box");
    expect(hello).toMatchObject({ kind: "tcp", subdomain: "db-box", plan: "selfhost" });
    expect(hello.limits.tcpStreamsPerTunnel).toBeNull();
  });

  it("bridges a consumer connection to the publisher with end-to-end credit", async () => {
    const { c: pub, hello } = await connect(TOKENS.sam, "/connect?kind=tcp");
    expect(hello.kind).toBe("tcp");
    const name = hello.subdomain!;

    // Public HTTP to a TCP tunnel explains itself instead of proxying.
    const web = await SELF.fetch(`https://${name}.${DOMAIN}/`);
    expect(web.status).toBe(404);
    expect(await web.text()).toMatch(/devall tcp connect/);

    const { c, stream, open: o } = await consumer(TOKENS.sam, name, pub);
    expect(o.via).toBe("owner");
    const con = c as MockClient;

    // Publisher → consumer (stream rewritten to 1), early data included.
    pub.send(F.TCP_DATA, stream, "hello from postgres");
    const d1 = await con.next(F.TCP_DATA, 1);
    expect(text(d1.payload)).toBe("hello from postgres");
    // Consumer → publisher.
    con.send(F.TCP_DATA, 1, "SELECT 1;");
    expect(text((await pub.next(F.TCP_DATA, stream)).payload)).toBe("SELECT 1;");
    // Credit flows both ways untouched.
    con.window(1, 4096);
    expect(frameU32(await pub.next(F.WINDOW, stream))).toBe(4096);
    pub.window(stream, 1234);
    expect(frameU32(await con.next(F.WINDOW, 1))).toBe(1234);

    // The consumer hangs up → the publisher hears TCP_CLOSE.
    con.close();
    const closed = frameJson<TcpClose>(await pub.next(F.TCP_CLOSE, stream));
    expect(closed.reason).toBe("eof");
  });

  it("accepts the tunnel's access secret and nobody else", async () => {
    const { c: pub, hello } = await connect(TOKENS.sam, "/connect?kind=tcp");
    const name = hello.subdomain!;
    const secret = "dtcp_correct-horse-battery-staple-0123456789";
    pub.sendJson(F.TUNNEL_CONFIG, 0, { tcp: { secretSha256: await sha256Hex(secret) } });
    await new Promise((r) => setTimeout(r, 50));

    const viaSecret = await consumer(secret, name, pub);
    expect(viaSecret.open.via).toBe("secret");
    expect(viaSecret.c).toBeInstanceOf(MockClient);

    const wrong = (await MockClient.open("dtcp_wrong-secret-0123456789", `/tcp?name=${name}`)) as Response;
    expect(wrong.status).toBe(403);
    const otherAccount = (await MockClient.open(TOKENS.bob, `/tcp?name=${name}`)) as Response;
    expect(otherAccount.status).toBe(403);
    const anonymous = (await MockClient.open(null, `/tcp?name=${name}`)) as Response;
    expect(anonymous.status).toBe(401);
    const unknown = (await MockClient.open(TOKENS.sam, "/tcp?name=no-such-tunnel")) as Response;
    expect(unknown.status).toBe(404);
  });

  it("reports local failures, caps concurrent connections and closes consumers when the publisher leaves", async () => {
    const { c: pub, hello } = await connect(TOKENS.sam, "/connect?kind=tcp");
    const name = hello.subdomain!;
    const refused = await consumer(TOKENS.sam, name, pub, false);
    expect(refused.c).toBeInstanceOf(Response);
    expect((refused.c as Response).status).toBe(502);

    // Supporter limit is 3 concurrent connections in tests.
    const conns: { c: MockClient; stream: number }[] = [];
    for (let i = 0; i < 3; i++) {
      const r = await consumer(TOKENS.sam, name, pub);
      conns.push({ c: r.c as MockClient, stream: r.stream });
    }
    const fourth = (await MockClient.open(TOKENS.sam, `/tcp?name=${name}`)) as Response;
    expect(fourth.status).toBe(429);

    // The publisher closes one stream → that consumer sees TCP_CLOSE and a normal close.
    const first = conns[0]!;
    pub.sendJson(F.TCP_CLOSE, first.stream, { reason: "eof" } satisfies TcpClose);
    expect(frameJson<TcpClose>(await first.c.next(F.TCP_CLOSE, 1)).reason).toBe("eof");
    expect((await first.c.waitClosed()).code).toBe(1000);

    // The publisher goes away → the others are closed as "going away".
    pub.close();
    await pub.waitClosed();
    for (const { c } of conns.slice(1)) expect((await c.waitClosed()).code).toBe(1001);
  });
});

describe("custom domains", () => {
  it("routes a self-hosted CUSTOM_HOSTS entry to its tunnel and lists it", async () => {
    const { c, hello } = await connect(TOKENS.root, "/connect?name=static-box");
    expect(hello.publicUrls).toEqual([`https://static-box.${DOMAIN}`, "https://api.customer.test"]);
    const pending = SELF.fetch("https://api.customer.test/orders?x=1", { headers: { accept: "application/json" } });
    const { frame, body } = await c.nextJson<RequestHead>(F.OPEN);
    expect(body.url).toBe("https://api.customer.test/orders?x=1");
    c.respond(frame.stream, 200, [["content-type", "text/plain"]], "via custom domain");
    expect(await (await pending).text()).toBe("via custom domain");

    const acct = (await (await SELF.fetch(`${RELAY}/v1/account`, { headers: { authorization: `Bearer ${TOKENS.root}` } })).json()) as {
      customDomains: { hostname: string; subdomain: string; status: string }[];
      customDomainTarget: string;
    };
    expect(acct.customDomains).toContainEqual(expect.objectContaining({ hostname: "api.customer.test", subdomain: "static-box", status: "static" }));
    expect(acct.customDomainTarget).toBe(`custom.${DOMAIN}`);
  });

  it("routes hosted custom domains (KV) only to the mapping account's tunnel", async () => {
    await env.RELAY_KV.put("sub:sam-shop", JSON.stringify({ uid: uidOf("sam") }));
    await env.RELAY_KV.put("host:shop.example.test", JSON.stringify({ sub: "sam-shop", uid: uidOf("sam") }));
    await env.RELAY_KV.put("subhosts:sam-shop", JSON.stringify(["shop.example.test"]));
    await env.RELAY_KV.put(
      `domains:${uidOf("sam")}`,
      JSON.stringify([{ hostname: "shop.example.test", subdomain: "sam-shop", status: "pending", sslStatus: "pending_validation", records: [{ type: "CNAME", name: "shop.example.test", value: `custom.${DOMAIN}`, purpose: "routing" }], errors: [], updatedAt: "2026-10-22T00:00:00.000Z" }]),
    );
    const { c, hello } = await connect(TOKENS.sam, "/connect?name=sam-shop");
    expect(hello.publicUrls).toContain("https://shop.example.test");
    const pending = SELF.fetch("https://shop.example.test/");
    const { frame } = await c.nextJson<RequestHead>(F.OPEN);
    c.respond(frame.stream, 200, [], "shop");
    expect(await (await pending).text()).toBe("shop");

    // A stale mapping that names another account never reaches this tunnel.
    await env.RELAY_KV.put("host:stale.example.test", JSON.stringify({ sub: "sam-shop", uid: "someone-else" }));
    const stale = await SELF.fetch("https://stale.example.test/");
    expect(stale.status).toBe(502);
    expect(c.has(F.OPEN)).toBe(false);

    expect((await SELF.fetch("https://unmapped.example.test/")).status).toBe(404);

    const domains = (await (await SELF.fetch(`${RELAY}/v1/domains`, { headers: { authorization: `Bearer ${TOKENS.sam}` } })).json()) as { domains: { hostname: string; status: string; records: unknown[] }[] };
    expect(domains.domains).toEqual([expect.objectContaining({ hostname: "shop.example.test", status: "pending" })]);
  });
});

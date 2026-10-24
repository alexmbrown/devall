import { env } from "cloudflare:test";
import { beforeAll, describe, expect, it } from "vitest";
import {
  AuthError,
  authenticate,
  b64urlEncode,
  HttpJwks,
  matchStaticToken,
  parseStaticTokens,
  staticPlan,
  timingSafeEqual,
  verifyFirebaseIdToken,
  type Jwk,
  type JwksSource,
} from "../src/auth";
import type { Env } from "../src/env";

const PROJECT = "devall-test";
const NOW = 1_800_000_000;

let keyPair: CryptoKeyPair;
let otherPair: CryptoKeyPair;
let jwk: Jwk;

async function makeKey(): Promise<CryptoKeyPair> {
  return (await crypto.subtle.generateKey(
    { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
    true,
    ["sign", "verify"],
  )) as CryptoKeyPair;
}

async function sign(claims: Record<string, unknown>, opts: { kid?: string; alg?: string; key?: CryptoKey } = {}): Promise<string> {
  const enc = (o: unknown) => b64urlEncode(new TextEncoder().encode(JSON.stringify(o)));
  const header = enc({ alg: opts.alg ?? "RS256", kid: opts.kid ?? "key-1", typ: "JWT" });
  const payload = enc(claims);
  const sig = new Uint8Array(await crypto.subtle.sign("RSASSA-PKCS1-v1_5", opts.key ?? keyPair.privateKey, new TextEncoder().encode(`${header}.${payload}`)));
  return `${header}.${payload}.${b64urlEncode(sig)}`;
}

function claims(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    iss: `https://securetoken.google.com/${PROJECT}`,
    aud: PROJECT,
    sub: "user-123",
    iat: NOW - 10,
    exp: NOW + 3600,
    auth_time: NOW - 100,
    email: "dev@example.com",
    ...over,
  };
}

class FakeJwks implements JwksSource {
  calls = 0;
  forced = 0;
  constructor(public current: Jwk[]) {}
  async keys(force = false): Promise<Jwk[]> {
    this.calls++;
    if (force) this.forced++;
    return this.current;
  }
}

beforeAll(async () => {
  keyPair = await makeKey();
  otherPair = await makeKey();
  const pub = (await crypto.subtle.exportKey("jwk", keyPair.publicKey)) as JsonWebKey;
  jwk = { kid: "key-1", kty: "RSA", n: pub.n!, e: pub.e!, alg: "RS256", use: "sig" };
});

describe("Firebase ID token verification", () => {
  const opts = (jwks: JwksSource) => ({ projectId: PROJECT, jwks, now: NOW });

  it("accepts a valid token and returns its claims", async () => {
    const c = await verifyFirebaseIdToken(await sign(claims({ plan: "supporter" })), opts(new FakeJwks([jwk])));
    expect(c.sub).toBe("user-123");
    expect(c.plan).toBe("supporter");
  });

  it.each([
    ["wrong audience", { aud: "someone-else" }, /audience/],
    ["wrong issuer", { iss: "https://securetoken.google.com/other" }, /issuer/],
    ["expired", { exp: NOW - 3600 }, /expired/],
    ["issued in the future", { iat: NOW + 3600 }, /future/],
    ["auth_time in the future", { auth_time: NOW + 3600 }, /auth_time/],
    ["empty subject", { sub: "" }, /subject/],
    ["over-long subject", { sub: "x".repeat(129) }, /subject/],
  ])("rejects %s", async (_name, over, re) => {
    await expect(verifyFirebaseIdToken(await sign(claims(over)), opts(new FakeJwks([jwk])))).rejects.toThrow(re);
  });

  it("tolerates small clock skew", async () => {
    await expect(verifyFirebaseIdToken(await sign(claims({ exp: NOW - 30, iat: NOW + 30 })), opts(new FakeJwks([jwk])))).resolves.toBeTruthy();
  });

  it("rejects a token signed by another key", async () => {
    const t = await sign(claims(), { key: otherPair.privateKey });
    await expect(verifyFirebaseIdToken(t, opts(new FakeJwks([jwk])))).rejects.toThrow(/signature/);
  });

  it("rejects tampered payloads", async () => {
    const [h, , s] = (await sign(claims())).split(".");
    const forged = b64urlEncode(new TextEncoder().encode(JSON.stringify(claims({ sub: "admin" }))));
    await expect(verifyFirebaseIdToken(`${h}.${forged}.${s}`, opts(new FakeJwks([jwk])))).rejects.toThrow(/signature/);
  });

  it("rejects other algorithms (alg=none / HS256 confusion)", async () => {
    const enc = (o: unknown) => b64urlEncode(new TextEncoder().encode(JSON.stringify(o)));
    const none = `${enc({ alg: "none", kid: "key-1" })}.${enc(claims())}.`;
    await expect(verifyFirebaseIdToken(none, opts(new FakeJwks([jwk])))).rejects.toThrow(/algorithm/);
    await expect(verifyFirebaseIdToken(await sign(claims(), { alg: "HS256" }), opts(new FakeJwks([jwk])))).rejects.toThrow(/algorithm/);
  });

  it("refetches the key set once for an unknown kid (rotation)", async () => {
    const jwks = new FakeJwks([]);
    const t = await sign(claims(), { kid: "key-1" });
    // First fetch has no keys; the forced refetch is still empty → unknown key.
    await expect(verifyFirebaseIdToken(t, opts(jwks))).rejects.toThrow(/unknown key/);
    expect(jwks.forced).toBe(1);
    jwks.current = [jwk];
    await expect(verifyFirebaseIdToken(t, opts(jwks))).resolves.toBeTruthy();
  });

  it("rejects malformed tokens", async () => {
    for (const t of ["", "a.b", "a.b.c.d", "!!!.e30.sig", "e30.e30.e30"]) {
      await expect(verifyFirebaseIdToken(t, opts(new FakeJwks([jwk])))).rejects.toBeInstanceOf(AuthError);
    }
  });

  it("HttpJwks caches by max-age and falls back to the last good set", async () => {
    let calls = 0;
    let fail = false;
    const fetcher = (async () => {
      calls++;
      if (fail) return new Response("nope", { status: 500 });
      return new Response(JSON.stringify({ keys: [jwk] }), { headers: { "cache-control": "public, max-age=3600" } });
    }) as unknown as typeof fetch;
    const src = new HttpJwks("https://jwks.invalid/keys", fetcher);
    expect((await src.keys())[0]!.kid).toBe("key-1");
    await src.keys();
    expect(calls).toBe(1);
    fail = true;
    expect((await src.keys(true))[0]!.kid).toBe("key-1");
    expect(calls).toBe(2);
  });
});

describe("authenticate()", () => {
  const e = (over: Partial<Env> = {}): Env => ({ ...(env as unknown as Env), AUTH_MODE: "both", FIREBASE_PROJECT_ID: PROJECT, ...over });
  const req = (token?: string) => new Request("https://relay.devall.test/v1/account", { headers: token ? { authorization: `Bearer ${token}` } : {} });

  it("maps the plan claim, with the KV entitlement taking precedence", async () => {
    const jwks = new FakeJwks([jwk]);
    const free = await authenticate(req(await sign(claims({ sub: "u-free" }))), e(), { jwks, now: NOW });
    expect(free).toMatchObject({ uid: "u-free", plan: "free", email: "dev@example.com", method: "firebase" });
    const sup = await authenticate(req(await sign(claims({ sub: "u-sup", plan: "supporter" }))), e(), { jwks, now: NOW });
    expect(sup.plan).toBe("supporter");
    await env.RELAY_KV.put("plan:u-sup", JSON.stringify({ plan: "free" }));
    const downgraded = await authenticate(req(await sign(claims({ sub: "u-sup", plan: "supporter" }))), e(), { jwks, now: NOW });
    expect(downgraded.plan).toBe("free");
    // Unknown claim values are never trusted as paid.
    const weird = await authenticate(req(await sign(claims({ sub: "u-x", plan: "selfhost" }))), e(), { jwks, now: NOW });
    expect(weird.plan).toBe("free");
  });

  it("refuses blocked accounts", async () => {
    await env.RELAY_KV.put("blocked-user:u-bad", "1");
    await expect(authenticate(req(await sign(claims({ sub: "u-bad" }))), e(), { jwks: new FakeJwks([jwk]), now: NOW })).rejects.toMatchObject({ status: 451 });
  });

  it("accepts static tokens in static/both mode only", async () => {
    const p = await authenticate(req("root-token-0123456789xx"), e());
    expect(p).toMatchObject({ uid: "static-root", plan: "selfhost", method: "static" });
    const free = await authenticate(req("alice-token-0123456789"), e());
    expect(free.plan).toBe("free");
    await expect(authenticate(req("root-token-0123456789xx"), e({ AUTH_MODE: "firebase" }), { jwks: new FakeJwks([jwk]) })).rejects.toBeInstanceOf(AuthError);
    await expect(authenticate(req("wrong-token-0123456789"), e({ AUTH_MODE: "static" }))).rejects.toThrow(/invalid token/);
    await expect(authenticate(req(), e())).rejects.toThrow(/missing bearer/);
  });

  it("parses static token lists and plans", async () => {
    expect(parseStaticTokens("a:0123456789abcdef, 0123456789abcdefgh\nshort")).toEqual([
      { name: "a", token: "0123456789abcdef" },
      { name: null, token: "0123456789abcdefgh" },
    ]);
    expect(await matchStaticToken("a:0123456789abcdef", "0123456789abcdef")).toEqual({ uid: "static-a", name: "a" });
    const anon = await matchStaticToken("0123456789abcdefgh", "0123456789abcdefgh");
    expect(anon?.uid).toMatch(/^static-[0-9a-f]{12}$/);
    expect(await matchStaticToken("a:0123456789abcdef", "0123456789abcdeX")).toBeNull();
    expect(staticPlan(e({ STATIC_PLANS: '{"a":"free"}' }), "a")).toBe("free");
    expect(staticPlan(e({ STATIC_PLANS: undefined, STATIC_PLAN: undefined }), "a")).toBe("selfhost");
    expect(timingSafeEqual("abc", "abc")).toBe(true);
    expect(timingSafeEqual("abc", "abd")).toBe(false);
    expect(timingSafeEqual("abc", "abcd")).toBe(false);
  });
});

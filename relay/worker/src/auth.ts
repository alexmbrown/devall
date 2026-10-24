// Authentication for client sockets and the account API.
//
// Hosted: Firebase ID tokens (RS256) verified against Google's securetoken JWKS with WebCrypto —
// no Firebase SDK in the Worker. Self-hosted: static bearer tokens from the RELAY_TOKENS secret.

import { authModes, type Env } from "./env";
import { isPlan, planFromClaim, type Plan } from "./plans";

export const GOOGLE_SECURETOKEN_JWKS = "https://www.googleapis.com/service_accounts/v1/jwk/securetoken@system.gserviceaccount.com";

export interface Principal {
  uid: string;
  plan: Plan;
  email: string | null;
  method: "firebase" | "static";
}

export class AuthError extends Error {
  override name = "AuthError";
  constructor(
    message: string,
    readonly status = 401,
    readonly code = "unauthorized",
  ) {
    super(message);
  }
}

export interface Jwk {
  kid: string;
  kty: string;
  n: string;
  e: string;
  alg?: string;
  use?: string;
}

export interface JwksSource {
  /** Current keys; `force` bypasses caches (key rotation: unknown `kid`). */
  keys(force?: boolean): Promise<Jwk[]>;
}

export interface FirebaseClaims {
  sub: string;
  aud: string;
  iss: string;
  iat: number;
  exp: number;
  auth_time?: number;
  email?: string;
  plan?: unknown;
  [k: string]: unknown;
}

// ---------- base64url / JSON helpers ----------

export function b64urlDecode(s: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]*$/.test(s)) throw new AuthError("malformed token");
  const pad = s.length % 4 === 2 ? "==" : s.length % 4 === 3 ? "=" : s.length % 4 === 1 ? null : "";
  if (pad === null) throw new AuthError("malformed token");
  const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/") + pad);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function b64urlEncode(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function decodeJsonPart(part: string): Record<string, unknown> {
  try {
    const v = JSON.parse(new TextDecoder().decode(b64urlDecode(part))) as unknown;
    if (v && typeof v === "object" && !Array.isArray(v)) return v as Record<string, unknown>;
  } catch {
    // fall through
  }
  throw new AuthError("malformed token");
}

// ---------- JWKS ----------

/** Google's JWKS with an in-isolate cache that honours `Cache-Control: max-age`. */
export class HttpJwks implements JwksSource {
  private cache: { keys: Jwk[]; until: number } | null = null;
  constructor(
    private readonly url: string,
    private readonly fetcher: typeof fetch = fetch,
  ) {}

  async keys(force = false): Promise<Jwk[]> {
    const now = Date.now();
    if (!force && this.cache && this.cache.until > now) return this.cache.keys;
    const res = await this.fetcher(this.url, { headers: { accept: "application/json" } });
    if (!res.ok) {
      if (this.cache) return this.cache.keys; // keep serving the last good set during a blip
      throw new AuthError(`could not fetch signing keys (${res.status})`, 503, "auth_unavailable");
    }
    const body = (await res.json()) as { keys?: Jwk[] };
    const keys = Array.isArray(body.keys) ? body.keys.filter((k) => k && typeof k.kid === "string") : [];
    const maxAge = /max-age=(\d+)/.exec(res.headers.get("cache-control") ?? "")?.[1];
    const ttl = Math.min(Math.max(Number(maxAge ?? 3600), 60), 6 * 3600) * 1000;
    this.cache = { keys, until: now + ttl };
    return keys;
  }
}

const jwksByUrl = new Map<string, HttpJwks>();

export function defaultJwks(env: Env): JwksSource {
  const url = env.FIREBASE_JWKS_URL || GOOGLE_SECURETOKEN_JWKS;
  let j = jwksByUrl.get(url);
  if (!j) {
    j = new HttpJwks(url);
    jwksByUrl.set(url, j);
  }
  return j;
}

const keyCache = new Map<string, CryptoKey>();

async function importRsaKey(jwk: Jwk): Promise<CryptoKey> {
  const cacheKey = `${jwk.kid}:${jwk.n.slice(0, 32)}`;
  const hit = keyCache.get(cacheKey);
  if (hit) return hit;
  const key = await crypto.subtle.importKey(
    "jwk",
    { kty: "RSA", n: jwk.n, e: jwk.e, alg: "RS256", ext: true },
    { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
    false,
    ["verify"],
  );
  if (keyCache.size > 32) keyCache.clear();
  keyCache.set(cacheKey, key);
  return key;
}

// ---------- Firebase ID token ----------

export interface VerifyOptions {
  projectId: string;
  jwks: JwksSource;
  /** Seconds since the epoch (tests). */
  now?: number;
  clockSkewSeconds?: number;
}

/**
 * Verifies a Firebase ID token as documented by Firebase ("Verify ID tokens using a third-party
 * JWT library"): RS256, `kid` from the securetoken JWKS, `aud` = project id,
 * `iss` = https://securetoken.google.com/<project id>, `exp` in the future, `iat`/`auth_time`
 * in the past, non-empty `sub` ≤ 128 chars.
 */
export async function verifyFirebaseIdToken(token: string, opts: VerifyOptions): Promise<FirebaseClaims> {
  if (!opts.projectId) throw new AuthError("Firebase project id is not configured", 500, "misconfigured");
  const parts = token.split(".");
  if (parts.length !== 3) throw new AuthError("malformed token");
  const [h, p, s] = parts as [string, string, string];
  const header = decodeJsonPart(h);
  if (header.alg !== "RS256") throw new AuthError("unexpected token algorithm");
  if (typeof header.kid !== "string" || !header.kid) throw new AuthError("token has no key id");
  const claims = decodeJsonPart(p) as FirebaseClaims;

  const now = opts.now ?? Math.floor(Date.now() / 1000);
  const skew = opts.clockSkewSeconds ?? 60;
  if (claims.aud !== opts.projectId) throw new AuthError("token audience mismatch");
  if (claims.iss !== `https://securetoken.google.com/${opts.projectId}`) throw new AuthError("token issuer mismatch");
  if (typeof claims.exp !== "number" || claims.exp <= now - skew) throw new AuthError("token expired");
  if (typeof claims.iat !== "number" || claims.iat > now + skew) throw new AuthError("token issued in the future");
  if (claims.auth_time !== undefined && (typeof claims.auth_time !== "number" || claims.auth_time > now + skew)) {
    throw new AuthError("token auth_time in the future");
  }
  if (typeof claims.sub !== "string" || !claims.sub || claims.sub.length > 128) throw new AuthError("token subject invalid");

  let keys = await opts.jwks.keys();
  let jwk = keys.find((k) => k.kid === header.kid);
  if (!jwk) {
    keys = await opts.jwks.keys(true);
    jwk = keys.find((k) => k.kid === header.kid);
  }
  if (!jwk || jwk.kty !== "RSA") throw new AuthError("token signed with an unknown key");
  const key = await importRsaKey(jwk);
  const signed = new TextEncoder().encode(`${h}.${p}`);
  const ok = await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key, b64urlDecode(s), signed);
  if (!ok) throw new AuthError("token signature invalid");
  return claims;
}

// ---------- static tokens (self-host) ----------

/** Constant-time comparison of two strings (length leaks, content doesn't). */
export function timingSafeEqual(a: string, b: string): boolean {
  const ea = new TextEncoder().encode(a);
  const eb = new TextEncoder().encode(b);
  let diff = ea.length ^ eb.length;
  const n = Math.max(ea.length, eb.length);
  for (let i = 0; i < n; i++) diff |= (ea[i] ?? 0) ^ (eb[i] ?? 0);
  return diff === 0;
}

async function sha256Hex(s: string): Promise<string> {
  const d = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)));
  return [...d].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** Parses `RELAY_TOKENS`: comma/newline separated `token` or `name:token` entries. */
export function parseStaticTokens(raw: string | undefined): { name: string | null; token: string }[] {
  if (!raw) return [];
  return raw
    .split(/[,\n]/)
    .map((s) => s.trim())
    .filter(Boolean)
    .map((entry) => {
      const i = entry.indexOf(":");
      if (i > 0 && /^[A-Za-z0-9_-]{1,32}$/.test(entry.slice(0, i))) return { name: entry.slice(0, i), token: entry.slice(i + 1) };
      return { name: null, token: entry };
    })
    .filter((t) => t.token.length >= 16);
}

export async function matchStaticToken(raw: string | undefined, presented: string): Promise<{ uid: string; name: string | null } | null> {
  let found: { uid: string; name: string | null } | null = null;
  for (const t of parseStaticTokens(raw)) {
    // Compare against every entry so timing doesn't reveal which one matched.
    if (timingSafeEqual(t.token, presented) && !found) {
      found = { uid: t.name ? `static-${t.name}` : `static-${(await sha256Hex(t.token)).slice(0, 12)}`, name: t.name };
    }
  }
  return found;
}

/** Plan for a static-token user: `STATIC_PLANS[name]`, else `STATIC_PLAN`, else `selfhost`. */
export function staticPlan(env: Env, name: string | null): Plan {
  if (name && env.STATIC_PLANS) {
    try {
      const map = JSON.parse(env.STATIC_PLANS) as Record<string, unknown>;
      const p = map[name];
      if (isPlan(p)) return p;
    } catch {
      // ignore malformed config
    }
  }
  return isPlan(env.STATIC_PLAN) ? env.STATIC_PLAN : "selfhost";
}

// ---------- request authentication ----------

export function bearerToken(request: Request): string | null {
  const h = request.headers.get("authorization");
  if (!h) return null;
  const m = /^Bearer\s+(.+)$/i.exec(h.trim());
  return m ? m[1]!.trim() : null;
}

export interface AuthDeps {
  jwks?: JwksSource;
  now?: number;
}

/**
 * Resolves the caller. Plan precedence: KV entitlement (`plan:<uid>`, written by the billing
 * function, so upgrades/cancellations apply before the 1 h token refresh) → `plan` custom claim →
 * free. Static-token users get `STATIC_PLANS[name]` / `STATIC_PLAN` (default `selfhost`).
 */
export async function authenticate(request: Request, env: Env, deps: AuthDeps = {}): Promise<Principal> {
  const token = bearerToken(request);
  if (!token) throw new AuthError("missing bearer token");
  const modes = authModes(env);

  if (modes.static && !token.includes(".")) {
    const hit = await matchStaticToken(env.RELAY_TOKENS, token);
    if (hit) return { uid: hit.uid, plan: staticPlan(env, hit.name), email: null, method: "static" };
    if (!modes.firebase) throw new AuthError("invalid token");
  }
  if (!modes.firebase) throw new AuthError("invalid token");

  const claims = await verifyFirebaseIdToken(token, {
    projectId: env.FIREBASE_PROJECT_ID ?? "",
    jwks: deps.jwks ?? defaultJwks(env),
    now: deps.now,
  });
  if (await env.RELAY_KV?.get(`blocked-user:${claims.sub}`)) throw new AuthError("account blocked", 451, "blocked");
  let plan = planFromClaim(claims.plan);
  const ent = await env.RELAY_KV?.get<{ plan?: string }>(`plan:${claims.sub}`, "json");
  if (ent && (ent.plan === "free" || ent.plan === "supporter")) plan = ent.plan;
  return { uid: claims.sub, plan, email: typeof claims.email === "string" ? claims.email : null, method: "firebase" };
}

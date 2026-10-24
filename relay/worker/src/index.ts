// /dev/all relay — Cloudflare Worker entry. Routes by host:
//
//   relay.<domain>          client sockets (/connect, /inbox, /tcp), account API, abuse report page, admin
//   hooks.<domain>          hosted webhook inboxes → InboxDO
//   share.<domain>/<id>     shared request/response snapshots → ShareDO
//   <name>.<mock domain>    hosted mocks → MockDO (also mock.<domain>/<name>/…)
//   <sub>.<domain>          public tunnel traffic → TunnelDO
//   any other host          a custom domain (CUSTOM_HOSTS or KV host:<h>) → that tunnel's TunnelDO
//   localhost / 127.0.0.1   the relay routes (local development with `wrangler dev`)
//
// Cloud load runs (`/v1/load/runs`) fan out to LoadDO objects placed with a locationHint.
//
// A cron trigger (every 5 minutes) arms due scheduled monitors (MonitorDO).
//
// Copyright the /dev/all authors. Licensed under the GNU AGPL-3.0 (see relay/LICENSE).

import { DEFAULT_INBOX_KEY } from "./account";
import { apiRoutes } from "./api";
import { AuthError, authenticate, bearerToken, timingSafeEqual, type Principal } from "./auth";
import { customDomainsFor, resolveCustomHost } from "./customhosts";
import { customDomainTarget, domainOf, inboxUrl, mockDomain, mockUrl, tunnelUrl, type Env } from "./env";
import { clientIp, isWebSocketUpgrade, json, jsonError, stripInternal } from "./http";
import { scheduledMonitors } from "./monitor";
import { checkClaimableSubdomain, checkSubdomainSyntax, classifyHost, isRandomStyle, isValidInboxId, isValidProjectKey, isValidShareId, randomSubdomain } from "./names";
import { configurePages, landingPage, problem, reportPage } from "./pages";
import { limitsFor } from "./plans";
import { negotiateVersion, SUPPORTED_VERSIONS } from "./protocol";
import { gone } from "./share";

export { AccountDO } from "./account";
export { InboxDO } from "./inbox";
export { LoadDO } from "./load";
export { MockDO } from "./mock";
export { MonitorDO } from "./monitor";
export { ShareDO } from "./share";
export { TunnelDO } from "./tunnel";

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    configurePages(env);
    const url = new URL(request.url);
    const domain = domainOf(env);
    try {
      const host = classifyHost(url.hostname, domain, mockDomain(env));
      switch (host.kind) {
        case "relay":
          return await relayRoutes(request, url, env);
        case "hooks":
          return await hooksRoute(request, url, env);
        case "share":
          return await shareRoute(request, url, env);
        case "mock":
          return await env.MOCK.get(env.MOCK.idFromName(host.name)).fetch(stripInternal(request));
        case "mockPath":
          return await mockPathRoute(request, url, env);
        case "tunnel":
          return await env.TUNNEL.get(env.TUNNEL.idFromName(host.sub)).fetch(stripInternal(request));
        case "custom":
          return await customHostRoute(request, host.host, env);
        case "apex":
          return landingPage(domain);
        default:
          return new Response("Not found\n", { status: 404 });
      }
    } catch (e) {
      if (e instanceof AuthError) return jsonError(e.status, e.code, e.message);
      console.error("relay error", e);
      return jsonError(500, "internal", "Internal error");
    }
  },

  async scheduled(controller: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {
    ctx.waitUntil(scheduledMonitors(env, controller.scheduledTime).then(() => undefined));
  },
} satisfies ExportedHandler<Env>;

async function shareRoute(request: Request, url: URL, env: Env): Promise<Response> {
  const id = url.pathname.split("/")[1] ?? "";
  if (url.pathname === "/" || url.pathname === "") return landingPage(domainOf(env));
  if (!isValidShareId(id)) return gone();
  return env.SHARE.get(env.SHARE.idFromName(id)).fetch(stripInternal(request));
}

/** `mock.<domain>/<name>/…` — path-style hosted mocks (covered by the zone's Universal SSL). */
async function mockPathRoute(request: Request, url: URL, env: Env): Promise<Response> {
  const name = (url.pathname.split("/")[1] ?? "").toLowerCase();
  if (!name) return landingPage(domainOf(env));
  if (!checkSubdomainSyntax(name).ok) return new Response("No hosted mock is published at this address.\n", { status: 404 });
  const clean = stripInternal(request);
  const headers = new Headers(clean.headers);
  headers.set("x-devall-mock-prefix", `/${name}`);
  return env.MOCK.get(env.MOCK.idFromName(name)).fetch(new Request(clean, { headers }));
}

/** A customer's hostname (CNAME → the relay): route to the mapped tunnel, only for its owner's tunnel. */
async function customHostRoute(request: Request, host: string, env: Env): Promise<Response> {
  const target = await resolveCustomHost(env, host);
  if (!target) return problem(request, 404, "Unknown host", `${host} isn't connected to a /dev/all tunnel.`);
  const clean = stripInternal(request);
  if (!target.uid) return env.TUNNEL.get(env.TUNNEL.idFromName(target.sub)).fetch(clean);
  const headers = new Headers(clean.headers);
  headers.set("x-devall-custom-uid", target.uid);
  return env.TUNNEL.get(env.TUNNEL.idFromName(target.sub)).fetch(new Request(clean, { headers }));
}

async function relayRoutes(request: Request, url: URL, env: Env): Promise<Response> {
  const path = url.pathname.replace(/\/+$/, "") || "/";
  switch (path) {
    case "/":
      return landingPage(domainOf(env));
    case "/healthz":
      return new Response("ok\n");
    case "/report":
      return reportPage(url.searchParams.get("host"), env.ABUSE_EMAIL || `abuse@${domainOf(env)}`);
    case "/connect":
      return connectTunnel(request, url, env);
    case "/inbox":
      return connectInbox(request, url, env);
    case "/tcp":
      return connectTcp(request, url, env);
    case "/v1/account":
      return account(request, env);
  }
  if (path.startsWith("/admin/")) return admin(request, url, path, env);
  const api = await apiRoutes(request, url, path, env);
  if (api) return api;
  return jsonError(404, "not_found", "Not found");
}

/** Shared checks for client sockets: upgrade, connect rate limit, version, auth. */
async function preflight(request: Request, env: Env): Promise<{ principal: Principal; version: number } | Response> {
  if (!isWebSocketUpgrade(request)) return jsonError(426, "upgrade_required", "Connect with a WebSocket (wss://).");
  if (env.CONNECT_LIMITER) {
    const { success } = await env.CONNECT_LIMITER.limit({ key: clientIp(request) ?? "unknown" });
    if (!success) return jsonError(429, "rate_limited", "Too many connection attempts; slow down.", {}, { "retry-after": "60" });
  }
  const version = negotiateVersion(request.headers.get("sec-websocket-protocol"));
  if (version === null) {
    return jsonError(426, "unsupported_version", "This relay speaks protocol versions " + SUPPORTED_VERSIONS.join(", ") + "; update /dev/all.", {
      supported: SUPPORTED_VERSIONS,
    });
  }
  const principal = await authenticate(request, env);
  return { principal, version };
}

/** Request forwarded to a Durable Object's connect handler (auth header removed, metadata added). */
function internalConnect(request: Request, meta: Record<string, string>): Request {
  const headers = new Headers(request.headers);
  for (const k of [...headers.keys()]) if (k.toLowerCase().startsWith("x-devall-")) headers.delete(k);
  headers.delete("authorization");
  headers.set("x-devall-control", "connect");
  for (const [k, v] of Object.entries(meta)) headers.set(`x-devall-${k}`, v);
  return new Request(request.url, { method: "GET", headers });
}

async function connectTunnel(request: Request, url: URL, env: Env): Promise<Response> {
  const pre = await preflight(request, env);
  if (pre instanceof Response) return pre;
  const { principal: p, version } = pre;
  const requested = url.searchParams.get("name")?.trim().toLowerCase() || null;
  const kind = url.searchParams.get("kind") === "tcp" ? "tcp" : "http";
  if (kind === "tcp" && !limitsFor(p.plan, env).tcpTunnels) {
    return jsonError(403, "plan_required", "Raw TCP tunnels are part of the Supporter plan (or self-host the relay for free).");
  }

  const attempt = (sub: string, mode: "fresh" | "resume" | "reserved" | "selfhost") =>
    env.TUNNEL.get(env.TUNNEL.idFromName(sub)).fetch(
      internalConnect(request, { uid: p.uid, plan: p.plan, sub, mode, version: String(version), kind }),
    );

  if (requested) {
    const syntax = checkSubdomainSyntax(requested);
    if (!syntax.ok) return jsonError(400, "invalid_subdomain", syntax.reason);
    if (isRandomStyle(requested)) {
      // Reconnecting after a drop: keep the same random URL if the session is still ours.
      const res = await attempt(requested, "resume");
      if (res.status !== 409) return res;
    } else if (p.plan === "selfhost") {
      const ok = checkClaimableSubdomain(requested);
      if (!ok.ok) return jsonError(400, "invalid_subdomain", ok.reason);
      return attempt(requested, "selfhost");
    } else if (p.plan === "supporter") {
      const owner = await env.RELAY_KV.get<{ uid?: string }>(`sub:${requested}`, "json");
      if (!owner || owner.uid !== p.uid) {
        return jsonError(403, "subdomain_not_reserved", `"${requested}" isn't one of your reserved subdomains. Reserve it in /dev/all first.`);
      }
      return attempt(requested, "reserved");
    } else {
      return jsonError(403, "plan_required", "Choosing a subdomain needs the Supporter plan (or a self-hosted relay); free tunnels get a random name.");
    }
  }
  for (let i = 0; i < 4; i++) {
    const res = await attempt(randomSubdomain(), "fresh");
    if (res.status !== 409) return res;
  }
  return jsonError(503, "no_subdomain", "Couldn't allocate a subdomain; try again.", {}, { "retry-after": "5" });
}

async function connectInbox(request: Request, url: URL, env: Env): Promise<Response> {
  const pre = await preflight(request, env);
  if (pre instanceof Response) return pre;
  const { principal: p, version } = pre;
  const limits = limitsFor(p.plan, env);
  let key = url.searchParams.get("project")?.trim() || DEFAULT_INBOX_KEY;
  if (key !== DEFAULT_INBOX_KEY && !isValidProjectKey(key)) return jsonError(400, "invalid_project", "project must be 1–64 characters of A-Z a-z 0-9 _ -");
  if (limits.maxInboxes === 1) key = DEFAULT_INBOX_KEY; // free: one inbox per account
  const r = await env.ACCOUNT.get(env.ACCOUNT.idFromName(p.uid)).allocateInbox(key, limits.maxInboxes);
  if (!r.ok) return jsonError(429, r.code, r.message);
  return env.INBOX.get(env.INBOX.idFromName(r.inboxId)).fetch(
    internalConnect(request, { uid: p.uid, plan: p.plan, version: String(version), inbox: r.inboxId, project: key }),
  );
}

/**
 * `wss://relay.<domain>/tcp?name=<sub>` — a consumer opening one TCP connection through a TCP
 * tunnel. Authorization is the tunnel owner's account token, or `Bearer dtcp_…` (the tunnel's
 * access secret; only its SHA-256 reaches the tunnel object).
 */
async function connectTcp(request: Request, url: URL, env: Env): Promise<Response> {
  if (!isWebSocketUpgrade(request)) return jsonError(426, "upgrade_required", "Connect with a WebSocket (wss://).");
  if (env.CONNECT_LIMITER) {
    const { success } = await env.CONNECT_LIMITER.limit({ key: clientIp(request) ?? "unknown" });
    if (!success) return jsonError(429, "rate_limited", "Too many connection attempts; slow down.", {}, { "retry-after": "60" });
  }
  const version = negotiateVersion(request.headers.get("sec-websocket-protocol"));
  if (version === null) return jsonError(426, "unsupported_version", "Update /dev/all.", { supported: SUPPORTED_VERSIONS });
  const name = url.searchParams.get("name")?.trim().toLowerCase() ?? "";
  if (!checkSubdomainSyntax(name).ok) return jsonError(400, "invalid_subdomain", "name must be the TCP tunnel's subdomain");
  const token = bearerToken(request);
  const meta: Record<string, string> = { version: String(version), "client-ip": clientIp(request) ?? "" };
  if (token?.startsWith("dtcp_")) {
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token)));
    meta["secret-sha256"] = [...digest].map((b) => b.toString(16).padStart(2, "0")).join("");
  } else {
    meta.uid = (await authenticate(request, env)).uid;
  }
  const headers = new Headers(internalConnect(request, meta).headers);
  headers.set("x-devall-control", "tcp-connect");
  return env.TUNNEL.get(env.TUNNEL.idFromName(name)).fetch(new Request(request.url, { method: "GET", headers }));
}

async function hooksRoute(request: Request, url: URL, env: Env): Promise<Response> {
  const m = /^\/([a-z0-9]{16,40})(\/.*)?$/.exec(url.pathname);
  if (!m || !isValidInboxId(m[1]!)) {
    if (url.pathname === "/" || url.pathname === "") return landingPage(domainOf(env));
    return jsonError(404, "unknown_inbox", "No webhook inbox at this address.");
  }
  if (isWebSocketUpgrade(request)) return jsonError(400, "bad_request", "Webhook inboxes don't accept WebSockets.");
  const clean = stripInternal(request);
  const headers = new Headers(clean.headers);
  headers.set("x-devall-control", "capture");
  headers.set("x-devall-inbox-path", (m[2] ?? "/") + url.search);
  return env.INBOX.get(env.INBOX.idFromName(m[1]!)).fetch(new Request(clean, { headers }));
}

async function account(request: Request, env: Env): Promise<Response> {
  if (request.method !== "GET") return jsonError(405, "method_not_allowed", "Use GET");
  const p = await authenticate(request, env);
  const limits = limitsFor(p.plan, env);
  const summary = await env.ACCOUNT.get(env.ACCOUNT.idFromName(p.uid)).summary();
  const reserved = (await env.RELAY_KV.get<string[]>(`subs:${p.uid}`, "json")) ?? [];
  const [customDomains, mocks] = await Promise.all([customDomainsFor(env, p.uid, p.method === "static" && p.plan === "selfhost"), env.ACCOUNT.get(env.ACCOUNT.idFromName(p.uid)).listMocks()]);
  return json({
    uid: p.uid,
    email: p.email,
    plan: p.plan,
    selfHosted: p.method === "static",
    domain: domainOf(env),
    limits,
    usage: summary.usage,
    tunnels: summary.tunnels.map((t) => ({ subdomain: t.sub, url: tunnelUrl(env, t.sub), since: new Date(t.since).toISOString() })),
    inboxes: summary.inboxes.map((i) => ({ projectKey: i.projectKey, inboxId: i.inboxId, url: inboxUrl(env, i.inboxId) })),
    reservedSubdomains: Array.isArray(reserved) ? reserved.filter((s) => typeof s === "string") : [],
    upgradeUrl: env.UPGRADE_URL || null,
    customDomains,
    customDomainTarget: customDomainTarget(env),
    hostedMocks: mocks.map((m) => ({ name: m.name, url: mockUrl(env, m.name) })),
  });
}

/** Takedown endpoints for the operator (bearer ADMIN_TOKEN). */
async function admin(request: Request, url: URL, path: string, env: Env): Promise<Response> {
  const token = bearerToken(request);
  if (!env.ADMIN_TOKEN || !token || !timingSafeEqual(token, env.ADMIN_TOKEN)) return jsonError(401, "unauthorized", "admin token required");
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "Use POST");
  const name = url.searchParams.get("name")?.toLowerCase() ?? "";
  const uid = url.searchParams.get("uid") ?? "";
  switch (path) {
    case "/admin/block":
      if (!checkSubdomainSyntax(name).ok) return jsonError(400, "invalid_subdomain", "name required");
      await env.TUNNEL.get(env.TUNNEL.idFromName(name)).block(url.searchParams.get("reason") ?? "abuse");
      return json({ ok: true, blocked: name });
    case "/admin/unblock":
      if (!checkSubdomainSyntax(name).ok) return jsonError(400, "invalid_subdomain", "name required");
      await env.TUNNEL.get(env.TUNNEL.idFromName(name)).unblock();
      return json({ ok: true, unblocked: name });
    case "/admin/block-user":
      if (!uid) return jsonError(400, "bad_request", "uid required");
      await env.RELAY_KV.put(`blocked-user:${uid}`, JSON.stringify({ at: Date.now(), reason: url.searchParams.get("reason") ?? "abuse" }));
      return json({ ok: true, blockedUser: uid });
    case "/admin/unblock-user":
      if (!uid) return jsonError(400, "bad_request", "uid required");
      await env.RELAY_KV.delete(`blocked-user:${uid}`);
      return json({ ok: true, unblockedUser: uid });
    case "/admin/load/disable":
      // Kill switch: refuses new cloud load runs and stops running ones at their next segment.
      await env.RELAY_KV.put("load:disabled", JSON.stringify({ at: Date.now(), reason: url.searchParams.get("reason") ?? "operator" }));
      return json({ ok: true, cloudLoad: "disabled" });
    case "/admin/load/enable":
      await env.RELAY_KV.delete("load:disabled");
      return json({ ok: true, cloudLoad: (env.CLOUD_LOAD ?? "on").toLowerCase() === "off" ? "disabled (CLOUD_LOAD = off)" : "enabled" });
    default:
      return problem(request, 404, "Not found", "Unknown admin endpoint");
  }
}

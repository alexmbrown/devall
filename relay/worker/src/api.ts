// Account API for the hosted extras (Batch D): hosted mocks, shared snapshots, monitors and custom
// domains. Everything requires a signed-in account (Firebase ID token) or a self-host token —
// nothing here is anonymous.

import { authenticate, type Principal } from "./auth";
import { customDomainsFor } from "./customhosts";
import { customDomainTarget, mockHostUrl, mockPathUrl, mockUrl, shareUrl, type Env } from "./env";
import { json, jsonError, readBodyCapped } from "./http";
import { loadRoutes } from "./load";
import type { MonitorInput } from "./monitor";
import { checkClaimableSubdomain, isValidMonitorId, isValidShareId, randomId, randomInboxId } from "./names";
import { limitsFor } from "./plans";
import { snapshotBytes, validateSnapshot, type Snapshot } from "./share";

const MiB = 1024 * 1024;

async function readJson(request: Request, max: number): Promise<{ ok: true; value: unknown } | { ok: false; res: Response }> {
  const body = await readBodyCapped(request.body, max);
  if (body === null) return { ok: false, res: jsonError(413, "too_large", `Request body is larger than ${max} bytes.`) };
  try {
    return { ok: true, value: JSON.parse(new TextDecoder().decode(body)) as unknown };
  } catch {
    return { ok: false, res: jsonError(400, "bad_request", "Body must be JSON.") };
  }
}

function account(env: Env, uid: string) {
  return env.ACCOUNT.get(env.ACCOUNT.idFromName(uid));
}

/** Routes `/v1/mocks…`, `/v1/shares…`, `/v1/monitors…`, `/v1/load…`, `/v1/domains`. Returns null when the path isn't one of them. */
export async function apiRoutes(request: Request, url: URL, path: string, env: Env): Promise<Response | null> {
  const seg = path.split("/").filter(Boolean); // ["v1", resource, id?, sub?]
  if (seg[0] !== "v1" || !["mocks", "shares", "monitors", "domains", "load"].includes(seg[1] ?? "")) return null;
  const p = await authenticate(request, env);
  switch (seg[1]) {
    case "mocks":
      return mocks(request, url, seg.slice(2), p, env);
    case "shares":
      return shares(request, seg.slice(2), p, env);
    case "monitors":
      return monitors(request, url, seg.slice(2), p, env);
    case "load":
      return loadRoutes(request, url, seg.slice(2), p, env);
    case "domains":
      if (request.method !== "GET") return jsonError(405, "method_not_allowed", "Custom domains are managed by the hosted billing functions (addCustomDomain) or CUSTOM_HOSTS on a self-hosted relay.");
      return json({ target: customDomainTarget(env), domains: await customDomainsFor(env, p.uid, p.method === "static" && p.plan === "selfhost") });
  }
  return null;
}

// ---------- hosted mocks ----------

async function mocks(request: Request, url: URL, rest: string[], p: Principal, env: Env): Promise<Response> {
  const limits = limitsFor(p.plan, env);
  const view = (name: string) => ({ url: mockUrl(env, name), hostUrl: mockHostUrl(env, name), pathUrl: mockPathUrl(env, name) });
  if (rest.length === 0) {
    if (request.method !== "GET") return jsonError(405, "method_not_allowed", "Use GET");
    const list = await account(env, p.uid).listMocks();
    const infos = await Promise.all(list.map(async (m) => ({ ...(await env.MOCK.get(env.MOCK.idFromName(m.name)).info(p.uid)), ...view(m.name) })));
    return json({ mocks: infos.filter((i) => "name" in i && i.name), limits: { hostedMocks: limits.hostedMocks, mockMaxStubs: limits.mockMaxStubs, mockMaxBytes: limits.mockMaxBytes, mockRequestsPerDay: limits.mockRequestsPerDay } });
  }
  const name = rest[0]!.toLowerCase();
  const check = checkClaimableSubdomain(name);
  if (!check.ok) return jsonError(400, "invalid_name", `Mock name: ${check.reason}`);
  const stub = env.MOCK.get(env.MOCK.idFromName(name));
  const sub = rest[1] ?? "";
  if (sub === "" && request.method === "PUT") {
    if (limits.hostedMocks === 0) return jsonError(403, "plan_required", "Hosted mocks aren't included in your plan.");
    const max = Math.ceil((limits.mockMaxBytes ?? 100 * MiB) * 1.4) + MiB;
    const body = await readJson(request, max);
    if (!body.ok) return body.res;
    const v = body.value as { mappings?: unknown; files?: unknown; settings?: unknown };
    const reg = await account(env, p.uid).registerMock(name, limits.hostedMocks);
    if (!reg.ok) return jsonError(429, "too_many_mocks", reg.message);
    const res = await stub.publish({
      uid: p.uid,
      plan: p.plan,
      name,
      mappings: Array.isArray(v.mappings) ? v.mappings : [],
      files: v.files && typeof v.files === "object" && !Array.isArray(v.files) ? (v.files as Record<string, string>) : {},
      settings: v.settings && typeof v.settings === "object" ? (v.settings as { cors?: boolean; globalTemplating?: boolean }) : undefined,
    });
    if (!res.ok) {
      // Don't leave a registration behind for a name that never got published.
      if (!(await stub.info(p.uid))) await account(env, p.uid).removeMock(name);
      return jsonError(res.status, res.code, res.message);
    }
    return json({ name, ...view(name), stubs: res.stubs, bytes: res.bytes, files: res.files, warnings: res.warnings, createdAt: new Date(res.createdAt).toISOString(), updatedAt: new Date(res.updatedAt).toISOString() });
  }
  if (sub === "" && request.method === "GET") {
    const info = await stub.info(p.uid);
    return info ? json({ ...info, ...view(name) }) : jsonError(404, "not_found", "No such hosted mock.");
  }
  if (sub === "" && request.method === "DELETE") {
    const ok = await stub.unpublish(p.uid);
    await account(env, p.uid).removeMock(name);
    return ok ? json({ ok: true, unpublished: name }) : jsonError(404, "not_found", "No such hosted mock.");
  }
  if (sub === "requests" && request.method === "GET") {
    const limit = Number(url.searchParams.get("limit") ?? "50");
    const entries = await stub.journal(p.uid, limit);
    return entries ? json({ requests: entries }) : jsonError(404, "not_found", "No such hosted mock.");
  }
  if (sub === "requests" && request.method === "DELETE") {
    return (await stub.clearJournal(p.uid)) ? json({ ok: true }) : jsonError(404, "not_found", "No such hosted mock.");
  }
  if (sub === "scenarios" && rest[2] === "reset" && request.method === "POST") {
    return (await stub.resetScenarios(p.uid)) ? json({ ok: true }) : jsonError(404, "not_found", "No such hosted mock.");
  }
  return jsonError(404, "not_found", "Not found");
}

// ---------- shared snapshots ----------

async function shares(request: Request, rest: string[], p: Principal, env: Env): Promise<Response> {
  const limits = limitsFor(p.plan, env);
  const acct = account(env, p.uid);
  if (rest.length === 0 && request.method === "GET") {
    const list = await acct.listShares();
    return json({
      shares: list.map((s) => ({ id: s.id, title: s.title, url: shareUrl(env, s.id), createdAt: new Date(s.createdAt).toISOString(), expiresAt: new Date(s.expiresAt).toISOString(), bytes: s.bytes })),
      limits: { shares: limits.shares, shareMaxBytes: limits.shareMaxBytes, shareTtlSeconds: limits.shareTtlSeconds },
    });
  }
  if (rest.length === 0 && request.method === "POST") {
    const body = await readJson(request, limits.shareMaxBytes + 64 * 1024);
    if (!body.ok) return body.res;
    const v = body.value as { snapshot?: unknown; ttlSeconds?: unknown };
    const err = validateSnapshot(v.snapshot);
    if (err) return jsonError(400, "invalid_snapshot", err);
    const snapshot = v.snapshot as Snapshot;
    const bytes = snapshotBytes(snapshot);
    if (bytes > limits.shareMaxBytes) return jsonError(413, "too_large", `Shared snapshots are limited to ${limits.shareMaxBytes} bytes on your plan (this one is ${bytes}).`);
    const ttl = v.ttlSeconds === undefined ? limits.shareTtlSeconds : Math.trunc(Number(v.ttlSeconds));
    if (!Number.isFinite(ttl) || ttl < 60 || ttl > limits.shareTtlSeconds) return jsonError(400, "invalid_ttl", `ttlSeconds must be between 60 and ${limits.shareTtlSeconds} on your plan.`);
    if (!(await acct.canShare(limits.shares))) return jsonError(429, "too_many_shares", `Your plan allows ${limits.shares} active shared links; revoke one first.`);
    const id = randomId(22);
    const meta = await env.SHARE.get(env.SHARE.idFromName(id)).create({ id, owner: p.uid, snapshot, ttlSeconds: ttl });
    await acct.addShare({ id, title: meta.title, createdAt: meta.createdAt, expiresAt: meta.expiresAt, bytes: meta.bytes });
    return json({ id, url: shareUrl(env, id), createdAt: new Date(meta.createdAt).toISOString(), expiresAt: new Date(meta.expiresAt).toISOString(), bytes: meta.bytes }, 201);
  }
  const id = rest[0] ?? "";
  if (rest.length === 1 && isValidShareId(id) && request.method === "DELETE") {
    const owned = await acct.removeShare(id);
    const revoked = await env.SHARE.get(env.SHARE.idFromName(id)).revoke(p.uid);
    return owned || revoked ? json({ ok: true, revoked: id }) : jsonError(404, "not_found", "No such shared link.");
  }
  return jsonError(404, "not_found", "Not found");
}

// ---------- monitors ----------

async function monitors(request: Request, url: URL, rest: string[], p: Principal, env: Env): Promise<Response> {
  const limits = limitsFor(p.plan, env);
  const acct = account(env, p.uid);
  const lim = { monitors: limits.monitors, monitorMinIntervalMinutes: limits.monitorMinIntervalMinutes, monitorMaxRequests: limits.monitorMaxRequests, monitorHistory: limits.monitorHistory, secretsAvailable: !!env.MONITOR_SECRET_KEY };
  if (rest.length === 0 && request.method === "GET") {
    const list = await acct.listMonitors();
    const views = await Promise.all(list.map((m) => env.MONITOR.get(env.MONITOR.idFromName(m.id)).get(p.uid)));
    return json({ monitors: views.filter((v) => v !== null), limits: lim });
  }
  if (rest.length === 0 && request.method === "POST") {
    const body = await readJson(request, 2 * MiB);
    if (!body.ok) return body.res;
    const id = randomInboxId();
    const input = body.value as MonitorInput;
    const reg = await acct.addMonitor(id, typeof input.name === "string" ? input.name : "monitor", limits.monitors);
    if (!reg.ok) return jsonError(limits.monitors === 0 ? 403 : 429, limits.monitors === 0 ? "plan_required" : "too_many_monitors", reg.message);
    const res = await env.MONITOR.get(env.MONITOR.idFromName(id)).save({ id, uid: p.uid, plan: p.plan, input, create: true });
    if (!res.ok) {
      await acct.removeMonitor(id);
      return jsonError(res.status, res.code, res.message);
    }
    return json(res.view, 201);
  }
  const id = rest[0] ?? "";
  if (!isValidMonitorId(id)) return jsonError(404, "not_found", "No such monitor.");
  const stub = env.MONITOR.get(env.MONITOR.idFromName(id));
  const sub = rest[1] ?? "";
  if (sub === "" && request.method === "GET") {
    const v = await stub.get(p.uid);
    return v ? json(v) : jsonError(404, "not_found", "No such monitor.");
  }
  if (sub === "" && request.method === "PUT") {
    const body = await readJson(request, 2 * MiB);
    if (!body.ok) return body.res;
    const res = await stub.save({ id, uid: p.uid, plan: p.plan, input: body.value as MonitorInput, create: false });
    return res.ok ? json(res.view) : jsonError(res.status, res.code, res.message);
  }
  if (sub === "" && request.method === "DELETE") {
    const ok = await stub.remove(p.uid);
    await acct.removeMonitor(id);
    return ok ? json({ ok: true, deleted: id }) : jsonError(404, "not_found", "No such monitor.");
  }
  if (sub === "results" && request.method === "GET") {
    const r = await stub.results(p.uid, Number(url.searchParams.get("limit") ?? "50"));
    return r ? json({ results: r }) : jsonError(404, "not_found", "No such monitor.");
  }
  if (sub === "run" && request.method === "POST") {
    const r = await stub.runNow(p.uid);
    return r.ok ? json(r.result) : jsonError(r.status, r.code, r.message);
  }
  return jsonError(404, "not_found", "Not found");
}

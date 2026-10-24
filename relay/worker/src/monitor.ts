// MonitorDO — one scheduled monitor: a few saved requests + assertions run from the edge on a
// Cron Trigger schedule, with results (last N, uptime) and alerts on state changes.
//
// Scheduling: the Worker's cron (`*/5 * * * *`) lists `mon:<id>` keys in KV (interval in the key's
// metadata) and arms each due monitor's alarm; the alarm runs the checks, so a slow target never
// holds up the cron invocation. Secrets the user explicitly chose to upload are stored encrypted
// (AES-GCM, a key per account derived from MONITOR_SECRET_KEY) and only decrypted while a run is
// in progress; results never contain response bodies or secret values.

import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { readJsonPath, JsonPathError } from "./jsonpath";
import { isPlan, limitsFor, type Plan } from "./plans";

export const ALLOWED_INTERVALS = [5, 10, 15, 30, 60, 180, 360, 720, 1440] as const;
export const MAX_TIMEOUT_MS = 30_000;
const MAX_BODY_READ = 1024 * 1024;
const MANUAL_RUN_MIN_GAP_MS = 30_000;
const MASK = "••••••";

export type AssertTarget = "status" | "header" | "jsonPath" | "body" | "responseTime" | "size" | "contentType";
export type AssertOp =
  | "eq"
  | "ne"
  | "lt"
  | "lte"
  | "gt"
  | "gte"
  | "contains"
  | "notContains"
  | "startsWith"
  | "endsWith"
  | "matches"
  | "exists"
  | "notExists"
  | "isEmpty"
  | "isNotEmpty"
  | "isType"
  | "oneOf";

/** Mirrors devall-core's `Assertion` (minus snapshots). */
export interface MonitorAssertion {
  assert: AssertTarget;
  name?: string;
  path?: string;
  op: AssertOp;
  value?: string | null;
}

export interface MonitorRequest {
  name: string;
  method: string;
  /** May contain `{{secret.NAME}}` placeholders (also headers, body and basicAuth). */
  url: string;
  headers: [string, string][];
  body?: string | null;
  basicAuth?: { username: string; password: string } | null;
  timeoutMs?: number;
  followRedirects?: boolean;
  assertions: MonitorAssertion[];
}

export interface MonitorAlerts {
  webhookUrl?: string | null;
  slackWebhookUrl?: string | null;
  /** Also alert when the monitor recovers (default true). */
  onRecovery?: boolean;
}

export interface MonitorInput {
  name: string;
  intervalMinutes: number;
  requests: MonitorRequest[];
  alerts?: MonitorAlerts | null;
  /** Secret values the user chose to upload (`null` removes a stored one on update). */
  secrets?: Record<string, string | null>;
  paused?: boolean;
}

export interface CheckResult {
  name: string;
  method: string;
  /** The URL template (placeholders, never secret values). */
  url: string;
  status: number | null;
  durationMs: number;
  ok: boolean;
  failures: string[];
  error: string | null;
}

export interface RunResult {
  at: string;
  ok: boolean;
  durationMs: number;
  checks: CheckResult[];
  trigger: "schedule" | "manual";
}

interface EncryptedValue {
  iv: string;
  ct: string;
}

interface Meta {
  id: string;
  owner: string;
  plan: Plan;
  name: string;
  intervalMinutes: number;
  requests: MonitorRequest[];
  alerts: MonitorAlerts;
  secrets: Record<string, EncryptedValue>;
  paused: boolean;
  pausedReason: string | null;
  createdAt: number;
  updatedAt: number;
  lastRunAt: number | null;
  lastOk: boolean | null;
  lastAlertAt: number | null;
  lastAlertError: string | null;
  pendingSlot: number | null;
  /** Cron slot (minutes since the epoch) of the last scheduled run; retries of a slot are skipped. */
  lastSlot?: number | null;
}

export interface MonitorView {
  id: string;
  name: string;
  intervalMinutes: number;
  effectiveIntervalMinutes: number;
  requests: MonitorRequest[];
  alerts: MonitorAlerts;
  secretNames: string[];
  paused: boolean;
  pausedReason: string | null;
  createdAt: string;
  updatedAt: string;
  lastRunAt: string | null;
  lastOk: boolean | null;
  lastAlertError: string | null;
  uptime: number | null;
  runs: number;
  /** Most recent runs, oldest first (for sparklines): ok + duration. */
  recent: { at: string; ok: boolean; durationMs: number }[];
  last: RunResult | null;
}

export type Fetcher = (input: Request) => Promise<Response>;

// ---------- validation ----------

const NAME_RE = /^[A-Za-z0-9_.-]{1,64}$/;
const OPS: ReadonlySet<string> = new Set(["eq", "ne", "lt", "lte", "gt", "gte", "contains", "notContains", "startsWith", "endsWith", "matches", "exists", "notExists", "isEmpty", "isNotEmpty", "isType", "oneOf"]);
const TARGETS: ReadonlySet<string> = new Set(["status", "header", "jsonPath", "body", "responseTime", "size", "contentType"]);

function isHttpUrl(s: string, allowPlaceholders: boolean): boolean {
  const probe = allowPlaceholders ? s.replace(/\{\{\s*secret\.[A-Za-z0-9_.-]+\s*\}\}/g, "x") : s;
  try {
    const u = new URL(probe);
    return u.protocol === "https:" || u.protocol === "http:";
  } catch {
    return false;
  }
}

/** Validates a create/update payload against the plan. Returns an error message or null. */
export function validateMonitor(input: Partial<MonitorInput>, plan: Plan, env?: Env): string | null {
  const limits = limitsFor(plan, env);
  if (typeof input.name !== "string" || !input.name.trim() || input.name.length > 100) return "name must be 1–100 characters";
  if (!ALLOWED_INTERVALS.includes(input.intervalMinutes as (typeof ALLOWED_INTERVALS)[number])) return `intervalMinutes must be one of ${ALLOWED_INTERVALS.join(", ")}`;
  if (input.intervalMinutes! < limits.monitorMinIntervalMinutes) return `Your plan runs monitors at most every ${limits.monitorMinIntervalMinutes} minutes.`;
  if (!Array.isArray(input.requests) || input.requests.length === 0) return "add at least one request";
  if (input.requests.length > limits.monitorMaxRequests) return `Your plan allows ${limits.monitorMaxRequests} requests per monitor.`;
  for (const [i, r] of input.requests.entries()) {
    const where = `requests[${i}]`;
    if (!r || typeof r !== "object") return `${where} must be an object`;
    if (typeof r.name !== "string" || r.name.length > 200) return `${where}.name must be a string`;
    if (typeof r.method !== "string" || !/^[A-Z]{3,10}$/.test(r.method)) return `${where}.method must be an HTTP method`;
    if (typeof r.url !== "string" || r.url.length > 4096 || !isHttpUrl(r.url, true)) return `${where}.url must be an http(s) URL`;
    if (!Array.isArray(r.headers) || r.headers.length > 50 || !r.headers.every((h) => Array.isArray(h) && h.length === 2 && typeof h[0] === "string" && typeof h[1] === "string")) {
      return `${where}.headers must be up to 50 [name, value] pairs`;
    }
    if (r.body !== undefined && r.body !== null && (typeof r.body !== "string" || r.body.length > 64 * 1024)) return `${where}.body must be a string up to 64 KiB`;
    if (r.timeoutMs !== undefined && (typeof r.timeoutMs !== "number" || r.timeoutMs < 100 || r.timeoutMs > MAX_TIMEOUT_MS)) return `${where}.timeoutMs must be 100–${MAX_TIMEOUT_MS}`;
    if (r.basicAuth && (typeof r.basicAuth.username !== "string" || typeof r.basicAuth.password !== "string")) return `${where}.basicAuth needs username and password`;
    if (!Array.isArray(r.assertions) || r.assertions.length > 20) return `${where}.assertions must be an array (up to 20)`;
    for (const a of r.assertions) {
      if (!a || !TARGETS.has(a.assert) || !OPS.has(a.op)) return `${where}: unsupported assertion ${JSON.stringify(a)}`;
      if (a.assert === "header" && typeof a.name !== "string") return `${where}: header assertions need a name`;
      if (a.assert === "jsonPath" && typeof a.path !== "string") return `${where}: jsonPath assertions need a path`;
    }
  }
  const al = input.alerts;
  if (al) {
    // Self-hosted relays may alert a plain-http listener on their own network; hosted ones only https.
    const plainHttp = plan === "selfhost" && typeof al.webhookUrl === "string" && al.webhookUrl.startsWith("http://");
    if (al.webhookUrl && (typeof al.webhookUrl !== "string" || !(al.webhookUrl.startsWith("https://") || plainHttp) || !isHttpUrl(al.webhookUrl, false))) {
      return plan === "selfhost" ? "alerts.webhookUrl must be an http(s):// URL" : "alerts.webhookUrl must be an https:// URL";
    }
    if (al.slackWebhookUrl && (typeof al.slackWebhookUrl !== "string" || !al.slackWebhookUrl.startsWith("https://hooks.slack.com/"))) return "alerts.slackWebhookUrl must be a Slack incoming webhook (https://hooks.slack.com/…)";
  }
  if (input.secrets !== undefined) {
    if (!input.secrets || typeof input.secrets !== "object") return "secrets must be an object";
    const names = Object.keys(input.secrets);
    if (names.length > 20) return "up to 20 secrets per monitor";
    for (const [k, v] of Object.entries(input.secrets)) {
      if (!NAME_RE.test(k)) return `invalid secret name ${k}`;
      if (v !== null && (typeof v !== "string" || v.length > 8192)) return `secret ${k} must be a string up to 8 KiB`;
    }
  }
  return null;
}

// ---------- secrets ----------

function b64(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}

function unb64(s: string): Uint8Array {
  return Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
}

/** AES-GCM key for one account: HKDF-SHA256(MONITOR_SECRET_KEY, salt "devall-monitor-v1", info uid). */
export async function accountKey(master: string, uid: string): Promise<CryptoKey> {
  const ikm = await crypto.subtle.importKey("raw", new TextEncoder().encode(master), "HKDF", false, ["deriveKey"]);
  return crypto.subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt: new TextEncoder().encode("devall-monitor-v1"), info: new TextEncoder().encode(uid) },
    ikm,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
}

export async function encryptSecret(key: CryptoKey, value: string, aad: string): Promise<EncryptedValue> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: new TextEncoder().encode(aad) }, key, new TextEncoder().encode(value)));
  return { iv: b64(iv), ct: b64(ct) };
}

export async function decryptSecret(key: CryptoKey, v: EncryptedValue, aad: string): Promise<string> {
  const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: unb64(v.iv), additionalData: new TextEncoder().encode(aad) }, key, unb64(v.ct));
  return new TextDecoder().decode(pt);
}

// ---------- running checks ----------

const PLACEHOLDER = /\{\{\s*secret\.([A-Za-z0-9_.-]+)\s*\}\}/g;

function substitute(s: string, secrets: Record<string, string>, missing: Set<string>): string {
  return s.replace(PLACEHOLDER, (m, name: string) => {
    const v = secrets[name];
    if (v === undefined) {
      missing.add(name);
      return m;
    }
    return v;
  });
}

function redact(s: string, secrets: Record<string, string>): string {
  let out = s;
  for (const v of Object.values(secrets).filter((x) => x.length >= 4).sort((a, b) => b.length - a.length)) out = out.split(v).join(MASK);
  return out;
}

function typeOf(v: unknown): string {
  if (v === null) return "null";
  if (Array.isArray(v)) return "array";
  return typeof v;
}

function asText(v: unknown): string {
  if (v === undefined || v === null) return "";
  return typeof v === "string" ? v : JSON.stringify(v);
}

/** Evaluates one assertion; returns a failure message or null. */
export function checkAssertion(a: MonitorAssertion, ctx: { status: number; headers: Headers; body: string; durationMs: number; size: number }): string | null {
  let actual: unknown;
  let label: string;
  switch (a.assert) {
    case "status":
      actual = ctx.status;
      label = "status";
      break;
    case "responseTime":
      actual = ctx.durationMs;
      label = "response time";
      break;
    case "size":
      actual = ctx.size;
      label = "size";
      break;
    case "contentType":
      actual = ctx.headers.get("content-type") ?? undefined;
      label = "content-type";
      break;
    case "header":
      actual = ctx.headers.get(a.name ?? "") ?? undefined;
      label = `header ${a.name}`;
      break;
    case "body":
      actual = ctx.body;
      label = "body";
      break;
    case "jsonPath": {
      label = `jsonPath ${a.path}`;
      try {
        actual = readJsonPath(JSON.parse(ctx.body), a.path ?? "$");
      } catch (e) {
        if (e instanceof JsonPathError) return `${label}: ${e.message}`;
        actual = undefined;
      }
      break;
    }
  }
  const want = a.value ?? "";
  const text = asText(actual);
  const numeric = (): { x: number; y: number } | null => {
    const x = Number(text);
    const y = Number(want);
    return Number.isNaN(x) || Number.isNaN(y) || text.trim() === "" ? null : { x, y };
  };
  const equal = () => {
    const n = numeric();
    return text === want || (n !== null && n.x === n.y);
  };
  let ok: boolean;
  switch (a.op) {
    case "eq":
      ok = equal();
      break;
    case "ne":
      ok = !equal();
      break;
    case "lt":
    case "lte":
    case "gt":
    case "gte": {
      const n = numeric();
      if (!n) {
        ok = false;
        break;
      }
      ok = a.op === "lt" ? n.x < n.y : a.op === "lte" ? n.x <= n.y : a.op === "gt" ? n.x > n.y : n.x >= n.y;
      break;
    }
    case "contains":
      ok = Array.isArray(actual) ? actual.some((x) => asText(x) === want) : text.includes(want);
      break;
    case "notContains":
      ok = Array.isArray(actual) ? !actual.some((x) => asText(x) === want) : !text.includes(want);
      break;
    case "startsWith":
      ok = text.startsWith(want);
      break;
    case "endsWith":
      ok = text.endsWith(want);
      break;
    case "matches":
      try {
        ok = new RegExp(want).test(text);
      } catch {
        return `${label}: invalid regex ${want}`;
      }
      break;
    case "exists":
      ok = actual !== undefined;
      break;
    case "notExists":
      ok = actual === undefined;
      break;
    case "isEmpty":
      ok = actual === undefined || actual === null || text === "" || (Array.isArray(actual) && actual.length === 0) || (typeof actual === "object" && Object.keys(actual as object).length === 0);
      break;
    case "isNotEmpty":
      ok = !(actual === undefined || actual === null || text === "" || (Array.isArray(actual) && actual.length === 0));
      break;
    case "isType":
      ok = typeOf(actual) === want;
      break;
    case "oneOf":
      ok = want.split(",").map((x) => x.trim()).includes(text);
      break;
  }
  if (ok) return null;
  const shown = text.length > 120 ? `${text.slice(0, 117)}...` : text;
  return `${label} ${a.op}${a.value !== undefined && a.value !== null ? ` ${a.value}` : ""} — got ${actual === undefined ? "nothing" : JSON.stringify(shown)}`;
}

export async function runCheck(req: MonitorRequest, secrets: Record<string, string>, fetcher: Fetcher): Promise<CheckResult> {
  const missing = new Set<string>();
  const url = substitute(req.url, secrets, missing);
  const headers = new Headers();
  for (const [k, v] of req.headers) {
    try {
      headers.append(k, substitute(v, secrets, missing));
    } catch {
      // invalid header: skip
    }
  }
  if (req.basicAuth) {
    const u = substitute(req.basicAuth.username, secrets, missing);
    const p = substitute(req.basicAuth.password, secrets, missing);
    headers.set("authorization", `Basic ${b64(new TextEncoder().encode(`${u}:${p}`))}`);
  }
  if (!headers.has("user-agent")) headers.set("user-agent", "devall-monitor/1 (+https://github.com/devall)");
  const body = req.body ? substitute(req.body, secrets, missing) : undefined;
  const base: CheckResult = { name: req.name, method: req.method, url: req.url, status: null, durationMs: 0, ok: false, failures: [], error: null };
  if (missing.size) {
    return { ...base, error: `secret${missing.size > 1 ? "s" : ""} ${[...missing].join(", ")} not uploaded for this monitor` };
  }
  const started = Date.now();
  try {
    const res = await fetcher(
      new Request(url, {
        method: req.method,
        headers,
        body: req.method === "GET" || req.method === "HEAD" ? undefined : body,
        redirect: req.followRedirects === false ? "manual" : "follow",
        signal: AbortSignal.timeout(Math.min(req.timeoutMs ?? 10_000, MAX_TIMEOUT_MS)),
      }),
    );
    const bytes = new Uint8Array(await readCapped(res, MAX_BODY_READ));
    const durationMs = Date.now() - started;
    const text = new TextDecoder().decode(bytes);
    const ctx = { status: res.status, headers: res.headers, body: text, durationMs, size: bytes.byteLength };
    const failures = req.assertions.map((a) => checkAssertion(a, ctx)).filter((x): x is string => x !== null).map((f) => redact(f, secrets));
    // Without assertions a check passes on any 2xx/3xx.
    if (!req.assertions.length && res.status >= 400) failures.push(`status ${res.status}`);
    return { ...base, status: res.status, durationMs, ok: failures.length === 0, failures };
  } catch (e) {
    const msg = e instanceof Error ? (e.name === "TimeoutError" ? `timed out after ${Math.min(req.timeoutMs ?? 10_000, MAX_TIMEOUT_MS)} ms` : e.message) : String(e);
    return { ...base, durationMs: Date.now() - started, error: redact(msg, secrets) };
  }
}

async function readCapped(res: Response, max: number): Promise<ArrayBuffer> {
  if (!res.body) return new ArrayBuffer(0);
  const reader = res.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    const room = max - total;
    if (value.byteLength >= room) {
      chunks.push(value.subarray(0, room));
      total += room;
      await reader.cancel().catch(() => {});
      break;
    }
    chunks.push(value);
    total += value.byteLength;
  }
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.byteLength;
  }
  return out.buffer;
}

/** Whether a 5-minute cron slot (minutes since the epoch) is due for `interval`. */
export function isDue(slotMinute: number, interval: number): boolean {
  return slotMinute % interval === 0;
}

// ---------- the Durable Object ----------

export class MonitorDO extends DurableObject<Env> {
  private sql: SqlStorage;
  /** Tests replace this to fake the targets. */
  fetcher: Fetcher = (r) => fetch(r);

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS results (seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, ok INTEGER NOT NULL, duration INTEGER NOT NULL, detail TEXT NOT NULL)");
  }

  private meta(): Promise<Meta | undefined> {
    return this.ctx.storage.get<Meta>("meta");
  }

  private async key(uid: string): Promise<CryptoKey | null> {
    return this.env.MONITOR_SECRET_KEY ? accountKey(this.env.MONITOR_SECRET_KEY, uid) : null;
  }

  async save(args: { id: string; uid: string; plan: Plan; input: MonitorInput; create: boolean }): Promise<{ ok: true; view: MonitorView } | { ok: false; status: number; code: string; message: string }> {
    const existing = await this.meta();
    if (args.create && existing) return { ok: false, status: 409, code: "exists", message: "monitor exists" };
    if (!args.create && (!existing || existing.owner !== args.uid)) return { ok: false, status: 404, code: "not_found", message: "No such monitor." };
    const err = validateMonitor(args.input, args.plan, this.env);
    if (err) return { ok: false, status: 400, code: "invalid_monitor", message: err };
    const secrets: Record<string, EncryptedValue> = { ...(existing?.secrets ?? {}) };
    const incoming = Object.entries(args.input.secrets ?? {});
    if (incoming.some(([, v]) => v !== null)) {
      const key = await this.key(args.uid);
      if (!key) return { ok: false, status: 400, code: "secrets_unavailable", message: "This relay can't store monitor secrets (MONITOR_SECRET_KEY isn't set)." };
      for (const [name, v] of incoming) {
        if (v === null) delete secrets[name];
        else secrets[name] = await encryptSecret(key, v, `${args.id}:${name}`);
      }
    } else for (const [name] of incoming) delete secrets[name];
    if (Object.keys(secrets).length > 20) return { ok: false, status: 400, code: "invalid_monitor", message: "up to 20 secrets per monitor" };
    const now = Date.now();
    const meta: Meta = {
      id: args.id,
      owner: args.uid,
      plan: args.plan,
      name: args.input.name.trim(),
      intervalMinutes: args.input.intervalMinutes,
      requests: args.input.requests.map((r) => ({
        name: r.name,
        method: r.method,
        url: r.url,
        headers: r.headers,
        body: r.body ?? null,
        basicAuth: r.basicAuth ?? null,
        timeoutMs: r.timeoutMs,
        followRedirects: r.followRedirects,
        assertions: r.assertions,
      })),
      alerts: { webhookUrl: args.input.alerts?.webhookUrl ?? null, slackWebhookUrl: args.input.alerts?.slackWebhookUrl ?? null, onRecovery: args.input.alerts?.onRecovery ?? true },
      secrets,
      paused: args.input.paused ?? existing?.paused ?? false,
      pausedReason: null,
      createdAt: existing?.createdAt ?? now,
      updatedAt: now,
      lastRunAt: existing?.lastRunAt ?? null,
      lastOk: existing?.lastOk ?? null,
      lastAlertAt: existing?.lastAlertAt ?? null,
      lastAlertError: existing?.lastAlertError ?? null,
      pendingSlot: null,
      lastSlot: existing?.lastSlot ?? null,
    };
    await this.ctx.storage.put("meta", meta);
    await this.env.RELAY_KV.put(`mon:${args.id}`, "1", { metadata: { i: meta.intervalMinutes, p: meta.paused ? 1 : 0 } });
    return { ok: true, view: await this.view(meta) };
  }

  async remove(uid: string): Promise<boolean> {
    const meta = await this.meta();
    if (!meta || meta.owner !== uid) return false;
    await this.env.RELAY_KV.delete(`mon:${meta.id}`);
    await this.ctx.storage.deleteAll();
    return true;
  }

  async get(uid: string | null): Promise<MonitorView | null> {
    const meta = await this.meta();
    if (!meta || (uid !== null && meta.owner !== uid)) return null;
    return this.view(meta);
  }

  async results(uid: string, limit: number): Promise<RunResult[] | null> {
    const meta = await this.meta();
    if (!meta || meta.owner !== uid) return null;
    const rows = this.sql.exec("SELECT detail FROM results ORDER BY seq DESC LIMIT ?", Math.max(1, Math.min(2016, Math.trunc(limit) || 50))).toArray();
    return rows.map((r) => JSON.parse(String(r.detail)) as RunResult);
  }

  /** Cron: arm the alarm for this slot (the alarm runs the checks). */
  async trigger(slotMinute: number): Promise<void> {
    const meta = await this.meta();
    if (!meta || meta.paused) return;
    meta.pendingSlot = slotMinute;
    await this.ctx.storage.put("meta", meta);
    const cur = await this.ctx.storage.getAlarm();
    if (cur === null || cur > Date.now()) await this.ctx.storage.setAlarm(Date.now());
  }

  async runNow(uid: string): Promise<{ ok: true; result: RunResult } | { ok: false; status: number; code: string; message: string }> {
    const meta = await this.meta();
    if (!meta || meta.owner !== uid) return { ok: false, status: 404, code: "not_found", message: "No such monitor." };
    if (meta.lastRunAt && Date.now() - meta.lastRunAt < MANUAL_RUN_MIN_GAP_MS) {
      return { ok: false, status: 429, code: "rate_limited", message: "This monitor ran less than 30 seconds ago." };
    }
    return { ok: true, result: await this.run(meta, "manual") };
  }

  override async alarm(): Promise<void> {
    const meta = await this.meta();
    if (!meta || meta.paused) return;
    const slot = meta.pendingSlot;
    meta.pendingSlot = null;
    const plan = await this.effectivePlan(meta);
    const limits = limitsFor(plan, this.env);
    const allowed = limits.monitors === null ? true : await this.env.ACCOUNT.get(this.env.ACCOUNT.idFromName(meta.owner)).monitorAllowed(meta.id, limits.monitors);
    if (!allowed) {
      meta.pausedReason = `Your plan includes ${limits.monitors} monitor${limits.monitors === 1 ? "" : "s"}; this one is paused.`;
      await this.ctx.storage.put("meta", meta);
      return;
    }
    const interval = Math.max(meta.intervalMinutes, limits.monitorMinIntervalMinutes);
    if (slot !== null && !isDue(slot, interval)) {
      await this.ctx.storage.put("meta", meta);
      return;
    }
    // Cron retries / overlapping triggers: never run twice within half an interval. Measured in
    // cron slots (the trigger's scheduled time), so a retried slot is skipped whatever the clock says.
    const lastSlot = meta.lastSlot ?? null;
    const tooSoon =
      slot !== null && lastSlot !== null
        ? slot - lastSlot < Math.max(1, interval / 2)
        : meta.lastRunAt !== null && Date.now() - meta.lastRunAt < (interval * 60_000) / 2 && slot === null;
    if (tooSoon) {
      await this.ctx.storage.put("meta", meta);
      return;
    }
    meta.pausedReason = null;
    if (slot !== null) meta.lastSlot = slot;
    await this.run(meta, "schedule");
  }

  private async effectivePlan(meta: Meta): Promise<Plan> {
    if (meta.owner.startsWith("static-")) return meta.plan;
    try {
      const ent = await this.env.RELAY_KV.get<{ plan?: string }>(`plan:${meta.owner}`, "json");
      if (ent && isPlan(ent.plan) && ent.plan !== "selfhost") return ent.plan;
    } catch {
      // fall back to the stored plan
    }
    return meta.plan;
  }

  private async decryptAll(meta: Meta): Promise<Record<string, string>> {
    const out: Record<string, string> = {};
    const names = Object.keys(meta.secrets);
    if (!names.length) return out;
    const key = await this.key(meta.owner);
    if (!key) return out;
    for (const name of names) {
      try {
        out[name] = await decryptSecret(key, meta.secrets[name]!, `${meta.id}:${name}`);
      } catch {
        // wrong key (rotated MONITOR_SECRET_KEY): the check reports the secret as missing
      }
    }
    return out;
  }

  private async run(meta: Meta, trigger: RunResult["trigger"]): Promise<RunResult> {
    const started = Date.now();
    const secrets = await this.decryptAll(meta);
    const checks: CheckResult[] = [];
    for (const r of meta.requests) checks.push(await runCheck(r, secrets, this.fetcher));
    const ok = checks.every((c) => c.ok);
    const result: RunResult = { at: new Date(started).toISOString(), ok, durationMs: Date.now() - started, checks, trigger };
    const limits = limitsFor(meta.plan, this.env);
    this.sql.exec("INSERT INTO results (at, ok, duration, detail) VALUES (?, ?, ?, ?)", started, ok ? 1 : 0, result.durationMs, JSON.stringify(result));
    this.sql.exec("DELETE FROM results WHERE seq <= (SELECT MAX(seq) FROM results) - ?", Math.max(1, limits.monitorHistory));
    const previous = meta.lastOk;
    meta.lastRunAt = started;
    meta.lastOk = ok;
    if (previous !== ok && (previous !== null || !ok) && (ok ? meta.alerts.onRecovery !== false : true)) {
      meta.lastAlertError = await this.alert(meta, result);
      meta.lastAlertAt = Date.now();
    }
    await this.ctx.storage.put("meta", meta);
    return result;
  }

  /** Posts to the configured webhook / Slack. Returns an error summary (or null). */
  private async alert(meta: Meta, r: RunResult): Promise<string | null> {
    const failures = r.checks.filter((c) => !c.ok).map((c) => `${c.name}: ${c.error ?? c.failures.join("; ")}`);
    const errors: string[] = [];
    const post = async (url: string, body: unknown, label: string) => {
      try {
        const res = await this.fetcher(
          new Request(url, { method: "POST", headers: { "content-type": "application/json", "user-agent": "devall-monitor/1" }, body: JSON.stringify(body), signal: AbortSignal.timeout(10_000) }),
        );
        if (!res.ok) errors.push(`${label}: HTTP ${res.status}`);
        await res.body?.cancel().catch(() => {});
      } catch (e) {
        errors.push(`${label}: ${e instanceof Error ? e.message : String(e)}`);
      }
    };
    const status = r.ok ? "up" : "down";
    if (meta.alerts.webhookUrl) {
      await post(meta.alerts.webhookUrl, { monitor: { id: meta.id, name: meta.name }, status, at: r.at, failures, checks: r.checks.map((c) => ({ name: c.name, ok: c.ok, status: c.status, durationMs: c.durationMs })) }, "webhook");
    }
    if (meta.alerts.slackWebhookUrl) {
      const text = r.ok ? `:large_green_circle: Monitor *${meta.name}* recovered.` : `:red_circle: Monitor *${meta.name}* is failing:\n${failures.map((f) => `• ${f}`).join("\n")}`;
      await post(meta.alerts.slackWebhookUrl, { text }, "slack");
    }
    return errors.length ? errors.join("; ") : null;
  }

  private async view(meta: Meta): Promise<MonitorView> {
    const limits = limitsFor(meta.plan, this.env);
    const agg = this.sql.exec("SELECT COUNT(*) AS n, SUM(ok) AS up FROM results").one();
    const n = Number(agg.n ?? 0);
    const recentRows = this.sql.exec("SELECT at, ok, duration FROM results ORDER BY seq DESC LIMIT 60").toArray().reverse();
    const lastRow = this.sql.exec("SELECT detail FROM results ORDER BY seq DESC LIMIT 1").toArray()[0];
    return {
      id: meta.id,
      name: meta.name,
      intervalMinutes: meta.intervalMinutes,
      effectiveIntervalMinutes: Math.max(meta.intervalMinutes, limits.monitorMinIntervalMinutes),
      requests: meta.requests,
      alerts: meta.alerts,
      secretNames: Object.keys(meta.secrets).sort(),
      paused: meta.paused,
      pausedReason: meta.pausedReason,
      createdAt: new Date(meta.createdAt).toISOString(),
      updatedAt: new Date(meta.updatedAt).toISOString(),
      lastRunAt: meta.lastRunAt ? new Date(meta.lastRunAt).toISOString() : null,
      lastOk: meta.lastOk,
      lastAlertError: meta.lastAlertError,
      uptime: n ? Number(agg.up ?? 0) / n : null,
      runs: n,
      recent: recentRows.map((r) => ({ at: new Date(Number(r.at)).toISOString(), ok: Number(r.ok) === 1, durationMs: Number(r.duration) })),
      last: lastRow ? (JSON.parse(String(lastRow.detail)) as RunResult) : null,
    };
  }
}

/** Cron handler: arm every due monitor. Bounded fan-out; KV list is paginated. */
export async function scheduledMonitors(env: Env, scheduledTime: number): Promise<number> {
  const slot = Math.round(scheduledTime / 60_000);
  let cursor: string | undefined;
  let armed = 0;
  do {
    const page = await env.RELAY_KV.list<{ i?: number; p?: number }>({ prefix: "mon:", cursor, limit: 1000 });
    const due = page.keys.filter((k) => k.metadata && !k.metadata.p && typeof k.metadata.i === "number" && isDue(slot, k.metadata.i));
    for (let i = 0; i < due.length; i += 50) {
      await Promise.all(
        due.slice(i, i + 50).map(async (k) => {
          const id = k.name.slice(4);
          try {
            await env.MONITOR.get(env.MONITOR.idFromName(id)).trigger(slot);
            armed++;
          } catch (e) {
            console.error("monitor trigger failed", id, e);
          }
        }),
      );
    }
    cursor = page.list_complete ? undefined : page.cursor;
  } while (cursor);
  return armed;
}

// InboxDO — one hosted webhook inbox (https://hooks.<domain>/<inbox-id>/…).
//
// Every captured request is written to the object's SQLite storage first and answered right away
// with the inbox's configured response. The desktop client receives the backlog and live events
// in receive order (at most INBOX_WINDOW unacknowledged) and acknowledges each one after its local
// handler succeeded; acknowledged events are deleted. Delivery is at-least-once (dedupe by id).

import { DurableObject } from "cloudflare:workers";
import { inboxUrl, type Env } from "./env";
import { buildHello } from "./hello";
import { bytesToBase64, clientIp, contentLength, forwardRequestHeaders, jsonError, messageBytes, NOINDEX, nullBodyStatus, readBodyCapped, safeClose, safeSend } from "./http";
import { ulid } from "./names";
import { configurePages } from "./pages";
import { limitsFor, type Limits, type Plan } from "./plans";
import {
  CLOSE,
  decodeFrame,
  encodeFrame,
  encodeJson,
  F,
  frameJson,
  HEARTBEAT_PING,
  HEARTBEAT_PONG,
  ProtocolError,
  subprotocolFor,
  type Frame,
  type GoAway,
  type HeaderList,
  type InboxAck,
  type InboxConfig,
  type InboxEvent,
  type InboxResponseConfig,
  type InboxStatus,
  type QuotaPayload,
} from "./protocol";
import { dayKey, nextUtcMidnight, RateLimiter } from "./quota";

export const INBOX_WINDOW = 100;
const MAX_RESPONSE_BODY = 64 * 1024;

export const DEFAULT_RESPONSE: InboxResponseConfig = {
  status: 200,
  headers: [["content-type", "application/json"]],
  body: '{"ok":true}',
};

interface Meta {
  inboxId: string;
  owner: string;
  projectKey: string;
  plan: Plan;
  response: InboxResponseConfig;
  createdAt: number;
}

interface DayCounter {
  day: string;
  count: number;
  rejected: number;
  notified: boolean;
}

interface ClientAtt {
  kind: "client";
  connId: string;
  uid: string;
  plan: Plan;
  version: number;
  /** Highest seq pushed on this connection; rows with seq ≤ this are in flight. */
  lastSentSeq: number;
}

type StoredHead = Omit<InboxEvent, "seq" | "bodyBase64">;

/** Makes a configured response safe to serve: valid status, bounded body, never HTML. */
export function sanitizeResponse(r: Partial<InboxResponseConfig> | null | undefined): InboxResponseConfig {
  if (!r || typeof r !== "object") return DEFAULT_RESPONSE;
  const status = Number.isInteger(r.status) && r.status! >= 200 && r.status! <= 599 ? r.status! : 200;
  const headers: HeaderList = [];
  for (const pair of Array.isArray(r.headers) ? r.headers.slice(0, 50) : []) {
    if (!Array.isArray(pair) || typeof pair[0] !== "string" || typeof pair[1] !== "string") continue;
    const name = pair[0].toLowerCase().trim();
    if (!/^[!#$%&'*+.^_`|~0-9a-z-]+$/.test(name) || /[\r\n]/.test(pair[1])) continue;
    if (["set-cookie", "content-length", "transfer-encoding", "connection"].includes(name)) continue;
    let value = pair[1];
    if (name === "content-type" && /html|xml|svg|javascript/i.test(value)) value = "text/plain; charset=utf-8";
    headers.push([name, value]);
  }
  const body = typeof r.body === "string" ? r.body.slice(0, MAX_RESPONSE_BODY) : "";
  return { status, headers, body };
}

export class InboxDO extends DurableObject<Env> {
  private sql: SqlStorage;
  private limiter: RateLimiter | null = null;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    configurePages(env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair(HEARTBEAT_PING, HEARTBEAT_PONG));
    this.sql = ctx.storage.sql;
    this.sql.exec(
      `CREATE TABLE IF NOT EXISTS events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL UNIQUE,
        received_at INTEGER NOT NULL,
        size INTEGER NOT NULL,
        head TEXT NOT NULL,
        body BLOB
      )`,
    );
  }

  override async fetch(request: Request): Promise<Response> {
    const control = request.headers.get("x-devall-control");
    if (control === "connect") return this.handleConnect(request);
    if (control === "capture") return this.capture(request, null);
    if (control === "capture-for") return this.capture(request, request.headers.get("x-devall-uid"));
    return jsonError(404, "not_found", "Not found");
  }

  private meta(): Promise<Meta | undefined> {
    return this.ctx.storage.get<Meta>("meta");
  }

  private client(): { ws: WebSocket; att: ClientAtt } | null {
    for (const ws of this.ctx.getWebSockets("client")) {
      if (ws.readyState !== 1) continue;
      const att = ws.deserializeAttachment() as ClientAtt | null;
      if (att?.kind === "client") return { ws, att };
    }
    return null;
  }

  private count(where = "1 = 1", ...args: SqlStorageValue[]): number {
    return Number(this.sql.exec(`SELECT COUNT(*) AS n FROM events WHERE ${where}`, ...args).one().n);
  }

  /** Deletes events older than the plan's retention. */
  private purge(now: number, limits: Limits): void {
    this.sql.exec("DELETE FROM events WHERE received_at < ?", now - limits.inboxRetentionSeconds * 1000);
  }

  private async scheduleAlarm(limits: Limits): Promise<void> {
    const oldest = this.sql.exec("SELECT MIN(received_at) AS t FROM events").one().t as number | null;
    if (oldest === null) return;
    const at = oldest + limits.inboxRetentionSeconds * 1000 + 1000;
    const current = await this.ctx.storage.getAlarm();
    if (current === null || current > at) await this.ctx.storage.setAlarm(at);
  }

  override async alarm(): Promise<void> {
    const meta = await this.meta();
    if (!meta) return;
    const limits = limitsFor(meta.plan, this.env);
    this.purge(Date.now(), limits);
    await this.scheduleAlarm(limits);
  }

  private async counters(now: number): Promise<DayCounter> {
    const d = await this.ctx.storage.get<DayCounter>("day");
    const key = dayKey(now);
    return d && d.day === key ? d : { day: key, count: 0, rejected: 0, notified: false };
  }

  // ---------- capture (public) ----------

  private async capture(request: Request, requiredOwner: string | null): Promise<Response> {
    const meta = await this.meta();
    if (!meta || (requiredOwner !== null && meta.owner !== requiredOwner)) return jsonError(404, "unknown_inbox", "No webhook inbox at this address.");
    const now = Date.now();
    const limits = limitsFor(meta.plan, this.env);
    this.purge(now, limits);

    const ip = clientIp(request);
    if (limits.ipRequestsPerMinute !== null) {
      this.limiter ??= new RateLimiter(limits.ipRequestsPerMinute);
      if (!this.limiter.allow(ip ?? "unknown", now)) {
        return jsonError(429, "rate_limited", "Too many requests from this address.", {}, { "retry-after": String(this.limiter.retryAfter(ip ?? "unknown", now)) });
      }
    }

    const day = await this.counters(now);
    if (limits.inboxEventsPerDay !== null && day.count >= limits.inboxEventsPerDay) {
      day.rejected += 1;
      const notify = !day.notified;
      day.notified = true;
      await this.ctx.storage.put("day", day);
      const resetAt = nextUtcMidnight(now);
      if (notify) {
        const c = this.client();
        if (c) {
          safeSend(
            c.ws,
            encodeJson(F.QUOTA, 0, {
              kind: "inbox_daily",
              used: day.count,
              limit: limits.inboxEventsPerDay,
              resetAt: new Date(resetAt).toISOString(),
              message: "Daily webhook limit reached; senders get 429 and will retry after midnight UTC.",
            } satisfies QuotaPayload),
          );
        }
      }
      return jsonError(429, "inbox_daily_limit", "This inbox reached its daily event limit.", {}, { "retry-after": String(Math.ceil((resetAt - now) / 1000)) });
    }
    if (limits.inboxMaxBuffered !== null && this.count() >= limits.inboxMaxBuffered) {
      return jsonError(429, "inbox_full", "This inbox is full until its owner comes online.", {}, { "retry-after": "300" });
    }
    const len = contentLength(request.headers);
    if (len !== null && len > limits.inboxMaxBodyBytes) return jsonError(413, "too_large", `Bodies are limited to ${limits.inboxMaxBodyBytes} bytes.`);
    const body = await readBodyCapped(request.body, limits.inboxMaxBodyBytes);
    if (body === null) return jsonError(413, "too_large", `Bodies are limited to ${limits.inboxMaxBodyBytes} bytes.`);

    const url = new URL(request.url);
    const id = `evt_${ulid(now)}`;
    const head: StoredHead = {
      id,
      inboxId: meta.inboxId,
      method: request.method,
      url: request.url,
      path: request.headers.get("x-devall-inbox-path") ?? url.pathname + url.search,
      headers: forwardRequestHeaders(request.headers),
      bodySize: body.byteLength,
      receivedAt: new Date(now).toISOString(),
      sourceIp: ip,
    };
    // Copy into an exact-length ArrayBuffer for the BLOB column.
    const blob = body.byteLength ? body.slice().buffer : null;
    this.sql.exec("INSERT INTO events (id, received_at, size, head, body) VALUES (?, ?, ?, ?, ?)", id, now, body.byteLength, JSON.stringify(head), blob);
    day.count += 1;
    await this.ctx.storage.put("day", day);
    await this.scheduleAlarm(limits);
    this.pump();
    return this.configuredResponse(meta.response, id, request.method);
  }

  private configuredResponse(cfg: InboxResponseConfig, eventId: string, method: string): Response {
    const r = sanitizeResponse(cfg);
    const h = new Headers();
    for (const [k, v] of r.headers) {
      try {
        h.append(k, v);
      } catch {
        // skip invalid header
      }
    }
    h.set("x-robots-tag", NOINDEX);
    h.set("x-devall-event-id", eventId);
    h.set("cache-control", "no-store");
    // Inboxes never serve active content, whatever was configured.
    h.set("content-security-policy", "sandbox; default-src 'none'");
    h.set("x-content-type-options", "nosniff");
    const noBody = nullBodyStatus(r.status) || method === "HEAD";
    return new Response(noBody ? null : r.body, { status: r.status, headers: h });
  }

  // ---------- delivery ----------

  /** Pushes stored events after the connection's cursor, keeping ≤ INBOX_WINDOW in flight. */
  private pump(): void {
    const c = this.client();
    if (!c) return;
    const { ws, att } = c;
    const inFlight = this.count("seq <= ?", att.lastSentSeq);
    const room = INBOX_WINDOW - inFlight;
    if (room <= 0) return;
    const rows = this.sql.exec("SELECT seq, head, body FROM events WHERE seq > ? ORDER BY seq LIMIT ?", att.lastSentSeq, room).toArray();
    if (!rows.length) return;
    for (const row of rows) {
      const head = JSON.parse(String(row.head)) as StoredHead;
      const body = row.body instanceof ArrayBuffer ? new Uint8Array(row.body) : new Uint8Array(0);
      const ev: InboxEvent = { ...head, seq: Number(row.seq), bodyBase64: bytesToBase64(body) };
      if (!safeSend(ws, encodeJson(F.INBOX_EVENT, 0, ev))) break;
      att.lastSentSeq = Number(row.seq);
    }
    ws.serializeAttachment(att);
  }

  private async status(att: ClientAtt | null, meta: Meta): Promise<InboxStatus> {
    const limits = limitsFor(meta.plan, this.env);
    const day = await this.counters(Date.now());
    const oldest = this.sql.exec("SELECT MIN(received_at) AS t FROM events").one().t as number | null;
    return {
      buffered: this.count(),
      inFlight: att ? this.count("seq <= ?", att.lastSentSeq) : 0,
      todayCount: day.count,
      dailyLimit: limits.inboxEventsPerDay,
      rejectedToday: day.rejected,
      retentionSeconds: limits.inboxRetentionSeconds,
      oldestReceivedAt: oldest === null ? null : new Date(oldest).toISOString(),
      response: meta.response,
    };
  }

  private async sendStatus(ws: WebSocket, att: ClientAtt): Promise<void> {
    const meta = await this.meta();
    if (meta) safeSend(ws, encodeJson(F.INBOX_STATUS, 0, await this.status(att, meta)));
  }

  // ---------- client connect ----------

  private async handleConnect(request: Request): Promise<Response> {
    const h = request.headers;
    const uid = h.get("x-devall-uid") ?? "";
    const plan = (h.get("x-devall-plan") ?? "free") as Plan;
    const version = Number(h.get("x-devall-version") ?? "1");
    const inboxId = h.get("x-devall-inbox") ?? "";
    const projectKey = h.get("x-devall-project") ?? "";
    if (!uid || !inboxId) return jsonError(400, "bad_request", "missing connect metadata");
    const now = Date.now();
    let meta = await this.meta();
    if (meta && meta.owner !== uid) return jsonError(403, "forbidden", "This inbox belongs to another account.");
    meta = meta ? { ...meta, plan } : { inboxId, owner: uid, projectKey, plan, response: DEFAULT_RESPONSE, createdAt: now };
    await this.ctx.storage.put("meta", meta);

    const current = this.client();
    if (current) {
      safeSend(current.ws, encodeJson(F.GOAWAY, 0, { reason: "replaced", reconnect: false, message: "A newer connection took over this inbox." } satisfies GoAway));
      safeClose(current.ws, CLOSE.REPLACED, "replaced");
    }
    const limits = limitsFor(plan, this.env);
    this.purge(now, limits);
    const usage = await this.env.ACCOUNT.get(this.env.ACCOUNT.idFromName(uid)).getUsage();

    const pair = new WebSocketPair();
    const clientEnd = pair[0];
    const server = pair[1];
    server.binaryType = "arraybuffer";
    this.ctx.acceptWebSocket(server, ["client"]);
    const att: ClientAtt = { kind: "client", connId: ulid(now), uid, plan, version, lastSentSeq: 0 };
    server.serializeAttachment(att);
    server.send(
      encodeJson(
        F.HELLO,
        0,
        buildHello({
          version,
          kind: "inbox",
          sessionId: `s_${att.connId}`,
          publicUrl: inboxUrl(this.env, inboxId),
          subdomain: null,
          inboxId,
          plan,
          limits,
          usage,
          sessionExpiresAt: null,
        }),
      ),
    );
    server.send(encodeJson(F.INBOX_STATUS, 0, await this.status(att, meta)));
    this.pump();
    return new Response(null, { status: 101, webSocket: clientEnd, headers: { "sec-websocket-protocol": subprotocolFor(version) } });
  }

  // ---------- socket events ----------

  override async webSocketMessage(ws: WebSocket, raw: string | ArrayBuffer): Promise<void> {
    const message = await messageBytes(raw);
    const att = ws.deserializeAttachment() as ClientAtt | null;
    if (att?.kind !== "client") return;
    if (typeof message === "string") {
      if (message === HEARTBEAT_PING) safeSend(ws, HEARTBEAT_PONG);
      return;
    }
    try {
      await this.onFrame(ws, att, decodeFrame(message));
    } catch (e) {
      if (e instanceof ProtocolError) {
        safeSend(ws, encodeJson(F.ERROR, 0, { code: "protocol_error", message: e.message, fatal: true }));
        safeClose(ws, CLOSE.PROTOCOL_ERROR, "protocol error");
        return;
      }
      throw e;
    }
  }

  private async onFrame(ws: WebSocket, att: ClientAtt, f: Frame): Promise<void> {
    switch (f.type) {
      case F.INBOX_ACK: {
        const { ids } = frameJson<InboxAck>(f);
        if (!Array.isArray(ids)) throw new ProtocolError("INBOX_ACK: ids must be an array");
        const valid = ids.filter((id) => typeof id === "string" && id.length <= 64).slice(0, 1000);
        // Durable Object SQL allows at most 100 bound parameters per statement.
        for (let i = 0; i < valid.length; i += 90) {
          const batch = valid.slice(i, i + 90);
          this.sql.exec(`DELETE FROM events WHERE id IN (${batch.map(() => "?").join(",")})`, ...batch);
        }
        this.pump();
        const fresh = ws.deserializeAttachment() as ClientAtt;
        await this.sendStatus(ws, fresh);
        return;
      }
      case F.INBOX_CONFIG: {
        const cfg = frameJson<InboxConfig>(f);
        const meta = await this.meta();
        if (!meta) return;
        if (cfg.response !== undefined) meta.response = cfg.response === null ? DEFAULT_RESPONSE : sanitizeResponse(cfg.response);
        await this.ctx.storage.put("meta", meta);
        await this.sendStatus(ws, att);
        return;
      }
      case F.PING:
        safeSend(ws, encodeFrame(F.PONG, 0, f.payload.slice(0, 64)));
        return;
      default:
        return;
    }
  }

  override async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    safeClose(ws, code, reason);
  }

  override async webSocketError(ws: WebSocket): Promise<void> {
    safeClose(ws, 1011, "error");
  }

  /** Account summary helper for tests and the admin API. */
  async info(): Promise<{ buffered: number; owner: string | null; plan: Plan | null }> {
    const meta = await this.meta();
    return { buffered: this.count(), owner: meta?.owner ?? null, plan: meta?.plan ?? null };
  }
}


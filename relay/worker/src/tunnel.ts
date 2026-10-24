// TunnelDO — one Durable Object per tunnel subdomain.
//
// The desktop client's WebSocket is accepted with the Hibernation API, so an idle tunnel costs
// nothing; public requests to https://<sub>.<domain>/… are framed onto that socket as streams
// (relay/PROTOCOL.md). In-flight streams live in memory: the object can't hibernate while a public
// request is pending, and everything that must survive hibernation (session, usage, pass-through
// WebSocket mapping) is kept in socket attachments or storage.

import { DurableObject } from "cloudflare:workers";
import type { AccountDO } from "./account";
import { hostsForSubdomain } from "./customhosts";
import { relayHost, tunnelUrl, type Env } from "./env";
import { buildHello, deferred, StreamError, withTimeout } from "./hello";
import {
  clientIp,
  contentLength,
  forwardRequestHeaders,
  isWebSocketUpgrade,
  jsonError,
  messageBytes,
  NOINDEX,
  nullBodyStatus,
  responseHeaders,
  safeClose,
  safeSend,
  sendableCloseCode,
} from "./http";
import { continueResponse, CONTINUE_PATH, shouldShowInterstitial } from "./interstitial";
import { isValidInboxId, ulid } from "./names";
import { blockedPage, configurePages, interstitialPage, offlinePage, problem, quotaPage } from "./pages";
import { limitsFor, overLimit, type Limits, type Plan } from "./plans";
import {
  CLOSE,
  DEFAULT_WINDOW,
  decodeFrame,
  encodeFrame,
  encodeJson,
  encodeU32,
  F,
  frameJson,
  frameU32,
  HEARTBEAT_PING,
  HEARTBEAT_PONG,
  MAX_FRAME,
  MAX_WS_MESSAGE,
  ProtocolError,
  subprotocolFor,
  TCP_CONSUMER_STREAM,
  type Frame,
  type GoAway,
  type QuotaPayload,
  type RequestHead,
  type ResetPayload,
  type ResponseHead,
  type TcpClose,
  type TcpOpen,
  type TunnelConfig,
  type UsagePayload,
  type WsAccept,
  type WsClose,
  type WsOpen,
  type WsReject,
} from "./protocol";
import { monthPeriod, rollUsage, shouldFlush, totalBytes, RateLimiter, type UsageSnapshot } from "./quota";

/** A free session can be resumed (same URL) this long after a disconnect. */
export const RESUME_GRACE_MS = 10 * 60_000;
/** Random names aren't handed to another account this soon after their last use. */
export const RECYCLE_AFTER_MS = 60 * 60_000;
export const MAX_STREAMS = 100;
/** How long a TCP consumer waits for the publisher to connect to its local port. */
export const TCP_ACCEPT_TIMEOUT_MS = 15_000;

/** `http` tunnels proxy HTTP/WebSocket; `tcp` tunnels bridge raw TCP connections (PROTOCOL.md §2.6). */
export type TunnelKind = "http" | "tcp";

interface Session {
  sessionId: string;
  sessionStart: number;
  sessionExpiresAt: number | null;
}

interface OfflineInbox {
  inboxId: string;
  pathPrefixes: string[];
}

interface Owner extends Session {
  uid: string;
  plan: Plan;
  connId: string;
  disconnectedAt: number | null;
  offlineInbox: OfflineInbox | null;
  kind?: TunnelKind;
  /** SHA-256 (hex) of the TCP tunnel's access secret; null = only the owner's account may connect. */
  tcpSecretSha256?: string | null;
}

interface ClientAtt extends Session {
  kind: "client";
  connId: string;
  uid: string;
  plan: Plan;
  sub: string;
  version: number;
  usage: UsageSnapshot;
  quotaNotified: boolean;
  /** Missing on sockets accepted before TCP tunnels existed = `http`. */
  tunnelKind?: TunnelKind;
}

interface PublicAtt {
  kind: "public";
  stream: number;
  connId: string;
}

/** A TCP consumer's socket (one per bridged TCP connection). */
interface TcpAtt {
  kind: "tcp";
  stream: number;
  connId: string;
}

interface PendingTcp {
  id: number;
  connId: string;
  result: ReturnType<typeof deferred<void>>;
  /** Frames from the publisher that arrived before the consumer's socket existed. */
  early: Uint8Array[];
}

interface HttpStream {
  id: number;
  connId: string;
  ws: WebSocket;
  head: ReturnType<typeof deferred<ResponseHead>>;
  headDone: boolean;
  discardBody: boolean;
  chunks: Uint8Array[];
  buffered: number;
  ended: boolean;
  failed: StreamError | null;
  wake: (() => void) | null;
  consumed: number;
  credit: number;
  creditWake: (() => void) | null;
  closed: boolean;
}

interface PendingWs {
  id: number;
  connId: string;
  result: ReturnType<typeof deferred<WsAccept>>;
  early: { text: boolean; data: Uint8Array }[];
}

const textDecoder = new TextDecoder();
const textEncoder = new TextEncoder();

export class TunnelDO extends DurableObject<Env> {
  private streams = new Map<number, HttpStream>();
  private pendingWs = new Map<number, PendingWs>();
  private pendingTcp = new Map<number, PendingTcp>();
  private nextId: number;
  private limiter: { perMinute: number; rl: RateLimiter } | null = null;
  private blockedCache: boolean | null = null;
  /** Usage not yet folded into the client socket's attachment. */
  private memBytes = 0;
  private memRequests = 0;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    configurePages(env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair(HEARTBEAT_PING, HEARTBEAT_PONG));
    this.nextId = 1 + Math.floor(Math.random() * 0x3fff_ffff);
  }

  // ---------- RPC (AccountDO, admin) ----------

  /** Whether the client connection `connId` is still attached. */
  async isLive(connId: string): Promise<boolean> {
    const c = this.client();
    return !!c && c.att.connId === connId;
  }

  async block(reason: string): Promise<void> {
    await this.ctx.storage.put("blocked", { reason, at: Date.now() });
    this.blockedCache = true;
    const c = this.client();
    if (c) {
      safeSend(c.ws, encodeJson(F.GOAWAY, 0, { reason: "blocked", reconnect: false, message: "This subdomain was disabled after an abuse report." } satisfies GoAway));
      safeClose(c.ws, CLOSE.BLOCKED, "blocked");
    }
  }

  async unblock(): Promise<void> {
    await this.ctx.storage.delete("blocked");
    this.blockedCache = false;
  }

  // ---------- fetch ----------

  override async fetch(request: Request): Promise<Response> {
    const control = request.headers.get("x-devall-control");
    if (control === "connect") return this.handleConnect(request);
    if (control === "tcp-connect") return this.tcpConnect(request);
    return this.publicRequest(request);
  }

  private account(uid: string): DurableObjectStub<AccountDO> {
    return this.env.ACCOUNT.get(this.env.ACCOUNT.idFromName(uid));
  }

  private async isBlocked(): Promise<boolean> {
    if (this.blockedCache === null) this.blockedCache = !!(await this.ctx.storage.get("blocked"));
    return this.blockedCache;
  }

  private client(): { ws: WebSocket; att: ClientAtt } | null {
    for (const ws of this.ctx.getWebSockets("client")) {
      if (ws.readyState !== 1) continue;
      const att = ws.deserializeAttachment() as ClientAtt | null;
      if (att?.kind === "client") return { ws, att };
    }
    return null;
  }

  private allocId(): number {
    for (;;) {
      this.nextId = this.nextId >= 0xffff_fff0 ? 1 : this.nextId + 1;
      const id = this.nextId;
      if (!this.streams.has(id) && !this.pendingWs.has(id) && !this.pendingTcp.has(id) && this.ctx.getWebSockets(`s:${id}`).length === 0 && this.ctx.getWebSockets(`t:${id}`).length === 0) {
        return id;
      }
    }
  }

  private rateLimiter(perMinute: number): RateLimiter {
    if (!this.limiter || this.limiter.perMinute !== perMinute) this.limiter = { perMinute, rl: new RateLimiter(perMinute) };
    return this.limiter.rl;
  }

  // ---------- client connect ----------

  private async handleConnect(request: Request): Promise<Response> {
    const h = request.headers;
    const uid = h.get("x-devall-uid") ?? "";
    const plan = (h.get("x-devall-plan") ?? "free") as Plan;
    const sub = h.get("x-devall-sub") ?? "";
    const mode = h.get("x-devall-mode") ?? "fresh";
    const version = Number(h.get("x-devall-version") ?? "1");
    const tunnelKind: TunnelKind = h.get("x-devall-kind") === "tcp" ? "tcp" : "http";
    if (!uid || !sub) return jsonError(400, "bad_request", "missing connect metadata");
    if (await this.isBlocked()) return jsonError(451, "blocked", "This subdomain was disabled after an abuse report.");

    const limits = limitsFor(plan, this.env);
    const now = Date.now();
    const owner = await this.ctx.storage.get<Owner>("owner");
    const current = this.client();
    const fresh = (): Session => ({
      sessionId: `s_${ulid(now)}`,
      sessionStart: now,
      sessionExpiresAt: limits.sessionSeconds === null ? null : now + limits.sessionSeconds * 1000,
    });
    const keep = (s: Session): Session => ({ sessionId: s.sessionId, sessionStart: s.sessionStart, sessionExpiresAt: s.sessionExpiresAt });
    const alive = (s: Session) => s.sessionExpiresAt === null || s.sessionExpiresAt > now;

    let session: Session;
    if (current) {
      if (current.att.uid !== uid) return jsonError(409, "subdomain_in_use", `${sub} is in use by another account.`);
      // Same account again (e.g. the old socket hasn't noticed a network drop yet): replace it.
      session = mode === "resume" && alive(current.att) ? keep(current.att) : fresh();
    } else if (mode === "resume") {
      const ok =
        !!owner &&
        owner.uid === uid &&
        alive(owner) &&
        (owner.disconnectedAt === null || now - owner.disconnectedAt <= RESUME_GRACE_MS);
      if (!ok) return jsonError(409, "resume_failed", "The previous session can't be resumed.");
      session = keep(owner);
    } else {
      if (mode === "fresh" && owner && owner.uid !== uid && (owner.disconnectedAt === null || now - owner.disconnectedAt < RECYCLE_AFTER_MS)) {
        return jsonError(409, "subdomain_in_use", `${sub} was used recently.`);
      }
      session = fresh();
    }

    const connId = ulid(now);
    const reg = await this.account(uid).registerTunnel({ sub, connId, maxTunnels: limits.maxTunnels });
    if (!reg.ok) return jsonError(429, reg.code, reg.message, { active: reg.active });

    if (current) {
      safeSend(current.ws, encodeJson(F.GOAWAY, 0, { reason: "replaced", reconnect: false, message: "A newer connection took over this tunnel." } satisfies GoAway));
      safeClose(current.ws, CLOSE.REPLACED, "replaced");
      this.failAll(current.att.connId, new StreamError("disconnected", "the tunnel reconnected"));
    }

    const usage = rollUsage(
      { period: monthPeriod(now).key, bytes: reg.usage.bandwidthBytes, requests: reg.usage.requests, pendingBytes: 0, pendingRequests: 0, lastFlush: now },
      now,
    );
    const att: ClientAtt = { kind: "client", connId, uid, plan, sub, version, ...session, usage, quotaNotified: false, tunnelKind };
    const pair = new WebSocketPair();
    const clientEnd = pair[0];
    const server = pair[1];
    server.binaryType = "arraybuffer";
    this.ctx.acceptWebSocket(server, ["client"]);
    server.serializeAttachment(att);
    const sameOwner = !!owner && owner.uid === uid;
    const newOwner: Owner = {
      uid,
      plan,
      connId,
      ...session,
      disconnectedAt: null,
      offlineInbox: sameOwner && tunnelKind === "http" ? owner.offlineInbox : null,
      kind: tunnelKind,
      // The access secret is (re)sent with TUNNEL_CONFIG after every connect.
      tcpSecretSha256: null,
    };
    await this.ctx.storage.put("owner", newOwner);
    if (session.sessionExpiresAt !== null) await this.ctx.storage.setAlarm(session.sessionExpiresAt);
    else await this.ctx.storage.deleteAlarm();
    const extraUrls = tunnelKind === "http" ? (await hostsForSubdomain(this.env, sub)).map((h) => `https://${h}`) : [];

    server.send(
      encodeJson(
        F.HELLO,
        0,
        buildHello({
          version,
          kind: tunnelKind === "tcp" ? "tcp" : "tunnel",
          sessionId: session.sessionId,
          publicUrl: tunnelUrl(this.env, sub),
          extraUrls,
          subdomain: sub,
          inboxId: null,
          plan,
          limits,
          usage: reg.usage,
          sessionExpiresAt: session.sessionExpiresAt,
        }),
      ),
    );
    return new Response(null, { status: 101, webSocket: clientEnd, headers: { "sec-websocket-protocol": subprotocolFor(version) } });
  }

  // ---------- public side ----------

  private reportUrl(host: string): string {
    return `https://${relayHost(this.env)}/report?host=${encodeURIComponent(host)}`;
  }

  private async publicRequest(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const host = url.host;
    if (await this.isBlocked()) return blockedPage(request);
    const c = this.client();
    if (!c) return this.offline(request, url);
    const { ws, att } = c;
    // A custom domain mapped by an account only reaches that account's tunnel.
    const customUid = request.headers.get("x-devall-custom-uid");
    if (customUid && customUid !== att.uid) return offlinePage(request, host);
    if (att.tunnelKind === "tcp") {
      return problem(
        request,
        404,
        "This is a TCP tunnel",
        `${att.sub} forwards raw TCP, not HTTP. Connect with: devall tcp connect ${att.sub} --local-port <port>`,
      );
    }
    const now = Date.now();
    if (att.sessionExpiresAt !== null && now >= att.sessionExpiresAt) {
      this.expire(ws, att);
      return offlinePage(request, host);
    }
    const limits = limitsFor(att.plan, this.env);
    const ip = clientIp(request);
    if (limits.ipRequestsPerMinute !== null) {
      const rl = this.rateLimiter(limits.ipRequestsPerMinute);
      const key = ip ?? "unknown";
      if (!rl.allow(key, now)) {
        return problem(request, 429, "Too many requests", "Slow down — this address is rate limited per visitor.", { "retry-after": String(rl.retryAfter(key, now)) });
      }
    }
    if (url.pathname === CONTINUE_PATH) return continueResponse(url);
    if (limits.interstitial && shouldShowInterstitial(request, true)) return interstitialPage(request, host, this.reportUrl(host));

    const usage = rollUsage(att.usage, now);
    if (overLimit(totalBytes(usage) + this.memBytes, limits.bandwidthBytesPerMonth)) {
      this.notifyQuota(ws, att, usage, limits);
      return quotaPage(request, "This tunnel used its monthly bandwidth.", monthPeriod(now).end);
    }
    if (isWebSocketUpgrade(request)) return this.publicWebSocket(request, url, ws, att, limits, ip);
    if (this.streams.size >= MAX_STREAMS) return problem(request, 503, "Tunnel busy", "Too many requests are in flight on this tunnel.", { "retry-after": "1" });
    const len = contentLength(request.headers);
    if (limits.maxRequestBodyBytes !== null && len !== null && len > limits.maxRequestBodyBytes) {
      return problem(request, 413, "Request too large", `Request bodies are limited to ${limits.maxRequestBodyBytes} bytes on this tunnel.`);
    }
    return this.proxyHttp(request, url, ws, att, limits, ip, len);
  }

  private async offline(request: Request, url: URL): Promise<Response> {
    const owner = await this.ctx.storage.get<Owner>("owner");
    const oi = owner?.offlineInbox;
    if (owner && oi && !isWebSocketUpgrade(request) && oi.pathPrefixes.some((p) => url.pathname.startsWith(p))) {
      // Buffered-webhook behaviour: capture into the owner's inbox while the laptop is away.
      const headers = new Headers(request.headers);
      headers.set("x-devall-control", "capture-for");
      headers.set("x-devall-uid", owner.uid);
      headers.set("x-devall-inbox-path", url.pathname + url.search);
      return this.env.INBOX.get(this.env.INBOX.idFromName(oi.inboxId)).fetch(new Request(request, { headers }));
    }
    return offlinePage(request, url.host);
  }

  private newStream(id: number, ws: WebSocket, connId: string): HttpStream {
    const s: HttpStream = {
      id,
      connId,
      ws,
      head: deferred<ResponseHead>(),
      headDone: false,
      discardBody: false,
      chunks: [],
      buffered: 0,
      ended: false,
      failed: null,
      wake: null,
      consumed: 0,
      credit: DEFAULT_WINDOW,
      creditWake: null,
      closed: false,
    };
    this.streams.set(id, s);
    return s;
  }

  private wake(s: HttpStream): void {
    const w = s.wake;
    s.wake = null;
    w?.();
    const cw = s.creditWake;
    s.creditWake = null;
    cw?.();
  }

  /** Stream finished normally (or was cancelled): forget it. */
  private drop(s: HttpStream): void {
    if (s.closed) return;
    s.closed = true;
    this.streams.delete(s.id);
    this.wake(s);
    this.settle();
  }

  private failStream(s: HttpStream, err: StreamError): void {
    if (s.closed) return;
    s.failed = err;
    if (!s.headDone) s.head.reject(err);
    this.drop(s);
  }

  private failAll(connId: string, err: StreamError): void {
    for (const s of [...this.streams.values()]) if (s.connId === connId) this.failStream(s, err);
    for (const p of [...this.pendingWs.values()]) {
      if (p.connId === connId) {
        this.pendingWs.delete(p.id);
        p.result.reject(new StreamError("disconnected", err.message, 502));
      }
    }
    for (const p of [...this.pendingTcp.values()]) {
      if (p.connId === connId) {
        this.pendingTcp.delete(p.id);
        p.result.reject(new StreamError("offline", err.message, 502));
      }
    }
  }

  private async proxyHttp(request: Request, url: URL, ws: WebSocket, att: ClientAtt, limits: Limits, ip: string | null, len: number | null): Promise<Response> {
    const id = this.allocId();
    const s = this.newStream(id, ws, att.connId);
    const head: RequestHead = {
      method: request.method,
      url: request.url,
      path: url.pathname + url.search,
      headers: forwardRequestHeaders(request.headers),
      clientIp: ip,
      bodyLength: len,
    };
    if (!safeSend(ws, encodeJson(F.OPEN, id, head))) {
      this.drop(s);
      return offlinePage(request, url.host);
    }
    this.memRequests += 1;
    this.ctx.waitUntil(this.pumpRequestBody(request, s, limits.maxRequestBodyBytes));

    let res: ResponseHead;
    try {
      res = await withTimeout(s.head.promise, limits.requestTimeoutMs);
    } catch (e) {
      const err = e instanceof StreamError ? e : new StreamError("internal", String(e));
      if (err.code === "timeout") safeSend(ws, encodeJson(F.RESET, id, { code: "timeout", message: err.message } satisfies ResetPayload));
      this.failStream(s, err);
      if (err.code === "timeout") return problem(request, 504, "Gateway timeout", "The app behind this tunnel didn't answer in time.");
      if (err.code === "too_large") return problem(request, 413, "Request too large", err.message);
      if (err.code === "cancelled") return problem(request, 499, "Request cancelled", err.message);
      return problem(request, 502, "Bad gateway", err.message || "The tunnel client failed to answer.");
    }

    const status = Number.isInteger(res.status) && res.status >= 200 && res.status <= 599 ? res.status : 502;
    const headers = responseHeaders(Array.isArray(res.headers) ? res.headers : []);
    headers.set("x-robots-tag", NOINDEX);
    if (nullBodyStatus(status) || request.method === "HEAD") {
      s.discardBody = true;
      if (s.ended) this.drop(s);
      return new Response(null, { status, headers });
    }
    return new Response(this.responseBody(s), { status, headers, encodeBody: "manual" });
  }

  /** Request body → client in ≤ MAX_FRAME chunks, never exceeding the stream's credit. */
  private async pumpRequestBody(request: Request, s: HttpStream, max: number | null): Promise<void> {
    const ws = s.ws;
    try {
      if (request.body) {
        const reader = request.body.getReader();
        let total = 0;
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          if (s.closed) {
            await reader.cancel().catch(() => {});
            return;
          }
          total += value.byteLength;
          if (max !== null && total > max) {
            await reader.cancel().catch(() => {});
            safeSend(ws, encodeJson(F.RESET, s.id, { code: "too_large", message: `request body exceeds ${max} bytes` } satisfies ResetPayload));
            this.failStream(s, new StreamError("too_large", `Request bodies are limited to ${max} bytes on this tunnel.`, 413));
            return;
          }
          let off = 0;
          while (off < value.byteLength) {
            while (s.credit <= 0 && !s.closed) await new Promise<void>((r) => (s.creditWake = r));
            if (s.closed) {
              await reader.cancel().catch(() => {});
              return;
            }
            const n = Math.min(value.byteLength - off, MAX_FRAME, s.credit);
            if (!safeSend(ws, encodeFrame(F.DATA, s.id, value.subarray(off, off + n)))) {
              this.failStream(s, new StreamError("disconnected", "the tunnel disconnected"));
              return;
            }
            s.credit -= n;
            off += n;
            this.memBytes += n;
          }
        }
      }
      if (!s.closed) safeSend(ws, encodeFrame(F.END, s.id));
    } catch {
      if (!s.closed) {
        safeSend(ws, encodeJson(F.RESET, s.id, { code: "cancelled", message: "the caller aborted the request" } satisfies ResetPayload));
        this.failStream(s, new StreamError("cancelled", "The caller aborted the request.", 499));
      }
    }
  }

  /** Pull-based response body: credit goes back to the client only as the caller reads. */
  private responseBody(s: HttpStream): ReadableStream<Uint8Array> {
    const half = DEFAULT_WINDOW / 2;
    const pull = async (controller: ReadableStreamDefaultController<Uint8Array>): Promise<void> => {
      for (;;) {
        const chunk = s.chunks.shift();
        if (chunk) {
          s.buffered -= chunk.byteLength;
          controller.enqueue(chunk);
          s.consumed += chunk.byteLength;
          if (s.consumed >= half && !s.ended) {
            safeSend(s.ws, encodeU32(F.WINDOW, s.id, s.consumed));
            s.consumed = 0;
          }
          return;
        }
        if (s.failed) {
          controller.error(s.failed);
          return;
        }
        if (s.ended) {
          controller.close();
          this.drop(s);
          return;
        }
        if (s.closed) {
          controller.close();
          return;
        }
        await new Promise<void>((r) => (s.wake = r));
      }
    };
    return new ReadableStream<Uint8Array>(
      {
        pull,
        cancel: () => {
          if (!s.closed) {
            safeSend(s.ws, encodeJson(F.RESET, s.id, { code: "cancelled", message: "the caller went away" } satisfies ResetPayload));
            this.drop(s);
          }
        },
      },
      { highWaterMark: 0 },
    );
  }

  private async publicWebSocket(request: Request, url: URL, ws: WebSocket, att: ClientAtt, limits: Limits, ip: string | null): Promise<Response> {
    const id = this.allocId();
    const protocols = (request.headers.get("sec-websocket-protocol") ?? "")
      .split(",")
      .map((p) => p.trim())
      .filter(Boolean);
    const open: WsOpen = {
      url: request.url,
      path: url.pathname + url.search,
      headers: forwardRequestHeaders(request.headers, { websocket: true }),
      protocols,
      clientIp: ip,
    };
    const pending: PendingWs = { id, connId: att.connId, result: deferred<WsAccept>(), early: [] };
    this.pendingWs.set(id, pending);
    if (!safeSend(ws, encodeJson(F.WS_OPEN, id, open))) {
      this.pendingWs.delete(id);
      return offlinePage(request, url.host);
    }
    this.memRequests += 1;
    let accept: WsAccept;
    try {
      accept = await withTimeout(pending.result.promise, limits.requestTimeoutMs);
    } catch (e) {
      this.pendingWs.delete(id);
      const err = e instanceof StreamError ? e : new StreamError("internal", String(e));
      if (err.code === "timeout") safeSend(ws, encodeJson(F.RESET, id, { code: "timeout", message: err.message } satisfies ResetPayload));
      return problem(request, err.status, "WebSocket refused", err.message);
    }
    this.pendingWs.delete(id);
    const pair = new WebSocketPair();
    const pub = pair[0];
    const server = pair[1];
    server.binaryType = "arraybuffer";
    this.ctx.acceptWebSocket(server, ["public", `s:${id}`]);
    server.serializeAttachment({ kind: "public", stream: id, connId: att.connId } satisfies PublicAtt);
    for (const m of pending.early) safeSend(server, m.text ? textDecoder.decode(m.data) : m.data);
    const headers = new Headers();
    if (typeof accept.protocol === "string" && accept.protocol) headers.set("sec-websocket-protocol", accept.protocol);
    return new Response(null, { status: 101, webSocket: pub, headers });
  }

  // ---------- raw TCP (consumer side) ----------

  /**
   * A consumer (`devall tcp connect`) wants a new TCP connection through this tunnel. The upgrade
   * completes only after the publisher connected to its local port (TCP_ACCEPT), so failures reach
   * the consumer as HTTP errors. Frames on the consumer socket use stream 1; this object rewrites
   * stream ids both ways and never buffers beyond the early frames (credit is end to end).
   */
  private async tcpConnect(request: Request): Promise<Response> {
    if (await this.isBlocked()) return jsonError(451, "blocked", "This tunnel was disabled after an abuse report.");
    const h = request.headers;
    const version = Number(h.get("x-devall-version") ?? "1");
    const c = this.client();
    if (!c || c.att.tunnelKind !== "tcp") return jsonError(404, "unknown_tunnel", "No TCP tunnel is online under this name.");
    const owner = await this.ctx.storage.get<Owner>("owner");
    const uid = h.get("x-devall-uid");
    const secret = h.get("x-devall-secret-sha256");
    let via: TcpOpen["via"];
    if (uid && uid === c.att.uid) via = "owner";
    else if (secret && owner?.tcpSecretSha256 && timingSafeHexEqual(secret, owner.tcpSecretSha256)) via = "secret";
    else return jsonError(403, "forbidden", "Use the tunnel owner's account or the tunnel's access secret.");

    const limits = limitsFor(c.att.plan, this.env);
    const now = Date.now();
    const ip = h.get("x-devall-client-ip") || null;
    if (limits.ipRequestsPerMinute !== null) {
      const rl = this.rateLimiter(limits.ipRequestsPerMinute);
      const key = `tcp:${ip ?? "unknown"}`;
      if (!rl.allow(key, now)) return jsonError(429, "rate_limited", "Too many connections; slow down.", {}, { "retry-after": String(rl.retryAfter(key, now)) });
    }
    const open = this.ctx.getWebSockets("tcp").length + this.pendingTcp.size;
    if (limits.tcpStreamsPerTunnel !== null && open >= limits.tcpStreamsPerTunnel) {
      return jsonError(429, "too_many_connections", `This tunnel allows ${limits.tcpStreamsPerTunnel} connections at a time.`, {}, { "retry-after": "5" });
    }
    const usage = rollUsage(c.att.usage, now);
    if (overLimit(totalBytes(usage) + this.memBytes, limits.bandwidthBytesPerMonth)) {
      this.notifyQuota(c.ws, c.att, usage, limits);
      return jsonError(429, "quota_exceeded", "This tunnel used its monthly bandwidth.");
    }

    const id = this.allocId();
    const pending: PendingTcp = { id, connId: c.att.connId, result: deferred<void>(), early: [] };
    this.pendingTcp.set(id, pending);
    if (!safeSend(c.ws, encodeJson(F.TCP_OPEN, id, { clientIp: ip, via } satisfies TcpOpen))) {
      this.pendingTcp.delete(id);
      return jsonError(502, "offline", "The tunnel went offline.");
    }
    this.memRequests += 1;
    try {
      await withTimeout(pending.result.promise, TCP_ACCEPT_TIMEOUT_MS);
    } catch (e) {
      this.pendingTcp.delete(id);
      const err = e instanceof StreamError ? e : new StreamError("internal", String(e));
      if (err.code === "timeout") safeSend(c.ws, encodeJson(F.TCP_CLOSE, id, { reason: "timeout", message: err.message } satisfies TcpClose));
      const status = err.code === "timeout" ? 504 : 502;
      return jsonError(status, err.code, err.message || "The tunnel couldn't open the connection.");
    }
    this.pendingTcp.delete(id);
    const pair = new WebSocketPair();
    const consumer = pair[0];
    const server = pair[1];
    server.binaryType = "arraybuffer";
    this.ctx.acceptWebSocket(server, ["tcp", `t:${id}`]);
    server.serializeAttachment({ kind: "tcp", stream: id, connId: c.att.connId } satisfies TcpAtt);
    for (const f of pending.early) safeSend(server, f);
    return new Response(null, { status: 101, webSocket: consumer, headers: { "sec-websocket-protocol": subprotocolFor(version) } });
  }

  /** Publisher → consumer: the same frame with the consumer's stream id. */
  private toConsumer(f: Frame): void {
    const out = encodeFrame(f.type, TCP_CONSUMER_STREAM, f.payload);
    const sock = this.ctx.getWebSockets(`t:${f.stream}`)[0];
    if (sock) {
      safeSend(sock, out);
      if (f.type === F.TCP_CLOSE) safeClose(sock, 1000, "closed");
      return;
    }
    const p = this.pendingTcp.get(f.stream);
    if (!p) return;
    if (f.type === F.TCP_CLOSE) {
      this.pendingTcp.delete(f.stream);
      let reason: TcpClose = { reason: "reset" };
      try {
        reason = frameJson<TcpClose>(f);
      } catch {
        // keep the default
      }
      p.result.reject(new StreamError(String(reason.reason ?? "reset"), String(reason.message ?? "the tunnel refused the connection"), 502));
      return;
    }
    if (p.early.length < 64) p.early.push(out);
  }

  private fromConsumer(ws: WebSocket, att: TcpAtt, message: string | ArrayBuffer): void {
    if (typeof message === "string") {
      if (message === HEARTBEAT_PING) safeSend(ws, HEARTBEAT_PONG);
      return;
    }
    const c = this.client();
    if (!c || c.att.connId !== att.connId) {
      safeSend(ws, encodeJson(F.TCP_CLOSE, TCP_CONSUMER_STREAM, { reason: "offline", message: "the tunnel went offline" } satisfies TcpClose));
      safeClose(ws, 1001, "tunnel offline");
      return;
    }
    let f: Frame;
    try {
      f = decodeFrame(message);
    } catch {
      safeClose(ws, CLOSE.PROTOCOL_ERROR, "protocol error");
      safeSend(c.ws, encodeJson(F.TCP_CLOSE, att.stream, { reason: "protocol_error" } satisfies TcpClose));
      return;
    }
    switch (f.type) {
      case F.TCP_DATA:
        this.memBytes += f.payload.byteLength;
        safeSend(c.ws, encodeFrame(F.TCP_DATA, att.stream, f.payload));
        if (this.memBytes >= 256 * 1024) this.settle();
        return;
      case F.WINDOW:
      case F.TCP_CLOSE:
        safeSend(c.ws, encodeFrame(f.type, att.stream, f.payload));
        if (f.type === F.TCP_CLOSE) safeClose(ws, 1000, "closed");
        return;
      default:
        return;
    }
  }

  // ---------- socket events ----------

  override async webSocketMessage(ws: WebSocket, raw: string | ArrayBuffer): Promise<void> {
    const message = await messageBytes(raw);
    const att = ws.deserializeAttachment() as ClientAtt | PublicAtt | TcpAtt | null;
    if (att?.kind === "public") return this.fromPublic(ws, att, message);
    if (att?.kind === "tcp") return this.fromConsumer(ws, att, message);
    if (att?.kind !== "client") return;
    if (typeof message === "string") {
      if (message === HEARTBEAT_PING) safeSend(ws, HEARTBEAT_PONG);
      return;
    }
    let f: Frame;
    try {
      f = decodeFrame(message);
      this.onClientFrame(ws, att, f);
    } catch (e) {
      if (e instanceof ProtocolError) {
        safeSend(ws, encodeJson(F.ERROR, 0, { code: "protocol_error", message: e.message, fatal: true }));
        safeClose(ws, CLOSE.PROTOCOL_ERROR, "protocol error");
        return;
      }
      throw e;
    }
  }

  private onClientFrame(ws: WebSocket, att: ClientAtt, f: Frame): void {
    switch (f.type) {
      case F.RESPONSE: {
        const s = this.streams.get(f.stream);
        if (!s || s.headDone) return;
        const head = frameJson<ResponseHead>(f);
        s.headDone = true;
        s.head.resolve(head);
        return;
      }
      case F.RESPONSE_DATA: {
        const s = this.streams.get(f.stream);
        if (!s) return;
        if (!s.headDone) throw new ProtocolError(`RESPONSE_DATA before RESPONSE on stream ${f.stream}`);
        this.memBytes += f.payload.byteLength;
        if (s.discardBody) return;
        s.chunks.push(f.payload);
        s.buffered += f.payload.byteLength;
        if (s.buffered > DEFAULT_WINDOW + MAX_FRAME) {
          safeSend(ws, encodeJson(F.RESET, s.id, { code: "protocol_error", message: "window exceeded" } satisfies ResetPayload));
          this.failStream(s, new StreamError("protocol_error", "the tunnel client ignored flow control"));
          return;
        }
        this.wake(s);
        return;
      }
      case F.RESPONSE_END: {
        const s = this.streams.get(f.stream);
        if (!s) return;
        s.ended = true;
        if (s.discardBody && s.headDone) this.drop(s);
        else this.wake(s);
        return;
      }
      case F.RESET: {
        const r = frameJson<ResetPayload>(f);
        const s = this.streams.get(f.stream);
        if (s) {
          this.failStream(s, new StreamError(String(r.code ?? "internal"), String(r.message ?? "stream reset by the tunnel client")));
          return;
        }
        const p = this.pendingWs.get(f.stream);
        if (p) {
          this.pendingWs.delete(f.stream);
          p.result.reject(new StreamError(String(r.code ?? "internal"), String(r.message ?? "refused"), 502));
          return;
        }
        for (const pub of this.ctx.getWebSockets(`s:${f.stream}`)) safeClose(pub, 1011, "tunnel stream reset");
        return;
      }
      case F.WINDOW: {
        const n = frameU32(f);
        const s = this.streams.get(f.stream);
        if (!s) {
          this.toConsumer(f); // credit for a TCP stream
          return;
        }
        s.credit += n;
        this.wake(s);
        return;
      }
      case F.TCP_ACCEPT: {
        const p = this.pendingTcp.get(f.stream);
        if (p) p.result.resolve();
        return;
      }
      case F.TCP_DATA:
        this.memBytes += f.payload.byteLength;
        this.toConsumer(f);
        if (this.memBytes >= 256 * 1024) this.settle();
        return;
      case F.TCP_CLOSE:
        this.toConsumer(f);
        return;
      case F.WS_ACCEPT: {
        const p = this.pendingWs.get(f.stream);
        if (p) p.result.resolve(frameJson<WsAccept>(f));
        return;
      }
      case F.WS_REJECT: {
        const p = this.pendingWs.get(f.stream);
        if (!p) return;
        const r = frameJson<WsReject>(f);
        this.pendingWs.delete(f.stream);
        const status = Number.isInteger(r.status) && r.status >= 400 && r.status <= 599 ? r.status : 502;
        p.result.reject(new StreamError("rejected", String(r.message ?? "the local server refused the WebSocket"), status));
        return;
      }
      case F.WS_TEXT:
      case F.WS_BINARY: {
        const text = f.type === F.WS_TEXT;
        this.memBytes += f.payload.byteLength;
        const pub = this.ctx.getWebSockets(`s:${f.stream}`)[0];
        if (pub) {
          safeSend(pub, text ? textDecoder.decode(f.payload) : f.payload);
          return;
        }
        this.pendingWs.get(f.stream)?.early.push({ text, data: f.payload.slice() });
        return;
      }
      case F.WS_CLOSE: {
        const c = frameJson<WsClose>(f);
        for (const pub of this.ctx.getWebSockets(`s:${f.stream}`)) safeClose(pub, sendableCloseCode(c.code), String(c.reason ?? ""));
        return;
      }
      case F.PING:
        safeSend(ws, encodeFrame(F.PONG, 0, f.payload.slice(0, 64)));
        return;
      case F.TUNNEL_CONFIG:
        this.ctx.waitUntil(this.applyConfig(ws, att, frameJson<TunnelConfig>(f)));
        return;
      default:
        return; // unknown / not for the server: ignore (forward compatibility)
    }
  }

  private async applyConfig(ws: WebSocket, att: ClientAtt, cfg: TunnelConfig): Promise<void> {
    const owner = await this.ctx.storage.get<Owner>("owner");
    if (!owner || owner.connId !== att.connId) return;
    if (cfg.tcp !== undefined) {
      const sha = cfg.tcp?.secretSha256;
      if (sha !== undefined && sha !== null && (typeof sha !== "string" || !/^[0-9a-f]{64}$/.test(sha))) {
        safeSend(ws, encodeJson(F.ERROR, 0, { code: "invalid_config", message: "tcp.secretSha256 must be 64 lower-case hex characters" }));
        return;
      }
      owner.tcpSecretSha256 = typeof sha === "string" ? sha : null;
      if (cfg.offlineInbox === undefined) {
        await this.ctx.storage.put("owner", owner);
        return;
      }
    }
    let offlineInbox: OfflineInbox | null = null;
    const oi = cfg.offlineInbox;
    if (oi && typeof oi === "object") {
      const prefixes = Array.isArray(oi.pathPrefixes) ? oi.pathPrefixes.filter((p) => typeof p === "string" && p.startsWith("/") && p.length <= 200).slice(0, 20) : [];
      if (typeof oi.inboxId !== "string" || !isValidInboxId(oi.inboxId) || prefixes.length === 0) {
        safeSend(ws, encodeJson(F.ERROR, 0, { code: "invalid_config", message: "offlineInbox needs an inbox id and path prefixes starting with /" }));
        return;
      }
      // Only the account's own inboxes can receive its tunnel's traffic.
      const summary = await this.account(att.uid).summary();
      if (!summary.inboxes.some((i) => i.inboxId === oi.inboxId)) {
        safeSend(ws, encodeJson(F.ERROR, 0, { code: "invalid_config", message: "that inbox doesn't belong to this account" }));
        return;
      }
      offlineInbox = { inboxId: oi.inboxId, pathPrefixes: prefixes };
    }
    owner.offlineInbox = offlineInbox;
    await this.ctx.storage.put("owner", owner);
  }

  private fromPublic(ws: WebSocket, att: PublicAtt, message: string | ArrayBuffer): void {
    const c = this.client();
    if (!c || c.att.connId !== att.connId) {
      safeClose(ws, 1001, "tunnel offline");
      return;
    }
    const text = typeof message === "string";
    const data = text ? textEncoder.encode(message) : new Uint8Array(message);
    if (data.byteLength > MAX_WS_MESSAGE) {
      safeClose(ws, CLOSE.TOO_BIG, "message too big");
      safeSend(c.ws, encodeJson(F.WS_CLOSE, att.stream, { code: CLOSE.TOO_BIG, reason: "message too big" } satisfies WsClose));
      return;
    }
    this.memBytes += data.byteLength;
    safeSend(c.ws, encodeFrame(text ? F.WS_TEXT : F.WS_BINARY, att.stream, data));
    if (this.memBytes >= 256 * 1024) this.settle();
  }

  override async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    const att = ws.deserializeAttachment() as ClientAtt | PublicAtt | TcpAtt | null;
    safeClose(ws, code, reason);
    if (att?.kind === "tcp") {
      const c = this.client();
      if (c && c.att.connId === att.connId) safeSend(c.ws, encodeJson(F.TCP_CLOSE, att.stream, { reason: "eof", message: "the consumer disconnected" } satisfies TcpClose));
      return;
    }
    if (att?.kind === "public") {
      const c = this.client();
      if (c && c.att.connId === att.connId) safeSend(c.ws, encodeJson(F.WS_CLOSE, att.stream, { code: sendableCloseCode(code), reason } satisfies WsClose));
      return;
    }
    if (att?.kind === "client") await this.clientGone(att);
  }

  override async webSocketError(ws: WebSocket): Promise<void> {
    await this.webSocketClose(ws, 1006, "error");
  }

  private async clientGone(att: ClientAtt): Promise<void> {
    const owner = await this.ctx.storage.get<Owner>("owner");
    this.failAll(att.connId, new StreamError("disconnected", "The tunnel client disconnected."));
    for (const pub of this.ctx.getWebSockets("public")) {
      const pa = pub.deserializeAttachment() as PublicAtt | null;
      if (pa?.connId === att.connId) safeClose(pub, 1001, "tunnel offline");
    }
    for (const sock of this.ctx.getWebSockets("tcp")) {
      const ta = sock.deserializeAttachment() as TcpAtt | null;
      if (ta?.connId === att.connId) {
        safeSend(sock, encodeJson(F.TCP_CLOSE, TCP_CONSUMER_STREAM, { reason: "offline", message: "the tunnel went offline" } satisfies TcpClose));
        safeClose(sock, 1001, "tunnel offline");
      }
    }
    // Record what this connection used (the socket is closed, so don't touch its attachment).
    const bytes = att.usage.pendingBytes + this.memBytes;
    const requests = att.usage.pendingRequests + this.memRequests;
    this.memBytes = 0;
    this.memRequests = 0;
    if (bytes > 0 || requests > 0) {
      await this.account(att.uid)
        .addUsage(bytes, requests)
        .catch(() => {});
    }
    if (!owner || owner.connId !== att.connId) return; // a newer connection replaced this one
    owner.disconnectedAt = Date.now();
    await this.ctx.storage.put("owner", owner);
    await this.account(att.uid)
      .tunnelEnded(att.sub, att.connId)
      .catch(() => {});
  }

  override async alarm(): Promise<void> {
    const c = this.client();
    if (!c || c.att.sessionExpiresAt === null) return;
    if (Date.now() >= c.att.sessionExpiresAt - 1000) this.expire(c.ws, c.att);
    else await this.ctx.storage.setAlarm(c.att.sessionExpiresAt);
  }

  private expire(ws: WebSocket, att: ClientAtt): void {
    const limits = limitsFor(att.plan, this.env);
    safeSend(
      ws,
      encodeJson(F.QUOTA, 0, {
        kind: "session",
        used: Math.round((Date.now() - att.sessionStart) / 1000),
        limit: limits.sessionSeconds ?? 0,
        resetAt: null,
        message: "Session time limit reached.",
      } satisfies QuotaPayload),
    );
    safeSend(ws, encodeJson(F.GOAWAY, 0, { reason: "session_expired", reconnect: true, retryAfterMs: 0, message: "Session ended; reconnect to continue." } satisfies GoAway));
    safeClose(ws, CLOSE.SESSION_EXPIRED, "session expired");
  }

  private notifyQuota(ws: WebSocket, att: ClientAtt, usage: UsageSnapshot, limits: Limits): void {
    if (att.quotaNotified) return;
    att.quotaNotified = true;
    ws.serializeAttachment(att);
    safeSend(
      ws,
      encodeJson(F.QUOTA, 0, {
        kind: "bandwidth",
        used: totalBytes(usage) + this.memBytes,
        limit: limits.bandwidthBytesPerMonth ?? 0,
        resetAt: new Date(monthPeriod().end).toISOString(),
        message: "Monthly bandwidth used up; public requests get 429 until the 1st.",
      } satisfies QuotaPayload),
    );
  }

  // ---------- usage accounting ----------

  /** Folds in-memory counters into the client socket's attachment (survives hibernation). */
  private settle(target?: WebSocket): void {
    const ws = target ?? this.client()?.ws;
    if (!ws || (this.memBytes === 0 && this.memRequests === 0)) return;
    const att = ws.deserializeAttachment() as ClientAtt | null;
    if (att?.kind !== "client") return;
    const now = Date.now();
    const u = rollUsage(att.usage, now);
    att.usage = { ...u, pendingBytes: u.pendingBytes + this.memBytes, pendingRequests: u.pendingRequests + this.memRequests };
    this.memBytes = 0;
    this.memRequests = 0;
    ws.serializeAttachment(att);
    if (!target && shouldFlush(att.usage, now)) this.ctx.waitUntil(this.flush(ws).catch(() => {}));
  }

  /** Sends pending usage to the AccountDO ledger and tells the client the new totals. */
  private async flush(ws: WebSocket): Promise<void> {
    const att = ws.deserializeAttachment() as ClientAtt | null;
    if (att?.kind !== "client") return;
    const { pendingBytes, pendingRequests } = att.usage;
    if (pendingBytes === 0 && pendingRequests === 0) return;
    att.usage = { ...att.usage, pendingBytes: 0, pendingRequests: 0, bytes: att.usage.bytes + pendingBytes, requests: att.usage.requests + pendingRequests, lastFlush: Date.now() };
    ws.serializeAttachment(att);
    let total: UsagePayload;
    try {
      total = await this.account(att.uid).addUsage(pendingBytes, pendingRequests);
    } catch (e) {
      const back = ws.deserializeAttachment() as ClientAtt | null;
      if (back?.kind === "client") {
        back.usage = { ...back.usage, pendingBytes: back.usage.pendingBytes + pendingBytes, pendingRequests: back.usage.pendingRequests + pendingRequests };
        try {
          ws.serializeAttachment(back);
        } catch {
          // socket gone
        }
      }
      throw e;
    }
    const latest = ws.deserializeAttachment() as ClientAtt | null;
    if (latest?.kind === "client") {
      latest.usage = { ...latest.usage, bytes: total.bandwidthBytes, requests: total.requests };
      try {
        ws.serializeAttachment(latest);
      } catch {
        // socket gone
      }
    }
    safeSend(ws, encodeJson(F.USAGE, 0, total));
  }
}


/** Constant-time comparison of two hex digests. */
function timingSafeHexEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

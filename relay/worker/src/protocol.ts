// /dev/all relay wire protocol v1 — see relay/PROTOCOL.md (the contract; change it first).
//
// Every protocol message is a binary WebSocket message: [u8 type][u32 stream id, BE][payload].
// Text messages are reserved for the hibernation-friendly heartbeat ("devall-relay:ping" -> "devall-relay:pong").

export const PROTOCOL_VERSION = 1;
export const SUPPORTED_VERSIONS: readonly number[] = [1];
export const SUBPROTOCOL_PREFIX = "devall-relay.";
export const HEADER_LEN = 5;

/**
 * Heartbeat text messages. Answered by the runtime's WebSocket auto-response without waking the
 * Durable Object. The auto-response applies to every socket of the object (also public
 * pass-through sockets), so the strings are deliberately unlikely to appear in an app's traffic.
 */
export const HEARTBEAT_PING = "devall-relay:ping";
export const HEARTBEAT_PONG = "devall-relay:pong";

/** Frame type codes (PROTOCOL.md §2). */
export const F = {
  HELLO: 0x01,
  ERROR: 0x02,
  QUOTA: 0x03,
  PING: 0x04,
  PONG: 0x05,
  GOAWAY: 0x06,
  USAGE: 0x07,
  TUNNEL_CONFIG: 0x08,

  OPEN: 0x10,
  DATA: 0x11,
  END: 0x12,
  RESPONSE: 0x13,
  RESPONSE_DATA: 0x14,
  RESPONSE_END: 0x15,
  RESET: 0x16,
  WINDOW: 0x17,

  WS_OPEN: 0x20,
  WS_ACCEPT: 0x21,
  WS_REJECT: 0x22,
  WS_TEXT: 0x23,
  WS_BINARY: 0x24,
  WS_CLOSE: 0x25,

  INBOX_EVENT: 0x30,
  INBOX_ACK: 0x31,
  INBOX_STATUS: 0x32,
  INBOX_CONFIG: 0x33,

  TCP_OPEN: 0x40,
  TCP_ACCEPT: 0x41,
  TCP_DATA: 0x42,
  TCP_CLOSE: 0x43,
} as const;

export type FrameType = (typeof F)[keyof typeof F];

export const FRAME_NAMES: Record<number, string> = Object.fromEntries(Object.entries(F).map(([k, v]) => [v, k]));

/** WebSocket close codes (PROTOCOL.md §4). */
export const CLOSE = {
  NORMAL: 1000,
  GOING_AWAY: 1001,
  TOO_BIG: 1009,
  PROTOCOL_ERROR: 4000,
  SESSION_EXPIRED: 4008,
  REPLACED: 4009,
  QUOTA: 4029,
  BLOCKED: 4051,
} as const;

export const DEFAULT_WINDOW = 256 * 1024;
export const MAX_FRAME = 64 * 1024;
export const MAX_WS_MESSAGE = 1024 * 1024;
export const HEARTBEAT_SECONDS = 25;

export interface Frame {
  type: number;
  stream: number;
  payload: Uint8Array;
}

export class ProtocolError extends Error {
  override name = "ProtocolError";
}

const enc = new TextEncoder();
const dec = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });

export function encodeFrame(type: number, stream: number, payload?: Uint8Array): Uint8Array {
  if (!Number.isInteger(type) || type < 0 || type > 0xff) throw new ProtocolError(`invalid frame type ${type}`);
  if (!Number.isInteger(stream) || stream < 0 || stream > 0xffffffff) throw new ProtocolError(`invalid stream id ${stream}`);
  const len = payload?.byteLength ?? 0;
  const out = new Uint8Array(HEADER_LEN + len);
  out[0] = type;
  new DataView(out.buffer).setUint32(1, stream, false);
  if (payload && len) out.set(payload, HEADER_LEN);
  return out;
}

export function encodeJson(type: number, stream: number, value: unknown): Uint8Array {
  return encodeFrame(type, stream, enc.encode(JSON.stringify(value)));
}

export function encodeU32(type: number, stream: number, n: number): Uint8Array {
  if (!Number.isInteger(n) || n < 0 || n > 0xffffffff) throw new ProtocolError(`invalid u32 ${n}`);
  const p = new Uint8Array(4);
  new DataView(p.buffer).setUint32(0, n, false);
  return encodeFrame(type, stream, p);
}

export function decodeFrame(data: ArrayBuffer | ArrayBufferView): Frame {
  const bytes =
    data instanceof Uint8Array
      ? data
      : ArrayBuffer.isView(data)
        ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
        : new Uint8Array(data);
  if (bytes.byteLength < HEADER_LEN) throw new ProtocolError(`frame too short (${bytes.byteLength} bytes)`);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return { type: bytes[0]!, stream: view.getUint32(1, false), payload: bytes.subarray(HEADER_LEN) };
}

export function frameJson<T>(f: Frame): T {
  let text: string;
  try {
    text = dec.decode(f.payload);
  } catch {
    throw new ProtocolError(`${FRAME_NAMES[f.type] ?? f.type}: payload is not UTF-8`);
  }
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    throw new ProtocolError(`${FRAME_NAMES[f.type] ?? f.type}: payload is not JSON`);
  }
  if (v === null || typeof v !== "object" || Array.isArray(v)) throw new ProtocolError(`${FRAME_NAMES[f.type] ?? f.type}: payload must be a JSON object`);
  return v as T;
}

export function frameU32(f: Frame): number {
  if (f.payload.byteLength !== 4) throw new ProtocolError(`${FRAME_NAMES[f.type] ?? f.type}: expected a 4-byte payload`);
  return new DataView(f.payload.buffer, f.payload.byteOffset, 4).getUint32(0, false);
}

/**
 * Picks the highest version both sides speak from a `Sec-WebSocket-Protocol` header
 * (`devall-relay.1, devall-relay.2`). Returns null when there is none.
 */
export function negotiateVersion(header: string | null | undefined): number | null {
  if (!header) return null;
  const offered = header
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.startsWith(SUBPROTOCOL_PREFIX))
    .map((s) => Number(s.slice(SUBPROTOCOL_PREFIX.length)))
    .filter((n) => Number.isInteger(n) && n > 0);
  const common = offered.filter((v) => SUPPORTED_VERSIONS.includes(v));
  return common.length ? Math.max(...common) : null;
}

export function subprotocolFor(version: number): string {
  return `${SUBPROTOCOL_PREFIX}${version}`;
}

// ---------- payload types (mirrored in crates/devall-relay/src/protocol.rs) ----------

export type HeaderList = [string, string][];

export type PlanName = "free" | "supporter" | "selfhost";

export interface LimitsPayload {
  sessionSeconds: number | null;
  bandwidthBytesPerMonth: number | null;
  maxRequestBodyBytes: number | null;
  requestTimeoutMs: number;
  maxTunnels: number | null;
  interstitial: boolean;
  inboxRetentionSeconds: number;
  inboxEventsPerDay: number | null;
  inboxMaxBuffered: number | null;
  inboxMaxBodyBytes: number;
  maxInboxes: number | null;
  reservedSubdomains: number | null;
  ipRequestsPerMinute: number | null;
  // ---- Batch D (additive; older clients ignore them) ----
  /** Custom hostnames pointing at reserved subdomains (0 = not on this plan). */
  customDomains: number | null;
  /** Raw TCP tunnels (TCP over WebSocket) may be published. */
  tcpTunnels: boolean;
  /** Concurrent TCP connections per TCP tunnel. */
  tcpStreamsPerTunnel: number | null;
  /** Published (hosted) mocks per account. */
  hostedMocks: number | null;
  mockMaxStubs: number | null;
  /** Mappings + files, bytes. */
  mockMaxBytes: number | null;
  mockRequestsPerDay: number | null;
  /** Request journal entries kept per hosted mock. */
  mockJournalSize: number;
  /** Active shared snapshots per account. */
  shares: number | null;
  shareMaxBytes: number;
  /** Longest expiry a share may ask for. */
  shareTtlSeconds: number;
  monitors: number | null;
  monitorMinIntervalMinutes: number;
  monitorMaxRequests: number;
  /** Results kept per monitor (history / uptime window). */
  monitorHistory: number;
  // ---- cloud load-generator regions (additive) ----
  /** Regions one cloud load run may use (0 = not on this plan). */
  loadRegions: number;
  loadMaxDurationSeconds: number;
  /** Requests per second across all regions of a run (null = unlimited, self-host). */
  loadMaxRps: number | null;
  loadRunsPerDay: number | null;
  /** Private, loopback and link-local targets (self-host only; still need verification). */
  loadPrivateTargets: boolean;
}

export interface UsagePayload {
  bandwidthBytes: number;
  requests: number;
  periodStart: string;
  periodEnd: string;
}

export interface Hello {
  version: number;
  kind: "tunnel" | "inbox" | "tcp";
  sessionId: string;
  publicUrl: string;
  publicUrls: string[];
  subdomain: string | null;
  inboxId: string | null;
  plan: PlanName;
  limits: LimitsPayload;
  usage: UsagePayload;
  sessionExpiresAt: string | null;
  window: number;
  maxFrame: number;
  maxWsMessage: number;
  heartbeatSeconds: number;
  serverTime: string;
}

export interface ErrorPayload {
  code: string;
  message: string;
  fatal?: boolean;
}

export interface QuotaPayload {
  kind: "bandwidth" | "session" | "inbox_daily" | "inbox_buffer" | "rate";
  used: number;
  limit: number;
  resetAt?: string | null;
  message: string;
}

export interface GoAway {
  reason: "session_expired" | "replaced" | "blocked" | "server_restart" | "quota";
  reconnect: boolean;
  retryAfterMs?: number;
  message?: string;
}

export interface TunnelConfig {
  offlineInbox?: { inboxId: string; pathPrefixes: string[] } | null;
  /** TCP tunnels: who may connect besides the owner's account (PROTOCOL.md §2.6). */
  tcp?: { secretSha256?: string | null } | null;
}

export interface RequestHead {
  method: string;
  url: string;
  path: string;
  headers: HeaderList;
  clientIp: string | null;
  bodyLength: number | null;
}

export interface ResponseHead {
  status: number;
  headers: HeaderList;
}

export interface ResetPayload {
  code: "timeout" | "cancelled" | "too_large" | "local_unreachable" | "protocol_error" | "quota" | "internal" | string;
  message: string;
}

export interface WsOpen {
  url: string;
  path: string;
  headers: HeaderList;
  protocols: string[];
  clientIp: string | null;
}

export interface WsAccept {
  protocol?: string | null;
}

export interface WsReject {
  status: number;
  message: string;
}

export interface WsClose {
  code: number;
  reason: string;
}

/** The event shape handed to the desktop (and to devall-webhooks `ingest`). */
export interface InboxEvent {
  id: string;
  seq: number;
  inboxId: string;
  method: string;
  url: string;
  path: string;
  headers: HeaderList;
  bodyBase64: string;
  bodySize: number;
  receivedAt: string;
  sourceIp: string | null;
}

export interface InboxAck {
  ids: string[];
}

export interface InboxResponseConfig {
  status: number;
  headers: HeaderList;
  body: string;
}

export interface InboxConfig {
  response?: InboxResponseConfig | null;
}

export interface InboxStatus {
  buffered: number;
  inFlight: number;
  todayCount: number;
  dailyLimit: number | null;
  rejectedToday: number;
  retentionSeconds: number;
  oldestReceivedAt: string | null;
  response: InboxResponseConfig;
}

// ---------- raw TCP tunnels (PROTOCOL.md §2.6) ----------

export interface TcpOpen {
  clientIp: string | null;
  /** How the consumer authenticated: the tunnel owner's account or the tunnel's access secret. */
  via: "owner" | "secret";
}

export interface TcpClose {
  reason: "eof" | "reset" | "local_unreachable" | "timeout" | "offline" | "protocol_error" | string;
  message?: string;
}

/** Stream id used on a TCP consumer socket (the relay rewrites it to the publisher's stream id). */
export const TCP_CONSUMER_STREAM = 1;

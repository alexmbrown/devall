// Header handling shared by the tunnel and the inbox.

import type { HeaderList } from "./protocol";

/** Hop-by-hop headers (RFC 9110 §7.6.1) never forwarded in either direction. */
export const HOP_BY_HOP = new Set([
  "connection",
  "keep-alive",
  "proxy-connection",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

/** Headers the edge adds that the local app shouldn't see as if the caller had sent them. */
const EDGE_PREFIXES = ["cf-", "x-devall-"];
const EDGE_HEADERS = new Set(["cdn-loop", "x-forwarded-for", "x-forwarded-proto", "x-forwarded-host", "x-real-ip"]);

function isEdgeHeader(name: string): boolean {
  return EDGE_HEADERS.has(name) || EDGE_PREFIXES.some((p) => name.startsWith(p));
}

/** Request headers forwarded to the desktop (ordered as the runtime exposes them). */
export function forwardRequestHeaders(h: Headers, opts: { websocket?: boolean } = {}): HeaderList {
  const out: HeaderList = [];
  for (const [k, v] of h) {
    const name = k.toLowerCase();
    if (HOP_BY_HOP.has(name) || isEdgeHeader(name)) continue;
    if (opts.websocket && name.startsWith("sec-websocket-")) continue;
    out.push([name, v]);
  }
  return out;
}

/** Strips relay-internal headers from a public request before it reaches a Durable Object. */
export function stripInternal(request: Request): Request {
  let dirty = false;
  for (const k of request.headers.keys()) if (k.toLowerCase().startsWith("x-devall-")) dirty = true;
  if (!dirty) return request;
  const headers = new Headers(request.headers);
  for (const k of [...headers.keys()]) if (k.toLowerCase().startsWith("x-devall-")) headers.delete(k);
  return new Request(request, { headers });
}

/** Builds response headers from the desktop's list (hop-by-hop and content-length dropped). */
export function responseHeaders(list: HeaderList): Headers {
  const h = new Headers();
  for (const pair of list) {
    if (!Array.isArray(pair) || pair.length !== 2) continue;
    const [k, v] = pair;
    if (typeof k !== "string" || typeof v !== "string") continue;
    const name = k.toLowerCase();
    if (HOP_BY_HOP.has(name) || name === "content-length") continue;
    try {
      h.append(name, v);
    } catch {
      // invalid header name/value from the local app: drop it rather than fail the response
    }
  }
  return h;
}

export function contentLength(h: Headers): number | null {
  const v = h.get("content-length");
  if (v === null || !/^\d+$/.test(v.trim())) return null;
  return Number(v.trim());
}

export function clientIp(request: Request): string | null {
  return request.headers.get("cf-connecting-ip") ?? request.headers.get("x-real-ip") ?? null;
}

/** Status codes that must not carry a body. */
export function nullBodyStatus(status: number): boolean {
  return status === 204 || status === 205 || status === 304 || (status >= 100 && status < 200);
}

export const NOINDEX = "noindex, nofollow, noarchive";

export function jsonError(status: number, code: string, message: string, extra: Record<string, unknown> = {}, headers: HeadersInit = {}): Response {
  const h = new Headers(headers);
  h.set("content-type", "application/json; charset=utf-8");
  h.set("cache-control", "no-store");
  h.set("x-robots-tag", NOINDEX);
  return new Response(JSON.stringify({ error: { code, message, ...extra } }), { status, headers: h });
}

export function json(value: unknown, status = 200, headers: HeadersInit = {}): Response {
  const h = new Headers(headers);
  h.set("content-type", "application/json; charset=utf-8");
  h.set("cache-control", "no-store");
  return new Response(JSON.stringify(value), { status, headers: h });
}

export function isWebSocketUpgrade(request: Request): boolean {
  return (request.headers.get("upgrade") ?? "").toLowerCase() === "websocket";
}

/** Hibernation handlers get ArrayBuffers; runtimes with the standard binaryType may hand over Blobs. */
export async function messageBytes(message: string | ArrayBuffer): Promise<string | ArrayBuffer> {
  const m = message as unknown;
  if (typeof Blob !== "undefined" && m instanceof Blob) return m.arrayBuffer();
  return message;
}

/** Sends on a WebSocket, swallowing errors from sockets that closed meanwhile. */
export function safeSend(ws: WebSocket | null | undefined, data: string | ArrayBuffer | ArrayBufferView): boolean {
  if (!ws) return false;
  try {
    ws.send(data);
    return true;
  } catch {
    return false;
  }
}

/** Close codes 1005/1006/1015 are reserved and can't be sent; map them to something sendable. */
export function sendableCloseCode(code: number | undefined): number {
  if (code === undefined || code === 1005 || code === 1006 || code === 1015) return 1000;
  if (code < 1000 || code > 4999 || (code >= 1016 && code < 3000)) return 1000;
  return code;
}

export function safeClose(ws: WebSocket | null | undefined, code?: number, reason?: string): void {
  if (!ws) return;
  try {
    ws.close(sendableCloseCode(code), (reason ?? "").slice(0, 120));
  } catch {
    // already closed
  }
}

export function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  return btoa(bin);
}

export function base64ToBytes(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Reads a body up to `max` bytes; returns null when it is larger. */
export async function readBodyCapped(body: ReadableStream<Uint8Array> | null, max: number): Promise<Uint8Array | null> {
  if (!body) return new Uint8Array(0);
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      await reader.cancel().catch(() => {});
      return null;
    }
    chunks.push(value);
  }
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.byteLength;
  }
  return out;
}

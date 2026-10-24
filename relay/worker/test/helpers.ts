// Test helpers: a mocked desktop client speaking the relay protocol over a real WebSocket to the
// Worker running in workerd (Miniflare).

import { SELF } from "cloudflare:test";
import { decodeFrame, encodeFrame, encodeJson, encodeU32, F, FRAME_NAMES, frameJson, type Frame } from "../src/protocol";

export const DOMAIN = "devall.test";
export const RELAY = `https://relay.${DOMAIN}`;

/** Static tokens configured in vitest.config.ts (plan in parentheses). */
export const TOKENS = {
  alice: "alice-token-0123456789", // free
  bob: "bob-token-0123456789x", // free
  sam: "sam-token-0123456789xx", // supporter
  root: "root-token-0123456789xx", // selfhost
} as const;

export function uidOf(name: keyof typeof TOKENS): string {
  return `static-${name}`;
}

export async function waitFor<T>(fn: () => T | undefined | null | false, timeoutMs = 5000, what = "condition"): Promise<T> {
  const start = Date.now();
  for (;;) {
    const v = fn();
    if (v) return v;
    if (Date.now() - start > timeoutMs) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 5));
  }
}

export class MockClient {
  readonly frames: Frame[] = [];
  readonly texts: string[] = [];
  closed: { code: number; reason: string } | null = null;

  constructor(readonly ws: WebSocket, readonly response: Response) {
    ws.binaryType = "arraybuffer";
    ws.addEventListener("message", (e: MessageEvent) => {
      if (typeof e.data === "string") this.texts.push(e.data);
      else this.frames.push(decodeFrame(e.data as ArrayBuffer));
    });
    ws.addEventListener("close", (e: CloseEvent) => {
      this.closed = { code: e.code, reason: e.reason };
    });
    ws.accept();
  }

  /** Opens a client socket; returns the HTTP response instead when the upgrade is refused. */
  static async open(token: string | null, path: string, protocols = "devall-relay.1"): Promise<MockClient | Response> {
    const headers: Record<string, string> = { upgrade: "websocket" };
    if (protocols) headers["sec-websocket-protocol"] = protocols;
    if (token) headers.authorization = `Bearer ${token}`;
    const res = await SELF.fetch(`${RELAY}${path}`, { headers });
    if (res.status !== 101 || !res.webSocket) return res;
    return new MockClient(res.webSocket, res);
  }

  static async connect(token: string, path = "/connect"): Promise<MockClient> {
    const c = await MockClient.open(token, path);
    if (c instanceof Response) throw new Error(`connect failed: ${c.status} ${await c.text()}`);
    return c;
  }

  /** Removes and returns the first frame matching `type` (and `stream`), waiting for it. */
  async next(type: number, stream?: number, timeoutMs = 5000): Promise<Frame> {
    const match = (f: Frame) => f.type === type && (stream === undefined || f.stream === stream);
    await waitFor(() => this.frames.some(match), timeoutMs, `${FRAME_NAMES[type] ?? type}${stream !== undefined ? ` on stream ${stream}` : ""}`);
    const i = this.frames.findIndex(match);
    return this.frames.splice(i, 1)[0]!;
  }

  async nextJson<T>(type: number, stream?: number, timeoutMs?: number): Promise<{ frame: Frame; body: T }> {
    const frame = await this.next(type, stream, timeoutMs);
    return { frame, body: frameJson<T>(frame) };
  }

  has(type: number, stream?: number): boolean {
    return this.frames.some((f) => f.type === type && (stream === undefined || f.stream === stream));
  }

  send(type: number, stream: number, payload?: Uint8Array | string): void {
    const bytes = typeof payload === "string" ? new TextEncoder().encode(payload) : payload;
    this.ws.send(encodeFrame(type, stream, bytes));
  }

  sendJson(type: number, stream: number, value: unknown): void {
    this.ws.send(encodeJson(type, stream, value));
  }

  window(stream: number, n: number): void {
    this.ws.send(encodeU32(F.WINDOW, stream, n));
  }

  /** Answers an OPEN on `stream` with a complete response. */
  respond(stream: number, status: number, headers: [string, string][], body?: string | Uint8Array): void {
    this.sendJson(F.RESPONSE, stream, { status, headers });
    if (body !== undefined && (typeof body === "string" ? body.length : body.byteLength)) this.send(F.RESPONSE_DATA, stream, body);
    this.send(F.RESPONSE_END, stream);
  }

  async waitClosed(timeoutMs = 5000): Promise<{ code: number; reason: string }> {
    return waitFor(() => this.closed, timeoutMs, "socket close");
  }

  close(): void {
    try {
      this.ws.close(1000, "bye");
    } catch {
      // already closed
    }
  }
}

export function concat(chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((n, c) => n + c.byteLength, 0);
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.byteLength;
  }
  return out;
}

export const text = (b: Uint8Array) => new TextDecoder().decode(b);

import type { Limits, Plan } from "./plans";
import { DEFAULT_WINDOW, HEARTBEAT_SECONDS, MAX_FRAME, MAX_WS_MESSAGE, type Hello, type UsagePayload } from "./protocol";

export function buildHello(a: {
  version: number;
  kind: "tunnel" | "inbox" | "tcp";
  sessionId: string;
  publicUrl: string;
  /** Extra addresses that reach the same tunnel (custom domains). */
  extraUrls?: string[];
  subdomain: string | null;
  inboxId: string | null;
  plan: Plan;
  limits: Limits;
  usage: UsagePayload;
  sessionExpiresAt: number | null;
}): Hello {
  return {
    version: a.version,
    kind: a.kind,
    sessionId: a.sessionId,
    publicUrl: a.publicUrl,
    publicUrls: [a.publicUrl, ...(a.extraUrls ?? []).filter((u) => u !== a.publicUrl)],
    subdomain: a.subdomain,
    inboxId: a.inboxId,
    plan: a.plan,
    limits: a.limits,
    usage: a.usage,
    sessionExpiresAt: a.sessionExpiresAt === null ? null : new Date(a.sessionExpiresAt).toISOString(),
    window: DEFAULT_WINDOW,
    maxFrame: MAX_FRAME,
    maxWsMessage: MAX_WS_MESSAGE,
    heartbeatSeconds: HEARTBEAT_SECONDS,
    serverTime: new Date().toISOString(),
  };
}

export class StreamError extends Error {
  override name = "StreamError";
  constructor(
    readonly code: string,
    message: string,
    readonly status = 502,
  ) {
    super(message);
  }
}

/** Resolves/rejects like `p`, or rejects with a `timeout` StreamError after `ms`. */
export function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const t = setTimeout(() => reject(new StreamError("timeout", `no answer within ${ms} ms`, 504)), ms);
    p.then(
      (v) => {
        clearTimeout(t);
        resolve(v);
      },
      (e: unknown) => {
        clearTimeout(t);
        reject(e);
      },
    );
  });
}

export function deferred<T>(): { promise: Promise<T>; resolve(v: T): void; reject(e: unknown): void } {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  // Avoid unhandled-rejection noise when nobody awaits a failed stream.
  promise.catch(() => {});
  return { promise, resolve, reject };
}

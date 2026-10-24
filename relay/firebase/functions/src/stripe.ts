// Stripe webhook verification and the mapping from Stripe events to /dev/all plans.
// No Stripe SDK: signatures are HMAC-SHA256 over `${timestamp}.${raw body}` (Stripe docs,
// "Verify webhook signatures manually"), and only a handful of event fields are read.

import { createHmac, timingSafeEqual } from "node:crypto";

export type Plan = "free" | "supporter";

export class SignatureError extends Error {
  override name = "SignatureError";
}

export interface StripeEvent {
  id: string;
  type: string;
  created: number;
  livemode?: boolean;
  data: { object: Record<string, unknown> };
}

/** Parses `Stripe-Signature: t=…,v1=…[,v1=…][,v0=…]`. */
export function parseSignatureHeader(header: string): { timestamp: number; signatures: string[] } {
  let timestamp = NaN;
  const signatures: string[] = [];
  for (const part of header.split(",")) {
    const i = part.indexOf("=");
    if (i <= 0) continue;
    const k = part.slice(0, i).trim();
    const v = part.slice(i + 1).trim();
    if (k === "t") timestamp = Number(v);
    else if (k === "v1") signatures.push(v); // only v1 is valid; ignoring others prevents downgrades
  }
  if (!Number.isInteger(timestamp) || signatures.length === 0) throw new SignatureError("malformed Stripe-Signature header");
  return { timestamp, signatures };
}

export function signPayload(payload: string | Buffer, secret: string, timestamp: number): string {
  const body = typeof payload === "string" ? payload : payload.toString("utf8");
  return createHmac("sha256", secret).update(`${timestamp}.${body}`, "utf8").digest("hex");
}

/**
 * Verifies a webhook and returns the parsed event. `rawBody` must be the exact bytes Stripe sent
 * (`req.rawBody` in Cloud Functions), never a re-serialized object.
 */
export function verifyStripeWebhook(rawBody: string | Buffer, header: string | undefined, secret: string, opts: { toleranceSeconds?: number; now?: number } = {}): StripeEvent {
  if (!secret) throw new SignatureError("webhook secret is not configured");
  if (!header) throw new SignatureError("missing Stripe-Signature header");
  const { timestamp, signatures } = parseSignatureHeader(header);
  const now = opts.now ?? Math.floor(Date.now() / 1000);
  const tolerance = opts.toleranceSeconds ?? 300;
  if (Math.abs(now - timestamp) > tolerance) throw new SignatureError("timestamp outside the tolerance window");
  const expected = Buffer.from(signPayload(rawBody, secret, timestamp), "hex");
  const ok = signatures.some((s) => {
    const got = Buffer.from(s, "hex");
    return got.length === expected.length && timingSafeEqual(got, expected);
  });
  if (!ok) throw new SignatureError("no matching signature");
  const body = typeof rawBody === "string" ? rawBody : rawBody.toString("utf8");
  const event = JSON.parse(body) as StripeEvent;
  if (!event || typeof event.id !== "string" || typeof event.type !== "string" || !event.data?.object) throw new SignatureError("not a Stripe event");
  return event;
}

/** Subscription statuses that keep Supporter (past_due: Stripe is still retrying the card). */
export const ACTIVE_STATUSES = new Set(["active", "trialing", "past_due"]);

export interface PlanChange {
  uid: string | null;
  customerId: string | null;
  subscriptionId: string | null;
  plan: Plan;
  status: string;
  currentPeriodEnd: number | null;
  eventId: string;
  eventCreated: number;
}

const str = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const idOf = (v: unknown): string | null => str(v) ?? (v && typeof v === "object" ? str((v as { id?: unknown }).id) : null);

/** What an event means for the plan, or null for events we don't care about. */
export function planChangeFromEvent(event: StripeEvent): PlanChange | null {
  const o = event.data.object;
  const base = { eventId: event.id, eventCreated: event.created };
  switch (event.type) {
    case "checkout.session.completed":
    case "checkout.session.async_payment_succeeded": {
      if (o.mode !== "subscription") return null;
      const paid = o.payment_status === "paid" || o.payment_status === "no_payment_required";
      if (!paid) return null;
      return {
        ...base,
        uid: str(o.client_reference_id),
        customerId: idOf(o.customer),
        subscriptionId: idOf(o.subscription),
        plan: "supporter",
        status: "active",
        currentPeriodEnd: null,
      };
    }
    case "customer.subscription.created":
    case "customer.subscription.updated":
    case "customer.subscription.resumed":
    case "customer.subscription.paused":
    case "customer.subscription.deleted": {
      const status = event.type === "customer.subscription.deleted" ? "canceled" : (str(o.status) ?? "unknown");
      const items = (o.items as { data?: { current_period_end?: unknown }[] } | undefined)?.data;
      const periodEnd = typeof o.current_period_end === "number" ? o.current_period_end : typeof items?.[0]?.current_period_end === "number" ? (items[0].current_period_end as number) : null;
      const metadata = (o.metadata ?? {}) as Record<string, unknown>;
      return {
        ...base,
        uid: str(metadata.uid) ?? str(metadata.firebaseUID),
        customerId: idOf(o.customer),
        subscriptionId: str(o.id),
        plan: ACTIVE_STATUSES.has(status) ? "supporter" : "free",
        status,
        currentPeriodEnd: periodEnd,
      };
    }
    default:
      return null;
  }
}

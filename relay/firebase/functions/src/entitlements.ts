// Applies plan changes everywhere the relay looks: Firebase custom claim (`plan`), the
// `entitlements/{uid}` document the user can read, and the Worker's KV (`plan:<uid>`) so the edge
// sees upgrades and cancellations before the user's next ID-token refresh.

import { planChangeFromEvent, type Plan, type StripeEvent } from "./stripe.js";

export interface Entitlement {
  plan: Plan;
  status: string;
  customerId: string | null;
  subscriptionId: string | null;
  currentPeriodEnd: number | null;
  subdomains?: string[];
  lastEventId: string;
  lastEventCreated: number;
  updatedAt: number;
}

/** Storage the logic needs (Firestore/Auth/KV in production, in-memory fakes in tests). */
export interface EntitlementDeps {
  getEntitlement(uid: string): Promise<Entitlement | null>;
  /** Merge-writes the entitlement document. */
  setEntitlement(uid: string, e: Partial<Entitlement>): Promise<void>;
  uidForCustomer(customerId: string): Promise<string | null>;
  linkCustomer(customerId: string, uid: string): Promise<void>;
  getClaims(uid: string): Promise<Record<string, unknown>>;
  setClaims(uid: string, claims: Record<string, unknown>): Promise<void>;
  kvPut(key: string, value: string): Promise<void>;
  now?(): number;
}

export type ApplyResult = { applied: true; uid: string; plan: Plan } | { applied: false; reason: string };

export async function applyStripeEvent(event: StripeEvent, deps: EntitlementDeps): Promise<ApplyResult> {
  const change = planChangeFromEvent(event);
  if (!change) return { applied: false, reason: `ignored ${event.type}` };
  let uid = change.uid;
  if (uid && change.customerId) await deps.linkCustomer(change.customerId, uid);
  if (!uid && change.customerId) uid = await deps.uidForCustomer(change.customerId);
  if (!uid) return { applied: false, reason: "no /dev/all account linked to this customer (checkout without client_reference_id?)" };

  const current = await deps.getEntitlement(uid);
  // Stripe doesn't guarantee ordering: never let an older event undo a newer one.
  if (current && current.lastEventCreated > change.eventCreated) return { applied: false, reason: "stale event" };

  const now = deps.now?.() ?? Date.now();
  await deps.setEntitlement(uid, {
    plan: change.plan,
    status: change.status,
    customerId: change.customerId ?? current?.customerId ?? null,
    subscriptionId: change.subscriptionId ?? current?.subscriptionId ?? null,
    currentPeriodEnd: change.currentPeriodEnd ?? current?.currentPeriodEnd ?? null,
    lastEventId: change.eventId,
    lastEventCreated: change.eventCreated,
    updatedAt: now,
  });
  await setPlan(uid, change.plan, deps, now);
  return { applied: true, uid, plan: change.plan };
}

/** Sets the custom claim (keeping other claims) and pushes the plan to the edge. */
export async function setPlan(uid: string, plan: Plan, deps: Pick<EntitlementDeps, "getClaims" | "setClaims" | "kvPut">, now = Date.now()): Promise<void> {
  const claims = await deps.getClaims(uid);
  if (claims.plan !== plan) await deps.setClaims(uid, { ...claims, plan });
  await deps.kvPut(`plan:${uid}`, JSON.stringify({ plan, updatedAt: now }));
}

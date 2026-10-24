// /dev/all relay — accounts and billing (Cloud Functions for Firebase, 2nd gen).
// Firebase never carries tunnel traffic: it only signs users in, records who pays, and pushes
// entitlements + reserved subdomains to the Cloudflare edge (Workers KV).
//
// Copyright the /dev/all authors. Licensed under the GNU AGPL-3.0 (see relay/LICENSE).

import { initializeApp } from "firebase-admin/app";
import { getAuth } from "firebase-admin/auth";
import { FieldValue, getFirestore } from "firebase-admin/firestore";
import { logger } from "firebase-functions";
import { defineSecret, defineString } from "firebase-functions/params";
import { HttpsError, onCall, onRequest } from "firebase-functions/v2/https";
import { onSchedule } from "firebase-functions/v2/scheduler";
import { createCustomHostname, deleteCustomHostname, findCustomHostname, getCustomHostname, kvDelete, kvPut, type KvConfig, type SaasConfig } from "./cloudflare.js";
import { addCustomDomain as addDomain, DomainError, refreshCustomDomain as refreshDomain, removeCustomDomain as removeDomain, subdomainInUse, type CustomDomain, type CustomDomainDeps } from "./customdomains.js";
import { applyStripeEvent, type Entitlement, type EntitlementDeps } from "./entitlements.js";
import { SignatureError, verifyStripeWebhook } from "./stripe.js";
import { decideClaim, validateSubdomain } from "./subdomains.js";

initializeApp();
const db = getFirestore();

const STRIPE_WEBHOOK_SECRET = defineSecret("STRIPE_WEBHOOK_SECRET");
const CF_API_TOKEN = defineSecret("CF_API_TOKEN");
const CF_ACCOUNT_ID = defineString("CF_ACCOUNT_ID");
const CF_KV_NAMESPACE_ID = defineString("CF_KV_NAMESPACE_ID");
/** Custom domains (Cloudflare for SaaS): the tunnel zone, its domain, and the CNAME target customers use. */
const CF_ZONE_ID = defineString("CF_ZONE_ID", { default: "" });
const PUBLIC_DOMAIN = defineString("PUBLIC_DOMAIN", { default: "devall.dev" });
const CUSTOM_DOMAIN_TARGET = defineString("CUSTOM_DOMAIN_TARGET", { default: "" });
const REGION = "us-central1";

function kvConfig(): KvConfig {
  return { accountId: CF_ACCOUNT_ID.value(), namespaceId: CF_KV_NAMESPACE_ID.value(), apiToken: CF_API_TOKEN.value() };
}

function deps(): EntitlementDeps {
  const kv = kvConfig();
  return {
    async getEntitlement(uid) {
      const snap = await db.doc(`entitlements/${uid}`).get();
      return snap.exists ? (snap.data() as Entitlement) : null;
    },
    async setEntitlement(uid, e) {
      await db.doc(`entitlements/${uid}`).set(e, { merge: true });
    },
    async uidForCustomer(customerId) {
      const snap = await db.doc(`stripeCustomers/${customerId}`).get();
      return snap.exists ? ((snap.get("uid") as string | undefined) ?? null) : null;
    },
    async linkCustomer(customerId, uid) {
      await db.doc(`stripeCustomers/${customerId}`).set({ uid, linkedAt: FieldValue.serverTimestamp() }, { merge: true });
    },
    async getClaims(uid) {
      return (await getAuth().getUser(uid)).customClaims ?? {};
    },
    async setClaims(uid, claims) {
      await getAuth().setCustomUserClaims(uid, claims);
    },
    async kvPut(key, value) {
      await kvPut(kv, key, value);
    },
  };
}

/**
 * Stripe → plan. Point a Stripe webhook endpoint at this function's URL (use the
 * cloudfunctions.net / run.app URL, not a Hosting rewrite) with the events:
 * checkout.session.completed, checkout.session.async_payment_succeeded,
 * customer.subscription.created|updated|deleted|paused|resumed.
 */
export const stripeWebhook = onRequest({ region: REGION, secrets: [STRIPE_WEBHOOK_SECRET, CF_API_TOKEN], maxInstances: 5, cors: false }, async (req, res) => {
  if (req.method !== "POST") {
    res.status(405).send("POST only");
    return;
  }
  let event;
  try {
    event = verifyStripeWebhook(req.rawBody, req.header("stripe-signature"), STRIPE_WEBHOOK_SECRET.value());
  } catch (e) {
    if (e instanceof SignatureError) {
      logger.warn("stripe signature rejected", { reason: e.message });
      res.status(400).send(`signature: ${e.message}`);
      return;
    }
    throw e;
  }
  try {
    const result = await applyStripeEvent(event, deps());
    logger.info("stripe event", { id: event.id, type: event.type, ...result });
    res.status(200).json({ received: true, ...result });
  } catch (e) {
    // 500 → Stripe retries with backoff, which is what we want for transient KV/Firestore errors.
    logger.error("stripe event failed", { id: event.id, type: event.type, error: String(e) });
    res.status(500).send("failed");
  }
});

async function currentPlan(uid: string, tokenPlan: unknown): Promise<string> {
  const snap = await db.doc(`entitlements/${uid}`).get();
  const p = snap.exists ? snap.get("plan") : undefined;
  return typeof p === "string" ? p : tokenPlan === "supporter" ? "supporter" : "free";
}

async function pushSubdomains(uid: string, subdomains: string[], added: string | null, removed: string | null): Promise<void> {
  const kv = kvConfig();
  if (added) await kvPut(kv, `sub:${added}`, JSON.stringify({ uid, claimedAt: Date.now() }));
  if (removed) await kvDelete(kv, `sub:${removed}`);
  await kvPut(kv, `subs:${uid}`, JSON.stringify(subdomains));
}

/** Supporter only: reserve `name` → `https://<name>.<domain>` for this account. */
export const claimSubdomain = onCall({ region: REGION, secrets: [CF_API_TOKEN], maxInstances: 5 }, async (request) => {
  const uid = request.auth?.uid;
  if (!uid) throw new HttpsError("unauthenticated", "Sign in first.");
  const plan = await currentPlan(uid, request.auth?.token.plan);
  const name = (request.data as { name?: unknown } | undefined)?.name;
  const v = validateSubdomain(name);
  if (!v.ok) throw new HttpsError("invalid-argument", v.reason);

  const decision = await db.runTransaction(async (tx) => {
    const subRef = db.doc(`subdomains/${v.name}`);
    const entRef = db.doc(`entitlements/${uid}`);
    const [sub, ent] = await Promise.all([tx.get(subRef), tx.get(entRef)]);
    const d = decideClaim({
      uid,
      plan,
      name: v.name,
      currentOwner: sub.exists ? ((sub.get("uid") as string | undefined) ?? null) : null,
      userSubdomains: ent.exists ? ((ent.get("subdomains") as string[] | undefined) ?? []) : [],
    });
    if (d.ok && d.changed) {
      tx.set(subRef, { uid, createdAt: FieldValue.serverTimestamp() });
      tx.set(entRef, { subdomains: d.subdomains, updatedAt: Date.now() }, { merge: true });
    }
    return d;
  });
  if (!decision.ok) throw new HttpsError(decision.code, decision.message);
  try {
    await pushSubdomains(uid, decision.subdomains, decision.name, null);
  } catch (e) {
    logger.error("kv push failed", { uid, name: decision.name, error: String(e) });
    throw new HttpsError("unavailable", "Saved, but the relay couldn't be updated yet. Try again in a minute.");
  }
  return { name: decision.name, subdomains: decision.subdomains };
});

function saasConfig(): SaasConfig {
  return { zoneId: CF_ZONE_ID.value(), apiToken: CF_API_TOKEN.value() };
}

function domainDeps(): CustomDomainDeps {
  const kv = kvConfig();
  const saas = saasConfig();
  const publicDomain = PUBLIC_DOMAIN.value();
  return {
    async plan(uid) {
      return currentPlan(uid, undefined);
    },
    async reservedSubdomains(uid) {
      const snap = await db.doc(`entitlements/${uid}`).get();
      return snap.exists ? ((snap.get("subdomains") as string[] | undefined) ?? []) : [];
    },
    async getDomain(hostname) {
      const snap = await db.doc(`customDomains/${hostname}`).get();
      return snap.exists ? (snap.data() as CustomDomain) : null;
    },
    async listDomains(uid) {
      const q = await db.collection("customDomains").where("uid", "==", uid).get();
      return q.docs.map((d) => d.data() as CustomDomain);
    },
    async claim(d) {
      return db.runTransaction(async (tx) => {
        const ref = db.doc(`customDomains/${d.hostname}`);
        const cur = await tx.get(ref);
        if (cur.exists && cur.get("uid") !== d.uid) return false;
        tx.set(ref, d);
        tx.set(db.doc(`entitlements/${d.uid}`), { customDomains: FieldValue.arrayUnion(d.hostname), updatedAt: Date.now() }, { merge: true });
        return true;
      });
    },
    async saveDomain(d) {
      await db.doc(`customDomains/${d.hostname}`).set(d);
    },
    async deleteDomain(hostname) {
      const ref = db.doc(`customDomains/${hostname}`);
      const snap = await ref.get();
      await ref.delete();
      const uid = snap.exists ? (snap.get("uid") as string | undefined) : undefined;
      if (uid) await db.doc(`entitlements/${uid}`).set({ customDomains: FieldValue.arrayRemove(hostname), updatedAt: Date.now() }, { merge: true });
    },
    cfCreate: (hostname) => createCustomHostname(saas, hostname),
    cfFind: (hostname) => findCustomHostname(saas, hostname),
    cfGet: (id) => getCustomHostname(saas, id),
    cfDelete: (id) => deleteCustomHostname(saas, id),
    kvPut: (key, value) => kvPut(kv, key, value),
    kvDelete: (key) => kvDelete(kv, key),
    publicDomain,
    cnameTarget: CUSTOM_DOMAIN_TARGET.value() || `custom.${publicDomain}`,
  };
}

function asHttpsError(e: unknown): HttpsError {
  if (e instanceof HttpsError) return e;
  if (e instanceof DomainError) return new HttpsError(e.code, e.message);
  logger.error("custom domain call failed", { error: String(e) });
  return new HttpsError("unavailable", "The relay couldn't be updated; try again in a minute.");
}

/**
 * Supporter: connect `hostname` (CNAME → CUSTOM_DOMAIN_TARGET) to one of the user's reserved
 * subdomains. Returns the status and the DNS records to add; Cloudflare issues the certificate.
 */
export const addCustomDomain = onCall({ region: REGION, secrets: [CF_API_TOKEN], maxInstances: 5 }, async (request) => {
  const uid = request.auth?.uid;
  if (!uid) throw new HttpsError("unauthenticated", "Sign in first.");
  try {
    return await addDomain(domainDeps(), uid, (request.data ?? {}) as { hostname?: unknown; subdomain?: unknown });
  } catch (e) {
    throw asHttpsError(e);
  }
});

export const removeCustomDomain = onCall({ region: REGION, secrets: [CF_API_TOKEN], maxInstances: 5 }, async (request) => {
  const uid = request.auth?.uid;
  if (!uid) throw new HttpsError("unauthenticated", "Sign in first.");
  try {
    return await removeDomain(domainDeps(), uid, (request.data ?? {}) as { hostname?: unknown });
  } catch (e) {
    throw asHttpsError(e);
  }
});

/** Polls Cloudflare for the domain's validation / certificate status (the desktop's "Check now"). */
export const refreshCustomDomain = onCall({ region: REGION, secrets: [CF_API_TOKEN], maxInstances: 5 }, async (request) => {
  const uid = request.auth?.uid;
  if (!uid) throw new HttpsError("unauthenticated", "Sign in first.");
  try {
    return await refreshDomain(domainDeps(), uid, (request.data ?? {}) as { hostname?: unknown });
  } catch (e) {
    throw asHttpsError(e);
  }
});

/** Every 30 minutes: advance pending custom domains without the user clicking "Check now". */
export const refreshPendingCustomDomains = onSchedule({ region: REGION, schedule: "every 30 minutes", secrets: [CF_API_TOKEN], maxInstances: 1 }, async () => {
  if (!CF_ZONE_ID.value()) return;
  const deps = domainDeps();
  const pending = await db.collection("customDomains").where("status", "==", "pending").limit(200).get();
  for (const doc of pending.docs) {
    try {
      await refreshDomain(deps, null, { hostname: doc.id });
    } catch (e) {
      logger.warn("custom domain refresh failed", { hostname: doc.id, error: String(e) });
    }
  }
});

/** Gives a reserved subdomain back. */
export const releaseSubdomain = onCall({ region: REGION, secrets: [CF_API_TOKEN], maxInstances: 5 }, async (request) => {
  const uid = request.auth?.uid;
  if (!uid) throw new HttpsError("unauthenticated", "Sign in first.");
  const raw = (request.data as { name?: unknown } | undefined)?.name;
  const name = typeof raw === "string" ? raw.trim().toLowerCase() : "";
  if (!name) throw new HttpsError("invalid-argument", "name required");
  const subdomains = await db.runTransaction(async (tx) => {
    const subRef = db.doc(`subdomains/${name}`);
    const entRef = db.doc(`entitlements/${uid}`);
    const [sub, ent] = await Promise.all([tx.get(subRef), tx.get(entRef)]);
    if (!sub.exists || sub.get("uid") !== uid) throw new HttpsError("not-found", `"${name}" isn't one of your subdomains.`);
    const domains = await subdomainInUse({ listDomains: async (u) => (await db.collection("customDomains").where("uid", "==", u).get()).docs.map((d) => d.data() as CustomDomain) }, uid, name);
    if (domains.length) throw new HttpsError("failed-precondition", `Remove the custom domains pointing at "${name}" first (${domains.join(", ")}).`);
    const next = ((ent.exists ? (ent.get("subdomains") as string[] | undefined) : undefined) ?? []).filter((s) => s !== name);
    tx.delete(subRef);
    tx.set(entRef, { subdomains: next, updatedAt: Date.now() }, { merge: true });
    return next;
  });
  await pushSubdomains(uid, subdomains, null, name);
  return { subdomains };
});

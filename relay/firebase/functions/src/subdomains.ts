// Reserved-subdomain rules (same as relay/worker/src/names.ts — keep them in sync).

import { RESERVED_SUBDOMAINS } from "./reserved.js";

const NAME_RE = /^[a-z0-9](?:[a-z0-9-]{1,38})[a-z0-9]$/;
const RANDOM_RE = /^[a-z]+-[a-z]+-\d{4}$/;
const RESERVED = new Set(RESERVED_SUBDOMAINS);

/** Supporter plan: reserved subdomains per account. */
export const MAX_RESERVED = 5;

export type Check = { ok: true; name: string } | { ok: false; reason: string };

export function validateSubdomain(input: unknown): Check {
  if (typeof input !== "string") return { ok: false, reason: "name must be a string" };
  const name = input.trim().toLowerCase();
  if (!NAME_RE.test(name)) return { ok: false, reason: "use 3–40 characters: lower-case letters, digits and inner hyphens" };
  if (name.includes("--")) return { ok: false, reason: "consecutive hyphens are not allowed" };
  if (RESERVED.has(name)) return { ok: false, reason: `"${name}" is reserved` };
  if (RANDOM_RE.test(name)) return { ok: false, reason: "names shaped like random tunnel names (word-word-1234) are reserved" };
  return { ok: true, name };
}

export type ClaimDecision =
  | { ok: true; name: string; subdomains: string[]; changed: boolean }
  | { ok: false; code: "permission-denied" | "invalid-argument" | "already-exists" | "resource-exhausted"; message: string };

/** Decides a claim given the current owner of the name and the user's existing reservations. */
export function decideClaim(a: { uid: string; plan: string; name: unknown; currentOwner: string | null; userSubdomains: string[]; max?: number }): ClaimDecision {
  if (a.plan !== "supporter") return { ok: false, code: "permission-denied", message: "Reserved subdomains are part of the Supporter plan (or self-host the relay for free)." };
  const v = validateSubdomain(a.name);
  if (!v.ok) return { ok: false, code: "invalid-argument", message: v.reason };
  if (a.currentOwner && a.currentOwner !== a.uid) return { ok: false, code: "already-exists", message: `"${v.name}" is taken.` };
  if (a.currentOwner === a.uid || a.userSubdomains.includes(v.name)) {
    return { ok: true, name: v.name, subdomains: [...new Set([...a.userSubdomains, v.name])], changed: false };
  }
  const max = a.max ?? MAX_RESERVED;
  if (a.userSubdomains.length >= max) return { ok: false, code: "resource-exhausted", message: `You can reserve up to ${max} subdomains; release one first.` };
  return { ok: true, name: v.name, subdomains: [...a.userSubdomains, v.name], changed: true };
}

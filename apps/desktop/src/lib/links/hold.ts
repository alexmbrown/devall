/**
 * Routes opened by a link from outside the app (an OS deep link, `./incoming.ts`). Such a link
 * only navigates, so a page that would otherwise start work by itself when it opens — connecting
 * to a database for a saved query, running a utility or recipe, auto-starting the mock server —
 * checks `isHeld` and waits for the user's click instead. Entries expire after a minute, so a
 * route the link never reached can't hold anything later. Pure (no stores).
 */
import type { Section } from '@/lib/sections';

const TTL_MS = 60_000;
const held = new Map<string, number>();
const key = (section: Section, param?: string) => `${section}\n${param ?? ''}`;

function prune(now: number) {
  for (const [k, until] of held) if (until <= now) held.delete(k);
}

/** Marks a route as opened by an outside link (called by the link's `go`). */
export function holdRoute(section: Section, param?: string, now = Date.now()) {
  prune(now);
  held.set(key(section, param), now + TTL_MS);
}

/** Whether an outside link opened `section` with exactly `param` — or with any param, when it is omitted — in the last minute. */
export function isHeld(section: Section, param?: string, now = Date.now()): boolean {
  prune(now);
  if (param !== undefined) return held.has(key(section, param));
  for (const k of held.keys()) if (k.startsWith(`${section}\n`)) return true;
  return false;
}

/** Forgets a held route (every route of `section` when `param` is omitted) — e.g. after the user pressed Run. */
export function releaseRoute(section: Section, param?: string) {
  if (param !== undefined) held.delete(key(section, param));
  else for (const k of [...held.keys()]) if (k.startsWith(`${section}\n`)) held.delete(k);
}

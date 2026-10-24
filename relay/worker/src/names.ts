import reservedList from "./reserved-subdomains.json";

/** Names nobody can claim or be assigned (infrastructure, brand, phishing bait). Kept in sync with relay/firebase/functions. */
export const RESERVED_SUBDOMAINS: ReadonlySet<string> = new Set(reservedList as string[]);

const ADJECTIVES = [
  "able", "amber", "ample", "azure", "bold", "brave", "brisk", "calm", "clever", "cosmic", "crisp", "curly",
  "dapper", "eager", "early", "fancy", "fluffy", "fond", "frosty", "gentle", "giddy", "glad", "golden", "grand",
  "happy", "hardy", "hazel", "honest", "humble", "jolly", "keen", "kind", "lively", "lucky", "lunar", "mellow",
  "merry", "mighty", "misty", "modest", "nimble", "noble", "olive", "polite", "proud", "quick", "quiet", "rapid",
  "rosy", "rustic", "sandy", "shiny", "silent", "silver", "snowy", "solar", "steady", "sunny", "swift", "tidy",
  "tranquil", "vivid", "witty", "zesty",
];

const NOUNS = [
  "badger", "beacon", "birch", "bison", "breeze", "brook", "canyon", "cedar", "comet", "coral", "crane", "delta",
  "dune", "eagle", "ember", "falcon", "fern", "finch", "fjord", "forest", "fox", "garnet", "glacier", "harbor",
  "heron", "island", "jaguar", "kestrel", "koala", "lagoon", "lark", "lemur", "lynx", "maple", "meadow", "meteor",
  "moose", "nebula", "oasis", "orchid", "osprey", "otter", "panda", "pebble", "pine", "plover", "quartz", "raven",
  "reef", "river", "robin", "sequoia", "sparrow", "spruce", "summit", "thistle", "tiger", "tundra", "valley",
  "walrus", "willow", "wombat", "yak", "zephyr",
];

const NAME_RE = /^[a-z0-9](?:[a-z0-9-]{1,38})[a-z0-9]$/;
const RANDOM_RE = /^[a-z]+-[a-z]+-\d{4}$/;

function randomInt(maxExclusive: number): number {
  // Rejection sampling over 32-bit values keeps the distribution uniform.
  const buf = new Uint32Array(1);
  const limit = Math.floor(0x1_0000_0000 / maxExclusive) * maxExclusive;
  for (;;) {
    crypto.getRandomValues(buf);
    if (buf[0]! < limit) return buf[0]! % maxExclusive;
  }
}

/** `adjective-noun-NNNN` (64 × 64 × 9000 ≈ 37 M names). */
export function randomSubdomain(): string {
  const adj = ADJECTIVES[randomInt(ADJECTIVES.length)]!;
  const noun = NOUNS[randomInt(NOUNS.length)]!;
  const num = 1000 + randomInt(9000);
  return `${adj}-${noun}-${num}`;
}

/** Whether a name has the shape of a free-tier random name (those can't be claimed). */
export function isRandomStyle(name: string): boolean {
  return RANDOM_RE.test(name);
}

export type NameCheck = { ok: true } | { ok: false; reason: string };

/** Syntax check for any subdomain we route (3–40 chars, a-z 0-9 and inner hyphens, no `xn--`/`--`). */
export function checkSubdomainSyntax(name: string): NameCheck {
  if (typeof name !== "string" || !NAME_RE.test(name)) {
    return { ok: false, reason: "use 3–40 characters: lower-case letters, digits and inner hyphens" };
  }
  if (name.includes("--")) return { ok: false, reason: "consecutive hyphens are not allowed" };
  return { ok: true };
}

/** Rules for a name a user picks (reserved by a Supporter or chosen on a self-hosted relay). */
export function checkClaimableSubdomain(name: string): NameCheck {
  const s = checkSubdomainSyntax(name);
  if (!s.ok) return s;
  if (RESERVED_SUBDOMAINS.has(name)) return { ok: false, reason: `"${name}" is reserved` };
  if (isRandomStyle(name)) return { ok: false, reason: "names shaped like random tunnel names (word-word-1234) are reserved" };
  return { ok: true };
}

const ID_ALPHABET = "abcdefghijklmnopqrstuvwxyz0123456789";

/** Unguessable inbox / monitor id (20 chars of [a-z0-9] ≈ 103 bits). */
export function randomInboxId(): string {
  let s = "";
  for (let i = 0; i < 20; i++) s += ID_ALPHABET[randomInt(ID_ALPHABET.length)];
  return s;
}

export function isValidInboxId(id: string): boolean {
  return /^[a-z0-9]{16,40}$/.test(id);
}

export function isValidProjectKey(key: string): boolean {
  return /^[A-Za-z0-9_-]{1,64}$/.test(key);
}

/** Random id with a sortable time prefix, e.g. `evt_01JD3…` style (Crockford base32 ULID). */
export function ulid(now = Date.now()): string {
  const B32 = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let t = now;
  let time = "";
  for (let i = 0; i < 10; i++) {
    time = B32[t % 32] + time;
    t = Math.floor(t / 32);
  }
  let rand = "";
  for (let i = 0; i < 16; i++) rand += B32[randomInt(32)];
  return time + rand;
}

export type HostKind =
  | { kind: "relay" }
  | { kind: "hooks" }
  | { kind: "share" }
  /** `mock.<domain>/<name>/…` (path-style hosted mocks). */
  | { kind: "mockPath" }
  /** `<name>.<mock domain>` (host-style hosted mocks). */
  | { kind: "mock"; name: string }
  | { kind: "tunnel"; sub: string }
  | { kind: "apex" }
  /** A hostname outside the relay's domain: maybe a customer's custom domain. */
  | { kind: "custom"; host: string }
  | { kind: "unknown" };

/** Lower-cased host without port or trailing dot. */
export function normalizeHost(host: string): string {
  return host.toLowerCase().replace(/:\d+$/, "").replace(/\.$/, "");
}

/** Classifies a request host against the relay's domain (and the hosted-mock domain). */
export function classifyHost(host: string, domain: string, mockDomain = `mock.${domain}`): HostKind {
  const h = normalizeHost(host);
  const d = domain.toLowerCase();
  const md = mockDomain.toLowerCase();
  // Local development (`wrangler dev` on http://127.0.0.1:8787): the bare loopback host is the
  // relay, so clients that can't send another Host header still reach /connect and /v1/…
  // Cloudflare never routes these names to a deployed Worker.
  if (h === "localhost" || h === "127.0.0.1" || h === "[::1]") return { kind: "relay" };
  if (h === d || h === `www.${d}`) return { kind: "apex" };
  if (h.endsWith(`.${md}`)) {
    const label = h.slice(0, -(md.length + 1));
    if (!label.includes(".") && checkSubdomainSyntax(label).ok) return { kind: "mock", name: label };
    return { kind: "unknown" };
  }
  if (!h.endsWith(`.${d}`)) return h.includes(".") && isValidHostname(h) ? { kind: "custom", host: h } : { kind: "unknown" };
  const label = h.slice(0, -(d.length + 1));
  if (label.includes(".")) return { kind: "unknown" };
  if (label === "relay") return { kind: "relay" };
  if (label === "hooks") return { kind: "hooks" };
  if (label === "share") return { kind: "share" };
  if (label === "mock") return { kind: "mockPath" };
  if (!checkSubdomainSyntax(label).ok) return { kind: "unknown" };
  return { kind: "tunnel", sub: label };
}

/** RFC 1123 host name (LDH labels, punycode allowed), at least two labels, not an IP literal. */
export function isValidHostname(h: string): boolean {
  if (h.length < 4 || h.length > 253) return false;
  if (/^[\d.]+$/.test(h) || h.includes(":")) return false;
  const labels = h.split(".");
  if (labels.length < 2) return false;
  return labels.every((l) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(l)) && !/^\d+$/.test(labels[labels.length - 1]!);
}

const B62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/** Unguessable URL-safe id (`len` × log2(62) bits; 22 chars ≈ 131 bits). */
export function randomId(len = 22): string {
  let s = "";
  for (let i = 0; i < len; i++) s += B62[randomInt(62)];
  return s;
}

export function isValidShareId(id: string): boolean {
  return /^[0-9A-Za-z]{22}$/.test(id);
}

export function isValidMonitorId(id: string): boolean {
  return /^[a-z0-9]{20}$/.test(id);
}

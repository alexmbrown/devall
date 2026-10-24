import type { Assertion } from '@/lib/types';

/**
 * Response snapshot tests: helpers shared by History ("Promote to snapshot test"), the Tests tab
 * and the response panel. The default path mirrors `devall_http::snapshot::default_path`.
 */

const ITEM_EXT = /\.(req|ws|grpc|mqtt|sio)\.yaml$/;

/** `users/list.req.yaml` → `http/__snapshots__/users/list.snap.json` (relative to the project root). */
export function snapshotPath(requestPath: string): string {
  const rel = requestPath.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  const stem = rel.replace(ITEM_EXT, '') || 'untitled';
  return `http/__snapshots__/${stem}.snap.json`;
}

export type SnapshotAssertion = Extract<Assertion, { assert: 'snapshot' }>;

/** Adds a snapshot assertion for `file`, or updates the ignore list of the existing one. */
export function upsertSnapshotTest(tests: Assertion[] | undefined, file: string, ignore: string[]): Assertion[] {
  const list = tests ?? [];
  const next: SnapshotAssertion = { assert: 'snapshot', file, op: 'eq', ...(ignore.length ? { ignore } : {}) };
  const i = list.findIndex((t) => t.assert === 'snapshot' && t.file === file);
  if (i < 0) return [...list, next];
  return list.map((t, j) => (j === i ? { ...next, enabled: t.enabled } : t));
}

/** One JSONPath per line or comma. */
export function parseIgnore(text: string): string[] {
  return [
    ...new Set(
      text
        .split(/[\n,]/)
        .map((s) => s.trim())
        .filter(Boolean),
    ),
  ];
}

const VOLATILE_KEY = /^(id|uuid|guid|etag|nonce|timestamp|requestId|request_id|traceId|trace_id|correlationId|correlation_id|createdAt|created_at|updatedAt|updated_at|modifiedAt|modified_at|expiresAt|expires_at|lastLogin|last_login|date|time)$/i;
const ISO_DATE = /^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}/;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;

function looksGenerated(v: unknown): boolean {
  if (typeof v === 'string') return ISO_DATE.test(v) || UUID.test(v) || ULID.test(v);
  return typeof v === 'number' && Number.isInteger(v) && v >= 1_000_000_000 && v <= 9_999_999_999_999;
}

/**
 * Fields that usually change between runs (timestamps, generated ids), as `$..key` JSONPaths —
 * the default "ignore" list when recording a snapshot. Keys with a known volatile name count only
 * when their value looks generated (so `{"id": 1}` in a fixture stays checked); ISO dates, UUIDs
 * and ULIDs count under any key.
 */
export function suggestIgnorePaths(body: string | null | undefined, limit = 8): string[] {
  if (!body) return [];
  let doc: unknown;
  try {
    doc = JSON.parse(body);
  } catch {
    return [];
  }
  const keys = new Set<string>();
  const walk = (v: unknown, depth: number) => {
    if (depth > 12 || keys.size >= limit) return;
    if (Array.isArray(v)) v.slice(0, 50).forEach((x) => walk(x, depth + 1));
    else if (v && typeof v === 'object')
      for (const [k, x] of Object.entries(v)) {
        if ((typeof x === 'string' && (ISO_DATE.test(x) || UUID.test(x) || ULID.test(x))) || (VOLATILE_KEY.test(k) && looksGenerated(x))) keys.add(k);
        else walk(x, depth + 1);
      }
  };
  walk(doc, 0);
  return [...keys].slice(0, limit).map((k) => (/^[A-Za-z_][A-Za-z0-9_]*$/.test(k) ? `$..${k}` : `$..['${k.replace(/'/g, "\\'")}']`));
}

/**
 * Links between /dev/all items: `devall:<kind>/<id>` — mirrors `devall_core::links` (Rust), which
 * the MCP server uses to give agents the same links.
 *
 *   [Create order](devall:request/orders/create.req.yaml)   a saved request (path under http/)
 *   [Auth flow](devall:note/design/auth.md)                  a note (path under notes/)
 *   [Fix refunds](devall:todo/01JC8Z6Q3W…)                   a personal to-do (by id)
 *   devall:env/staging · devall:variables · devall:page/settings/sidebar …
 *
 * Each `/`-separated id segment is percent-encoded where Markdown needs it (spaces, parentheses,
 * `#`, `?`, non-ASCII), so a link stays a valid Markdown link destination. Pure (no stores) so the
 * picker, renderer, backlinks and tests can share it.
 */

export const SCHEME = 'devall:';

export type LinkKind = 'request' | 'folder' | 'note' | 'todo' | 'env' | 'variables' | 'stub' | 'load' | 'fakedata' | 'query' | 'connection' | 'webhook' | 'utility' | 'repo' | 'page';

export interface KindMeta {
  /** Singular name ("Request"), shown in tooltips. */
  label: string;
  /** Picker group heading. */
  group: string;
  /** Words that filter the picker to this kind (`todo: refunds`, `note:auth`). */
  aliases: string[];
}

/** Every kind, in picker order. */
export const KIND_META: Record<LinkKind, KindMeta> = {
  request: { label: 'Request', group: 'Requests', aliases: ['request', 'requests', 'req'] },
  note: { label: 'Note', group: 'Notes', aliases: ['note', 'notes', 'doc', 'docs'] },
  todo: { label: 'To-do', group: 'To-dos', aliases: ['todo', 'todos', 'task', 'tasks'] },
  env: { label: 'Environment', group: 'Environments', aliases: ['env', 'envs', 'environment', 'environments'] },
  variables: { label: 'Project variables', group: 'Environments', aliases: ['variables', 'vars'] },
  folder: { label: 'Request folder', group: 'Request folders', aliases: ['folder', 'folders'] },
  stub: { label: 'Mock stub', group: 'Mock stubs', aliases: ['stub', 'stubs', 'mock', 'mocks'] },
  load: { label: 'Load test', group: 'Load tests', aliases: ['load', 'loadtest'] },
  fakedata: { label: 'Fake-data schema', group: 'Fake-data schemas', aliases: ['fakedata', 'data', 'schema', 'schemas'] },
  query: { label: 'Saved query', group: 'Saved queries', aliases: ['query', 'queries', 'sql'] },
  connection: { label: 'Database connection', group: 'Connections', aliases: ['connection', 'connections', 'conn', 'db'] },
  webhook: { label: 'Webhook inbox', group: 'Webhook inboxes', aliases: ['webhook', 'webhooks', 'inbox'] },
  utility: { label: 'Utility', group: 'Utilities', aliases: ['utility', 'utilities', 'util', 'tool'] },
  repo: { label: 'Repository', group: 'Repositories', aliases: ['repo', 'repos', 'repository', 'git'] },
  page: { label: 'Page', group: 'Pages', aliases: ['page', 'pages', 'go'] },
};

export const LINK_KINDS = Object.keys(KIND_META) as LinkKind[];

export const isLinkKind = (k: string): k is LinkKind => Object.prototype.hasOwnProperty.call(KIND_META, k);

export interface ParsedLink {
  kind: LinkKind;
  /** Decoded, `/`-separated; empty for `variables`. */
  id: string;
  /** After `#`, if any. */
  anchor?: string;
}

/** Bytes kept as they are in an id segment (RFC 3986 `pchar` minus `( ) % # ? /`). */
const KEEP = /^[A-Za-z0-9\-._~!$&'*+,;=:@]$/;
const utf8 = new TextEncoder();
const utf8Decode = new TextDecoder('utf-8', { fatal: false });

function encodeSegment(s: string): string {
  let out = '';
  for (const b of utf8.encode(s)) {
    const c = String.fromCharCode(b);
    out += b < 128 && KEEP.test(c) ? c : `%${b.toString(16).toUpperCase().padStart(2, '0')}`;
  }
  return out;
}

function decodeSegment(s: string): string {
  const bytes: number[] = [];
  for (let i = 0; i < s.length; i++) {
    const hex = s[i] === '%' ? s.slice(i + 1, i + 3) : '';
    if (/^[0-9a-fA-F]{2}$/.test(hex)) {
      bytes.push(parseInt(hex, 16));
      i += 2;
    } else for (const b of utf8.encode(s[i])) bytes.push(b);
  }
  return utf8Decode.decode(new Uint8Array(bytes));
}

/** Starts with `devall:` (any case). */
export function isDevallLink(s: string | null | undefined): boolean {
  return !!s && s.trim().slice(0, SCHEME.length).toLowerCase() === SCHEME;
}

/** `devall:<kind>/<id>` with each id segment percent-encoded where needed. */
export function formatLink(kind: LinkKind, id = ''): string {
  const clean = id.replace(/^\/+|\/+$/g, '');
  return clean ? `${SCHEME}${kind}/${clean.split('/').map(encodeSegment).join('/')}` : `${SCHEME}${kind}`;
}

/**
 * Parses a `devall:` link: scheme in any case, `devall://…` tolerated (`devall://notes/<path>` is
 * how the MCP server names note resources), `?query` ignored. Null for other links, unknown kinds,
 * missing ids and ids that climb (`..`).
 */
export function parseLink(s: string | null | undefined): ParsedLink | null {
  if (!isDevallLink(s)) return null;
  let rest = s!.trim().slice(SCHEME.length).replace(/^\/+/, '');
  const hash = rest.indexOf('#');
  const anchor = hash >= 0 ? decodeSegment(rest.slice(hash + 1)) : undefined;
  rest = rest.split(/[?#]/)[0];
  const slash = rest.indexOf('/');
  let kind = (slash >= 0 ? rest.slice(0, slash) : rest).toLowerCase();
  if (kind === 'notes') kind = 'note';
  if (!isLinkKind(kind)) return null;
  const parts: string[] = [];
  for (const seg of slash >= 0 ? rest.slice(slash + 1).split('/') : []) {
    const d = decodeSegment(seg);
    if (!d) continue;
    if (d === '.' || d === '..' || /[/\\\0]/.test(d)) return null;
    parts.push(d);
  }
  const id = parts.join('/');
  if (kind === 'variables') return { kind, id: '', anchor };
  if (!id) return null;
  return anchor ? { kind, id, anchor } : { kind, id };
}

/** The canonical form of a `devall:` link (null when it isn't one). */
export function canonicalLink(s: string | null | undefined): string | null {
  const p = parseLink(s);
  return p ? formatLink(p.kind, p.id) : null;
}

/** `devall:page/<section>[/<param>]`: any page of the app with its route param. */
export function pageLink(section: string, param?: string): string {
  return formatLink('page', param ? `${section}/${param}` : section);
}

/** Section and param of a `page` link id. */
export function parsePageId(id: string): { section: string; param?: string } {
  const i = id.indexOf('/');
  return i < 0 ? { section: id } : { section: id.slice(0, i), param: id.slice(i + 1) || undefined };
}

/** Link text that survives Markdown: `\`, `[` and `]` escaped, line breaks flattened. */
export function escapeMarkdownLabel(s: string): string {
  return s
    .replace(/\s*[\r\n]+\s*/g, ' ')
    .replace(/[\\[\]]/g, (c) => `\\${c}`)
    .trim();
}

/** `[title](link)`: what the picker inserts and "Copy as Markdown link" copies. */
export function markdownLink(title: string, link: string): string {
  return `[${escapeMarkdownLabel(title) || link}](${link})`;
}

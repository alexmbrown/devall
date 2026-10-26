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
 *
 * Links copied for use outside the app name their project: `devall:note/auth.md?project=<devall.yaml
 * id>` (`withProject`; `./external.ts` decides which links get it). Inside a project links stay
 * unqualified; parsing reads the qualifier into `project` and it is never part of the id, so both
 * spellings are the same item (`canonicalLink` drops it).
 */

export const SCHEME = 'devall:';

/** The query parameter naming a link's project (its `devall.yaml` id). */
export const PROJECT_PARAM = 'project';

export type LinkKind = 'request' | 'folder' | 'note' | 'todo' | 'env' | 'variables' | 'stub' | 'load' | 'fakedata' | 'query' | 'connection' | 'webhook' | 'utility' | 'repo' | 'commit' | 'compare' | 'page';

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
  commit: { label: 'Commit', group: 'Commits', aliases: ['commit', 'commits', 'sha'] },
  compare: { label: 'Branch comparison', group: 'Comparisons', aliases: ['compare', 'comparison', 'review', 'mr', 'pr'] },
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
  /** The `?project=` qualifier (a `devall.yaml` id), if any and well-formed. */
  project?: string;
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

/** Percent-decodes one id segment (invalid escapes stay as they are). */
export function decodeSegment(s: string): string {
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

/** A plausible project id: 1–128 characters, no whitespace or control characters (as in Rust). */
export function validProjectId(p: string): boolean {
  return p.length > 0 && p.length <= 128 && !/[\s\u0000-\u001f\u007f-\u009f]/.test(p);
}

/** The decoded `project=` value in a `devall:` link's query (before any `#`), valid or not. */
export function projectParam(s: string): string | undefined {
  const rest = s.trim().split('#')[0];
  const q = rest.indexOf('?');
  if (q < 0) return undefined;
  for (const kv of rest.slice(q + 1).split('&')) {
    const eq = kv.indexOf('=');
    if (eq >= 0 && kv.slice(0, eq) === PROJECT_PARAM) return decodeSegment(kv.slice(eq + 1));
  }
  return undefined;
}

/** `devall:<kind>/<id>` with each id segment percent-encoded where needed. */
export function formatLink(kind: LinkKind, id = ''): string {
  const clean = id.replace(/^\/+|\/+$/g, '');
  return clean ? `${SCHEME}${kind}/${clean.split('/').map(encodeSegment).join('/')}` : `${SCHEME}${kind}`;
}

/**
 * Parses a `devall:` link: scheme in any case, `devall://…` tolerated (`devall://notes/<path>` is
 * how the MCP server names note resources), query parameters other than `project` ignored (a
 * malformed qualifier is dropped). Null for other links, unknown kinds, missing ids and ids that
 * climb (`..`).
 */
export function parseLink(s: string | null | undefined): ParsedLink | null {
  if (!isDevallLink(s)) return null;
  let rest = s!.trim().slice(SCHEME.length).replace(/^\/+/, '');
  const hash = rest.indexOf('#');
  const anchor = hash >= 0 ? decodeSegment(rest.slice(hash + 1)) : undefined;
  const qualifier = projectParam(s!);
  const project = qualifier !== undefined && validProjectId(qualifier) ? qualifier : undefined;
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
  if (kind !== 'variables' && !id) return null;
  return { kind, id: kind === 'variables' ? '' : id, ...(anchor ? { anchor } : {}), ...(project ? { project } : {}) };
}

/**
 * `link` naming its project: canonical, `?project=<id>` (encoded like an id segment), the anchor
 * kept. Links that don't parse come back unchanged; an existing qualifier is replaced.
 */
export function withProject(link: string, project: string): string {
  const p = parseLink(link);
  if (!p) return link;
  return `${formatLink(p.kind, p.id)}?${PROJECT_PARAM}=${encodeSegment(project)}${p.anchor ? `#${encodeSegment(p.anchor)}` : ''}`;
}

/** The canonical form of a `devall:` link, without project qualifier or anchor (null when it isn't one). */
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

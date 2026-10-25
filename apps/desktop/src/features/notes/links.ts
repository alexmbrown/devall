import type { Section } from '@/store/nav';
import { parseRepoLink } from '@/lib/api/git';

/** Where a link inside a note points. */
export type LinkTarget =
  /** A request-like file under `http/` (path relative to `http/`). */
  | { kind: 'request'; path: string }
  /** A mock stub file (path relative to `mocks/`, e.g. `mappings/orders.json`). */
  | { kind: 'stub'; file: string }
  /** Another page of the app, with its deep-link param. */
  | { kind: 'section'; section: Section; param?: string }
  /** `repo:<name>/<path>`: a file or folder in one of the project's repositories (revealed in the file manager). */
  | { kind: 'repo'; repo: string; path: string }
  | { kind: 'unknown'; path: string };

/** Top-level project folders: links starting with one of these are project-root relative. */
const ROOTS = ['http', 'notes', 'mocks', 'webhooks', 'load', 'connections', 'queries', 'data', 'environments', 'utilities'];
const ITEM_EXT = /\.(req|ws|grpc|mqtt|sio)\.yaml$/;

/** Collapses `.` / `..` segments; returns null when the path climbs above the project root. */
function normalize(parts: string[]): string | null {
  const out: string[] = [];
  for (const p of parts) {
    if (!p || p === '.') continue;
    if (p === '..') {
      if (!out.length) return null;
      out.pop();
    } else out.push(p);
  }
  return out.join('/');
}

/**
 * Resolves a link from a note to a project location. `href` is what the Markdown contains
 * (`http/users/list.req.yaml`, `../http/x.req.yaml`, `other-note.md`, `mocks/mappings/a.json#x`);
 * `fromNote` is the note's path relative to `notes/` so relative links work. Paths that start with
 * a top-level project folder (or `/`) are project-root relative. `repo:<name>/<path>` points into
 * one of the project's repositories, wherever it is on this machine.
 */
export function resolveProjectLink(href: string, fromNote?: string): LinkTarget {
  const repo = parseRepoLink(href.trim().replace(/[?#].*$/, ''));
  if (repo) return { kind: 'repo', ...repo };
  let raw = href.trim().replace(/\\/g, '/').replace(/[?#].*$/, '');
  try {
    raw = decodeURIComponent(raw);
  } catch {
    /* keep as-is */
  }
  const absolute = raw.startsWith('/');
  const first = raw.replace(/^\/+/, '').split('/')[0];
  const rootRelative = absolute || (ROOTS.includes(first) && !raw.startsWith('.'));
  const base = rootRelative || !fromNote ? [] : ['notes', ...fromNote.replace(/\\/g, '/').split('/').slice(0, -1)];
  const path = normalize([...base, ...raw.replace(/^\/+/, '').split('/')]);
  if (path === null || !path) return { kind: 'unknown', path: href };
  // A bare note name without a folder and no note context: notes/ is the natural home.
  if (!rootRelative && !fromNote && /\.(md|ipynb)$/.test(path) && !ROOTS.includes(path.split('/')[0])) return { kind: 'section', section: 'notes', param: path };

  const [top, ...restParts] = path.split('/');
  const rest = restParts.join('/');
  switch (top) {
    case 'http':
      if (ITEM_EXT.test(rest)) return { kind: 'request', path: rest };
      return { kind: 'section', section: 'http' };
    case 'notes':
      return { kind: 'section', section: 'notes', param: /\.(md|ipynb)$/.test(rest) ? rest : undefined };
    case 'mocks':
      if (rest.startsWith('mappings/') && rest.endsWith('.json')) return { kind: 'stub', file: rest };
      if (rest.startsWith('__files')) return { kind: 'section', section: 'mocks', param: 'files' };
      return { kind: 'section', section: 'mocks', param: 'stubs' };
    case 'webhooks': {
      const m = /^([^/]+)\.webhook\.yaml$/.exec(rest);
      return { kind: 'section', section: 'webhooks', param: m ? m[1] : undefined };
    }
    case 'load':
      return { kind: 'section', section: 'load', param: rest.endsWith('.load.yaml') && !rest.startsWith('runs/') ? rest : undefined };
    case 'connections':
      return { kind: 'section', section: 'data', param: rest.endsWith('.conn.yaml') ? rest : undefined };
    case 'queries':
      return { kind: 'section', section: 'data', param: rest ? `query:${rest}` : undefined };
    case 'data':
      return { kind: 'section', section: 'fakedata', param: rest.endsWith('.data.yaml') ? rest : undefined };
    case 'environments': {
      const m = /^([^/]+)\.env\.yaml$/.exec(rest);
      return { kind: 'section', section: 'environments', param: m ? m[1] : undefined };
    }
    case 'utilities':
      return { kind: 'section', section: 'utilities', param: rest.endsWith('.regex.yaml') ? 'regex.cases' : undefined };
    default:
      return { kind: 'unknown', path };
  }
}

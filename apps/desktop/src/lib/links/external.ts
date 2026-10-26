/**
 * `devall:` links that leave or enter the app. Pure (no stores) and unit-tested; the handler that
 * uses them is `./incoming.ts`.
 *
 * - Leaving: "Copy link" names the project a project item belongs to (`shareableLink` →
 *   `devall:note/auth.md?project=<devall.yaml id>`), since copied links are mostly pasted outside
 *   the app and the OS hands them back without context. To-dos, utilities and app pages that don't
 *   need a project stay unqualified.
 * - Entering: a link from the OS is untrusted input. `checkIncomingLink` mirrors
 *   `devall_core::links::parse_incoming` (length cap, scheme, known kinds and pages, no `..`, `|`,
 *   control characters, backslashes or absolute paths), and `resolveLinkProject` decides where it
 *   opens: the open project, a recent project with that id, or nowhere (ask the user).
 */
import { isSection, sectionMeta } from '@/lib/sections';
import { LINK_KINDS, SCHEME, decodeSegment, isDevallLink, isLinkKind, parseLink, parsePageId, projectParam, validProjectId, withProject, type LinkKind, type ParsedLink } from './syntax';

/** Kinds whose ids live in a project (to-dos are personal, utilities device-wide, pages vary). */
const PROJECT_KINDS = new Set<LinkKind>(['request', 'folder', 'note', 'env', 'variables', 'stub', 'load', 'fakedata', 'query', 'connection', 'webhook', 'repo', 'commit', 'compare']);

/** Whether a link points into a project: a project item, or a page that needs one (Requests, Notes…). */
export function isProjectScoped(p: Pick<ParsedLink, 'kind' | 'id'>): boolean {
  if (p.kind !== 'page') return PROJECT_KINDS.has(p.kind);
  const { section } = parsePageId(p.id);
  return isSection(section) && !!sectionMeta(section).needsProject;
}

/**
 * The link to copy for use outside the app: a project item's link with `?project=<projectId>`.
 * Other links, links already naming a project, and everything while no project is open come back
 * unchanged.
 */
export function shareableLink(link: string, projectId: string | null | undefined): string {
  const p = parseLink(link);
  if (!p || p.project || !projectId || !isProjectScoped(p)) return link;
  return withProject(link, projectId);
}

/** Longest link accepted from outside the app (`devall_core::links::MAX_INCOMING_LEN`). */
export const MAX_INCOMING_LENGTH = 2048;

export type IncomingCheck = { ok: true; link: ParsedLink } | { ok: false; reason: string };

const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;

/** Checks a link that arrived from outside the app (see the module comment). */
export function checkIncomingLink(raw: unknown): IncomingCheck {
  const bad = (reason: string): IncomingCheck => ({ ok: false, reason });
  if (typeof raw !== 'string') return bad('not a link');
  const s = raw.trim();
  if (s.length > MAX_INCOMING_LENGTH) return bad(`the link is longer than ${MAX_INCOMING_LENGTH} characters`);
  if (!isDevallLink(s)) return bad('not a devall: link');
  if (CONTROL.test(s) || s.includes('\\')) return bad('the link contains control characters or backslashes');
  const qualifier = projectParam(s);
  if (qualifier !== undefined && !validProjectId(qualifier)) return bad('the project id is not valid');
  const rest = s.slice(SCHEME.length).replace(/^\/+/, '').split(/[?#]/)[0];
  const slash = rest.indexOf('/');
  const kind = (slash >= 0 ? rest.slice(0, slash) : rest).toLowerCase();
  if (!isLinkKind(kind) && kind !== 'notes') return bad(`unknown kind “${kind}” (use one of ${LINK_KINDS.join(', ')})`);
  const parts = slash >= 0 ? rest.slice(slash + 1).split('/').map(decodeSegment) : [];
  if (parts.some((p) => CONTROL.test(p) || /[|\\/]/.test(p))) return bad('the id contains characters links never use');
  if (parts.flatMap((p) => p.split(':')).some((p) => p === '.' || p === '..')) return bad('the id climbs out of its folder');
  const link = parseLink(s);
  if (!link) return bad('the link has no id');
  const first = link.id.split('/')[0];
  if (/^[A-Za-z]:$/.test(first) || first.startsWith('~')) return bad('the id is an absolute path');
  if (link.kind === 'page' && !isSection(parsePageId(link.id).section)) return bad(`there is no “${parsePageId(link.id).section}” page`);
  return { ok: true, link };
}

/** A project on this device a link may open (the `recent_project_ids` command). */
export interface ProjectCandidate {
  path: string;
  name: string;
  /** Its `devall.yaml` id. */
  id: string;
  openedAt: number;
  /** The scratch pad (opened with `openScratch`, not by path). */
  scratch?: boolean;
}

export type ProjectResolution =
  /** Follow it here: the open project is the right one, or the link needs none. */
  | { action: 'follow' }
  /** Open this project first (the top bar's switcher), then follow it. */
  | { action: 'switch'; to: ProjectCandidate }
  /** It needs a project and none is open: ask the user to open one. */
  | { action: 'ask' }
  /** It names a project that isn't on this device's recent list. */
  | { action: 'missing'; project: string };

/**
 * Where a link opens. Unqualified: in the open project (or ask when none is open). Qualified with
 * the open project's id: here. Otherwise the recent project with exactly that id (the most
 * recently opened when several folders share it, e.g. two clones), else `missing`. Links that
 * need no project are followed wherever the user is. Never a path from the link itself.
 */
export function resolveLinkProject(link: Pick<ParsedLink, 'kind' | 'id' | 'project'>, open: { id: string } | null, candidates: ProjectCandidate[]): ProjectResolution {
  if (!isProjectScoped(link)) return { action: 'follow' };
  if (!link.project) return open ? { action: 'follow' } : { action: 'ask' };
  if (open?.id === link.project) return { action: 'follow' };
  const to = candidates.filter((c) => c.id === link.project).sort((a, b) => b.openedAt - a.openedAt)[0];
  return to ? { action: 'switch', to } : { action: 'missing', project: link.project };
}

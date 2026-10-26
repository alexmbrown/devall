/**
 * The link registry: what a `devall:` link points at, live. Features register a provider per kind
 * (with the feature: `features/<name>/linkProvider.tsx`, registered in `./providers.ts`) that
 * lists items for the picker, resolves one id to its current title/icon (so renames show up),
 * and opens it — selecting the to-do, the stub, the saved query… not just showing the page.
 *
 * Providers read their stores (or lazily loaded lists, see `./sources.ts`) and call
 * `notifyLinks()` when that data changes; `useLinkInfo` re-describes every rendered link then.
 * No React components and no stores here, so it stays testable.
 */
import { useMemo, useSyncExternalStore, type ReactNode } from 'react';
import type { Section } from '@/lib/sections';
import { isProjectScoped } from './external';
import type { LinkItem } from './fuzzy';
import { KIND_META, parseLink, type LinkKind, type ParsedLink } from './syntax';

export type { LinkItem } from './fuzzy';

/**
 * - `ok`: found;
 * - `missing`: gone (deleted, renamed outside the app);
 * - `loading`: not known yet;
 * - `unavailable`: can't be resolved here — no project open, a personal to-do that isn't on this
 *   machine, the mock server isn't running…;
 * - `invalid`: not a valid `devall:` link.
 */
export type LinkState = 'ok' | 'missing' | 'loading' | 'unavailable' | 'invalid';

export interface LinkInfo {
  /** The link as given (canonical when it parsed). */
  link: string;
  kind: LinkKind | null;
  kindLabel: string;
  /** The item's current title (or the fallback while unknown / gone). */
  title: string;
  icon?: ReactNode;
  /** Small trailing element (a to-do's status). */
  badge?: ReactNode;
  state: LinkState;
  /** Path or id, for tooltips. */
  detail: string;
  /** Why it's unavailable / missing, or extra context. */
  note?: string;
  /** A completed to-do (rendered struck through, but not as missing). */
  done?: boolean;
}

/** What a provider knows about one id. */
export interface Resolved {
  title: string;
  icon?: ReactNode;
  badge?: ReactNode;
  detail?: string;
  note?: string;
  done?: boolean;
}

/** `undefined` = still loading, `null` = gone, `{ unavailable }` = can't tell here. */
export type Resolution = Resolved | null | undefined | { unavailable: string };

export type GoFn = (section: Section, param?: string, opts?: { preview?: boolean; newTab?: boolean; toSide?: boolean }) => void;

export interface OpenContext {
  /** Open in the neighbouring editor group (Ctrl/Cmd+click, middle click). */
  toSide: boolean;
  /** Navigates from where the link was clicked (`toSide` already applied). */
  go: GoFn;
}

export interface LinkProvider {
  kinds: LinkKind[];
  /** Picker items from the data at hand (may start loading it and `notifyLinks()` later). */
  items(): LinkItem[];
  resolve(kind: LinkKind, id: string): Resolution;
  open(kind: LinkKind, id: string, ctx: OpenContext): void | Promise<void>;
  /** Default icon for this kind (unresolved links, picker rows without their own). */
  icon(kind: LinkKind): ReactNode;
}

const providers = new Map<LinkKind, LinkProvider>();

/** Registers a provider for its kinds (a later registration replaces an earlier one). */
export function registerLinkProvider(p: LinkProvider) {
  for (const k of p.kinds) providers.set(k, p);
  notifyLinks();
}

export function linkProvider(kind: LinkKind): LinkProvider | undefined {
  return providers.get(kind);
}

// ---------- change notification ----------

let version = 0;
let pending = false;
const listeners = new Set<() => void>();

/** Data behind some links changed: re-describe rendered links (batched per microtask). */
export function notifyLinks() {
  if (pending) return;
  pending = true;
  queueMicrotask(() => {
    pending = false;
    version++;
    for (const l of [...listeners]) l();
  });
}

export function subscribeLinks(l: () => void): () => void {
  listeners.add(l);
  return () => void listeners.delete(l);
}

export const linksVersion = () => version;

/** Re-renders when link data changes. */
export function useLinksVersion(): number {
  return useSyncExternalStore(subscribeLinks, linksVersion);
}

// ---------- describe / open / search ----------

/** What `href` points at right now. `fallback` (the Markdown link text) is the title while it's unknown or gone. */
export function describe(href: string, fallback?: string): LinkInfo {
  const p = parseLink(href);
  if (!p) return { link: href, kind: null, kindLabel: 'Link', title: fallback?.trim() || href, state: 'invalid', detail: href, note: 'not a valid link' };
  const meta = KIND_META[p.kind];
  const link = href.trim();
  const base = { link, kind: p.kind, kindLabel: meta.label, detail: p.id || meta.label };
  const fb = fallback?.trim() && fallback.trim() !== link ? fallback.trim() : p.id.split('/').pop() || meta.label;
  const prov = providers.get(p.kind);
  if (!prov) return { ...base, title: fb, state: 'unavailable', note: `${meta.label} links aren't supported here` };
  const icon = prov.icon(p.kind);
  // A copied link naming another project: the open project's item of that name isn't it.
  if (inOtherProject(p)) return { ...base, title: fb, icon, state: 'unavailable', note: 'in another project' };
  let r: Resolution;
  try {
    r = prov.resolve(p.kind, p.id);
  } catch {
    r = undefined;
  }
  if (r === undefined) return { ...base, title: fb, icon, state: 'loading' };
  if (r === null) return { ...base, title: fb, icon, state: 'missing', note: `${meta.label} not found` };
  if ('unavailable' in r) return { ...base, title: fb, icon, state: 'unavailable', note: r.unavailable };
  return { ...base, title: r.title || fb, icon: r.icon ?? icon, badge: r.badge, detail: r.detail ?? base.detail, note: r.note, done: r.done, state: 'ok' };
}

/** `describe`, kept current as link data changes. */
export function useLinkInfo(href: string, fallback?: string): LinkInfo {
  const v = useLinksVersion();
  // eslint-disable-next-line react-hooks/exhaustive-deps -- `v` invalidates on data changes
  return useMemo(() => describe(href, fallback), [href, fallback, v]);
}

let defaultGo: GoFn | null = null;
let report: ((title: string, detail?: string) => void) | null = null;
let currentProject: (() => string | null) | null = null;
let followElsewhere: ((link: ParsedLink) => Promise<void>) | null = null;

/**
 * Wires navigation outside any view, user messages, the open project's `devall.yaml` id and how
 * links naming another project are followed (`./incoming.ts`: switch project, then open) — see
 * `./providers.ts`.
 */
export function configureLinks(opts: { go: GoFn; info(title: string, detail?: string): void; projectId?(): string | null; openInProject?(link: ParsedLink): Promise<void> }) {
  defaultGo = opts.go;
  report = opts.info;
  currentProject = opts.projectId ?? null;
  followElsewhere = opts.openInProject ?? null;
}

/** A project item's link qualified with a project other than the open one (or with none open). */
function inOtherProject(p: ParsedLink): boolean {
  return !!p.project && !!currentProject && p.project !== currentProject() && isProjectScoped(p);
}

/** Tells the user something about a link they followed (a toast in the app). */
export function linkMessage(title: string, detail?: string) {
  report?.(title, detail);
}

/**
 * Opens what `href` points at, inside the app only (a `devall:` link never reaches the browser,
 * the shell or the file system). `go` navigates from the clicked view; `toSide` uses the
 * neighbouring group. A link naming another project opens that project first. Resolves false when
 * the link is invalid or has no provider.
 */
export async function openLink(href: string, opts: { toSide?: boolean; go?: GoFn } = {}): Promise<boolean> {
  const p = parseLink(href);
  const prov = p && providers.get(p.kind);
  if (!p || !prov) {
    linkMessage('Not a link /dev/all can open', href);
    return false;
  }
  if (inOtherProject(p) && followElsewhere) {
    await followElsewhere(p);
    return true;
  }
  const base = opts.go ?? defaultGo;
  if (!base) return false;
  const toSide = !!opts.toSide;
  const go: GoFn = (section, param, o) => base(section, param, toSide ? { ...o, toSide: true } : o);
  await prov.open(p.kind, p.id, { toSide, go });
  return true;
}

/** Every item the picker can offer right now (providers may still be loading some). */
export function allLinkItems(): LinkItem[] {
  const out: LinkItem[] = [];
  for (const p of new Set(providers.values())) {
    try {
      out.push(...p.items());
    } catch {
      /* a provider without data yet */
    }
  }
  return out;
}

/** Tooltip text: kind, path/id, state. */
export function linkTooltip(info: LinkInfo): string {
  const head = info.kind ? `${info.kindLabel} · ${info.detail}` : info.link;
  return [head, info.note].filter(Boolean).join('\n');
}

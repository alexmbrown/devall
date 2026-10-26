/**
 * "Linked from": which notes and to-dos link to an item. Notes come pre-scanned from Rust
 * (`notes_links`: every note's link targets), to-dos from the in-memory snapshot (links,
 * description, comments). Targets are canonicalized (`devall:` links as such, older
 * project-relative links like `http/x.req.yaml` mapped to their `devall:` form, the `?project=`
 * qualifier of copied links dropped) so every spelling counts; a link naming another project is
 * about that project's item and doesn't count here. Pure: the store and UI live in
 * `components/links/Backlinks.tsx`.
 */
import { isProjectScoped } from './external';
import { isDevallLink, parseLink } from './syntax';

/** One note's link targets (`devall_core::links::NoteLinks`). */
export interface NoteLinks {
  /** Relative to `notes/`. */
  path: string;
  title: string;
  targets: string[];
}

/** The parts of a to-do that can hold links. */
export interface TaskLinks {
  id: string;
  content: string;
  done?: boolean;
  links?: string[];
  description?: string;
  comments?: { text: string }[];
}

export interface Backlink {
  kind: 'note' | 'todo';
  /** Note path or task id. */
  id: string;
  title: string;
  /** The linking item's own `devall:` link. */
  link: string;
  done?: boolean;
}

/** Canonical `devall:` link of a target, or null (web links, files outside the project…). `fromNote` resolves relative note links. */
export type Canonicalize = (target: string, fromNote?: string) => string | null;

const INLINE = /\]\(\s*(?:<([^>\n]+)>|([^\s)]+))(?:\s+"[^"\n]*")?\s*\)/g;
const AUTO = /<([a-zA-Z][a-zA-Z0-9+.-]{1,31}:[^\s<>]+)>/g;
const REFDEF = /^\s{0,3}\[[^\]]+\]:\s*<?([^\s>]+)>?/;
const BARE = /devall:[^\s)\]>"'`<]+/gi;

/** URLs with another scheme (web, mail…) and drive paths aren't project links. */
function external(t: string): boolean {
  const m = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(t);
  if (!m) return false;
  const scheme = m[1].toLowerCase();
  return scheme !== 'devall' && scheme !== 'repo';
}

/**
 * Link targets in Markdown that may point at project items (mirrors
 * `devall_core::links::extract_targets`): `devall:` links (inline, autolinks, reference
 * definitions, bare), project paths and `repo:` links — not web links, anchors or code.
 */
export function extractTargets(markdown: string): string[] {
  const out: string[] = [];
  const push = (raw: string | undefined) => {
    const t = raw?.trim() ?? '';
    if (!t || t.startsWith('#') || external(t) || (isDevallLink(t) && !parseLink(t)) || out.includes(t)) return;
    out.push(t);
  };
  let fence: string | null = null;
  for (const line of markdown.split(/\r?\n/)) {
    const trimmed = line.trimStart();
    if (fence) {
      if (trimmed.startsWith(fence)) fence = null;
      continue;
    }
    if (trimmed.startsWith('```') || trimmed.startsWith('~~~')) {
      fence = trimmed.slice(0, 3);
      continue;
    }
    let text = line.replace(/`[^`\n]*`/g, '');
    const def = REFDEF.exec(text);
    if (def) push(def[1]);
    text = text.replace(INLINE, (_m, a?: string, b?: string) => (push(a ?? b), ' '));
    text = text.replace(AUTO, (_m, a: string) => (push(a), ' '));
    for (const m of text.matchAll(BARE)) push(m[0].replace(/[.,;:!?]+$/, ''));
  }
  return out;
}

/**
 * Every item → the notes and to-dos that link to it (keyed by canonical link). `project` is the open
 * project's `devall.yaml` id: links qualified with another id are skipped.
 */
export function buildBacklinkIndex(
  notes: NoteLinks[],
  tasks: TaskLinks[],
  canon: Canonicalize,
  linkOf: { note(path: string): string; todo(id: string): string },
  { project }: { project?: string | null } = {},
): Map<string, Backlink[]> {
  const index = new Map<string, Backlink[]>();
  const elsewhere = (target: string) => {
    const p = isDevallLink(target) ? parseLink(target) : null;
    return !!p?.project && !!project && p.project !== project && isProjectScoped(p);
  };
  const add = (target: string, from: Backlink) => {
    if (target === from.link) return;
    const list = index.get(target) ?? [];
    if (!list.some((b) => b.kind === from.kind && b.id === from.id)) list.push(from);
    index.set(target, list);
  };
  for (const n of notes) {
    const from: Backlink = { kind: 'note', id: n.path, title: n.title, link: linkOf.note(n.path) };
    for (const t of n.targets) {
      if (elsewhere(t)) continue;
      const c = canon(t, n.path);
      if (c) add(c, from);
    }
  }
  for (const task of tasks) {
    const from: Backlink = { kind: 'todo', id: task.id, title: task.content, link: linkOf.todo(task.id), done: task.done };
    const targets = [...(task.links ?? []), ...extractTargets(task.description ?? ''), ...(task.comments ?? []).flatMap((c) => extractTargets(c.text))];
    for (const t of targets) {
      if (elsewhere(t)) continue;
      const c = canon(t);
      if (c) add(c, from);
    }
  }
  for (const list of index.values()) list.sort((a, b) => (a.kind === b.kind ? Number(!!a.done) - Number(!!b.done) || a.title.localeCompare(b.title) : a.kind === 'note' ? -1 : 1));
  return index;
}

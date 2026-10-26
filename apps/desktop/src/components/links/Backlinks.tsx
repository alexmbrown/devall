import { useEffect, useMemo } from 'react';
import { create } from 'zustand';
import clsx from 'clsx';
import { ArrowUpLeft, ChevronRight, NotebookText } from 'lucide-react';
import { Menu } from '@/components/ui/Menu';
import { toDevallLink } from '@/features/notes/links';
import { notesApi, type NoteLinks } from '@/lib/api/notes';
import { buildBacklinkIndex, type Backlink } from '@/lib/links/backlinks';
import { canonicalLink, formatLink } from '@/lib/links/syntax';
import { usePrefs } from '@/store/prefs';
import { useTodos } from '@/store/todos';
import { useFsVersion, useWorkspace } from '@/store/workspace';
import { TodoIcon } from './kindIcons';
import { OPEN_HINT, useFollowLink } from './LinkChip';
import './links.css';

/**
 * Which notes link where: every note's link targets, scanned in Rust (`notes_links`), loaded on
 * first use and re-read when notes change on disk (app saves included).
 */
let inflight: { root: string; job: Promise<void> } | null = null;

const useNoteLinks = create<{ root: string | null; notes: NoteLinks[] | null; load(root: string): Promise<void> }>((set, get) => ({
  root: null,
  notes: null,
  load(root) {
    if (inflight?.root === root) return inflight.job;
    if (get().root !== root) set({ root, notes: null });
    const job = notesApi
      .links(root)
      .catch(() => [] as NoteLinks[])
      .then((notes) => {
        if (get().root === root) set({ notes });
      })
      .finally(() => {
        if (inflight?.job === job) inflight = null;
      });
    inflight = { root, job };
    return job;
  },
}));

let timer: ReturnType<typeof setTimeout> | undefined;
useFsVersion.subscribe((s, prev) => {
  const { root, notes } = useNoteLinks.getState();
  if (s.v === prev.v || !root || !notes || !s.paths.some((p) => p.startsWith('notes/'))) return;
  clearTimeout(timer);
  timer = setTimeout(() => void useNoteLinks.getState().load(root), 400);
});

let cache: { notes: NoteLinks[] | null; tasks: unknown; projectList: unknown; index: Map<string, Backlink[]> } | null = null;

/**
 * The notes and to-dos linking to `link` (any spelling: `devall:` or the older project paths).
 * To-dos count from the Inbox, the user's own lists and the open project's list (other projects'
 * lists are about their own items).
 */
export function useBacklinks(link: string | null): Backlink[] {
  const root = useWorkspace((s) => s.project?.root ?? null);
  const loadedFor = useNoteLinks((s) => s.root);
  const notes = useNoteLinks((s) => s.notes);
  const snap = useTodos((s) => s.snap);
  useEffect(() => {
    if (root && link && (loadedFor !== root || !useNoteLinks.getState().notes)) void useNoteLinks.getState().load(root);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- load once per project
  }, [root, loadedFor, !!link]);
  return useMemo(() => {
    const key = link ? canonicalLink(link) : null;
    if (!key) return [];
    const projectNotes = loadedFor === root ? notes : null;
    if (!cache || cache.notes !== projectNotes || cache.tasks !== snap?.tasks || cache.projectList !== snap?.projectList) {
      const lists = new Map(snap?.lists.map((l) => [l.id, l]) ?? []);
      const tasks = (snap?.tasks ?? []).filter((t) => lists.get(t.listId)?.kind !== 'project' || t.listId === snap?.projectList);
      cache = { notes: projectNotes, tasks: snap?.tasks, projectList: snap?.projectList, index: buildBacklinkIndex(projectNotes ?? [], tasks, toDevallLink, { note: (p) => formatLink('note', p), todo: (id) => formatLink('todo', id) }) };
    }
    return cache.index.get(key) ?? [];
  }, [link, notes, loadedFor, root, snap]);
}

function BacklinkIcon({ b }: { b: Backlink }) {
  return b.kind === 'note' ? <NotebookText /> : <TodoIcon done={b.done} />;
}

/**
 * "Linked from" as a section (task detail, notes): nothing when no item links here. Collapsed to its
 * header (with the count) unless opened; open/closed is remembered (`prefs.backlinksOpen`).
 */
export function BacklinksSection({ link, className }: { link: string; className?: string }) {
  const items = useBacklinks(link);
  const follow = useFollowLink();
  const open = usePrefs((s) => s.backlinksOpen);
  const setPref = usePrefs((s) => s.set);
  if (!items.length) return null;
  const n = items.length;
  return (
    <div className={clsx('dl-backlinks', open && 'open', className)}>
      <button
        className="dl-backlinks-title"
        aria-expanded={open}
        onClick={() => setPref('backlinksOpen', !open)}
        title={open ? 'Hide the list' : `Show the ${n === 1 ? 'note or to-do' : `${n} notes and to-dos`} linking here`}
      >
        <ChevronRight size={13} className="dl-chev" />
        <ArrowUpLeft size={13} /> Linked from <span className="count">{n}</span>
      </button>
      {open && (
        <div className="dl-backlinks-list">
          {items.map((b) => (
            <button
              key={`${b.kind}:${b.id}`}
              className={b.done ? 'dl-backlink done' : 'dl-backlink'}
              onClick={(e) => follow(b.link, e)}
              onAuxClick={(e) => e.button === 1 && follow(b.link, e)}
              onMouseDown={(e) => e.button === 1 && e.preventDefault()}
              title={`${b.kind === 'note' ? `notes/${b.id}` : 'Personal to-do'}\n${OPEN_HINT}`}
            >
              <span className="dl-icon">
                <BacklinkIcon b={b} />
              </span>
              <span className="truncate">{b.title}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** "Linked from" as a compact header button with a menu (request and environment editors); hidden when nothing links here. */
export function BacklinksButton({ link }: { link: string }) {
  const items = useBacklinks(link);
  const follow = useFollowLink();
  if (!items.length) return null;
  return (
    <Menu
      align="right"
      items={() => [
        { heading: 'Linked from' },
        ...items.map((b) => ({ label: b.title, icon: <BacklinkIcon b={b} />, hint: b.kind === 'note' ? 'note' : 'to-do', onSelect: () => follow(b.link) })),
      ]}
      trigger={(p) => (
        <button className="btn btn-sm btn-ghost dl-backlinks-btn" {...p} title={`Linked from ${items.length} note${items.length === 1 ? '' : 's'} or to-do${items.length === 1 ? '' : 's'}`}>
          <ArrowUpLeft /> {items.length}
        </button>
      )}
    />
  );
}

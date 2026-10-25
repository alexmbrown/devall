import { BookOpenCheck, NotebookText } from 'lucide-react';
import { notesApi, type NoteNode } from '@/lib/api/notes';
import { linkMessage, type LinkProvider } from '@/lib/links/registry';
import { listSource, NO_PROJECT, projectRoot } from '@/lib/links/sources';
import { formatLink } from '@/lib/links/syntax';

type NoteEntry = Extract<NoteNode, { kind: 'note' }>;

function flatten(n: NoteNode, out: NoteEntry[] = []): NoteEntry[] {
  if (n.kind === 'note') out.push(n);
  else n.children.forEach((c) => flatten(c, out));
  return out;
}

/** Every note of the open project (title, path, format), kept current. */
export const notesSource = listSource({ folders: ['notes/'], load: (root) => notesApi.tree(root).then((t) => flatten(t)) });

const icon = (n?: Pick<NoteEntry, 'format' | 'path'>) => (n?.format === 'notebook' || n?.path.endsWith('.ipynb') ? <BookOpenCheck /> : <NotebookText />);

/** `devall:note/<path under notes/>` (Markdown notes and notebooks). */
export const noteLinks: LinkProvider = {
  kinds: ['note'],
  items: () => (notesSource.get() ?? []).map((n) => ({ link: formatLink('note', n.path), kind: 'note', title: n.title, detail: n.path, keywords: n.tags.join(' '), icon: icon(n) })),
  resolve(_kind, id) {
    if (!projectRoot()) return NO_PROJECT;
    const list = notesSource.get();
    if (!list) return undefined;
    const n = list.find((x) => x.path === id);
    return n ? { title: n.title, icon: icon(n), detail: `notes/${n.path}` } : null;
  },
  open(_kind, id, { go }) {
    if (!projectRoot()) return linkMessage('Open the project to follow this link');
    go('notes', id);
  },
  icon: () => <NotebookText />,
};

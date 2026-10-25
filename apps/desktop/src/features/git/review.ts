import type { ChangedFile, Reviewed } from '@/lib/api/git';

/**
 * Review state of the files in a comparison — pure, so the compare view and its tests share it.
 * Marks (`path → { blob }`, stored per repo + base + head on this machine) remember the version
 * of a file that was reviewed; a file whose current version differs is "changed since review".
 */
export type ReviewState = 'unreviewed' | 'reviewed' | 'changed';

/** The version a mark is keyed to: the new blob, or `-<old blob>` for a deleted file. */
export function fileVersion(f: Pick<ChangedFile, 'newBlob' | 'oldBlob'>): string {
  return f.newBlob ?? `-${f.oldBlob ?? ''}`;
}

export function reviewState(f: ChangedFile, marks: Record<string, Reviewed>): ReviewState {
  const m = marks[f.path];
  if (!m) return 'unreviewed';
  return m.blob === fileVersion(f) ? 'reviewed' : 'changed';
}

export interface Progress {
  reviewed: number;
  /** Marked, but the file changed since. */
  changed: number;
  total: number;
}

export function reviewProgress(files: ChangedFile[], marks: Record<string, Reviewed>): Progress {
  let reviewed = 0;
  let changed = 0;
  for (const f of files) {
    const s = reviewState(f, marks);
    if (s === 'reviewed') reviewed++;
    else if (s === 'changed') changed++;
  }
  return { reviewed, changed, total: files.length };
}

export interface FileFilter {
  /** Space-separated terms, all of which must appear in the path (case-insensitive). */
  text: string;
  /** Hide files marked reviewed (files changed since review stay). */
  hideReviewed: boolean;
  /** Only files that changed since they were reviewed. */
  onlyChanged: boolean;
}

export function visibleFiles(files: ChangedFile[], marks: Record<string, Reviewed>, filter: FileFilter): ChangedFile[] {
  const terms = filter.text.toLowerCase().split(/\s+/).filter(Boolean);
  return files.filter((f) => {
    const path = `${f.oldPath ?? ''} ${f.path}`.toLowerCase();
    if (terms.length && !terms.every((t) => path.includes(t))) return false;
    const s = reviewState(f, marks);
    if (filter.onlyChanged && s !== 'changed') return false;
    if (filter.hideReviewed && s === 'reviewed') return false;
    return true;
  });
}

/** A folder or a file in the tree view. Single-child folder chains are joined (`src/lib/api`). */
export interface TreeNode {
  name: string;
  /** Folder path (folders) or file path (files). */
  path: string;
  file?: ChangedFile;
  children: TreeNode[];
}

/** Files as a folder tree: folders first, then files, each sorted by name. */
export function fileTree(files: ChangedFile[]): TreeNode[] {
  const root: TreeNode = { name: '', path: '', children: [] };
  for (const f of files) {
    const parts = f.path.split('/');
    let node = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const path = parts.slice(0, i + 1).join('/');
      let child = node.children.find((c) => !c.file && c.path === path);
      if (!child) {
        child = { name: parts[i], path, children: [] };
        node.children.push(child);
      }
      node = child;
    }
    node.children.push({ name: parts[parts.length - 1], path: f.path, file: f, children: [] });
  }
  const finish = (n: TreeNode): TreeNode => {
    let cur = n;
    // Join chains of folders that only hold one folder.
    while (!cur.file && cur.children.length === 1 && !cur.children[0].file && cur.name) {
      const only = cur.children[0];
      cur = { name: `${cur.name}/${only.name}`, path: only.path, children: only.children };
    }
    const children = cur.children.map(finish).sort((a, b) => (a.file ? 1 : 0) - (b.file ? 1 : 0) || a.name.localeCompare(b.name));
    return { ...cur, children };
  };
  return finish(root).children;
}

/** Files in the order a tree shows them (depth first), for keyboard navigation. */
export function treeOrder(nodes: TreeNode[]): ChangedFile[] {
  const out: ChangedFile[] = [];
  const walk = (list: TreeNode[]) => {
    for (const n of list) {
      if (n.file) out.push(n.file);
      else walk(n.children);
    }
  };
  walk(nodes);
  return out;
}

/** Display order: regular files (as a tree or by path), then generated ones (collapsed group). */
export function displayOrder(files: ChangedFile[], tree: boolean): { main: ChangedFile[]; generated: ChangedFile[] } {
  const main = files.filter((f) => !f.generated);
  const generated = files.filter((f) => f.generated).sort((a, b) => a.path.localeCompare(b.path));
  return { main: tree ? treeOrder(fileTree(main)) : [...main].sort((a, b) => a.path.localeCompare(b.path)), generated };
}

/** The next file after `current` (wrapping) that still needs a look — unreviewed or changed since. */
export function nextUnreviewed(order: ChangedFile[], marks: Record<string, Reviewed>, current: string | null): string | null {
  const start = current ? order.findIndex((f) => f.path === current) : -1;
  for (let k = 1; k <= order.length; k++) {
    const f = order[(start + k + order.length) % order.length];
    if (f.path !== current && reviewState(f, marks) !== 'reviewed') return f.path;
  }
  return null;
}

/** Changes to send for (un)marking files: their current versions, or null to unmark. */
export function markChanges(files: ChangedFile[], mark: boolean): { path: string; blob: string | null }[] {
  return files.map((f) => ({ path: f.path, blob: mark ? fileVersion(f) : null }));
}

/** Applies changes locally (optimistic update before the store answers). */
export function applyMarks(marks: Record<string, Reviewed>, changes: { path: string; blob: string | null }[], now: number): Record<string, Reviewed> {
  const next = { ...marks };
  for (const c of changes) {
    if (c.blob) next[c.path] = { blob: c.blob, at: now };
    else delete next[c.path];
  }
  return next;
}

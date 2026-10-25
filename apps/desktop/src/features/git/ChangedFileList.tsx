import { useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { ChevronDown, ChevronRight, Folder } from 'lucide-react';
import { useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import type { ChangedFile, Reviewed } from '@/lib/api/git';
import { fileTree, reviewState, type TreeNode } from './review';

const STATUS: Record<string, string> = { M: 'Modified', A: 'Added', D: 'Deleted', R: 'Renamed', C: 'Copied', T: 'Type changed', U: 'Unmerged' };

export function StatusLetter({ status }: { status: string }) {
  return (
    <span className={clsx('git-letter', `g-${status}`)} title={STATUS[status] ?? status}>
      {status}
    </span>
  );
}

export function Counts({ f }: { f: ChangedFile }) {
  if (f.binary) return <span className="xs subtle shrink-0">bin</span>;
  return (
    <span className="git-counts shrink-0">
      {!!f.added && <span className="git-add">+{f.added}</span>}
      {!!f.deleted && <span className="git-del">−{f.deleted}</span>}
    </span>
  );
}

/**
 * Changed files as a tree (compact folder chains) or a flat list, with A/M/D/R letters and +/−
 * counts; with `review`, a "reviewed" checkbox per file (and "changed since review" flags).
 * Generated files (lockfiles, bundles) sit in their own collapsed group.
 */
export function ChangedFileList({
  main,
  generated,
  tree,
  selected,
  onSelect,
  review,
  showGenerated,
  onToggleGenerated,
  menu,
}: {
  main: ChangedFile[];
  generated: ChangedFile[];
  tree: boolean;
  selected: string | null;
  onSelect(path: string): void;
  review?: { marks: Record<string, Reviewed>; onToggle(files: ChangedFile[], mark: boolean): void };
  showGenerated: boolean;
  onToggleGenerated(v: boolean): void;
  menu?(f: ChangedFile): MenuItemDef[];
}) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const nodes = useMemo(() => (tree ? fileTree(main) : null), [tree, main]);
  const ctx = useContextMenu();
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (selected) host.current?.querySelector<HTMLElement>(`[data-path="${CSS.escape(selected)}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [selected]);

  const row = (f: ChangedFile, depth: number, name?: string) => {
    const state = review ? reviewState(f, review.marks) : null;
    const slash = f.path.lastIndexOf('/');
    return (
      <div
        key={f.path}
        data-path={f.path}
        className={clsx('list-item git-file-row', selected === f.path && 'on', state === 'reviewed' && 'git-reviewed')}
        style={{ paddingLeft: 8 + depth * 14 }}
        onClick={() => onSelect(f.path)}
        onContextMenu={menu ? (e) => ctx.open(e, menu(f)) : undefined}
        title={f.oldPath ? `${f.oldPath} → ${f.path}` : f.path}
      >
        {review && (
          <input
            type="checkbox"
            className="git-review-check"
            checked={state === 'reviewed'}
            title={state === 'changed' ? 'Changed since you reviewed it' : state === 'reviewed' ? 'Reviewed (v)' : 'Mark reviewed (v)'}
            onClick={(e) => e.stopPropagation()}
            onChange={(e) => review.onToggle([f], e.target.checked)}
          />
        )}
        <StatusLetter status={f.status} />
        <span className="truncate grow small">
          {name ?? (slash >= 0 ? f.path.slice(slash + 1) : f.path)}
          {!name && slash >= 0 && <span className="xs subtle"> {f.path.slice(0, slash)}</span>}
          {f.untracked && <span className="xs subtle"> (untracked)</span>}
        </span>
        {state === 'changed' && (
          <span className="badge warning git-changed-badge" title="Changed since you reviewed it">
            changed
          </span>
        )}
        <Counts f={f} />
      </div>
    );
  };

  const renderNodes = (list: TreeNode[], depth: number): React.ReactNode[] =>
    list.flatMap((n) => {
      if (n.file) return [row(n.file, depth, n.name)];
      const closed = collapsed.has(n.path);
      return [
        <div
          key={`d:${n.path}`}
          className="list-item git-folder-row"
          style={{ paddingLeft: 8 + depth * 14 }}
          onClick={() => setCollapsed((s) => (s.has(n.path) ? new Set([...s].filter((x) => x !== n.path)) : new Set([...s, n.path])))}
          title={n.path}
        >
          {closed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
          <Folder size={13} className="subtle" />
          <span className="truncate small">{n.name}</span>
        </div>,
        ...(closed ? [] : renderNodes(n.children, depth + 1)),
      ];
    });

  return (
    <div className="list git-file-list" ref={host}>
      {nodes ? renderNodes(nodes, 0) : main.map((f) => row(f, 0))}
      {generated.length > 0 && (
        <>
          <div className="list-group-header git-gen-header" onClick={() => onToggleGenerated(!showGenerated)} title="Lockfiles, minified bundles and linguist-generated paths">
            {showGenerated ? <ChevronDown size={13} /> : <ChevronRight size={13} />} Generated files ({generated.length})
          </div>
          {showGenerated && generated.map((f) => row(f, 0))}
        </>
      )}
      {ctx.node}
    </div>
  );
}

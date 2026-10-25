import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { Check, ChevronDown, ChevronRight, Copy, GitBranch, GitCompare, Globe, Tag } from 'lucide-react';
import { useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { SearchInput, Spinner, copyText } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type CompareMode, type RefEntry, type RefKind, type RefList } from '@/lib/api/git';
import { useGraphNav } from './GraphView';
import { pickRef } from './RefPicker';

const SECTIONS: { kind: RefKind; label: string; icon: React.ReactNode }[] = [
  { kind: 'branch', label: 'Branches', icon: <GitBranch size={13} /> },
  { kind: 'remote', label: 'Remote branches', icon: <Globe size={13} /> },
  { kind: 'tag', label: 'Tags', icon: <Tag size={13} /> },
];

/**
 * The graph tab's side pane: local branches, remote branches and tags (newest first). Click one to
 * find its commit in the graph; right-click to check it out or compare it. (Branch management —
 * rename, delete, upstream, stash — extends this list next.)
 */
export function RefsSide({ root, repo, version, onCompare, onChanged }: { root: string; repo: string | null; version: unknown; onCompare(base: string, head: string, mode: CompareMode): void; onChanged(): void }) {
  const [refs, setRefs] = useState<RefList | null>(null);
  const [q, setQ] = useState('');
  const [closed, setClosed] = useState<Set<RefKind>>(new Set(['tag']));
  const ctx = useContextMenu();
  const go = useGraphNav((s) => s.go);

  useEffect(() => {
    let alive = true;
    gitApi
      .refs(root, repo)
      .then((r) => alive && setRefs(r))
      .catch((e) => alive && toast.error('Could not read branches', errorMessage(e)));
    return () => {
      alive = false;
    };
  }, [root, repo, version]);

  const filtered = useMemo(() => {
    const t = q.trim().toLowerCase();
    return (refs?.refs ?? []).filter((r) => !t || r.name.toLowerCase().includes(t));
  }, [refs, q]);

  const current = refs?.current ?? null;
  const menu = (r: RefEntry): MenuItemDef[] => [
    { heading: r.name },
    ...(r.kind !== 'tag'
      ? [
          {
            label: r.kind === 'branch' ? `Check out ${r.name}` : `Check out ${r.name} as a local branch`,
            icon: <GitBranch />,
            disabled: r.current,
            onSelect: async () => {
              try {
                await gitApi.checkoutRef(root, repo, r.name, r.kind === 'branch' ? 'branch' : 'remote');
                toast.success(`Switched to ${r.name}`);
              } catch (e) {
                toast.error('Checkout failed', errorMessage(e));
              }
              onChanged();
            },
          },
        ]
      : []),
    ...(current && !r.current ? [{ label: `Compare ${current} with ${r.name}`, icon: <GitCompare />, onSelect: () => onCompare(r.name, current, 'mergeBase') }] : []),
    {
      label: 'Compare with…',
      icon: <GitCompare />,
      onSelect: async () => {
        const other = await pickRef({ root, repo, title: `Compare ${r.name} against…` });
        if (other) onCompare(other, r.name, 'mergeBase');
      },
    },
    { separator: true },
    { label: 'Copy name', icon: <Copy />, onSelect: () => void copyText(r.name, 'Copied') },
  ];

  if (!refs) return <div className="p-5 row-sm subtle small"><Spinner /> Reading branches…</div>;
  return (
    <div className="git-refs">
      <div className="git-compare-filter">
        <SearchInput value={q} onChange={setQ} placeholder="Filter branches and tags…" />
      </div>
      <div className="list">
        {SECTIONS.map((sec) => {
          const items = filtered.filter((r) => r.kind === sec.kind);
          if (!items.length) return null;
          const isClosed = closed.has(sec.kind) && !q;
          return (
            <div key={sec.kind}>
              <div className="list-group-header" onClick={() => setClosed((s) => (s.has(sec.kind) ? new Set([...s].filter((k) => k !== sec.kind)) : new Set([...s, sec.kind])))}>
                {isClosed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
                {sec.label} ({items.length})
              </div>
              {!isClosed &&
                items.map((r) => (
                  <div key={r.refname} className={clsx('list-item git-ref-row', r.current && 'current')} onClick={() => go(r.sha)} onContextMenu={(e) => ctx.open(e, menu(r))} title={`${r.name} — ${r.subject}`}>
                    <span className="subtle shrink-0">{sec.icon}</span>
                    <span className={clsx('mono small truncate grow', r.current && 'strong')}>{r.name}</span>
                    {r.current && <Check size={13} className="git-ok shrink-0" />}
                    {r.track && <span className="xs subtle shrink-0">{r.track.replace('ahead ', '↑').replace('behind ', '↓').replace(', ', ' ')}</span>}
                  </div>
                ))}
            </div>
          );
        })}
        {!filtered.length && <p className="muted small p-5">No branches match.</p>}
      </div>
      {ctx.node}
    </div>
  );
}

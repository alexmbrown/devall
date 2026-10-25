import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import {
  ArrowDownToLine,
  ArrowUp,
  Archive,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  FastForward,
  FolderGit2,
  FolderOpen,
  GitBranch,
  GitCompare,
  GitMerge,
  Globe,
  Link2,
  ListOrdered,
  Lock,
  Pencil,
  Plus,
  RefreshCw,
  Scissors,
  Server,
  Tag,
  Trash2,
  Upload,
  Waypoints,
} from 'lucide-react';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { RelTime, SearchInput, Spinner, copyText } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type CompareMode, type RefEntry, type RefList, type RemoteInfo, type StashEntry, type Worktree } from '@/lib/api/git';
import * as op from './branchOps';
import { useGraphNav } from './GraphView';
import { pickRef } from './RefPicker';

type SectionId = 'branch' | 'remote' | 'tag' | 'remotes' | 'stash' | 'worktree';

export interface RefsActions {
  compare(base: string, head: string, mode: CompareMode): void;
  changed(): void;
  /** Show a stash's files in the main pane. */
  previewStash(e: StashEntry): void;
  /** Open the interactive rebase of the current branch onto/after `base`. */
  interactiveRebase(base: string, label: string): void;
  /** A worktree was opened as a repository (its name in the list). */
  openRepo(name: string): void;
}

/**
 * The graph tab's side pane — branches, remote branches, tags, remotes, stashes and worktrees.
 * Click a ref to find it in the graph, a stash to preview it; right-click for everything else
 * (checkout, merge, rebase, fast-forward, upstream, rename, delete — remote ones with a typed
 * confirmation —, push tags, fetch/prune remotes, apply/pop/drop stashes, lock/remove worktrees).
 */
export function RefsSide({ root, repo, version, selectedStash, actions }: { root: string; repo: string | null; version: unknown; selectedStash: string | null; actions: RefsActions }) {
  const [refs, setRefs] = useState<RefList | null>(null);
  const [remotes, setRemotes] = useState<RemoteInfo[]>([]);
  const [stashes, setStashes] = useState<StashEntry[]>([]);
  const [worktrees, setWorktrees] = useState<Worktree[]>([]);
  const [q, setQ] = useState('');
  const [closed, setClosed] = useState<Set<SectionId>>(new Set(['tag', 'remotes']));
  const ctx = useContextMenu();
  const go = useGraphNav((s) => s.go);

  useEffect(() => {
    let alive = true;
    gitApi
      .refs(root, repo)
      .then((r) => alive && setRefs(r))
      .catch((e) => alive && toast.error('Could not read branches', errorMessage(e)));
    gitApi.remotes(root, repo).then((r) => alive && setRemotes(r)).catch(() => alive && setRemotes([]));
    gitApi.stashList(root, repo).then((s) => alive && setStashes(s)).catch(() => alive && setStashes([]));
    gitApi.worktrees(root, repo).then((w) => alive && setWorktrees(w)).catch(() => alive && setWorktrees([]));
    return () => {
      alive = false;
    };
  }, [root, repo, version]);

  const current = refs?.current ?? null;
  const c: op.GitCtx = { root, repo, current, changed: actions.changed };
  const all = refs?.refs ?? [];
  const t = q.trim().toLowerCase();
  const match = (s: string) => !t || s.toLowerCase().includes(t);
  const filtered = useMemo(() => all.filter((r) => match(r.name)), [all, t]); // eslint-disable-line react-hooks/exhaustive-deps

  const checkout = async (r: RefEntry) => {
    try {
      await gitApi.checkoutRef(root, repo, r.name, r.kind === 'branch' ? 'branch' : 'remote');
      toast.success(`Switched to ${r.name}`);
    } catch (e) {
      toast.error('Checkout failed', errorMessage(e));
    }
    actions.changed();
  };

  const compareItems = (r: RefEntry): MenuItemDef[] => [
    ...(current && !r.current ? [{ label: `Compare ${current} with ${r.name}`, icon: <GitCompare />, onSelect: () => actions.compare(r.name, current, 'mergeBase') }] : []),
    {
      label: 'Compare with…',
      icon: <GitCompare />,
      onSelect: async () => {
        const other = await pickRef({ root, repo, title: `Compare ${r.name} against…` });
        if (other) actions.compare(other, r.name, 'mergeBase');
      },
    },
  ];

  const historyItems = (r: RefEntry): MenuItemDef[] =>
    current && !r.current
      ? [
          { label: `Merge ${r.name} into ${current}…`, icon: <GitMerge />, onSelect: () => void op.merge(c, r.name) },
          { label: `Rebase ${current} onto ${r.name}…`, icon: <Waypoints />, onSelect: () => void op.rebaseOnto(c, r.name) },
          { label: `Interactive rebase ${current} onto ${r.name}…`, icon: <ListOrdered />, onSelect: () => actions.interactiveRebase(r.name, r.name) },
        ]
      : [];

  const branchMenu = (r: RefEntry): MenuItemDef[] => [
    { heading: r.name },
    { label: `Check out ${r.name}`, icon: <GitBranch />, disabled: r.current || !!r.worktree, hint: r.worktree ? 'in a worktree' : undefined, onSelect: () => void checkout(r) },
    ...(r.upstream && r.behind > 0 && r.ahead === 0 ? [{ label: `Fast-forward to ${r.upstream}`, icon: <FastForward />, onSelect: () => void op.fastForward(c, r) }] : []),
    ...(r.current ? [{ label: 'Push', icon: <ArrowUp />, onSelect: () => void op.push(c) }] : []),
    ...historyItems(r),
    { separator: true },
    ...compareItems(r),
    { separator: true },
    { label: 'New branch from here…', icon: <Plus />, onSelect: () => void op.newBranch(c, r.name, r.name) },
    { label: 'New worktree for this branch…', icon: <FolderGit2 />, disabled: r.current || !!r.worktree, onSelect: () => void op.addWorktree(c, all, r.name) },
    { label: r.upstream ? `Change upstream (${r.upstream})…` : 'Set upstream…', icon: <Link2 />, onSelect: () => void op.setUpstream(c, r, all) },
    { label: 'Rename…', icon: <Pencil />, onSelect: () => void op.renameBranch(c, r.name) },
    { label: 'Delete…', icon: <Trash2 />, danger: true, disabled: r.current, onSelect: () => void op.deleteBranch(c, r) },
    { separator: true },
    { label: 'Copy name', icon: <Copy />, onSelect: () => void copyText(r.name, 'Copied') },
  ];

  const remoteBranchMenu = (r: RefEntry): MenuItemDef[] => [
    { heading: r.name },
    { label: `Check out as a local branch`, icon: <GitBranch />, onSelect: () => void checkout(r) },
    ...historyItems(r),
    { separator: true },
    ...compareItems(r),
    { label: 'New branch from here…', icon: <Plus />, onSelect: () => void op.newBranch(c, r.name, r.name) },
    { separator: true },
    { label: 'Delete on the remote…', icon: <Trash2 />, danger: true, onSelect: () => void op.deleteRemoteBranch(c, r.name) },
    { label: 'Copy name', icon: <Copy />, onSelect: () => void copyText(r.name, 'Copied') },
  ];

  const tagMenu = (r: RefEntry): MenuItemDef[] => [
    { heading: `${r.name}${r.annotated ? ' (annotated)' : ''}` },
    { label: 'Check out (detached)…', icon: <GitBranch />, onSelect: () => void gitApi.checkoutCommit(root, repo, r.sha).then(() => toast.success(`Detached at ${r.name}`), (e) => toast.error('Checkout failed', errorMessage(e))).finally(actions.changed) },
    ...compareItems(r),
    ...historyItems(r).slice(0, 1),
    { separator: true },
    { label: 'Push to remote…', icon: <Upload />, disabled: !remotes.length, onSelect: () => void op.pushTag(c, r.name, remotes.map((x) => x.name)) },
    { label: 'Delete…', icon: <Trash2 />, danger: true, onSelect: () => void op.deleteTag(c, r.name) },
    { label: 'Delete on a remote…', icon: <Trash2 />, danger: true, disabled: !remotes.length, onSelect: () => void op.deleteRemoteTag(c, r.name, remotes.map((x) => x.name)) },
    { label: 'Copy name', icon: <Copy />, onSelect: () => void copyText(r.name, 'Copied') },
  ];

  const remoteMenu = (r: RemoteInfo): MenuItemDef[] => [
    { heading: r.name },
    { label: 'Fetch (prune deleted branches)', icon: <RefreshCw />, onSelect: () => void op.fetchRemote(c, r.name, true) },
    { label: 'Prune deleted branches', icon: <Scissors />, onSelect: () => void op.pruneRemote(c, r.name) },
    { separator: true },
    { label: 'Rename…', icon: <Pencil />, onSelect: () => void op.renameRemote(c, r) },
    { label: 'Change URL…', icon: <Link2 />, onSelect: () => void op.setRemoteUrl(c, r) },
    { label: 'Copy URL', icon: <Copy />, disabled: !r.fetchUrl, onSelect: () => void copyText(r.fetchUrl ?? '', 'URL copied') },
    { separator: true },
    { label: 'Remove…', icon: <Trash2 />, danger: true, onSelect: () => void op.removeRemote(c, r) },
  ];

  const stashMenu = (e: StashEntry): MenuItemDef[] => [
    { heading: e.name },
    { label: 'Show changes', icon: <Archive />, onSelect: () => actions.previewStash(e) },
    { label: 'Apply', icon: <ArrowDownToLine />, onSelect: () => void op.applyStash(c, e, false) },
    { label: 'Pop (apply and drop)', icon: <ArrowDownToLine />, onSelect: () => void op.applyStash(c, e, true) },
    { separator: true },
    { label: 'Drop…', icon: <Trash2 />, danger: true, onSelect: () => void op.dropStash(c, e) },
    { label: 'Copy message', icon: <Copy />, onSelect: () => void copyText(e.title, 'Copied') },
  ];

  const worktreeMenu = (w: Worktree): MenuItemDef[] => [
    { heading: w.branch ?? (w.detached ? `detached ${w.head?.slice(0, 7)}` : w.path) },
    { label: 'Open in Source control', icon: <FolderGit2 />, disabled: w.current || !!w.prunable, onSelect: async () => { const n = await op.openWorktree(c, w); if (n) actions.openRepo(n); } },
    { label: 'Reveal in file manager', icon: <FolderOpen />, disabled: !!w.prunable, onSelect: () => void revealItemInDir(w.path).catch((e) => toast.error('Could not reveal it', errorMessage(e))) },
    { label: 'Copy path', icon: <Copy />, onSelect: () => void copyText(w.path, 'Path copied') },
    ...(w.main ? [] : [{ separator: true }, { label: w.locked !== null ? 'Unlock' : 'Lock…', icon: <Lock />, onSelect: () => void op.lockWorktree(c, w) }, { label: 'Remove…', icon: <Trash2 />, danger: true, disabled: w.current, onSelect: () => void op.removeWorktree(c, w) }]),
  ];

  if (!refs) return <div className="p-5 row-sm subtle small"><Spinner /> Reading branches…</div>;

  const toggle = (id: SectionId) => setClosed((s) => (s.has(id) ? new Set([...s].filter((k) => k !== id)) : new Set([...s, id])));
  const header = (id: SectionId, label: string, count: number, extra?: React.ReactNode) => {
    const isClosed = closed.has(id) && !t;
    return (
      <div className="list-group-header git-refs-head" onClick={() => toggle(id)}>
        {isClosed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
        {label} ({count})<span className="spacer" />
        <span className="git-refs-head-actions" onClick={(e) => e.stopPropagation()}>
          {extra}
        </span>
      </div>
    );
  };
  const open = (id: SectionId) => !closed.has(id) || !!t;

  const branches = filtered.filter((r) => r.kind === 'branch');
  const remoteBranches = filtered.filter((r) => r.kind === 'remote');
  const tags = filtered.filter((r) => r.kind === 'tag');
  const shownRemotes = remotes.filter((r) => match(r.name));
  const shownStashes = stashes.filter((s) => match(s.title) || match(s.name));
  const shownWorktrees = worktrees.filter((w) => match(w.path) || match(w.branch ?? ''));

  const refRow = (r: RefEntry, icon: React.ReactNode, menu: (r: RefEntry) => MenuItemDef[]) => (
    <div key={r.refname} className={clsx('list-item git-ref-row', r.current && 'current')} onClick={() => go(r.sha)} onDoubleClick={() => r.kind !== 'tag' && !r.current && void checkout(r)} onContextMenu={(e) => ctx.open(e, menu(r))} title={`${r.name} — ${r.subject}${r.upstream ? `\ntracks ${r.upstream}` : ''}${r.worktree ? `\nchecked out in ${r.worktree}` : ''}`}>
      <span className="subtle shrink-0">{icon}</span>
      <span className={clsx('mono small truncate grow', r.current && 'strong')}>{r.name}</span>
      {r.current && <Check size={13} className="git-ok shrink-0" />}
      {r.worktree && <FolderGit2 size={12} className="subtle shrink-0" />}
      {r.gone && <span className="badge outline shrink-0" title="Its upstream was deleted on the remote">gone</span>}
      {(r.ahead > 0 || r.behind > 0) && (
        <span className="xs subtle shrink-0 git-track">
          {r.ahead > 0 && `↑${r.ahead}`}
          {r.behind > 0 && ` ↓${r.behind}`}
        </span>
      )}
    </div>
  );

  return (
    <div className="git-refs">
      <div className="git-compare-filter">
        <SearchInput value={q} onChange={setQ} placeholder="Filter branches, tags, stashes…" />
        <button className="icon-btn sm" title="Fetch all remotes (prune deleted branches)" onClick={() => void op.fetchRemote(c, null, true)}>
          <RefreshCw />
        </button>
      </div>
      <div className="list">
        {header('branch', 'Branches', branches.length, <button className="icon-btn sm" title="New branch (Ctrl/Cmd+B)" onClick={() => void op.newBranch(c)}><Plus /></button>)}
        {open('branch') && branches.map((r) => refRow(r, <GitBranch size={13} />, branchMenu))}
        {remoteBranches.length > 0 && header('remote', 'Remote branches', remoteBranches.length)}
        {open('remote') && remoteBranches.map((r) => refRow(r, <Globe size={13} />, remoteBranchMenu))}
        {header('tag', 'Tags', tags.length, <button className="icon-btn sm" title="New tag at HEAD" onClick={() => void op.createTag(c, 'HEAD', current ?? 'HEAD')}><Plus /></button>)}
        {open('tag') && tags.map((r) => refRow(r, <Tag size={13} />, tagMenu))}
        {header('remotes', 'Remotes', shownRemotes.length, <button className="icon-btn sm" title="Add remote" onClick={() => void op.addRemote(c)}><Plus /></button>)}
        {open('remotes') &&
          shownRemotes.map((r) => (
            <div key={r.name} className="list-item git-ref-row" onContextMenu={(e) => ctx.open(e, remoteMenu(r))} title={`${r.fetchUrl ?? ''}${r.pushUrl && r.pushUrl !== r.fetchUrl ? `\npush: ${r.pushUrl}` : ''}`}>
              <span className="subtle shrink-0"><Server size={13} /></span>
              <span className="small strong shrink-0">{r.name}</span>
              <span className="mono xs subtle truncate grow">{r.fetchUrl}</span>
              <span className="xs subtle shrink-0">{r.branches}</span>
            </div>
          ))}
        {header('stash', 'Stashes', shownStashes.length, <button className="icon-btn sm" title="Stash changes (Ctrl/Cmd+Alt+S)" onClick={() => void op.saveStash(c)}><Plus /></button>)}
        {open('stash') &&
          shownStashes.map((s) => (
            <div key={s.oid} className={clsx('list-item git-stash-row', selectedStash === s.oid && 'on')} onClick={() => actions.previewStash(s)} onContextMenu={(e) => ctx.open(e, stashMenu(s))} title={s.message}>
              <span className="small truncate w-full">{s.title}</span>
              <span className="xs subtle row-sm">
                <span className="mono">{s.name}</span>
                {s.branch && <span className="truncate">· {s.branch}</span>}
                {s.untracked && <span>· +untracked</span>}
                <span>· <RelTime ts={s.time * 1000} /></span>
              </span>
            </div>
          ))}
        {header('worktree', 'Worktrees', shownWorktrees.length, (
          <>
            <button className="icon-btn sm" title="New worktree" onClick={() => void op.addWorktree(c, all)}><Plus /></button>
            <button className="icon-btn sm" title="Prune worktrees whose folders are gone" onClick={() => void op.pruneWorktrees(c)}><Scissors /></button>
          </>
        ))}
        {open('worktree') &&
          shownWorktrees.map((w) => (
            <div key={w.path} className={clsx('list-item git-stash-row', w.current && 'current')} onContextMenu={(e) => ctx.open(e, worktreeMenu(w))} onDoubleClick={async () => { if (!w.current && !w.prunable) { const n = await op.openWorktree(c, w); if (n) actions.openRepo(n); } }} title={w.path}>
              <span className="small truncate w-full row-sm">
                <FolderGit2 size={12} className="subtle shrink-0" />
                <span className={clsx('truncate', w.current && 'strong')}>{w.branch ?? (w.detached ? `detached at ${w.head?.slice(0, 7)}` : 'bare')}</span>
                {w.main && <span className="badge outline">main</span>}
                {w.locked !== null && <Lock size={11} className="subtle" />}
                {w.prunable && <span className="badge outline" title={w.prunable}>missing</span>}
              </span>
              <span className="xs subtle mono truncate w-full">{w.path}</span>
            </div>
          ))}
        {!filtered.length && !shownStashes.length && t && <p className="muted small p-5">Nothing matches.</p>}
      </div>
      {ctx.node}
    </div>
  );
}

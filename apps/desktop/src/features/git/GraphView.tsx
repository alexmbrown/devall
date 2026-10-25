import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { create } from 'zustand';
import clsx from 'clsx';
import { ArrowDown, ArrowUp, Copy, GitBranch, GitCommitHorizontal, GitCompare, Link2, RefreshCw, RotateCcw, Tag, Undo2, Waypoints } from 'lucide-react';
import { useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { Empty, RelTime, Resizer, SearchInput, Spinner, copyText } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { formatLink } from '@/lib/links/syntax';
import { gitApi, type CompareMode, type GraphRow, type RefLabel } from '@/lib/api/git';
import { useActiveViewRef, useViewState } from '@/store/nav';
import { CommitDetails } from './CommitDetails';
import { pickRef } from './RefPicker';

const ROW_H = 26;
const LANE_W = 14;
const PAGE = 300;
const MAX_LANES_SHOWN = 24;
export const WIP = 'WIP';

/** Jump requests from outside the graph (the branches list, links): `n` makes repeats count. */
export const useGraphNav = create<{ jump: { hash: string; n: number } | null; go(hash: string): void }>((set, get) => ({
  jump: null,
  go: (hash) => set({ jump: { hash, n: (get().jump?.n ?? 0) + 1 } }),
}));

export interface GraphActions {
  /** Open Compare between two refs/commits. */
  compare(base: string, head: string, mode: CompareMode): void;
  /** After an operation changed the repository. */
  changed(): void;
  /** The WIP row was clicked. */
  openChanges(): void;
}

/**
 * The commit graph: every branch, tag and remote branch, laid out in Rust (`git_graph_*`) and
 * read in pages while the view scrolls (only visible rows are rendered). Columns: graph,
 * message with ref pills, author, date, SHA. Click selects a commit (details below); Ctrl/Cmd+click
 * a second one to compare the two. Right-click for commit actions.
 */
export function GraphView({
  root,
  repo,
  repoName,
  current,
  wip,
  wipCount,
  version,
  selectedCommit,
  onSelectCommit,
  actions,
}: {
  root: string;
  repo: string | null;
  /** For links (`devall:commit/<repo>/<sha>`). */
  repoName: string | null;
  current: string | null;
  wip: boolean;
  wipCount: number;
  /** Changes when the repository may have changed (status reloads). */
  version: unknown;
  selectedCommit: string | null;
  onSelectCommit(sha: string | null): void;
  actions: GraphActions;
}) {
  const [gen, setGen] = useState<number | null>(null);
  const rows = useRef<(GraphRow | undefined)[]>([]);
  const index = useRef(new Map<string, number>());
  const pages = useRef(new Set<number>());
  const [, setTick] = useState(0);
  const redraw = () => setTick((t) => t + 1);
  const [meta, setMeta] = useState({ loaded: 0, done: false, maxWidth: 1, error: null as string | null, elapsedMs: 0 });
  const [scroll, setScroll] = useState({ top: 0, height: 600 });
  const body = useRef<HTMLDivElement>(null);
  const [sel, setSel] = useState<string[]>(selectedCommit ? [selectedCommit] : []);
  const [query, setQuery] = useState('');
  const [matches, setMatches] = useState<number[]>([]);
  const [matchAt, setMatchAt] = useState(-1);
  const [detailsH, setDetailsH] = useViewState('git.detailsH', 320);
  const [graphW, setGraphW] = useViewState<number | null>('git.graphW', null);
  const ctx = useContextMenu();
  const active = useActiveViewRef();
  const pendingJump = useRef<string | null>(selectedCommit);

  // (Re)open: a reused graph keeps its rows; a new generation starts over.
  const refreshNext = useRef(false);
  const [reopen, setReopen] = useState(0);
  useEffect(() => {
    let alive = true;
    const refresh = refreshNext.current;
    refreshNext.current = false;
    gitApi
      .graphOpen(root, repo, wip, refresh)
      .then(({ generation, reused }) => {
        if (!alive) return;
        if (reused && generation === gen) return;
        rows.current = [];
        index.current = new Map();
        pages.current = new Set();
        setMeta({ loaded: 0, done: false, maxWidth: 1, error: null, elapsedMs: 0 });
        setGen(generation);
      })
      .catch((e) => alive && setMeta((m) => ({ ...m, error: errorMessage(e), done: true })));
    return () => {
      alive = false;
    };
    // `gen` is read, not watched: only the repo, the WIP flag and changes reopen.
  }, [root, repo, wip, version, reopen]); // eslint-disable-line react-hooks/exhaustive-deps

  const loadPage = useCallback(
    async (p: number) => {
      if (gen === null || pages.current.has(p)) return;
      pages.current.add(p);
      try {
        const page = await gitApi.graphRows(root, repo, gen, p * PAGE, PAGE);
        page.rows.forEach((r, i) => {
          rows.current[page.offset + i] = r;
          index.current.set(r.hash, page.offset + i);
        });
        if (page.rows.length < PAGE && !page.done) pages.current.delete(p); // not there yet: ask again later
        setMeta({ loaded: page.loaded, done: page.done, maxWidth: page.maxWidth, error: page.error, elapsedMs: page.elapsedMs });
        redraw();
      } catch (e) {
        pages.current.delete(p);
        const msg = errorMessage(e);
        // The backend let this graph go (another repo's was opened): read it again.
        if (msg.includes('stale graph')) setReopen((n) => n + 1);
        else setMeta((m) => ({ ...m, error: msg }));
      }
    },
    [gen, root, repo],
  );

  const count = meta.done ? meta.loaded : Math.max(meta.loaded, rows.current.length) + PAGE;
  const first = Math.max(0, Math.floor(scroll.top / ROW_H) - 10);
  const last = Math.min(count, Math.ceil((scroll.top + scroll.height) / ROW_H) + 10);
  useEffect(() => {
    if (gen === null) return;
    for (let p = Math.floor(first / PAGE); p <= Math.floor(Math.max(first, last - 1) / PAGE); p++) void loadPage(p);
  }, [gen, first, last, loadPage, meta.loaded]);

  useLayoutEffect(() => {
    const el = body.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setScroll((s) => ({ ...s, height: el.clientHeight })));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const scrollToRow = useCallback((i: number) => {
    const el = body.current;
    if (!el) return;
    const top = i * ROW_H;
    if (top < el.scrollTop || top + ROW_H > el.scrollTop + el.clientHeight) el.scrollTop = Math.max(0, top - el.clientHeight / 3);
  }, []);

  const jumpTo = useCallback(
    async (hash: string, select = true) => {
      if (gen === null) {
        pendingJump.current = hash;
        return;
      }
      let i = index.current.get(hash);
      if (i === undefined) i = (await gitApi.graphFind(root, repo, gen, hash).catch(() => null)) ?? undefined;
      if (i === undefined) return toast.info('Commit not in the graph', hash.slice(0, 12));
      scrollToRow(i);
      if (select) {
        const r = rows.current[i];
        const h = r?.hash ?? hash;
        setSel([h]);
        onSelectCommit(h);
      }
    },
    [gen, root, repo, scrollToRow, onSelectCommit],
  );

  // Pending jumps (from the route) once the graph is open; jumps from the branches list.
  useEffect(() => {
    if (gen !== null && pendingJump.current) {
      const h = pendingJump.current;
      pendingJump.current = null;
      void jumpTo(h);
    }
  }, [gen, jumpTo]);
  const jump = useGraphNav((s) => s.jump);
  const handledJump = useRef(jump?.n ?? 0);
  useEffect(() => {
    if (!jump || jump.n === handledJump.current) return;
    handledJump.current = jump.n;
    void jumpTo(jump.hash);
  }, [jump, jumpTo]);

  // Search: matching rows (message, author, SHA) among the rows read so far.
  useEffect(() => {
    if (gen === null || !query.trim()) {
      setMatches([]);
      setMatchAt(-1);
      return;
    }
    const t = setTimeout(() => {
      gitApi
        .graphSearch(root, repo, gen, query)
        .then((m) => {
          setMatches(m);
          setMatchAt(m.length ? 0 : -1);
          if (m.length) scrollToRow(m[0]);
        })
        .catch(() => setMatches([]));
    }, 200);
    return () => clearTimeout(t);
  }, [query, gen, root, repo, meta.done, scrollToRow]);
  const matchSet = useMemo(() => new Set(matches), [matches]);
  const nextMatch = (d: 1 | -1) => {
    if (!matches.length) return;
    const k = (matchAt + d + matches.length) % matches.length;
    setMatchAt(k);
    scrollToRow(matches[k]);
    const r = rows.current[matches[k]];
    if (r) {
      setSel([r.hash]);
      onSelectCommit(r.hash);
    }
  };

  // Up/down move the selection.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!active.current || e.ctrlKey || e.metaKey || e.altKey) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable || t.closest('.monaco-editor'))) return;
      if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
      const curIdx = sel[0] ? (index.current.get(sel[0]) ?? -1) : -1;
      const next = Math.min(count - 1, Math.max(0, curIdx + (e.key === 'ArrowDown' ? 1 : -1)));
      const r = rows.current[next];
      if (!r) return;
      e.preventDefault();
      setSel([r.hash]);
      if (r.hash !== WIP) onSelectCommit(r.hash);
      scrollToRow(next);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [active, sel, count, onSelectCommit, scrollToRow]);

  const click = (e: React.MouseEvent, r: GraphRow) => {
    if (r.hash === WIP) return actions.openChanges();
    if ((e.ctrlKey || e.metaKey) && sel.length && sel[0] !== r.hash) {
      setSel([sel[0], r.hash]);
      return;
    }
    setSel([r.hash]);
    onSelectCommit(r.hash);
  };

  const lanes = Math.min(MAX_LANES_SHOWN, Math.max(1, meta.maxWidth));
  const autoW = lanes * LANE_W + 10;
  const gw = Math.max(40, Math.min(graphW ?? autoW, MAX_LANES_SHOWN * LANE_W + 10));
  const ordered = sel.length === 2 ? [...sel].sort((a, b) => (index.current.get(b) ?? 0) - (index.current.get(a) ?? 0)) : sel;

  const run = async (label: string, fn: () => Promise<unknown>, success?: string) => {
    try {
      const out = await fn();
      if (out && typeof out === 'object' && 'conflicts' in out && (out as { conflicts: boolean }).conflicts) {
        toast.error(`${label} stopped with conflicts`, 'Resolve them in Changes, then continue — or abort.');
      } else if (success) toast.success(success);
    } catch (e) {
      toast.error(`${label} failed`, errorMessage(e));
    }
    actions.changed();
  };

  const rowMenu = (r: GraphRow): MenuItemDef[] => {
    const short = r.hash.slice(0, 7);
    const subject = r.subject.length > 60 ? `${r.subject.slice(0, 57)}…` : r.subject;
    const branchRefs = r.refs.filter((x) => x.kind === 'branch' || x.kind === 'remote');
    const cur = current ?? 'HEAD';
    return [
      { heading: `${short} ${subject}` },
      ...branchRefs.map((x) => ({
        label: x.kind === 'branch' ? `Check out ${x.name}` : `Check out ${x.name} as a local branch`,
        icon: <GitBranch />,
        disabled: x.current,
        onSelect: () => void run('Checkout', () => gitApi.checkoutRef(root, repo, x.name, x.kind === 'branch' ? 'branch' : 'remote'), `Switched to ${x.name}`),
      })),
      {
        label: 'Check out this commit (detached)…',
        icon: <GitCommitHorizontal />,
        onSelect: async () => {
          if (await confirmDialog({ title: `Check out ${short}?`, message: `HEAD is detached at ${short} — no branch moves. Git refuses if uncommitted changes would be overwritten. Commits made here belong to no branch until you create one.`, confirmText: 'Check out' }))
            void run('Checkout', () => gitApi.checkoutCommit(root, repo, r.hash), `Detached at ${short}`);
        },
      },
      {
        label: 'Create branch here…',
        icon: <GitBranch />,
        onSelect: async () => {
          const name = await promptDialog({ title: `New branch at ${short}`, label: 'Branch name', validate: validName });
          if (!name) return;
          try {
            await gitApi.branchCreate(root, repo, name.trim(), r.hash, false);
            toast.success(`Created ${name.trim()}`, `at ${short}`, { label: 'Check out', run: () => void run('Checkout', () => gitApi.checkoutRef(root, repo, name.trim(), 'branch'), `Switched to ${name.trim()}`) });
          } catch (e) {
            toast.error('Create branch failed', errorMessage(e));
          }
          actions.changed();
        },
      },
      {
        label: 'Create tag here…',
        icon: <Tag />,
        onSelect: async () => {
          const name = await promptDialog({ title: `New tag at ${short}`, label: 'Tag name', validate: validName });
          if (!name) return;
          const message = await promptDialog({ title: `Tag ${name.trim()}`, label: 'Message (optional — makes an annotated tag)', confirmText: 'Create tag' });
          if (message === null) return;
          void run('Create tag', () => gitApi.tagCreate(root, repo, name.trim(), r.hash, message || null), `Tagged ${short} as ${name.trim()}`);
        },
      },
      { separator: true },
      { label: `Compare with ${cur}`, icon: <GitCompare />, onSelect: () => actions.compare(r.hash, cur, 'direct') },
      {
        label: 'Compare with…',
        icon: <GitCompare />,
        onSelect: async () => {
          const other = await pickRef({ root, repo, title: `Compare ${short} against…` });
          if (other) actions.compare(other, r.hash, 'direct');
        },
      },
      ...(ordered.length === 2 && ordered.includes(r.hash) ? [{ label: `Compare the 2 selected commits`, icon: <GitCompare />, onSelect: () => actions.compare(ordered[0], ordered[1], 'direct') }] : []),
      { separator: true },
      {
        label: `Cherry-pick onto ${cur}…`,
        icon: <Waypoints />,
        disabled: r.refs.some((x) => x.current),
        onSelect: async () => {
          if (await confirmDialog({ title: `Cherry-pick ${short} onto ${cur}?`, message: `Applies the changes of “${subject}” on top of ${cur} as a new commit${r.parents.length > 1 ? ' (a merge: its changes against the first parent)' : ''}. If they conflict, git stops so you can resolve or abort.`, confirmText: 'Cherry-pick' }))
            void run('Cherry-pick', () => gitApi.cherryPick(root, repo, r.hash), `Cherry-picked ${short}`);
        },
      },
      {
        label: `Revert on ${cur}…`,
        icon: <Undo2 />,
        onSelect: async () => {
          if (await confirmDialog({ title: `Revert ${short}?`, message: `Creates a new commit on ${cur} that undoes “${subject}”${r.parents.length > 1 ? ' (a merge: undone against its first parent)' : ''}. History isn't rewritten.`, confirmText: 'Revert' }))
            void run('Revert', () => gitApi.revert(root, repo, r.hash), `Reverted ${short}`);
        },
      },
      { label: `Reset ${cur} to here…`, icon: <RotateCcw />, disabled: true, hint: 'coming next' },
      { label: `Rebase ${cur} onto here…`, icon: <Waypoints />, disabled: true, hint: 'coming next' },
      { separator: true },
      { label: 'Copy SHA', icon: <Copy />, onSelect: () => void copyText(r.hash, 'SHA copied') },
      ...(repoName ? [{ label: 'Copy link', icon: <Link2 />, onSelect: () => void copyText(formatLink('commit', `${repoName}/${r.hash}`), 'Link copied') }] : []),
    ];
  };

  const pillMenu = (l: RefLabel, r: GraphRow): MenuItemDef[] => {
    const cur = current ?? 'HEAD';
    return [
      { heading: l.name },
      ...(l.kind === 'branch' || l.kind === 'remote'
        ? [{ label: l.kind === 'branch' ? `Check out ${l.name}` : `Check out ${l.name} as a local branch`, icon: <GitBranch />, disabled: l.current, onSelect: () => void run('Checkout', () => gitApi.checkoutRef(root, repo, l.name, l.kind === 'branch' ? 'branch' : 'remote'), `Switched to ${l.name}`) }]
        : []),
      { label: `Compare ${cur} with ${l.name}`, icon: <GitCompare />, disabled: l.current, onSelect: () => actions.compare(l.name, cur, 'mergeBase') },
      {
        label: 'Compare with…',
        icon: <GitCompare />,
        onSelect: async () => {
          const other = await pickRef({ root, repo, title: `Compare ${l.name} against…` });
          if (other) actions.compare(other, l.name, 'mergeBase');
        },
      },
      { separator: true },
      { label: 'Copy name', icon: <Copy />, onSelect: () => void copyText(l.name, 'Copied') },
      { label: 'Copy SHA', icon: <Copy />, onSelect: () => void copyText(r.hash, 'SHA copied') },
    ];
  };

  const visible: React.ReactNode[] = [];
  for (let i = first; i < last; i++) {
    const r = rows.current[i];
    const style = { top: i * ROW_H, height: ROW_H };
    if (!r) {
      if (i < (meta.done ? meta.loaded : Infinity)) visible.push(<div key={`p${i}`} className="git-graph-row git-graph-skeleton" style={style} />);
      continue;
    }
    const isWip = r.hash === WIP;
    visible.push(
      <div
        key={r.hash}
        className={clsx('git-graph-row', sel.includes(r.hash) && 'on', matchSet.has(i) && 'match', matches[matchAt] === i && 'match-on', isWip && 'wip')}
        style={style}
        onClick={(e) => click(e, r)}
        onContextMenu={isWip ? undefined : (e) => ctx.open(e, rowMenu(r))}
        title={isWip ? 'Uncommitted changes — open Changes' : undefined}
      >
        <div className="git-graph-cell" style={{ width: gw }}>
          <GraphCell row={r} />
        </div>
        <div className="git-graph-msg">
          {r.refs.slice(0, 4).map((l) => (
            <span key={`${l.kind}:${l.name}`} className={clsx('git-pill', `git-pill-${l.kind}`, l.current && 'current')} title={l.kind} onContextMenu={(e) => ctx.open(e, pillMenu(l, r))}>
              {l.kind === 'tag' ? <Tag size={10} /> : <GitBranch size={10} />}
              {l.name}
            </span>
          ))}
          {r.refs.length > 4 && <span className="git-pill git-pill-other" title={r.refs.slice(4).map((l) => l.name).join(', ')}>+{r.refs.length - 4}</span>}
          <span className="truncate">{isWip ? `Uncommitted changes (${wipCount})` : r.subject}</span>
        </div>
        <div className="git-graph-author truncate">{r.author}</div>
        <div className="git-graph-date">{r.time ? <RelTime ts={r.time * 1000} /> : null}</div>
        <div className="git-graph-sha mono">{isWip ? '' : r.hash.slice(0, 7)}</div>
      </div>,
    );
  }

  const selRow = sel.length === 1 && sel[0] !== WIP ? sel[0] : null;
  return (
    <div className="col h-full min-h-0 git-graph" style={{ gap: 0 }}>
      <div className="toolbar">
        <SearchInput value={query} onChange={setQuery} placeholder="Search message, author or SHA…" className="git-graph-search" />
        {query && (
          <span className="xs subtle row-sm shrink-0">
            {matches.length ? `${matchAt + 1} / ${matches.length}` : 'no matches'}
            <button className="icon-btn sm" title="Previous match" disabled={!matches.length} onClick={() => nextMatch(-1)}>
              <ArrowUp />
            </button>
            <button className="icon-btn sm" title="Next match" disabled={!matches.length} onClick={() => nextMatch(1)}>
              <ArrowDown />
            </button>
          </span>
        )}
        <span className="spacer" />
        <span className="xs subtle shrink-0" title={meta.done ? `Laid out in ${meta.elapsedMs} ms` : undefined}>
          {meta.done ? `${meta.loaded.toLocaleString()} commits` : `${meta.loaded.toLocaleString()} commits…`}
        </span>
        {!meta.done && <Spinner />}
        <button
          className="icon-btn sm"
          title="Reload the graph"
          onClick={() => {
            refreshNext.current = true;
            setReopen((n) => n + 1);
          }}
        >
          <RefreshCw />
        </button>
      </div>
      <div className="git-graph-header" style={{ ['--gw' as string]: `${gw}px` }}>
        <div style={{ width: gw }} className="git-graph-colhead">
          Graph
          <Resizer width={gw} onChange={(w) => setGraphW(w)} min={40} max={MAX_LANES_SHOWN * LANE_W + 10} defaultValue={autoW} className="git-graph-resizer" />
        </div>
        <div>Message</div>
        <div className="git-graph-author">Author</div>
        <div className="git-graph-date">Date</div>
        <div className="git-graph-sha">SHA</div>
      </div>
      {meta.error && <div className="callout-stack"><div className="small git-fail">{meta.error}</div></div>}
      <div className="grow min-h-0 git-graph-body" ref={body} onScroll={(e) => setScroll({ top: e.currentTarget.scrollTop, height: e.currentTarget.clientHeight })}>
        {gen === null && !meta.error ? (
          <Empty icon={<Spinner />} title="Reading history…" />
        ) : meta.done && meta.loaded === 0 ? (
          <Empty icon={<GitCommitHorizontal />} title="No commits yet" />
        ) : (
          <div className="git-graph-rows" style={{ height: count * ROW_H, ['--gw' as string]: `${gw}px` }}>
            {visible}
          </div>
        )}
      </div>
      {ordered.length === 2 && (
        <div className="git-graph-pair">
          <span className="small grow">
            2 commits selected: <span className="mono">{ordered[0].slice(0, 7)}</span> → <span className="mono">{ordered[1].slice(0, 7)}</span>
          </span>
          <button className="btn btn-sm btn-primary" onClick={() => actions.compare(ordered[0], ordered[1], 'direct')}>
            <GitCompare /> Compare
          </button>
          <button className="btn btn-sm btn-ghost" onClick={() => setSel([ordered[1]])}>
            Clear
          </button>
        </div>
      )}
      {selRow && (
        <>
          <Resizer axis="y" invert width={detailsH} onChange={setDetailsH} min={140} max={900} defaultValue={320} />
          <div className="git-graph-details" style={{ height: detailsH }}>
            <CommitDetails root={root} repo={repo} sha={selRow} onJump={(h) => void jumpTo(h)} onCompareParent={(p, s) => actions.compare(p, s, 'direct')} />
          </div>
        </>
      )}
      {ctx.node}
    </div>
  );
}

const validName = (v: string) => (/^[^\s~^:?*[\\]+$/.test(v.trim()) && !v.trim().startsWith('-') && !v.includes('..') && !v.trim().endsWith('.lock') ? undefined : 'Not a valid name');

/** One row's slice of the graph: lines through the band and the commit's node. */
function GraphCell({ row }: { row: GraphRow }) {
  const H = ROW_H;
  const mid = H / 2;
  const x = (c: number) => c * LANE_W + LANE_W / 2 + 4;
  const w = Math.min(row.width, MAX_LANES_SHOWN) * LANE_W + 10;
  const color = (c: number) => `var(--git-lane-${c % 8})`;
  return (
    <svg width={w} height={H} className="git-graph-svg" aria-hidden>
      {row.segs.map(([k, f, t, c], i) => {
        const stroke = color(c);
        if (k === 0) return <line key={i} x1={x(f)} y1={0} x2={x(t)} y2={H} stroke={stroke} />;
        if (k === 1) return f === t ? <line key={i} x1={x(f)} y1={0} x2={x(t)} y2={mid} stroke={stroke} /> : <path key={i} d={`M${x(f)},0 C${x(f)},${mid * 0.9} ${x(t)},${mid * 0.3} ${x(t)},${mid}`} stroke={stroke} />;
        return f === t ? <line key={i} x1={x(f)} y1={mid} x2={x(t)} y2={H} stroke={stroke} /> : <path key={i} d={`M${x(f)},${mid} C${x(f)},${mid * 1.7} ${x(t)},${mid * 1.1} ${x(t)},${H}`} stroke={stroke} />;
      })}
      {row.hash === WIP ? (
        <circle cx={x(row.col)} cy={mid} r={4.5} className="git-node-wip" stroke={color(row.color)} />
      ) : row.parents.length > 1 ? (
        <circle cx={x(row.col)} cy={mid} r={3.6} className="git-node-merge" stroke={color(row.color)} />
      ) : (
        <circle cx={x(row.col)} cy={mid} r={4.2} fill={color(row.color)} className="git-node" />
      )}
    </svg>
  );
}

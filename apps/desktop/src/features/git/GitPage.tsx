import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import {
  AlertTriangle,
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  Download,
  FolderSearch,
  GitBranch,
  GitCommitHorizontal,
  GitCompare,
  LayoutList,
  Minus,
  Plus,
  RefreshCw,
  Rows2,
  Save,
  Settings2,
  ShieldCheck,
  Split,
  Undo2,
} from 'lucide-react';
import { CodeDiff } from '@/components/editor/CodeEditor';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Callout, Empty, RelTime, Segmented, Spinner } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type CompareHint, type CompareMode, type GitFile, type GitOperation, type GitStatus, type Repo, type RepoSummary } from '@/lib/api/git';
import { useNav, useViewLabel, useViewState } from '@/store/nav';
import { listenGitChanges, useRepos } from '@/store/repos';
import { useFsVersion, useWorkspace } from '@/store/workspace';
import { SidePane } from '@/components/layout/SidePane';
import { RepoBadges, cloneRepo, locateRepo, openReposDialog, openSwitchBranch, runBulk, saveDiscovered } from './dialogs';
import { compareTitle, formatGitParam, gitTabLabel, groupRepos, isMultiRepo, parseGitParam, upstreamLabel } from './model';
import { CompareFooter, CompareMain, CompareSide, compareTabLabel } from './CompareView';
import { rememberedBase, useCompare, type CompareSpec } from './compareStore';
import { diffLanguage } from './DiffPane';
import { GraphView, useGraphNav } from './GraphView';
import { HunkView } from './HunkView';
import { RefPickerHost } from './RefPicker';
import { RefsSide } from './RefsSide';
import './git.css';

const LETTER: Record<string, string> = { M: 'Modified', A: 'Added', D: 'Deleted', R: 'Renamed', C: 'Copied', U: 'Conflict', '?': 'Untracked' };
const OPERATION: Record<GitOperation, string> = { merge: 'A merge', rebase: 'A rebase', cherryPick: 'A cherry-pick', revert: 'A revert' };

type Tab = 'changes' | 'history' | 'compare';

/**
 * Source control. A single-repo project (its folder is, or is inside, one repo) shows that repo's
 * changes (with hunk and line staging), commit graph, comparisons and branches. A multi-repo project
 * lists its repositories (grouped) in the side pane, with an "All repositories" overview and bulk
 * fetch / pull / branch switch; the selected repo gets the same per-repo tools.
 *
 * The view's param (see `parseGitParam`) names the repo and, when set, a comparison
 * (`web:compare:main...feature/x`) or a selected commit (`web:commit:<sha>`), so both survive restarts
 * and have `devall:` links.
 */
export default function GitPage() {
  const root = useWorkspace((s) => s.project!.root);
  const param = useNav((s) => s.param);
  const go = useNav((s) => s.go);
  const fsVersion = useFsVersion((s) => s.v);
  const repos = useRepos((s) => s.repos);
  const summaries = useRepos((s) => s.summaries);
  const loaded = useRepos((s) => s.loaded);
  const changed = useRepos((s) => s.changed);
  const multi = isMultiRepo(repos);
  const route = useMemo(() => parseGitParam(param), [param]);
  const selected = multi ? (repos.find((r) => r.name === route.repo) ?? null) : null;
  /** The repo the per-repo tools act on: the selected one, or (single-repo) the default. */
  const repoArg = multi ? (selected?.name ?? null) : null;
  const showRepo = loaded && (!multi || !!selected);
  const repoReady = showRepo && (!multi || selected?.state === 'found');

  const [status, setStatus] = useState<GitStatus | null>(null);
  const [sel, setSel] = useState<{ path: string; staged: boolean } | null>(null);
  const [diff, setDiff] = useState<{ original: string; modified: string; binary: boolean } | null>(null);
  const [messages, setMessages] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [viewTab, setViewTab] = useViewState<'changes' | 'history'>('tab', 'changes');
  const [reposOpen, setReposOpen] = useViewState('reposOpen', true);
  const [diffMode, setDiffMode] = useViewState<'hunks' | 'split'>('diffMode', 'hunks');
  const [ignoreWs, setIgnoreWs] = useViewState('ignoreWs', false);
  const [menuItems, setMenuItems] = useState<MenuItemDef[]>([]);
  const [hint, setHint] = useState<CompareHint | null>(null);
  const message = messages[repoArg ?? ''] ?? '';
  const setMessage = (m: string) => setMessages((x) => ({ ...x, [repoArg ?? ''] : m }));
  const tab: Tab = route.compare ? 'compare' : route.commit ? 'history' : viewTab;
  /** The repo's name for params and links (single-repo projects carry it too). */
  const repoName = multi ? (selected?.name ?? null) : (status?.repo ?? null);

  useViewLabel(
    !loaded
      ? undefined
      : route.compare
        ? compareTabLabel(multi, repoArg, route.compare)
        : multi
          ? gitTabLabel(true, selected?.name ?? null, selected ? (status?.repo === selected.name ? status.branch : summaries[selected.name]?.branch) : null)
          : status
            ? gitTabLabel(false, null, status.isRepo ? status.branch : null)
            : undefined,
  );

  const loadStatus = useCallback(async () => {
    if (!repoReady) return;
    try {
      const s = await gitApi.status(root, repoArg);
      setStatus(s);
    } catch (e) {
      toast.error('git status failed', errorMessage(e));
    }
  }, [root, repoArg, repoReady]);
  const refreshOverview = useCallback((names?: string[]) => useRepos.getState().refreshOverview(names).catch((e) => toast.error('Could not read the repositories', errorMessage(e))), []);

  // A different repo: start from a clean slate.
  useEffect(() => {
    setStatus(null);
    setSel(null);
    setHint(null);
  }, [repoArg, root]);
  useEffect(() => {
    void loadStatus();
  }, [loadStatus]);

  // The list + summaries, and a light watch of each repo's HEAD/index while the page is open.
  const repoKey = repos.map((r) => `${r.name}:${r.state}`).join('|');
  useEffect(() => {
    listenGitChanges();
    void refreshOverview();
  }, [root, refreshOverview]);
  useEffect(() => {
    if (!loaded) return;
    void gitApi.watch(root).catch(() => {});
  }, [root, loaded, repoKey]);
  useEffect(() => () => void gitApi.unwatch().catch(() => {}), []);

  // Refresh: files saved in the app (content folders), the window regaining focus (edits in an
  // editor, a terminal), and git-changed events from the watch. Debounced so bursts collapse.
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const soon = useCallback(
    (names?: string[]) => {
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => {
        void loadStatus();
        if (multi) void refreshOverview(names);
      }, 250);
    },
    [loadStatus, refreshOverview, multi],
  );
  const handled = useRef({ fs: fsVersion, git: 0 });
  useEffect(() => {
    if (fsVersion === handled.current.fs) return;
    handled.current.fs = fsVersion;
    soon();
  }, [fsVersion, soon]);
  useEffect(() => {
    const onFocus = () => soon();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [soon]);
  useEffect(() => {
    if (!changed.v || changed.v === handled.current.git) return;
    handled.current.git = changed.v;
    soon(changed.repos);
  }, [changed, soon]);
  useEffect(() => () => void (timer.current && clearTimeout(timer.current)), []);

  const selFile = sel ? status?.files.find((f) => f.path === sel.path) : undefined;
  const untrackedSel = !!sel && !sel.staged && selFile?.unstaged === '?';
  const showSplit = !!sel && (untrackedSel || diffMode === 'split' || selFile?.staged === 'U');
  useEffect(() => {
    if (!sel || !repoReady || !showSplit) return setDiff(null);
    gitApi.diff(root, repoArg, sel.path, sel.staged).then(setDiff).catch(() => setDiff(null));
  }, [sel, root, repoArg, repoReady, status, showSplit]);

  // "Review N commits before opening an MR" when the branch is ahead of its base.
  useEffect(() => {
    if (!repoReady || !status?.branch) return setHint(null);
    let alive = true;
    gitApi
      .compareHint(root, repoArg, rememberedBase(root, repoArg))
      .then((h) => alive && setHint(h))
      .catch(() => alive && setHint(null));
    return () => {
      alive = false;
    };
  }, [root, repoArg, repoReady, status?.branch, status?.ahead, changed.v]);

  // ---------- compare ----------

  const cmp = route.compare;
  useEffect(() => {
    if (!cmp || !repoReady) return;
    const st = useCompare.getState();
    const same = !!st.spec && st.root === root && st.repo === repoArg && st.spec.base === cmp.base && st.spec.head === cmp.head;
    // Always re-read on (re)entry: the repository may have moved on while another tab was shown
    // (the previous result stays on screen meanwhile).
    void st.load(root, repoArg, { ...cmp, uncommitted: same ? st.spec!.uncommitted : false }, true);
  }, [root, repoArg, repoReady, cmp?.base, cmp?.head, cmp?.mode]); // eslint-disable-line react-hooks/exhaustive-deps

  // Re-compare when the repository changes (commits, checkouts, fetches; files when uncommitted
  // changes are included).
  const compareSig = `${changed.v}|${status?.branch}|${status?.ahead}|${status?.behind}|${useCompare.getState().spec?.uncommitted ? (status?.files.map((f) => f.path + f.staged + f.unstaged).join() ?? '') : ''}`;
  const lastSig = useRef(compareSig);
  useEffect(() => {
    if (compareSig === lastSig.current) return;
    lastSig.current = compareSig;
    if (cmp && status) void useCompare.getState().refresh();
  }, [compareSig, cmp, status]);

  const routeTo = (r: { compare?: { base: string; head: string; mode: CompareMode }; commit?: string } = {}) => go('git', formatGitParam({ repo: multi ? repoArg : r.compare || r.commit ? repoName : null, ...r }));

  const openCompare = async (want?: { base?: string; head?: string; mode?: CompareMode }) => {
    const st = useCompare.getState();
    if (!want && st.spec && st.root === root && st.repo === repoArg) return routeTo({ compare: { base: st.spec.base, head: st.spec.head, mode: st.spec.mode } });
    let base = want?.base;
    let head = want?.head;
    if (!base || !head) {
      const d = await gitApi.compareDefaults(root, repoArg).catch(() => null);
      head ??= d?.current ?? status?.branch ?? 'HEAD';
      base ??= rememberedBase(root, repoArg) ?? d?.base ?? 'main';
    }
    routeTo({ compare: { base, head, mode: want?.mode ?? 'mergeBase' } });
  };

  const onSpec = (next: CompareSpec) => {
    if (!cmp) return;
    if (next.base !== cmp.base || next.head !== cmp.head || next.mode !== cmp.mode) {
      // The load effect picks the new route up; carry the uncommitted switch across a mode change.
      if (next.base === cmp.base && next.head === cmp.head) useCompare.setState((s) => (s.spec ? { spec: { ...s.spec, uncommitted: next.uncommitted } } : {}));
      routeTo({ compare: { base: next.base, head: next.head, mode: next.mode } });
    } else void useCompare.getState().load(root, repoArg, next);
  };

  const onPickRepo = async (name: string) => {
    const d = await gitApi.compareDefaults(root, name).catch(() => null);
    const base = rememberedBase(root, name) ?? d?.base ?? 'main';
    go('git', formatGitParam({ repo: name, compare: { base, head: d?.current ?? 'HEAD', mode: cmp?.mode ?? 'mergeBase' } }));
  };

  const switchTab = (t: Tab) => {
    if (t === 'compare') return void openCompare();
    setViewTab(t);
    if (route.compare || route.commit) go('git', formatGitParam({ repo: multi ? repoArg : null }));
  };

  // ---------- graph ----------

  const lastCommit = useRef<string | null>(route.commit ?? null);
  useEffect(() => {
    if (route.commit && route.commit !== lastCommit.current) {
      lastCommit.current = route.commit;
      useGraphNav.getState().go(route.commit);
    }
  }, [route.commit]);
  const onSelectCommit = useCallback(
    (sha: string | null) => {
      lastCommit.current = sha;
      go('git', formatGitParam({ repo: multi ? repoArg : repoName, commit: sha ?? undefined }));
    },
    [go, multi, repoArg, repoName],
  );

  const staged = useMemo(() => status?.files.filter((f) => f.staged !== '.' && f.staged !== '?') ?? [], [status]);
  const changes = useMemo(() => status?.files.filter((f) => f.unstaged !== '.') ?? [], [status]);

  const run = async (label: string, fn: () => Promise<unknown>, success?: string) => {
    setBusy(label);
    try {
      const out = await fn();
      if (success) toast.success(success, typeof out === 'string' && out.trim() ? out.trim().split('\n').slice(-3).join('\n') : undefined);
      await loadStatus();
    } catch (e) {
      toast.error(`${label} failed`, errorMessage(e));
    } finally {
      setBusy(null);
      if (multi) void refreshOverview(repoArg ? [repoArg] : undefined);
    }
  };

  if (!loaded) return <Empty icon={<Spinner />} title="Reading repositories…" />;
  if (!multi && !status) return <Empty icon={<Spinner />} title="Reading git status…" />;

  // ---------- single-repo project without a repo ----------
  if (!multi && status && !status.isRepo)
    return (
      <Empty
        icon={<GitBranch />}
        title="Not a git repository"
        action={
          <div className="row-sm">
            <button className="btn btn-primary" onClick={() => run('git init', () => gitApi.init(root), 'Repository created').then(() => useRepos.getState().refreshOverview())}>
              Initialize repository
            </button>
            <button className="btn" onClick={openReposDialog}>
              <Plus /> Add repositories…
            </button>
          </div>
        }
      >
        Everything in this project is plain files, made for version control. Initialize a repository to commit and share it — or, if the project's code lives in other repositories (several frontends and backends), add them.
      </Empty>
    );

  const commit = () =>
    run(
      'Commit',
      async () => {
        if (staged.length === 0) {
          const ok = await confirmDialog({ title: 'Nothing staged', message: 'Stage all changes and commit them?', confirmText: 'Stage all & commit' });
          if (!ok) throw new Error('nothing to commit');
          await gitApi.stage(root, repoArg, changes.map((f) => f.path));
        }
        const out = await gitApi.commit(root, repoArg, message);
        setMessage('');
        return out;
      },
      'Committed',
    );

  const branchMenu = async (): Promise<MenuItemDef[]> => {
    const branches = await gitApi.branches(root, repoArg).catch(() => [] as string[]);
    return [
      { heading: multi && selected ? `Switch branch — ${selected.name}` : 'Switch branch' },
      ...branches.map((b) => ({ label: b, hint: b === status?.branch ? <Check size={13} /> : undefined, onSelect: () => run('Switch branch', () => gitApi.checkout(root, repoArg, b)) })),
      { separator: true },
      {
        label: 'New branch…',
        icon: <Plus />,
        onSelect: async () => {
          const name = await promptDialog({ title: 'New branch', label: 'Branch name', validate: (v) => (/^[\w./-]+$/.test(v) && !v.startsWith('-') ? undefined : 'Invalid branch name') });
          if (name) run('Create branch', () => gitApi.checkout(root, repoArg, name, true));
        },
      },
      ...(multi ? [{ separator: true }, { label: 'Switch branch across repositories…', icon: <Split />, onSelect: () => openSwitchBranch(selected ? [selected.name] : undefined) }] : []),
    ];
  };

  const row = (f: GitFile, isStaged: boolean) => {
    const letter = isStaged ? f.staged : f.unstaged;
    return (
      <div key={`${isStaged}|${f.path}`} className={clsx('list-item git-row', sel?.path === f.path && sel.staged === isStaged && 'on')} onClick={() => setSel({ path: f.path, staged: isStaged })} title={LETTER[letter] ?? letter}>
        <span className={clsx('git-letter', `g-${letter === '?' ? 'U' : letter}`)}>{letter === '?' ? 'U' : letter}</span>
        <span className="truncate grow mono xs">{f.path}</span>
        {isStaged ? (
          <button className="icon-btn sm" title="Unstage" onClick={(e) => (e.stopPropagation(), run('Unstage', () => gitApi.unstage(root, repoArg, [f.path])))}>
            <Minus />
          </button>
        ) : (
          <>
            {letter !== '?' && (
              <button
                className="icon-btn sm danger"
                title="Discard changes"
                onClick={async (e) => {
                  e.stopPropagation();
                  if (await confirmDialog({ title: `Discard changes to ${f.path}?`, message: 'The working-tree file goes back to its staged (or committed) version. This cannot be undone.', confirmText: 'Discard', danger: true }))
                    run('Discard', () => gitApi.discard(root, repoArg, [f.path]));
                }}
              >
                <Undo2 />
              </button>
            )}
            <button className="icon-btn sm" title="Stage" onClick={(e) => (e.stopPropagation(), run('Stage', () => gitApi.stage(root, repoArg, [f.path])))}>
              <Plus />
            </button>
          </>
        )}
      </div>
    );
  };

  const present = repos.filter((r) => r.state === 'found').map((r) => r.name);
  const bulkButtons = (
    <>
      <button className="icon-btn sm" title="Fetch all repositories" disabled={!present.length} onClick={(e) => (e.stopPropagation(), void runBulk('fetch', present))}>
        <RefreshCw />
      </button>
      <button className="icon-btn sm" title="Pull all repositories (fast-forward only)" disabled={!present.length} onClick={(e) => (e.stopPropagation(), void runBulk('pull', present))}>
        <ArrowDown />
      </button>
      <button className="icon-btn sm" title="Switch branch across repositories…" disabled={!present.length} onClick={(e) => (e.stopPropagation(), openSwitchBranch())}>
        <Split />
      </button>
      <button className="icon-btn sm" title="Manage repositories…" onClick={(e) => (e.stopPropagation(), openReposDialog())}>
        <Settings2 />
      </button>
    </>
  );

  const branchSwitcher = status?.isRepo && (
    <span className="row-sm" style={{ minWidth: 0 }}>
      <Menu
        items={menuItems}
        trigger={(p) => (
          <button
            className="switcher"
            {...p}
            onClick={async (e) => {
              setMenuItems(await branchMenu());
              p.onClick(e);
            }}
          >
            <GitBranch />
            {multi && selected && <span className="truncate">{selected.name}</span>}
            <span className="truncate strong">{status.branch ?? '(detached)'}</span>
          </button>
        )}
      />
      {status.upstream && (
        <span className="xs subtle row-sm shrink-0" title={status.upstream}>
          <ArrowUp size={12} />
          {status.ahead} <ArrowDown size={12} />
          {status.behind}
        </span>
      )}
    </span>
  );

  const repoActions = (
    <>
      <button className="icon-btn sm" title={`Compare ${status?.branch ?? 'HEAD'} with its base… (review before a merge request)`} disabled={!repoReady} onClick={() => void openCompare()}>
        <GitCompare />
      </button>
      <button className="icon-btn sm" title="Pull (fast-forward only)" disabled={!!busy || !repoReady} onClick={() => run('Pull', () => gitApi.sync(root, repoArg, 'pull'), 'Pulled')}>
        <ArrowDown />
      </button>
      <button className="icon-btn sm" title="Push" disabled={!!busy || !repoReady} onClick={() => run('Push', () => gitApi.sync(root, repoArg, 'push'), 'Pushed')}>
        <ArrowUp />
      </button>
      <button className="icon-btn sm" title="Refresh" onClick={() => (loadStatus(), multi && refreshOverview())}>
        <RefreshCw />
      </button>
    </>
  );

  const conflicted = status?.files.filter((f) => f.staged === 'U').length ?? 0;
  const banners = status && (
    <>
      {status.operation && (
        <div className="p-3">
          <Callout kind="warning">
            <div className="col-sm">
              <span className="small">
                {OPERATION[status.operation]} is in progress{conflicted ? ` — ${conflicted} conflicted file${conflicted === 1 ? '' : 's'}: resolve, stage, then continue` : ''}.
              </span>
              <div className="row-sm">
                <button className="btn btn-sm" disabled={!!busy || conflicted > 0} onClick={() => run('Continue', () => gitApi.operationContinue(root, repoArg), 'Continued')}>
                  Continue
                </button>
                <button
                  className="btn btn-sm btn-danger-ghost"
                  disabled={!!busy}
                  onClick={async () => {
                    if (await confirmDialog({ title: `Abort the ${status.operation === 'cherryPick' ? 'cherry-pick' : status.operation}?`, message: 'The branch and working tree go back to where they were before it started; changes made while resolving are lost.', confirmText: 'Abort', danger: true }))
                      void run('Abort', () => gitApi.operationAbort(root, repoArg), 'Aborted');
                  }}
                >
                  Abort…
                </button>
              </div>
            </div>
          </Callout>
        </div>
      )}
      {status.indexLock && status.indexLock.ageSecs >= 120 && (
        <div className="p-3">
          <Callout kind="warning">
            <div className="col-sm">
              <span className="small">
                <span className="mono xs">{status.indexLock.path}</span> has been there for {Math.round(status.indexLock.ageSecs / 60)} min — probably left behind by a git that crashed. It blocks staging and commits.
              </span>
              <button
                className="btn btn-sm"
                onClick={async () => {
                  if (await confirmDialog({ title: 'Remove the stale lock?', message: 'Only do this when no git process (editor, terminal) is working in this repository.', confirmText: 'Remove lock' }))
                    void run('Remove lock', () => gitApi.clearLock(root, repoArg), 'Lock removed');
                }}
              >
                <AlertTriangle /> Remove lock…
              </button>
            </div>
          </Callout>
        </div>
      )}
    </>
  );

  const compareBar = status?.isRepo && (
    <button className={clsx('git-compare-bar', hint && 'ahead')} onClick={() => void openCompare(hint ? { base: hint.base, head: hint.head } : undefined)} title="Compare and review the branch's changes against its base before opening a merge request">
      <GitCompare size={14} className="shrink-0" />
      {hint ? (
        <span className="grow truncate">
          Review {hint.ahead} commit{hint.ahead === 1 ? '' : 's'} before opening an MR <span className="subtle mono">({compareTitle(hint.base, hint.head)})</span>
        </span>
      ) : (
        <span className="grow truncate">Compare… {status.branch ? <span className="subtle mono">{status.branch} → base</span> : null}</span>
      )}
    </button>
  );

  const changesPane = (
    <>
      {banners}
      <div className="git-commit">
        {compareBar}
        <textarea className="textarea" rows={3} placeholder={`Commit message (${staged.length ? `${staged.length} staged` : 'stages everything'})`} value={message} onChange={(e) => setMessage(e.target.value)} onKeyDown={(e) => (e.ctrlKey || e.metaKey) && e.key === 'Enter' && commit()} />
        <button className="btn btn-primary w-full" disabled={!!busy || !message.trim() || staged.length + changes.length === 0} onClick={commit}>
          {busy === 'Commit' ? <Spinner /> : <GitCommitHorizontal />} Commit{multi && selected ? ` to ${selected.name}` : ''}
        </button>
      </div>
      <div className="list">
        {staged.length > 0 && (
          <>
            <div className="list-group-header">
              Staged ({staged.length})<span className="spacer" />
              <button className="icon-btn sm" title="Unstage all" onClick={() => run('Unstage', () => gitApi.unstage(root, repoArg, staged.map((f) => f.path)))}>
                <Minus />
              </button>
            </div>
            {staged.map((f) => row(f, true))}
          </>
        )}
        <div className="list-group-header">
          Changes ({changes.length})<span className="spacer" />
          {changes.length > 0 && (
            <button className="icon-btn sm" title="Stage all" onClick={() => run('Stage', () => gitApi.stage(root, repoArg, changes.map((f) => f.path)))}>
              <Plus />
            </button>
          )}
        </div>
        {changes.map((f) => row(f, false))}
        {status && staged.length + changes.length === 0 && <p className="muted small p-5">Working tree clean.</p>}
        {!status && <p className="muted small p-5">Reading status…</p>}
      </div>
      {status && !status.hasScanHook && (
        <div className="p-4">
          <Callout>
            <div className="col-sm">
              <span className="small">
                Block commits that contain credentials with a pre-commit hook that runs <code className="code-inline">devall scan --staged</code>
                {multi && selected ? ` (installed in ${selected.name})` : ''}.
              </span>
              <button className="btn btn-sm" onClick={() => run('Install hook', () => gitApi.installScanHook(root, repoArg), 'Secret-scan hook installed')}>
                <ShieldCheck /> Install hook
              </button>
            </div>
          </Callout>
        </div>
      )}
    </>
  );

  const total = Object.values(summaries).reduce((n, s) => n + (s?.changes ?? 0), 0);
  const graphActions = {
    compare: (base: string, head: string, mode: CompareMode) => void openCompare({ base, head, mode }),
    changed: () => void loadStatus(),
    openChanges: () => switchTab('changes'),
  };

  const sideContent = !repoReady ? null : tab === 'changes' ? (
    changesPane
  ) : tab === 'history' ? (
    <RefsSide root={root} repo={repoArg} version={status} onCompare={graphActions.compare} onChanged={() => void loadStatus()} />
  ) : (
    <CompareSide />
  );

  const changesMain =
    sel && showSplit ? (
      diff ? (
        diff.binary ? (
          <Empty title="Binary file" />
        ) : (
          <div className="col h-full min-h-0" style={{ gap: 0 }}>
            <div className="toolbar">
              {multi && selected && <span className="badge outline">{selected.name}</span>}
              <span className="mono xs truncate grow">{sel.path}</span>
              <span className="badge outline">{sel.staged ? 'HEAD ↔ staged' : untrackedSel ? 'new file' : 'staged ↔ working tree'}</span>
              {!untrackedSel && selFile?.staged !== 'U' && (
                <button className="icon-btn sm" title="Hunks — stage or discard parts of the file" onClick={() => setDiffMode('hunks')}>
                  <Rows2 />
                </button>
              )}
            </div>
            <div className="grow min-h-0">
              <CodeDiff original={diff.original} modified={diff.modified} language={diffLanguage(sel.path, diff.modified)} />
            </div>
          </div>
        )
      ) : (
        <Empty icon={<Spinner />} title="Loading diff…" />
      )
    ) : sel ? (
      <HunkView root={root} repo={repoArg} path={sel.path} staged={sel.staged} version={status} ignoreWs={ignoreWs} onIgnoreWs={setIgnoreWs} onSideBySide={() => setDiffMode('split')} onChanged={() => void loadStatus()} />
    ) : (
      <Empty icon={<GitBranch />} title={multi && selected ? selected.name : 'Source control'}>
        {multi && selected ? (
          <>
            <span className="mono xs">{selected.path}</span>
            <br />
          </>
        ) : null}
        Select a changed file to see its diff and stage parts of it. Uses your own git — hooks, credentials and config apply.
      </Empty>
    );

  return (
    <div className="split">
      <SidePane
        label="Source control"
        title={repoReady && status?.isRepo ? branchSwitcher : multi ? <span className="strong">Source control</span> : undefined}
        actions={repoReady ? repoActions : multi ? bulkButtons : undefined}
        toolbar={
          repoReady ? (
            <Segmented
              value={tab}
              onChange={switchTab}
              options={[
                { value: 'changes', label: `Changes (${staged.length + changes.length})` },
                { value: 'history', label: 'Graph', title: 'Commit graph of every branch' },
                { value: 'compare', label: 'Compare', title: 'Review a branch against its base before a merge request' },
              ]}
            />
          ) : undefined
        }
        footer={repoReady && tab === 'compare' ? <CompareFooter /> : undefined}
      >
        {multi && (
          <div className="git-repos">
            <div className="list-group-header" onClick={() => setReposOpen(!reposOpen)}>
              {reposOpen ? <ChevronDown /> : <ChevronRight />}
              Repositories ({repos.length})<span className="spacer" />
              {repoReady && bulkButtons}
            </div>
            {reposOpen && (
              <div className="list">
                <div className={clsx('list-item', !selected && 'on')} onClick={() => go('git')} title="Branch, upstream and changes of every repository">
                  <LayoutList size={14} className="subtle" />
                  <span className="small strong grow">All repositories</span>
                  {total > 0 && <span className="xs subtle">{total} changed</span>}
                </div>
                {groupRepos(repos).map((g) => (
                  <div key={g.group ?? ''}>
                    {g.group && <div className="git-group">{g.group}</div>}
                    {g.repos.map((r) => (
                      <RepoRow key={r.name} repo={r} summary={summaries[r.name]} on={selected?.name === r.name} onOpen={() => go('git', r.name)} onRefresh={() => void refreshOverview([r.name])} />
                    ))}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
        {showRepo && selected && selected.state !== 'found' ? <RepoUnavailable repo={selected} onInit={() => run('git init', () => gitApi.init(root, selected.name), 'Repository created').then(() => refreshOverview())} /> : sideContent}
      </SidePane>
      <div className="pane fill">
        {multi && !selected ? (
          <Overview repos={repos} summaries={summaries} onOpen={(n) => go('git', n)} onRefresh={() => void refreshOverview()} />
        ) : !repoReady ? null : tab === 'compare' ? (
          <CompareMain repos={repos} multi={multi} onSpec={onSpec} onPickRepo={(n) => void onPickRepo(n)} />
        ) : tab === 'history' ? (
          <GraphView
            root={root}
            repo={repoArg}
            repoName={repoName}
            current={status?.branch ?? null}
            wip={!!status && status.files.length > 0}
            wipCount={status?.files.length ?? 0}
            version={status}
            selectedCommit={route.commit ?? null}
            onSelectCommit={onSelectCommit}
            actions={graphActions}
          />
        ) : (
          changesMain
        )}
      </div>
      <RefPickerHost />
    </div>
  );
}

function RepoRow({ repo, summary, on, onOpen, onRefresh }: { repo: Repo; summary: RepoSummary | null | undefined; on: boolean; onOpen(): void; onRefresh(): void }) {
  const up = upstreamLabel(summary);
  return (
    <div className={clsx('list-item git-repo-row', on && 'on')} onClick={onOpen} title={repo.path}>
      <div className="git-repo-line">
        <span className="small strong truncate">{repo.name}</span>
        <RepoBadges repo={repo} />
        <span className="spacer" />
        {summary && summary.changes > 0 && (
          <span className="badge accent" title={`${summary.changes} changed${summary.staged ? `, ${summary.staged} staged` : ''}`}>
            {summary.changes}
          </span>
        )}
        <button className="icon-btn sm git-row-refresh" title="Refresh" onClick={(e) => (e.stopPropagation(), onRefresh())}>
          <RefreshCw />
        </button>
      </div>
      {summary && (
        <div className="git-repo-line xs subtle">
          <GitBranch size={11} />
          <span className="mono truncate">{summary.error ? 'status failed' : (summary.branch ?? '(detached)')}</span>
          {up && up !== 'in sync' && up !== 'no upstream' && <span className="shrink-0">{up}</span>}
        </div>
      )}
    </div>
  );
}

function RepoUnavailable({ repo, onInit }: { repo: Repo; onInit(): void }) {
  return (
    <div className="p-4">
      <Empty icon={<GitBranch />} title={repo.state === 'missing' ? `${repo.name} isn't on this machine` : `${repo.name} isn't a git repository`}>
        <span className="mono xs">{repo.path}</span>
      </Empty>
      <div className="row-sm wrap" style={{ justifyContent: 'center' }}>
        {repo.state === 'missing' ? (
          <>
            {repo.remote && (
              <button className="btn btn-sm btn-primary" onClick={() => void cloneRepo(repo)}>
                <Download /> Clone…
              </button>
            )}
            <button className="btn btn-sm" onClick={() => void locateRepo(repo)}>
              <FolderSearch /> Locate…
            </button>
          </>
        ) : (
          <button className="btn btn-sm" onClick={onInit}>
            Initialize repository
          </button>
        )}
      </div>
    </div>
  );
}

/** "All repositories": one row per repo with branch, upstream state, changes and last commit. */
function Overview({ repos, summaries, onOpen, onRefresh }: { repos: Repo[]; summaries: Record<string, RepoSummary | null>; onOpen(name: string): void; onRefresh(): void }) {
  const root = useWorkspace((s) => s.project!.root);
  const present = repos.filter((r) => r.state === 'found').map((r) => r.name);
  const discovered = repos.filter((r) => r.origin === 'discovered').map((r) => r.name);
  const total = Object.values(summaries).reduce((n, s) => n + (s?.changes ?? 0), 0);
  const single = async (r: Repo, action: 'fetch' | 'pull') => {
    try {
      const out = await gitApi.sync(root, r.name, action);
      toast.success(`${action === 'fetch' ? 'Fetched' : 'Pulled'} ${r.name}`, out.trim().split('\n').slice(-3).join('\n') || undefined);
    } catch (e) {
      toast.error(`${action === 'fetch' ? 'Fetch' : 'Pull'} ${r.name} failed`, errorMessage(e));
    }
    void useRepos.getState().refreshOverview([r.name]).catch(() => {});
  };
  return (
    <div className="col h-full min-h-0" style={{ gap: 0 }}>
      <div className="toolbar">
        <span className="strong">All repositories</span>
        <span className="xs subtle">
          {repos.length} {repos.length === 1 ? 'repository' : 'repositories'}
          {total ? ` · ${total} changed files` : ''}
        </span>
        <span className="spacer" />
        <button className="btn btn-sm btn-ghost" disabled={!present.length} onClick={() => void runBulk('fetch', present)} title="git fetch --prune in every repository">
          <RefreshCw /> Fetch all
        </button>
        <button className="btn btn-sm btn-ghost" disabled={!present.length} onClick={() => void runBulk('pull', present)} title="git pull --ff-only in every repository">
          <ArrowDown /> Pull all
        </button>
        <button className="btn btn-sm btn-ghost" disabled={!present.length} onClick={() => openSwitchBranch()} title="Check out or create the same branch in several repositories">
          <Split /> Switch branch…
        </button>
        <button className="btn btn-sm btn-ghost" onClick={openReposDialog}>
          <Settings2 /> Manage…
        </button>
        <button className="icon-btn sm" title="Refresh" onClick={onRefresh}>
          <RefreshCw />
        </button>
      </div>
      {discovered.length > 0 && (
        <div className="callout-stack">
          <Callout>
            <div className="row-sm wrap">
              <span className="small grow">
                Found {discovered.length === 1 ? 'a repository' : `${discovered.length} repositories`} under the project folder that {discovered.length === 1 ? "isn't" : "aren't"} listed in devall.yaml: {discovered.join(', ')}.
              </span>
              <button className="btn btn-sm" onClick={() => void saveDiscovered(discovered)}>
                <Save /> Save to devall.yaml
              </button>
            </div>
          </Callout>
        </div>
      )}
      <div className="grow min-h-0 git-overview">
        <table className="table">
          <thead>
            <tr>
              <th>Repository</th>
              <th>Branch</th>
              <th>Upstream</th>
              <th>Changes</th>
              <th>Last commit</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {groupRepos(repos).flatMap((g) => [
              ...(g.group
                ? [
                    <tr key={`g:${g.group}`} className="git-group-row">
                      <td colSpan={6}>{g.group}</td>
                    </tr>,
                  ]
                : []),
              ...g.repos.map((r) => {
                const s = summaries[r.name];
                return (
                  <tr key={r.name} className={clsx(r.state === 'found' && 'clickable')} onClick={() => r.state === 'found' && onOpen(r.name)} title={r.path}>
                    <td>
                      <span className="row-sm">
                        <span className="strong">{r.name}</span>
                        <RepoBadges repo={r} />
                      </span>
                    </td>
                    <td className="mono xs">{s ? (s.error ? <span className="subtle" title={s.error}>status failed</span> : (s.branch ?? '(detached)')) : <span className="subtle">—</span>}</td>
                    <td className="xs">{s && !s.error ? <span className={clsx(s.behind > 0 && 'git-fail')}>{upstreamLabel(s)}</span> : null}</td>
                    <td className="xs">{s && !s.error ? (s.changes ? `${s.changes}${s.staged ? ` (${s.staged} staged)` : ''}${s.conflicts ? ` · ${s.conflicts} conflicts` : ''}` : 'clean') : null}</td>
                    <td className="xs git-commit-cell">
                      {s?.lastCommit ? (
                        <span title={`${s.lastCommit.short} · ${s.lastCommit.author}`}>
                          {s.lastCommit.subject} · <span className="subtle">
                            <RelTime ts={s.lastCommit.date} />
                          </span>
                        </span>
                      ) : null}
                    </td>
                    <td onClick={(e) => e.stopPropagation()}>
                      <div className="git-actions">
                        {r.state === 'found' && (
                          <>
                            <button className="icon-btn sm" title={`Fetch ${r.name}`} onClick={() => void single(r, 'fetch')}>
                              <RefreshCw />
                            </button>
                            <button className="icon-btn sm" title={`Pull ${r.name} (fast-forward only)`} onClick={() => void single(r, 'pull')}>
                              <ArrowDown />
                            </button>
                          </>
                        )}
                        {r.state === 'missing' && r.remote && (
                          <button className="btn btn-sm" onClick={() => void cloneRepo(r)}>
                            <Download /> Clone…
                          </button>
                        )}
                        {r.state === 'missing' && (
                          <button className="btn btn-sm" onClick={() => void locateRepo(r)}>
                            <FolderSearch /> Locate…
                          </button>
                        )}
                        {r.state === 'notARepo' && (
                          <button className="btn btn-sm" onClick={() => onOpen(r.name)}>
                            Initialize…
                          </button>
                        )}
                      </div>
                    </td>
                  </tr>
                );
              }),
            ])}
          </tbody>
        </table>
        {!repos.length && (
          <Empty icon={<GitBranch />} title="No repositories" action={<button className="btn btn-primary btn-sm" onClick={openReposDialog}><Plus /> Add repositories…</button>}>
            List the repositories this project's requests and environments are for.
          </Empty>
        )}
      </div>
    </div>
  );
}

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  Download,
  FolderSearch,
  GitBranch,
  GitCommitHorizontal,
  LayoutList,
  Minus,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Settings2,
  ShieldCheck,
  Split,
  Undo2,
} from 'lucide-react';
import { CodeDiff, languageFor } from '@/components/editor/CodeEditor';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Callout, Empty, RelTime, Segmented, Spinner } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type GitCommit, type GitFile, type GitStatus, type Repo, type RepoSummary } from '@/lib/api/git';
import { useNav, useViewLabel, useViewState } from '@/store/nav';
import { listenGitChanges, useRepos } from '@/store/repos';
import { useFsVersion, useWorkspace } from '@/store/workspace';
import { SidePane } from '@/components/layout/SidePane';
import { RepoBadges, cloneRepo, locateRepo, openReposDialog, openSwitchBranch, runBulk, saveDiscovered } from './dialogs';
import { gitTabLabel, groupRepos, isMultiRepo, upstreamLabel } from './model';
import './git.css';

const LETTER: Record<string, string> = { M: 'Modified', A: 'Added', D: 'Deleted', R: 'Renamed', C: 'Copied', U: 'Conflict', '?': 'Untracked' };

/**
 * Source control. A single-repo project (its folder is, or is inside, one repo) shows that repo's
 * changes, history, commit and branches. A multi-repo project lists its repositories (grouped) in
 * the side pane, with an "All repositories" overview and bulk fetch / pull / branch switch; the
 * selected repo (the view's param) gets the same per-repo tools.
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
  const selected = multi ? (repos.find((r) => r.name === param) ?? null) : null;
  /** The repo the per-repo tools act on: the selected one, or (single-repo) the default. */
  const repoArg = multi ? (selected?.name ?? null) : null;
  const showRepo = loaded && (!multi || !!selected);
  const repoReady = showRepo && (!multi || selected?.state === 'found');

  const [status, setStatus] = useState<GitStatus | null>(null);
  const [sel, setSel] = useState<{ path: string; staged: boolean } | null>(null);
  const [diff, setDiff] = useState<{ original: string; modified: string; binary: boolean } | null>(null);
  const [messages, setMessages] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [tab, setTab] = useViewState<'changes' | 'history'>('tab', 'changes');
  const [reposOpen, setReposOpen] = useViewState('reposOpen', true);
  const [log, setLog] = useState<GitCommit[]>([]);
  const [menuItems, setMenuItems] = useState<MenuItemDef[]>([]);
  const message = messages[repoArg ?? ''] ?? '';
  const setMessage = (m: string) => setMessages((x) => ({ ...x, [repoArg ?? '']: m }));

  useViewLabel(!loaded ? undefined : multi ? gitTabLabel(true, selected?.name ?? null, selected ? (status?.repo === selected.name ? status.branch : summaries[selected.name]?.branch) : null) : status ? gitTabLabel(false, null, status.isRepo ? status.branch : null) : undefined);

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
    setLog([]);
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

  useEffect(() => {
    if (tab === 'history' && repoReady) gitApi.log(root, repoArg).then(setLog).catch(() => setLog([]));
  }, [tab, root, repoArg, repoReady, status]);
  useEffect(() => {
    if (!sel || !repoReady) return setDiff(null);
    gitApi.diff(root, repoArg, sel.path, sel.staged).then(setDiff).catch(() => setDiff(null));
  }, [sel, root, repoArg, repoReady, status]);

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
                  if (await confirmDialog({ title: `Discard changes to ${f.path}?`, message: 'This cannot be undone.', confirmText: 'Discard', danger: true })) run('Discard', () => gitApi.discard(root, repoArg, [f.path]));
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

  const changesPane =
    tab === 'changes' ? (
      <>
        <div className="git-commit">
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
    ) : (
      <div className="list">
        {log.map((c) => (
          <div key={c.hash} className="list-item" style={{ flexDirection: 'column', alignItems: 'flex-start', gap: 2 }} title={c.hash}>
            <span className="small strong truncate w-full">{c.subject}</span>
            <span className="xs subtle">
              <span className="mono">{c.short}</span> · {c.author} · <RelTime ts={c.date} />
            </span>
          </div>
        ))}
      </div>
    );

  const total = Object.values(summaries).reduce((n, s) => n + (s?.changes ?? 0), 0);

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
              onChange={setTab}
              options={[
                { value: 'changes', label: `Changes (${staged.length + changes.length})` },
                { value: 'history', label: 'History' },
              ]}
            />
          ) : undefined
        }
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
        {showRepo && selected && selected.state !== 'found' ? (
          <RepoUnavailable repo={selected} onInit={() => run('git init', () => gitApi.init(root, selected.name), 'Repository created').then(() => refreshOverview())} />
        ) : repoReady ? (
          changesPane
        ) : null}
      </SidePane>
      <div className="pane fill">
        {multi && !selected ? (
          <Overview repos={repos} summaries={summaries} onOpen={(n) => go('git', n)} onRefresh={() => void refreshOverview()} />
        ) : sel && diff ? (
          diff.binary ? (
            <Empty title="Binary file" />
          ) : (
            <div className="col h-full min-h-0" style={{ gap: 0 }}>
              <div className="toolbar">
                {multi && selected && <span className="badge outline">{selected.name}</span>}
                <span className="mono xs truncate grow">{sel.path}</span>
                <span className="badge outline">{sel.staged ? 'HEAD ↔ staged' : 'staged ↔ working tree'}</span>
                <button className="icon-btn sm" title="Reload" onClick={() => setSel({ ...sel })}>
                  <RotateCcw />
                </button>
              </div>
              <div className="grow min-h-0">
                <CodeDiff original={diff.original} modified={diff.modified} language={languageFor(sel.path, diff.modified)} />
              </div>
            </div>
          )
        ) : (
          <Empty icon={<GitBranch />} title={multi && selected ? selected.name : 'Source control'}>
            {multi && selected ? (
              <>
                <span className="mono xs">{selected.path}</span>
                <br />
              </>
            ) : null}
            Select a changed file to see its diff. Uses your own git — hooks, credentials and config apply.
          </Empty>
        )}
      </div>
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

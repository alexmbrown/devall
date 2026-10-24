import { useCallback, useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { ArrowDown, ArrowUp, Check, GitBranch, GitCommitHorizontal, Minus, Plus, RefreshCw, RotateCcw, ShieldCheck, Undo2 } from 'lucide-react';
import { CodeDiff, languageFor } from '@/components/editor/CodeEditor';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Callout, Empty, RelTime, Segmented, Spinner } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type GitCommit, type GitFile, type GitStatus } from '@/lib/api/git';
import { useViewLabel, useViewState } from '@/store/nav';
import { useFsVersion, useWorkspace } from '@/store/workspace';
import { SidePane } from '@/components/layout/SidePane';

const LETTER: Record<string, string> = { M: 'Modified', A: 'Added', D: 'Deleted', R: 'Renamed', C: 'Copied', U: 'Conflict', '?': 'Untracked' };

export default function GitPage() {
  const project = useWorkspace((s) => s.project!);
  const fsVersion = useFsVersion((s) => s.v);
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [sel, setSel] = useState<{ path: string; staged: boolean } | null>(null);
  const [diff, setDiff] = useState<{ original: string; modified: string; binary: boolean } | null>(null);
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [tab, setTab] = useViewState<'changes' | 'history'>('tab', 'changes');
  // The tab names the checked-out branch (the section icon is a branch).
  useViewLabel(status ? (status.isRepo && status.branch ? status.branch : null) : undefined);
  const [log, setLog] = useState<GitCommit[]>([]);
  const [menuItems, setMenuItems] = useState<MenuItemDef[]>([]);
  const root = project.root;

  const load = useCallback(() => gitApi.status(root).then(setStatus).catch((e) => toast.error('git status failed', errorMessage(e))), [root]);
  useEffect(() => {
    load();
  }, [load, fsVersion]);
  useEffect(() => {
    if (tab === 'history') gitApi.log(root).then(setLog).catch(() => setLog([]));
  }, [tab, root, fsVersion]);
  useEffect(() => {
    if (!sel) return setDiff(null);
    gitApi.diff(root, sel.path, sel.staged).then(setDiff).catch(() => setDiff(null));
  }, [sel, root, status]);

  const staged = useMemo(() => status?.files.filter((f) => f.staged !== '.' && f.staged !== '?') ?? [], [status]);
  const changes = useMemo(() => status?.files.filter((f) => f.unstaged !== '.') ?? [], [status]);

  const run = async (label: string, fn: () => Promise<unknown>, success?: string) => {
    setBusy(label);
    try {
      const out = await fn();
      if (success) toast.success(success, typeof out === 'string' && out.trim() ? out.trim().split('\n').slice(-3).join('\n') : undefined);
      await load();
    } catch (e) {
      toast.error(`${label} failed`, errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  if (!status) return <Empty icon={<Spinner />} title="Reading git status…" />;
  if (!status.isRepo)
    return (
      <Empty
        icon={<GitBranch />}
        title="Not a git repository"
        action={
          <button className="btn btn-primary" onClick={() => run('git init', () => gitApi.init(root), 'Repository created')}>
            Initialize repository
          </button>
        }
      >
        Everything in this project is plain files, made for version control. Initialize a repository to commit and share it.
      </Empty>
    );

  const commit = () =>
    run(
      'Commit',
      async () => {
        if (staged.length === 0) {
          const ok = await confirmDialog({ title: 'Nothing staged', message: 'Stage all changes and commit them?', confirmText: 'Stage all & commit' });
          if (!ok) throw new Error('nothing to commit');
          await gitApi.stage(root, changes.map((f) => f.path));
        }
        const out = await gitApi.commit(root, message);
        setMessage('');
        return out;
      },
      'Committed',
    );

  const branchMenu = async (): Promise<MenuItemDef[]> => {
    const branches = await gitApi.branches(root).catch(() => [] as string[]);
    return [
      { heading: 'Switch branch' },
      ...branches.map((b) => ({ label: b, hint: b === status.branch ? <Check size={13} /> : undefined, onSelect: () => run('Switch branch', () => gitApi.checkout(root, b)) })),
      { separator: true },
      {
        label: 'New branch…',
        icon: <Plus />,
        onSelect: async () => {
          const name = await promptDialog({ title: 'New branch', label: 'Branch name', validate: (v) => (/^[\w./-]+$/.test(v) ? undefined : 'Invalid branch name') });
          if (name) run('Create branch', () => gitApi.checkout(root, name, true));
        },
      },
    ];
  };
  const row = (f: GitFile, isStaged: boolean) => {
    const letter = isStaged ? f.staged : f.unstaged;
    return (
      <div key={`${isStaged}|${f.path}`} className={clsx('list-item git-row', sel?.path === f.path && sel.staged === isStaged && 'on')} onClick={() => setSel({ path: f.path, staged: isStaged })} title={LETTER[letter] ?? letter}>
        <span className={clsx('git-letter', `g-${letter === '?' ? 'U' : letter}`)}>{letter === '?' ? 'U' : letter}</span>
        <span className="truncate grow mono xs">{f.path}</span>
        {isStaged ? (
          <button className="icon-btn sm" title="Unstage" onClick={(e) => (e.stopPropagation(), run('Unstage', () => gitApi.unstage(root, [f.path])))}>
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
                  if (await confirmDialog({ title: `Discard changes to ${f.path}?`, message: 'This cannot be undone.', confirmText: 'Discard', danger: true })) run('Discard', () => gitApi.discard(root, [f.path]));
                }}
              >
                <Undo2 />
              </button>
            )}
            <button className="icon-btn sm" title="Stage" onClick={(e) => (e.stopPropagation(), run('Stage', () => gitApi.stage(root, [f.path])))}>
              <Plus />
            </button>
          </>
        )}
      </div>
    );
  };

  return (
    <div className="split">
      <SidePane
        label="Source control"
        title={
          <span className="row-sm">
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
                <span className="truncate strong">{status.branch ?? '(detached)'}</span>
              </button>
            )}
          />
          {status.upstream && (
            <span className="xs subtle row-sm" title={status.upstream}>
              <ArrowUp size={12} />
              {status.ahead} <ArrowDown size={12} />
              {status.behind}
            </span>
          )}
          </span>
        }
        actions={
          <>
            <button className="icon-btn sm" title="Pull (fast-forward only)" disabled={!!busy} onClick={() => run('Pull', () => gitApi.sync(root, 'pull'), 'Pulled')}>
              <ArrowDown />
            </button>
            <button className="icon-btn sm" title="Push" disabled={!!busy} onClick={() => run('Push', () => gitApi.sync(root, 'push'), 'Pushed')}>
              <ArrowUp />
            </button>
            <button className="icon-btn sm" title="Refresh" onClick={() => load()}>
              <RefreshCw />
            </button>
          </>
        }
        toolbar={
          <Segmented
            value={tab}
            onChange={setTab}
            options={[
              { value: 'changes', label: `Changes (${staged.length + changes.length})` },
              { value: 'history', label: 'History' },
            ]}
          />
        }
      >
        {tab === 'changes' ? (
          <>
            <div className="git-commit">
              <textarea className="textarea" rows={3} placeholder={`Commit message (${staged.length ? `${staged.length} staged` : 'stages everything'})`} value={message} onChange={(e) => setMessage(e.target.value)} onKeyDown={(e) => (e.ctrlKey || e.metaKey) && e.key === 'Enter' && commit()} />
              <button className="btn btn-primary w-full" disabled={!!busy || !message.trim() || staged.length + changes.length === 0} onClick={commit}>
                {busy === 'Commit' ? <Spinner /> : <GitCommitHorizontal />} Commit
              </button>
            </div>
            <div className="list">
              {staged.length > 0 && (
                <>
                  <div className="list-group-header">
                    Staged ({staged.length})<span className="spacer" />
                    <button className="icon-btn sm" title="Unstage all" onClick={() => run('Unstage', () => gitApi.unstage(root, staged.map((f) => f.path)))}>
                      <Minus />
                    </button>
                  </div>
                  {staged.map((f) => row(f, true))}
                </>
              )}
              <div className="list-group-header">
                Changes ({changes.length})<span className="spacer" />
                {changes.length > 0 && (
                  <button className="icon-btn sm" title="Stage all" onClick={() => run('Stage', () => gitApi.stage(root, changes.map((f) => f.path)))}>
                    <Plus />
                  </button>
                )}
              </div>
              {changes.map((f) => row(f, false))}
              {staged.length + changes.length === 0 && <p className="muted small p-5">Working tree clean.</p>}
            </div>
            {!status.hasScanHook && (
              <div className="p-4">
                <Callout>
                  <div className="col-sm">
                    <span className="small">Block commits that contain credentials with a pre-commit hook that runs <code className="code-inline">devall scan --staged</code>.</span>
                    <button className="btn btn-sm" onClick={() => run('Install hook', () => gitApi.installScanHook(root), 'Secret-scan hook installed')}>
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
        )}
      </SidePane>
      <div className="pane fill">
        {sel && diff ? (
          diff.binary ? (
            <Empty title="Binary file" />
          ) : (
            <div className="col h-full min-h-0" style={{ gap: 0 }}>
              <div className="toolbar">
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
          <Empty icon={<GitBranch />} title="Source control">
            Select a changed file to see its diff. Uses your own git — hooks, credentials and config apply.
          </Empty>
        )}
      </div>
    </div>
  );
}

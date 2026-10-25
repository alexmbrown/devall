import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { Activity, AlertTriangle, CloudUpload, Gauge, History, Network, Play, Plus, Save, Server, Trash2 } from 'lucide-react';
import { useContextMenu } from '@/components/ui/Menu';
import { copyLinkItems } from '@/components/links/LinkChip';
import { formatLink } from '@/lib/links/syntax';
import { openLocalHistory } from '@/features/history/LocalHistoryDialog';
import { Callout, Empty, Kbd, MOD, Spinner } from '@/components/ui/misc';
import { SidePane } from '@/components/layout/SidePane';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { loadApi, type CloudRun, type DistributedRun, type Issue, type LoadTestEntry, type LoadTestFile, type Preflight } from '@/lib/api/load';
import { useNav, useViewLabel } from '@/store/nav';
import { recentDoc } from '@/store/workbench';
import { usePrefs } from '@/store/prefs';
import { parseDocParam, useWorkspace } from '@/store/workspace';
import { TestEditor, flattenRequests } from './TestEditor';
import { LiveView, SummaryView } from './ResultsView';
import { RunsPanel } from './RunsPanel';
import { CloudRegionsDialog } from './CloudRegionsDialog';
import { DistributeDialog } from './DistributeDialog';
import { EXECUTOR_INFO, fmtDuration, fmtRate } from './format';
import { runKey, useLoadRuns } from './store';
import './load.css';

export default function LoadPage() {
  const project = useWorkspace((s) => s.project);
  const param = useNav((s) => s.param);
  const go = useNav((s) => s.go);
  const live = useLoadRuns((s) => s.live);
  const runsVersion = useLoadRuns((s) => s.runsVersion);
  const [tests, setTests] = useState<LoadTestEntry[] | null>(null);
  const root = project?.root;

  const refresh = useCallback(async () => {
    if (!root) return;
    try {
      setTests(await loadApi.list(root));
    } catch (e) {
      setTests([]);
      toast.error('Could not list load tests', errorMessage(e));
    }
  }, [root]);

  useEffect(() => {
    refresh();
  }, [refresh, runsVersion]);

  const selected = param && tests?.some((t) => t.path === param) ? param : tests?.[0]?.path;
  useViewLabel(tests ? (tests.find((t) => t.path === selected)?.name ?? null) : undefined);
  const ctx = useContextMenu();

  if (!project || !root)
    return (
      <Empty icon={<Gauge />} title="Open a project to load test it">
        Load tests reuse the saved requests of a project.
      </Empty>
    );

  const create = async () => {
    const name = await promptDialog({ title: 'New load test', label: 'Name', placeholder: 'Checkout smoke', validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!name) return;
    // Start from the request most recently looked at in a Requests tab.
    const request = recentDoc('http', (p) => parseDocParam(p).kind === 'request');
    try {
      const path = await loadApi.create(root, name, request);
      await refresh();
      go('load', path);
    } catch (e) {
      toast.error('Could not create load test', errorMessage(e));
    }
  };

  return (
    <div className="split load-page">
      <SidePane
        title="Load tests"
        actions={
          <button className="icon-btn" onClick={create} title="New load test" aria-label="New load test">
            <Plus />
          </button>
        }
      >
        {!tests ? (
          <div className="empty">
            <Spinner />
          </div>
        ) : tests.length === 0 ? (
          <Empty
            icon={<Activity />}
            title="No load tests yet"
            action={
              <button className="btn btn-sm btn-primary" onClick={create}>
                <Plus /> New load test
              </button>
            }
          >
            Load test any saved request — or a weighted mix or flow of them — with honest latency maths.
          </Empty>
        ) : (
          <div className="list load-list" role="listbox" aria-label="Load tests">
            {tests.map((t) => {
              const running = !!live[runKey(root, t.path)];
              return (
                <div
                  key={t.path}
                  role="option"
                  aria-selected={t.path === selected}
                  tabIndex={0}
                  className={clsx('list-item', t.path === selected && 'on', t.error && 'invalid')}
                  onClick={() => go('load', t.path)}
                  onContextMenu={(e) => ctx.open(e, [{ label: 'Local history…', icon: <History />, onSelect: () => openLocalHistory({ root, path: `load/${t.path}`, title: t.name }) }, ...copyLinkItems(formatLink('load', t.path), t.name)])}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault();
                      go('load', t.path);
                    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                      e.preventDefault();
                      const sib = (e.key === 'ArrowDown' ? e.currentTarget.nextElementSibling : e.currentTarget.previousElementSibling) as HTMLElement | null;
                      sib?.focus();
                    }
                  }}
                  title={t.error ?? t.path}
                >
                  {running ? <span className="dot ok pulse" aria-label="running" /> : t.error ? <AlertTriangle size={14} className="text-danger" /> : <Gauge size={14} className="subtle" />}
                  <span className="grow truncate">{t.name}</span>
                  <span className="meta">
                    {t.executor ? EXECUTOR_INFO[t.executor].short : 'invalid'}
                    {t.runs > 0 ? ` · ${t.runs}` : ''}
                  </span>
                </div>
              );
            })}
          </div>
        )}
        {ctx.node}
      </SidePane>
      <div className="pane fill">
        {selected ? (
          <TestView key={`${root}|${selected}`} root={root} path={selected} onChanged={refresh} />
        ) : tests && tests.length > 0 ? null : (
          <Empty icon={<Gauge />} title="Create a load test">
            Tests are YAML files under <code className="code-inline">load/</code>; every run saves a summary on this computer (never in the project) so you can compare runs over time — locally, for free.
          </Empty>
        )}
      </div>
    </div>
  );
}

type Tab = 'design' | 'results' | 'history';

function TestView({ root, path, onChanged }: { root: string; path: string; onChanged(): void }) {
  const go = useNav((s) => s.go);
  const tree = useWorkspace((s) => s.tree);
  const project = useWorkspace((s) => s.project);
  const activeEnv = useWorkspace((s) => s.activeEnv());
  const autoSave = usePrefs((s) => s.autoSave);
  const key = runKey(root, path);
  const live = useLoadRuns((s) => s.live[key]);
  const shown = useLoadRuns((s) => s.shown[key]);
  const [draft, setDraft] = useState<LoadTestFile | null>(null);
  const [saved, setSaved] = useState('');
  const [issues, setIssues] = useState<Issue[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>(live ? 'results' : 'design');
  const [preflight, setPreflight] = useState<Preflight | null>(null);
  const [distribute, setDistribute] = useState(false);
  const [cloudOpen, setCloudOpen] = useState(false);
  const [preflightError, setPreflightError] = useState<string | null>(null);
  const [loadingLatest, setLoadingLatest] = useState(false);
  const requests = useMemo(() => flattenRequests(tree), [tree]);
  const dirty = draft != null && JSON.stringify(draft) !== saved;
  const draftRef = useRef(draft);
  draftRef.current = draft;

  useEffect(() => {
    loadApi
      .read(root, path)
      .then((t) => {
        setDraft(t);
        setSaved(JSON.stringify(t));
        return loadApi.validate(t).then(setIssues);
      })
      .catch((e) => setLoadError(errorMessage(e)));
  }, [root, path]);

  const save = useCallback(async (): Promise<Issue[] | null> => {
    const t = draftRef.current;
    if (!t) return null;
    try {
      const found = await loadApi.save(root, path, t);
      setSaved(JSON.stringify(t));
      setIssues(found);
      onChanged();
      return found;
    } catch (e) {
      toast.error('Save failed', errorMessage(e));
      return null;
    }
  }, [root, path, onChanged]);

  // Autosave (or validate only) shortly after edits.
  useEffect(() => {
    if (!draft || !dirty) return;
    const timer = setTimeout(() => (autoSave ? save() : loadApi.validate(draft).then(setIssues).catch(() => {})), autoSave ? 600 : 300);
    return () => clearTimeout(timer);
  }, [draft, dirty, autoSave, save]);

  // Preflight (hosts, guard rails, warnings) for the current draft once it is valid.
  useEffect(() => {
    if (!draft || issues.length) {
      setPreflight(null);
      return;
    }
    const timer = setTimeout(() => {
      loadApi
        .preflight(root, path, draft, activeEnv?.id)
        .then((p) => {
          setPreflight(p);
          setPreflightError(null);
        })
        .catch((e) => {
          setPreflight(null);
          setPreflightError(errorMessage(e));
        });
    }, 700);
    return () => clearTimeout(timer);
  }, [draft, issues, root, path, activeEnv?.id]);

  // Results tab with nothing in memory: show the latest saved run.
  useEffect(() => {
    if (tab !== 'results' || live || shown) return;
    let alive = true;
    setLoadingLatest(true);
    loadApi
      .runs(root, path)
      .then(async (runs) => {
        if (runs[0] && alive) useLoadRuns.getState().show(root, path, { summary: await loadApi.readRun(root, runs[0].file), file: runs[0].file });
      })
      .catch(() => {})
      .finally(() => alive && setLoadingLatest(false));
    return () => {
      alive = false;
    };
  }, [tab, live, shown, root, path]);

  const run = useCallback(async (againstMock = false, distributed?: DistributedRun, cloud?: CloudRun) => {
    if (!draftRef.current || useLoadRuns.getState().live[key]) return;
    const found = await save();
    if (found == null) return;
    if (found.length) {
      setTab('design');
      toast.error('Fix the test first', found.map((i) => `${i.field}: ${i.message}`).join('\n'));
      return;
    }
    setTab('results');
    useLoadRuns.getState().start(root, path, useWorkspace.getState().activeEnv()?.id, againstMock, distributed, cloud);
  }, [key, root, path, save]);

  const stop = () => useLoadRuns.getState().stop(root, path);

  const remove = async () => {
    const ok = await confirmDialog({
      title: `Delete “${draft?.name ?? path}”?`,
      message: 'The test file moves to your system trash. Its saved runs stay.',
      confirmText: 'Move to trash',
      danger: true,
    });
    if (!ok) return;
    try {
      await loadApi.remove(root, path);
      onChanged();
      go('load');
      toast.success('Moved to the trash', `load/${path}`);
    } catch (e) {
      toast.error('Delete failed', errorMessage(e));
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key === 'Enter') {
      e.preventDefault();
      if (!live) run();
    } else if (mod && e.key.toLowerCase() === 's') {
      e.preventDefault();
      save();
    }
  };

  if (loadError)
    return (
      <div className="page">
        <div className="page-body">
          <Callout kind="danger">
            <div className="strong">load/{path} can’t be read</div>
            <div className="mono xs break">{loadError}</div>
            <div className="small mt-2">Fix the YAML in your editor; the app reloads it when you come back.</div>
          </Callout>
        </div>
      </div>
    );
  if (!draft)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );

  return (
    <div className="page" onKeyDown={onKeyDown}>
      <div className="page-header">
        <input className="name-input" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} aria-label="Test name" />
        <span className="subtle xs mono truncate">load/{path}</span>
        {dirty && !autoSave && <span className="badge outline">unsaved</span>}
        <span className="spacer" />
        {!autoSave && (
          <button className="btn btn-sm" onClick={() => save()} disabled={!dirty} title={`Save (${MOD}+S)`}>
            <Save /> Save
          </button>
        )}
        <button className="icon-btn danger" onClick={remove} title="Delete test" aria-label="Delete test">
          <Trash2 />
        </button>
        {live ? (
          <button className="btn btn-sm btn-danger" onClick={stop} disabled={live.stopping}>
            {live.stopping ? 'Stopping…' : 'Stop'}
          </button>
        ) : (
          <>
            {draft.target !== 'mock' && (
              <button className="btn btn-sm" onClick={() => setDistribute(true)} title="Split this test across load agents on your own machines (devall load agent) and merge the results">
                <Network /> Distribute…
              </button>
            )}
            {draft.target !== 'mock' && (
              <button
                className="btn btn-sm"
                onClick={() => void save().then(() => setCloudOpen(true))}
                title="Run from cloud regions… — the relay's edge in up to 3 regions (Supporter, or any self-hosted relay), against hosts you verified"
              >
                <CloudUpload /> Cloud regions…
              </button>
            )}
            {draft.target !== 'mock' && (
              <button
                className="btn btn-sm"
                onClick={() => run(true)}
                title="Same test against the project's mock server (started if needed): a baseline of client and tooling overhead to compare real runs with"
              >
                <Server /> Run against mock
              </button>
            )}
            <button className="btn btn-sm btn-primary" onClick={() => run()} title={`Run (${MOD}+Enter)${draft.target === 'mock' ? ' — against the project mock server' : ''}`}>
              <Play /> Run
            </button>
          </>
        )}
      </div>
      <DistributeDialog open={distribute} onClose={() => setDistribute(false)} preflight={preflight} onRun={(d) => void run(false, d)} />
      <CloudRegionsDialog open={cloudOpen} onClose={() => setCloudOpen(false)} root={root} path={path} onRun={(c) => void run(false, undefined, c)} />
      <div className="subtabs" role="tablist">
        {(['design', 'results', 'history'] as Tab[]).map((t) => (
          <button key={t} role="tab" aria-selected={tab === t} className={clsx('subtab', tab === t && 'on')} onClick={() => setTab(t)}>
            {t === 'design' ? 'Design' : t === 'results' ? 'Results' : 'History'}
            {t === 'design' && issues.length > 0 && <span className="badge danger">{issues.length}</span>}
            {t === 'results' && live && <span className="dot ok pulse" />}
          </button>
        ))}
      </div>
      <div className="page-body">
        {tab === 'design' && (
          <div className="col-lg">
            {issues.length > 0 && (
              <Callout kind="danger">
                {issues.length === 1 ? 'One problem' : `${issues.length} problems`} to fix before running:
                <ul className="small" style={{ margin: '4px 0 0', paddingLeft: 18 }}>
                  {issues.map((i, k) => (
                    <li key={k}>
                      <span className="mono xs">{i.field || 'test'}</span> — {i.message}
                    </li>
                  ))}
                </ul>
              </Callout>
            )}
            {preflightError && <Callout kind="danger">{preflightError}</Callout>}
            {preflight && <PreflightInfo p={preflight} />}
            <TestEditor test={draft} onChange={setDraft} issues={issues} requests={requests} environments={project?.environments ?? []} activeEnvName={activeEnv?.file.name} />
            <p className="subtle xs">
              <Kbd>{MOD}</Kbd> <Kbd>Enter</Kbd> runs · CLI: <code className="code-inline">devall load run load/{path}</code> (add{' '}
              <code className="code-inline">--against-mock</code> for a mock baseline)
            </p>
          </div>
        )}
        {tab === 'results' &&
          (live ? (
            <LiveView live={live} onStop={stop} />
          ) : shown ? (
            <SummaryView summary={shown.summary} file={shown.file} />
          ) : loadingLatest ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : (
            <Empty
              icon={<Gauge />}
              title="No runs yet"
              action={
                <button className="btn btn-primary" onClick={() => run()}>
                  <Play /> Run now
                </button>
              }
            >
              Results stream in live; the summary is saved on this computer when the run ends.
            </Empty>
          ))}
        {tab === 'history' && (
          <RunsPanel
            root={root}
            path={path}
            onOpen={(r) => {
              useLoadRuns.getState().show(root, path, r);
              setTab('results');
            }}
          />
        )}
      </div>
    </div>
  );
}

function PreflightInfo({ p }: { p: Preflight }) {
  return (
    <div className="col-sm">
      <div className="load-meta">
        <span>
          <b>Targets</b> {p.hosts.join(', ') || '—'}
        </span>
        <span>
          <b>Duration</b> {fmtDuration(p.plannedDurationMs)}
        </span>
        {p.peakRate != null && (
          <span>
            <b>Peak rate</b> {fmtRate(p.peakRate).replace('/s', '')} iterations/s
          </span>
        )}
        {p.expectedIterations != null && (
          <span>
            <b>Planned iterations</b> {p.expectedIterations.toLocaleString()}
          </span>
        )}
        <span>
          <b>Max VUs</b> {p.maxVus}
        </span>
        <span>
          <b>Environment</b> {p.environment ?? 'none'}
        </span>
        <span>
          <b>Latency</b> {p.openModel ? 'from intended start (CO-corrected)' : 'from send (closed model)'}
        </span>
      </div>
      {p.requiresConfirmation && <Callout kind="warning">{p.requiresConfirmation.reason} You’ll be asked to confirm when you run it.</Callout>}
      {p.warnings.map((w, i) => (
        <Callout key={i} kind="warning">
          {w}
        </Callout>
      ))}
      {p.secretErrors.map((w, i) => (
        <Callout key={`s${i}`} kind="danger">
          Keychain: {w}
        </Callout>
      ))}
    </div>
  );
}

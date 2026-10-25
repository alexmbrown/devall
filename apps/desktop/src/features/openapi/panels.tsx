/**
 * OpenAPI panels: lint a spec, check drift (spec ↔ mocks ↔ traffic) and manage a request folder's
 * linked spec. Used by the OpenAPI dialog, the mock server's Drift tab and the inspector.
 */
import { useCallback, useEffect, useState } from 'react';
import { FolderOpen, Link2, Play, RefreshCw, ShieldCheck, Unlink } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { Callout, Checkbox, Empty, Field, Spinner, Switch } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { openapiApi, syncSummary, type DriftFinding, type DriftOptions, type DriftReport, type LinkedFolder, type LintReport, type SyncReport } from '@/lib/api/openapi';
import { useWorkspace } from '@/store/workspace';
import { reposApi } from '@/lib/api/git';
import { DriftReportView, LintReportView } from './reports';
import { toProjectPath } from './logic';
import './openapi.css';

const LAST_SPEC = (root: string) => `devall.openapi.spec:${root}`;

export function rememberSpec(root: string, spec: string) {
  try {
    localStorage.setItem(LAST_SPEC(root), spec);
  } catch {
    /* storage unavailable */
  }
}

export function lastSpec(root: string): string {
  try {
    return localStorage.getItem(LAST_SPEC(root)) ?? '';
  } catch {
    return '';
  }
}

/**
 * Picks a spec: the OpenAPI files found in the project and its repositories (`repo:<name>/…`), or
 * any file via the OS dialog — stored repo-qualified when it's in one of the project's repos, so
 * the reference works wherever teammates keep that repo.
 */
export function SpecPicker({ root, value, onChange, label = 'OpenAPI spec' }: { root: string; value: string; onChange(v: string): void; label?: string }) {
  const [specs, setSpecs] = useState<string[] | null>(null);
  useEffect(() => {
    let alive = true;
    openapiApi
      .specs(root)
      .then((s) => {
        if (!alive) return;
        setSpecs(s);
        if (!value && s.length) onChange(lastSpec(root) && s.includes(lastSpec(root)) ? lastSpec(root) : s[0]);
      })
      .catch(() => alive && setSpecs([]));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root]);
  const browse = async () => {
    const picked = await openDialog({ multiple: false, directory: false, filters: [{ name: 'OpenAPI', extensions: ['yaml', 'yml', 'json'] }] });
    if (typeof picked !== 'string') return;
    onChange(await reposApi.portable(root, picked).catch(() => toProjectPath(root, picked)));
  };
  const listId = `oa-specs-${root.length}`;
  return (
    <Field label={label} hint={specs && !specs.length ? 'No OpenAPI files found in the project or its repositories — browse for one.' : undefined}>
      <div className="row-sm">
        <input className="input mono grow" list={listId} value={value} placeholder="openapi/api.yaml" onChange={(e) => onChange(e.target.value)} />
        <datalist id={listId}>
          {(specs ?? []).map((s) => (
            <option key={s} value={s} />
          ))}
        </datalist>
        <button className="btn btn-sm" onClick={browse} title="Choose a file">
          <FolderOpen /> Browse…
        </button>
      </div>
    </Field>
  );
}

export function LintPanel({ initialSpec }: { initialSpec?: string }) {
  const root = useWorkspace((s) => s.project?.root);
  const [spec, setSpec] = useState(initialSpec ?? '');
  const [report, setReport] = useState<LintReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback(
    async (path: string) => {
      if (!root || !path.trim()) return;
      setBusy(true);
      setError(null);
      try {
        setReport(await openapiApi.lintFile(root, path.trim()));
        rememberSpec(root, path.trim());
      } catch (e) {
        setReport(null);
        setError(errorMessage(e));
      } finally {
        setBusy(false);
      }
    },
    [root],
  );
  useEffect(() => {
    if (initialSpec) run(initialSpec);
  }, [initialSpec, run]);
  if (!root) return <Empty title="Open a project" />;
  return (
    <div className="col">
      <div className="oa-controls">
        <SpecPicker root={root} value={spec} onChange={setSpec} />
        <button className="btn btn-primary" disabled={busy || !spec.trim()} onClick={() => run(spec)}>
          {busy ? <Spinner /> : <ShieldCheck />} Lint
        </button>
      </div>
      <p className="subtle xs">
        Built-in rules, no network: operationIds, summaries, path parameters, responses and schemas, $refs, unused components, security schemes, examples. CLI:{' '}
        <code className="code-inline">devall openapi lint {spec || '<file>'}</code> (exits 1 on errors).
      </p>
      {error && <Callout kind="danger">{error}</Callout>}
      {report && <LintReportView report={report} />}
    </div>
  );
}

export function DriftPanel({ initial, onOpenStub, autoRun }: { initial?: DriftOptions & { spec?: string }; onOpenStub?(f: DriftFinding): void; autoRun?: boolean }) {
  const root = useWorkspace((s) => s.project?.root);
  const [spec, setSpec] = useState(initial?.spec ?? '');
  const [opts, setOpts] = useState<DriftOptions>({ mocks: true, inspector: false, history: false, allHosts: false, ...initial });
  const [report, setReport] = useState<DriftReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (p: Partial<DriftOptions>) => setOpts((o) => ({ ...o, ...p }));
  const nothing = !opts.mocks && !opts.inspector && !opts.history && !opts.har;
  const run = async () => {
    if (!root || !spec.trim() || nothing) return;
    setBusy(true);
    setError(null);
    try {
      setReport(await openapiApi.drift(root, spec.trim(), opts));
      rememberSpec(root, spec.trim());
    } catch (e) {
      setReport(null);
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const [ran, setRan] = useState(false);
  useEffect(() => {
    if (autoRun && !ran && spec && root) {
      setRan(true);
      void run();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRun, spec, root]);
  const pickHar = async () => {
    const picked = await openDialog({ multiple: false, directory: false, filters: [{ name: 'HAR', extensions: ['har', 'json'] }] });
    if (typeof picked === 'string' && root) set({ har: toProjectPath(root, picked) });
  };
  if (!root) return <Empty title="Open a project" />;
  return (
    <div className="col">
      <div className="oa-controls">
        <SpecPicker root={root} value={spec} onChange={setSpec} />
        <button className="btn btn-primary" disabled={busy || !spec.trim() || nothing} onClick={run} title={nothing ? 'Pick at least one source' : undefined}>
          {busy ? <Spinner /> : <Play />} Check drift
        </button>
      </div>
      <div className="oa-sources">
        <span className="field-label">Check</span>
        <Checkbox checked={!!opts.mocks} onChange={(mocks) => set({ mocks })} label="Mock stubs (mocks/mappings)" />
        <Checkbox
          checked={!!opts.inspector}
          onChange={(inspector) => set({ inspector })}
          label={opts.inspectorIds?.length ? `Inspector (${opts.inspectorIds.length} selected)` : 'Inspector captures'}
        />
        <Checkbox checked={!!opts.history} onChange={(history) => set({ history })} label="Request history" />
        <span className="row-sm">
          <button className="btn btn-sm btn-ghost" onClick={pickHar}>
            <FolderOpen /> {opts.har ? 'HAR:' : 'HAR file…'}
          </button>
          {opts.har && (
            <>
              <span className="mono xs truncate" style={{ maxWidth: 240 }} title={opts.har}>
                {opts.har}
              </span>
              <button className="icon-btn sm" onClick={() => set({ har: undefined })} aria-label="Remove HAR file">
                ×
              </button>
            </>
          )}
        </span>
        <Switch checked={!!opts.allHosts} onChange={(allHosts) => set({ allHosts })} label={<span className="xs">All hosts</span>} title="Check traffic to every host, not only the spec's servers" />
      </div>
      <p className="subtle xs">
        Stubs must map to a documented operation and status, and JSON bodies must match the response schema; traffic is checked the same way, request bodies included. CLI:{' '}
        <code className="code-inline">devall openapi drift --spec {spec || '<file>'} --mocks</code>
      </p>
      {error && <Callout kind="danger">{error}</Callout>}
      {report && <DriftReportView report={report} onOpenStub={onOpenStub} />}
    </div>
  );
}

/** A request folder's linked spec: link / change, auto-sync, re-sync now, unlink. */
export function LinkPanel({ folder }: { folder: string }) {
  const root = useWorkspace((s) => s.project?.root);
  const [link, setLink] = useState<LinkedFolder | null | undefined>(undefined);
  const [source, setSource] = useState('');
  const [auto, setAuto] = useState(true);
  const [busy, setBusy] = useState(false);
  const [last, setLast] = useState<SyncReport | null>(null);
  const load = useCallback(async () => {
    if (!root) return;
    const all = await openapiApi.links(root).catch(() => []);
    const l = all.find((x) => x.folder === folder) ?? null;
    setLink(l);
    if (l) {
      setSource(l.link.source);
      setAuto(!!l.link.autoSync);
    }
  }, [root, folder]);
  useEffect(() => {
    load();
  }, [load]);
  if (!root) return <Empty title="Open a project" />;
  if (link === undefined)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  const after = async (r: SyncReport | null) => {
    setLast(r);
    await useWorkspace.getState().refresh();
    await load();
  };
  const save = async (resync: boolean) => {
    if (!source.trim()) return;
    setBusy(true);
    try {
      const [, report] = await openapiApi.link(root, folder, source.trim(), auto, resync);
      if (report) toast.success(`Re-synced http/${folder}`, syncSummary(report));
      else toast.success('Spec linked', source.trim());
      await after(report);
    } catch (e) {
      toast.error('Could not link the spec', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const resync = async () => {
    setBusy(true);
    try {
      const r = await openapiApi.resync(root, folder);
      toast.success(`Re-synced http/${folder}`, syncSummary(r));
      await after(r);
    } catch (e) {
      toast.error('Re-sync failed', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const unlink = async () => {
    const ok = await confirmDialog({ title: 'Unlink the spec?', message: 'The requests stay; the folder just stops following the spec file.', confirmText: 'Unlink' });
    if (!ok) return;
    await openapiApi.unlink(root, folder).catch((e) => toast.error('Unlink failed', errorMessage(e)));
    setLast(null);
    await load();
  };
  const isUrl = /^https?:\/\//i.test(source.trim());
  return (
    <div className="col oa-link-card">
      <p className="subtle small">
        A linked folder is re-imported from its spec in place: requests are updated by name (ids, tests and scripts kept), new operations are added, and requests whose operation disappeared are listed — never deleted. The link is stored in{' '}
        <code className="code-inline">http/{folder}/_folder.yaml</code> so the whole team shares it.
      </p>
      {link && !link.exists && <Callout kind="warning">The linked file {link.link.source} does not exist.</Callout>}
      <SpecPicker root={root} value={source} onChange={setSource} label="Spec file or URL" />
      <Switch
        checked={auto && !isUrl}
        onChange={setAuto}
        disabled={isUrl}
        label={<span className="small">Re-sync automatically when the file changes</span>}
        title={isUrl ? 'URLs are only fetched when you click Re-sync' : 'Watches the project folder; specs outside the project need a manual re-sync'}
      />
      <div className="row-sm">
        <button className="btn btn-primary" disabled={busy || !source.trim()} onClick={() => save(true)}>
          {busy ? <Spinner /> : <Link2 />} {link ? 'Save & re-sync' : 'Link & sync now'}
        </button>
        {link && (
          <>
            <button className="btn" disabled={busy} onClick={resync}>
              <RefreshCw /> Re-sync now
            </button>
            <button className="btn btn-ghost" disabled={busy} onClick={unlink}>
              <Unlink /> Unlink
            </button>
          </>
        )}
        {!link && (
          <button className="btn btn-ghost" disabled={busy || !source.trim()} onClick={() => save(false)}>
            Link only
          </button>
        )}
      </div>
      {last && (
        <Callout kind={last.stale.length ? 'warning' : 'success'}>
          <div>
            {syncSummary(last)} from <code className="code-inline">{last.source}</code>
          </div>
          {last.stale.length > 0 && (
            <ul className="small" style={{ margin: '4px 0 0', paddingLeft: 18 }}>
              {last.stale.map((s) => (
                <li key={s}>
                  <code className="code-inline">http/{s}</code> is no longer in the spec
                </li>
              ))}
            </ul>
          )}
          {last.report.warnings.map((w, i) => (
            <div key={i} className="xs">
              {w}
            </div>
          ))}
        </Callout>
      )}
    </div>
  );
}

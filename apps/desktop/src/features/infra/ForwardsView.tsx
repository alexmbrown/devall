import { useCallback, useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { AlertTriangle, Cable, Container, DatabaseZap, ExternalLink, FileDown, MoreHorizontal, Pencil, Play, Plus, RefreshCcw, Save, Square, Trash2 } from 'lucide-react';
import { Callout, Empty, Spinner } from '@/components/ui/misc';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { infraApi, type ForwardEntry, type ForwardFile, type ForwardListing, type ForwardStatus } from '@/lib/api/infra';
import { useVarsVersion } from '@/store/variables';
import { useWorkspace } from '@/store/workspace';
import { forwardTemplate, openHttpRequest, startForward, stopForward, useInfraUi } from './store';

interface Row {
  id: string;
  spec?: ForwardFile;
  saved?: ForwardEntry;
  running?: ForwardStatus;
}

function targetText(s?: ForwardFile): string {
  if (!s) return '';
  if (s.source === 'docker') return `container ${s.target.name}:${s.remotePort}`;
  return `${s.namespace ? `${s.namespace}/` : ''}${s.target.kind}/${s.target.name}:${s.remotePort}`;
}

function stateDot(st?: ForwardStatus): string {
  if (!st) return '';
  if (st.state === 'active') return 'ok';
  if (st.state === 'reconnecting' || st.state === 'starting') return 'warn pulse';
  if (st.state === 'error') return 'err';
  return '';
}

export function ForwardsView() {
  const project = useWorkspace((s) => s.project);
  const version = useInfraUi((s) => s.forwardsVersion);
  const [listing, setListing] = useState<ForwardListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const root = project?.root;

  const load = useCallback(async () => {
    if (!root) return;
    try {
      setListing(await infraApi.forward.list(root));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [root]);

  useEffect(() => {
    void load();
    const t = setInterval(load, 2000);
    return () => clearInterval(t);
  }, [load, version]);

  const rows: Row[] = useMemo(() => {
    if (!listing) return [];
    const out: Row[] = listing.saved.map((s) => ({ id: s.id, spec: s.file, saved: s, running: listing.running.find((r) => r.id === s.id) }));
    for (const r of listing.running) if (!out.some((o) => o.id === r.id)) out.push({ id: r.id, spec: r.spec, running: r });
    return out;
  }, [listing]);

  if (!project || !root)
    return (
      <Empty icon={<Cable />} title="Open a project to manage port-forwards">
        Forwards are saved as <code className="code-inline">connections/*.forward.yaml</code> and bind URLs into the project's environments.
      </Empty>
    );

  const envName = (id?: string) => (id ? (project.environments.find((e) => e.id === id)?.file.name ?? id) : 'any environment');

  const run = async (key: string, fn: () => Promise<unknown>) => {
    setBusy(key);
    try {
      await fn();
    } finally {
      setBusy(null);
      void load();
    }
  };

  const saveAdhoc = async (r: Row) => {
    if (!r.spec) return;
    try {
      const id = await infraApi.forward.save(root, undefined, r.spec, r.id);
      toast.success(`Saved connections/${id}.forward.yaml`, 'Commit it to share the forward with your team.');
      void load();
    } catch (e) {
      toast.error('Could not save', errorMessage(e));
    }
  };

  const saveToEnv = async (r: Row) => {
    const st = r.running;
    if (!st?.binding) return;
    let envId = st.binding.environment;
    if (!envId) {
      envId = useWorkspace.getState().activeEnv()?.id;
      if (!envId) {
        toast.error('Pick an environment first', 'This forward binds "whichever environment is active" and none is active.');
        return;
      }
    }
    const ok = await confirmDialog({
      title: `Save ${st.binding.variable} to "${envName(envId)}"?`,
      message: (
        <>
          Writes <code className="code-inline">{`${st.binding.variable}: ${st.binding.value}`}</code> into <code className="code-inline">environments/{envId}.env.yaml</code>. The value stays after the forward
          stops.
        </>
      ),
      confirmText: 'Save to environment',
    });
    if (!ok) return;
    try {
      await infraApi.forward.saveToEnvironment(root, r.id, envId);
      await useWorkspace.getState().refresh();
      useVarsVersion.getState().bump();
      toast.success(`Saved ${st.binding.variable} to ${envName(envId)}`);
    } catch (e) {
      toast.error('Could not save to the environment', errorMessage(e));
    }
  };

  const remove = async (r: Row) => {
    const ok = await confirmDialog({
      title: `Delete forward "${r.spec?.name ?? r.id}"?`,
      message: `Stops it and moves connections/${r.id}.forward.yaml to the trash.`,
      confirmText: 'Delete',
      danger: true,
    });
    if (!ok) return;
    try {
      await infraApi.forward.delete(root, r.id);
      useVarsVersion.getState().bump();
      void load();
    } catch (e) {
      toast.error('Could not delete', errorMessage(e));
    }
  };

  const menu = (r: Row): MenuItemDef[] => {
    const items: MenuItemDef[] = [];
    if (r.spec) items.push({ label: 'Edit…', icon: <Pencil />, onSelect: () => useInfraUi.getState().openForward({ spec: r.spec!, id: r.saved ? r.id : undefined }) });
    if (!r.saved && r.spec) items.push({ label: 'Save to connections/', icon: <Save />, onSelect: () => saveAdhoc(r) });
    if (r.running?.binding) items.push({ label: 'Save value to environment…', icon: <FileDown />, onSelect: () => saveToEnv(r) });
    if (r.running?.url) items.push({ label: 'Copy URL', onSelect: () => navigator.clipboard.writeText(r.running!.url!).then(() => toast.success('Copied')) });
    if (r.saved) items.push({ separator: true }, { label: 'Delete…', icon: <Trash2 />, danger: true, onSelect: () => remove(r) });
    return items;
  };

  const newForward = () =>
    useInfraUi.getState().openForward({ spec: forwardTemplate({ name: '', target: { kind: 'service', name: '' }, remotePort: 80 }) });

  const activeBindings = listing?.running.filter((r) => r.binding) ?? [];

  return (
    <div className="infra-view">
      <div className="toolbar">
        <span className="strong small">Port forwards</span>
        <span className="xs subtle">
          {listing ? `${listing.running.length} running · ${listing.saved.length} saved in connections/` : ''}
        </span>
        <span className="grow" />
        <button className="btn btn-sm btn-primary" onClick={newForward}>
          <Plus /> New forward
        </button>
      </div>
      {error && (
        <div className="p-4">
          <Callout kind="danger">{error}</Callout>
        </div>
      )}
      {activeBindings.length > 0 && (
        <div className="px-5 mt-4">
          <Callout kind="success">
            {activeBindings.map((b) => (
              <div key={b.id} className="small">
                <code className="code-inline">{`{{${b.binding!.variable}}}`}</code> → <code className="code-inline">{b.binding!.value}</code> in {envName(b.binding!.environment)} while "{b.name}" runs
              </div>
            ))}
          </Callout>
        </div>
      )}
      {!listing ? (
        <div className="empty">
          <Spinner />
        </div>
      ) : rows.length === 0 ? (
        <Empty
          icon={<Cable />}
          title="No forwards yet"
          action={
            <button className="btn btn-sm btn-primary" onClick={newForward}>
              <Plus /> New forward
            </button>
          }
        >
          Forward a Kubernetes service or pod — or bind a Docker container's port — and its URL fills an environment variable like <code className="code-inline">{'{{baseUrl}}'}</code>, so your saved requests just work.
        </Empty>
      ) : (
        <div className="scroll grow">
          <table className="table infra-table">
            <thead>
              <tr>
                <th style={{ width: 18 }} />
                <th>Name</th>
                <th>Target</th>
                <th>Local</th>
                <th>Bound variable</th>
                <th>Health</th>
                <th style={{ width: 90 }} />
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => {
                const st = r.running;
                const s = r.spec;
                return (
                  <tr key={r.id}>
                    <td>
                      <span className={clsx('dot', stateDot(st))} title={st?.state ?? 'stopped'} />
                    </td>
                    <td className="nowrap">
                      <div className="row-sm">
                        {s?.source === 'docker' ? <Container size={13} className="subtle" /> : <DatabaseZap size={13} className="subtle" />}
                        <span className="strong small">{s?.name ?? r.id}</span>
                        {!r.saved && <span className="badge" title="Not saved to connections/">ad-hoc</span>}
                      </div>
                      {r.saved?.error && (
                        <div className="xs text-danger" title={r.saved.error}>
                          <AlertTriangle size={11} /> invalid file
                        </div>
                      )}
                      {s?.context && <div className="xs subtle truncate cell-max">{s.context}</div>}
                    </td>
                    <td className="mono xs">{targetText(s)}</td>
                    <td className="nowrap">
                      {st?.url ? (
                        <button className="port-chip link" onClick={() => openHttpRequest(st.url!, `${st.name} (forward)`)} title={`New HTTP request to ${st.url}`}>
                          {st.url.replace(/^https?:\/\//, '')}
                          <ExternalLink />
                        </button>
                      ) : (
                        <span className="xs subtle">{s?.localPort ? `:${s.localPort}` : 'auto'}</span>
                      )}
                    </td>
                    <td className="small">
                      {s?.bindEnvironment ? (
                        <>
                          <code className="code-inline">{`{{${s.bindEnvironment.variable}}}`}</code>
                          <span className="xs subtle"> in {envName(s.bindEnvironment.environment)}</span>
                          {st?.binding && <div className="mono xs truncate cell-max" title={st.binding.value}>= {st.binding.value}</div>}
                        </>
                      ) : (
                        <span className="xs subtle">—</span>
                      )}
                    </td>
                    <td className="xs">
                      {st ? (
                        <div title={st.lastError ?? undefined}>
                          <span className={clsx(st.state === 'reconnecting' && 'text-warning')}>{st.state}</span>
                          {st.reconnects > 0 && (
                            <span className="reconnects" title={`Re-attached ${st.reconnects}× after the backend changed (pod restart / new port)`}>
                              <RefreshCcw size={11} /> {st.reconnects}
                            </span>
                          )}
                          <div className="subtle truncate cell-max">
                            {st.backend ? `→ ${st.backend}` : ''}
                            {st.connections > 0 ? ` · ${st.activeConnections}/${st.connections} conn` : ''}
                          </div>
                          {st.lastError && st.state !== 'active' && <div className="text-danger truncate cell-max">{st.lastError}</div>}
                        </div>
                      ) : (
                        <span className="subtle">stopped</span>
                      )}
                    </td>
                    <td>
                      <div className="row-actions">
                        {busy === r.id ? (
                          <Spinner />
                        ) : st ? (
                          <button className="icon-btn sm" title="Stop" aria-label="Stop forward" onClick={() => run(r.id, () => stopForward(root, r.id))}>
                            <Square />
                          </button>
                        ) : (
                          <button
                            className="icon-btn sm"
                            title="Start"
                            aria-label="Start forward"
                            disabled={!r.saved?.file}
                            onClick={() => run(r.id, () => startForward(root, r.id))}
                          >
                            <Play />
                          </button>
                        )}
                        <Menu
                          align="right"
                          items={() => menu(r)}
                          trigger={(t) => (
                            <button className="icon-btn sm" title="More" aria-label="More actions" {...t}>
                              <MoreHorizontal />
                            </button>
                          )}
                        />
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

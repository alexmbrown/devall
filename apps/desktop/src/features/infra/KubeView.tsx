import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { AlertTriangle, Cable, Eye, EyeOff, FileCode2, Info, MoreHorizontal, RefreshCw, RotateCw, Scaling, ScrollText, Settings2, Shapes, SquareTerminal, Trash2 } from 'lucide-react';
import { Callout, CopyButton, Empty, Field, SearchInput, Spinner, Switch, copyText } from '@/components/ui/misc';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Modal } from '@/components/ui/Modal';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { errorMessage } from '@/lib/ipc';
import { uid } from '@/lib/id';
import {
  infraApi,
  type ClusterInfo,
  type ContextInfo,
  type Describe,
  type KindInfo,
  type KindRef,
  type KubeContexts,
  type KubeOptions,
  type ResourceRow,
  type SecretEntry,
  type TargetKind,
  type WatchMsg,
} from '@/lib/api/infra';
import { LogPanel } from './LogPanel';
import { AiAssistButton } from '@/features/ai';
import type { KubectlSuggestion } from '@/lib/api/ai';
import { ExecTerminal } from './Terminal';
import { age, forwardTemplate, useInfraUi } from './store';
import { SidePane } from '@/components/layout/SidePane';

const POD: KindRef = { group: '', version: 'v1', kind: 'Pod', plural: 'pods', namespaced: true };
const ALL = '__all__';

const sameKind = (a: KindRef, b: KindRef) => a.group === b.group && a.kind === b.kind;

/** Forward target kind for a resource kind, when it can be forwarded. */
function forwardKind(k: KindRef): TargetKind | null {
  if (k.group === '' && k.kind === 'Pod') return 'pod';
  if (k.group === '' && k.kind === 'Service') return 'service';
  if (k.group === 'apps') return ({ Deployment: 'deployment', StatefulSet: 'statefulset', DaemonSet: 'daemonset', ReplicaSet: 'replicaset' } as Record<string, TargetKind>)[k.kind] ?? null;
  return null;
}

const canRestart = (k: KindRef) => (k.group === 'apps' && ['Deployment', 'StatefulSet', 'DaemonSet'].includes(k.kind)) || (k.group === '' && k.kind === 'Pod');
const canScale = (k: KindRef) => k.group === 'apps' && ['Deployment', 'StatefulSet', 'ReplicaSet'].includes(k.kind);
const isPod = (k: KindRef) => k.group === '' && k.kind === 'Pod';
const isSecret = (k: KindRef) => k.group === '' && k.kind === 'Secret';

function authProblem(c?: ContextInfo): string | null {
  if (!c) return null;
  if (c.auth.type === 'exec' && !c.auth.found) return `Auth plugin \`${c.auth.command}\` not found. ${c.auth.installHint ?? ''}`;
  if (c.auth.type === 'authProvider') return `Legacy auth provider "${c.auth.name}" isn't supported; switch the kubeconfig to its exec plugin (e.g. gke-gcloud-auth-plugin).`;
  return null;
}

/** Copies an AI-suggested kubectl command; for delete/scale/restart offers to run the equivalent
 * in-app action — only after an explicit confirmation. */
async function applyKubectl(r: KubectlSuggestion, context: string, done: () => void) {
  await copyText(r.command, 'Command copied');
  const a = r.check?.action;
  if (!a || !r.validated) return;
  const what = a.type === 'scale' ? `Scale ${a.kind.kind} ${a.name} to ${a.replicas}` : a.type === 'restart' ? `Restart ${a.kind.kind} ${a.name}` : `Delete ${a.kind.kind} ${a.name}`;
  const ok = await confirmDialog({ title: `${what}?`, message: `In namespace ${a.namespace} of ${context}. The dry run passed; this changes the cluster.`, confirmText: 'Run it', danger: true });
  if (!ok) return;
  try {
    if (a.type === 'scale') await infraApi.kube.scale(context, a.kind, a.namespace, a.name, a.replicas ?? 0);
    else if (a.type === 'restart') await infraApi.kube.restart(context, a.kind, a.namespace, a.name);
    else await infraApi.kube.delete(context, a.kind, a.namespace, a.name);
    toast.success('Done', what);
    done();
  } catch (e) {
    toast.error('Failed', errorMessage(e));
  }
}

export function KubeView() {
  const context = useInfraUi((s) => s.kubeContext);
  const namespaceSel = useInfraUi((s) => s.kubeNamespace);
  const setUi = useInfraUi((s) => s.set);
  const [contexts, setContexts] = useState<KubeContexts | null>(null);
  const [ctxError, setCtxError] = useState<string | null>(null);
  const [cluster, setCluster] = useState<ClusterInfo | null>(null);
  const [clusterError, setClusterError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [namespaces, setNamespaces] = useState<string[]>([]);
  const [kinds, setKinds] = useState<KindInfo[]>([]);
  const [customLoaded, setCustomLoaded] = useState(false);
  const [kind, setKind] = useState<KindRef>(POD);
  const [settings, setSettings] = useState(false);
  const [reload, setReload] = useState(0);

  const loadContexts = useCallback(async () => {
    try {
      const c = await infraApi.kube.contexts();
      setContexts(c);
      setCtxError(null);
    } catch (e) {
      setContexts(null);
      setCtxError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void loadContexts();
  }, [loadContexts, reload]);

  const ctxName = context && contexts?.contexts.some((c) => c.name === context) ? context : (contexts?.current ?? contexts?.contexts[0]?.name ?? '');
  const ctxInfo = contexts?.contexts.find((c) => c.name === ctxName);

  // Connect (runs the auth plugin once) whenever the context changes.
  useEffect(() => {
    if (!ctxName) return;
    let live = true;
    setConnecting(true);
    setCluster(null);
    setClusterError(null);
    setNamespaces([]);
    setCustomLoaded(false);
    infraApi.kube
      .test(ctxName)
      .then(async (info) => {
        if (!live) return;
        setCluster(info);
        const [ns, k] = await Promise.all([infraApi.kube.namespaces(ctxName).catch(() => [info.defaultNamespace]), infraApi.kube.kinds(ctxName, false)]);
        if (!live) return;
        setNamespaces(ns);
        setKinds(k);
      })
      .catch((e) => live && setClusterError(errorMessage(e)))
      .finally(() => live && setConnecting(false));
    return () => {
      live = false;
    };
  }, [ctxName, reload]);

  const namespace = namespaceSel === ALL ? undefined : namespaceSel && (namespaces.length === 0 || namespaces.includes(namespaceSel)) ? namespaceSel : cluster?.defaultNamespace;

  const loadCustom = async () => {
    try {
      setKinds(await infraApi.kube.kinds(ctxName, true));
      setCustomLoaded(true);
    } catch (e) {
      toast.error('Could not list custom resources', errorMessage(e));
    }
  };

  const grouped = useMemo(() => {
    const m = new Map<string, KindInfo[]>();
    for (const k of kinds) m.set(k.category, [...(m.get(k.category) ?? []), k]);
    return [...m.entries()];
  }, [kinds]);

  if (ctxError)
    return (
      <div className="p-6">
        <Callout kind="warning">
          <div className="strong">No Kubernetes configuration</div>
          <div className="small mt-2 break">{ctxError}</div>
          <div className="row mt-4">
            <button className="btn btn-sm" onClick={() => setReload((n) => n + 1)}>
              <RefreshCw /> Retry
            </button>
            <button className="btn btn-sm" onClick={() => setSettings(true)}>
              <Settings2 /> Kubeconfig files…
            </button>
          </div>
        </Callout>
        <KubeSettings open={settings} onClose={() => setSettings(false)} onSaved={() => setReload((n) => n + 1)} />
      </div>
    );
  if (!contexts)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );

  const problem = authProblem(ctxInfo);

  return (
    <div className="infra-view">
      <div className="toolbar">
        <span className={clsx('dot', cluster ? 'ok' : connecting ? 'warn pulse' : 'err')} aria-hidden />
        <select className="select sm ctx-select" value={ctxName} onChange={(e) => setUi({ kubeContext: e.target.value, kubeNamespace: undefined })} aria-label="Context">
          {contexts.contexts.map((c) => (
            <option key={c.name} value={c.name}>
              {c.name}
              {c.current ? ' (current)' : ''}
              {authProblem(c) ? ' ⚠' : ''}
            </option>
          ))}
        </select>
        <select className="select sm" value={namespaceSel === ALL ? ALL : (namespace ?? '')} onChange={(e) => setUi({ kubeNamespace: e.target.value })} aria-label="Namespace" disabled={!cluster}>
          <option value={ALL}>All namespaces</option>
          {(namespaces.length ? namespaces : namespace ? [namespace] : []).map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
        {cluster && (
          <span className="xs subtle truncate" title={ctxInfo?.server}>
            {cluster.version} · {ctxInfo?.server}
          </span>
        )}
        <span className="grow" />
        {cluster && (
          <AiAssistButton
            feature="nlKubectl"
            compact
            align="right"
            title="kubectl from a description — checked against the cluster (lookups and server-side dry runs); nothing is applied"
            applyLabel="Copy command"
            input={(q) => ({ question: q, context: ctxName, namespace: namespace ?? '', kinds: kinds.map((k) => k.plural).join(', ') })}
            onResult={(r) => void applyKubectl(r, ctxName, () => setReload((n) => n + 1))}
          />
        )}
        <button className="icon-btn sm" title="Reconnect / reload kubeconfig" aria-label="Reconnect" onClick={() => setReload((n) => n + 1)}>
          <RefreshCw />
        </button>
        <button className="icon-btn sm" title="Kubeconfig files & auth plugins" aria-label="Kubernetes settings" onClick={() => setSettings(true)}>
          <Settings2 />
        </button>
      </div>
      <KubeSettings open={settings} onClose={() => setSettings(false)} onSaved={() => setReload((n) => n + 1)} />
      {(clusterError || problem) && !cluster && !connecting ? (
        <div className="p-6">
          <Callout kind="danger">
            <div className="strong">Can't connect to "{ctxName}"</div>
            <div className="small mt-2 break pre-line">{clusterError ?? problem}</div>
            {ctxInfo?.auth.type === 'exec' && (
              <div className="xs subtle mt-2">
                Auth plugin: <code className="code-inline">{ctxInfo.auth.command}</code> {ctxInfo.auth.found ? `→ ${ctxInfo.auth.resolved}` : '(not found on PATH or in the usual install folders)'}
              </div>
            )}
            <button className="btn btn-sm mt-4" onClick={() => setReload((n) => n + 1)}>
              <RefreshCw /> Retry
            </button>
          </Callout>
        </div>
      ) : !cluster ? (
        <div className="empty">
          <Spinner />
          <p className="small subtle">Connecting to {ctxName}{ctxInfo?.auth.type === 'exec' ? ` (running ${ctxInfo.auth.command})` : ''}…</p>
        </div>
      ) : (
        <div className="split">
          <SidePane className="kinds-pane">
            <div className="list" role="listbox" aria-label="Resource kinds">
              {grouped.map(([cat, ks]) => (
                <div key={cat}>
                  <div className="list-group-header">{cat}</div>
                  {ks.map((k) => (
                    <div
                      key={`${k.group}/${k.kind}`}
                      role="option"
                      aria-selected={sameKind(k, kind)}
                      tabIndex={0}
                      className={clsx('list-item', sameKind(k, kind) && 'on')}
                      onClick={() => setKind(k)}
                      onKeyDown={(e) => e.key === 'Enter' && setKind(k)}
                      title={`${k.group ? `${k.group}/` : ''}${k.version} ${k.kind}`}
                    >
                      <span className="grow truncate">{k.kind}</span>
                    </div>
                  ))}
                </div>
              ))}
              {!customLoaded && (
                <div className="p-4">
                  <button className="btn btn-sm w-full" onClick={loadCustom}>
                    <Shapes /> Custom resources
                  </button>
                </div>
              )}
            </div>
          </SidePane>
          <div className="pane fill">
            <ResourceTable key={`${ctxName}|${namespace ?? ALL}|${kind.group}/${kind.kind}`} context={ctxName} namespace={kind.namespaced ? namespace : undefined} kind={kind} />
          </div>
        </div>
      )}
    </div>
  );
}

type DetailTab = 'describe' | 'yaml' | 'logs' | 'terminal';

function ResourceTable({ context, namespace, kind }: { context: string; namespace?: string; kind: KindRef }) {
  const [columns, setColumns] = useState<string[]>([]);
  const [rows, setRows] = useState<ResourceRow[] | null>(null);
  const [watchError, setWatchError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<DetailTab>('describe');
  const pending = useRef<WatchMsg[]>([]);

  useEffect(() => {
    const id = `watch-${uid()}`;
    let live = true;
    let scheduled = false;
    const apply = () => {
      scheduled = false;
      if (!live) return;
      const msgs = pending.current;
      pending.current = [];
      const lastReset = [...msgs].reverse().find((m) => m.type === 'reset');
      if (lastReset?.type === 'reset') setColumns(lastReset.columns);
      setRows((prev) => {
        let next = prev ?? [];
        for (const m of msgs) {
          if (m.type === 'reset') next = m.rows;
          else if (m.type === 'upsert') {
            const i = next.findIndex((r) => r.uid === m.row.uid);
            next = i >= 0 ? next.map((r, j) => (j === i ? m.row : r)) : [...next, m.row];
          } else if (m.type === 'delete') next = next.filter((r) => r.uid !== m.uid);
        }
        return next;
      });
    };
    infraApi.kube
      .watch(id, context, kind, namespace, (m) => {
        if (!live) return;
        if (m.type === 'error') {
          setWatchError(m.message);
          return;
        }
        setWatchError(null);
        pending.current.push(m);
        if (!scheduled) {
          scheduled = true;
          setTimeout(apply, 60);
        }
      })
      .catch((e) => live && setWatchError(errorMessage(e)));
    return () => {
      live = false;
      void infraApi.streamStop(id);
    };
  }, [context, kind, namespace]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = (rows ?? []).filter((r) => !q || `${r.name} ${r.namespace ?? ''} ${r.cells.join(' ')}`.toLowerCase().includes(q));
    // Events: newest first (live upserts arrive at the end).
    if (kind.group === '' && kind.kind === 'Event') return [...list].sort((a, b) => (b.cells[b.cells.length - 1] ?? '').localeCompare(a.cells[a.cells.length - 1] ?? ''));
    return list;
  }, [rows, query, kind]);
  const sel = rows?.find((r) => r.uid === selected);
  const showNs = kind.namespaced && !namespace;

  const action = async (label: string, fn: () => Promise<unknown>) => {
    try {
      await fn();
      toast.success(label);
    } catch (e) {
      toast.error(`${label} failed`, errorMessage(e));
    }
  };

  const del = async (r: ResourceRow) => {
    const clusterWide = !kind.namespaced;
    const ok = await confirmDialog({
      title: `Delete ${kind.kind} ${r.name}?`,
      message: `${r.namespace ? `Namespace ${r.namespace} · ` : ''}context ${context}. This can't be undone from here.`,
      confirmText: 'Delete',
      danger: true,
      typeToConfirm: clusterWide || kind.kind === 'Namespace' ? r.name : undefined,
    });
    if (ok) await action(`Deleted ${kind.kind} ${r.name}`, () => infraApi.kube.delete(context, kind, r.namespace, r.name));
  };

  const restart = async (r: ResourceRow) => {
    const pod = isPod(kind);
    const ok = await confirmDialog({
      title: pod ? `Delete pod ${r.name} so it is recreated?` : `Rollout restart ${kind.kind} ${r.name}?`,
      message: pod ? 'Its controller (Deployment, StatefulSet, …) starts a replacement. A bare pod is gone for good.' : 'Pods are replaced gradually, following the rollout strategy.',
      confirmText: pod ? 'Delete pod' : 'Restart',
      danger: pod,
    });
    if (ok) await action(pod ? `Deleted pod ${r.name}` : `Restarting ${r.name}`, () => infraApi.kube.restart(context, kind, r.namespace, r.name));
  };

  const scale = async (r: ResourceRow) => {
    const current = r.cells[0]?.split('/')[1] ?? r.cells[0] ?? '1';
    const v = await promptDialog({ title: `Scale ${kind.kind} ${r.name}`, label: 'Replicas', initial: current, validate: (x) => (/^\d+$/.test(x.trim()) ? undefined : 'A number ≥ 0') });
    if (v === null) return;
    await action(`Scaled ${r.name} to ${v.trim()}`, () => infraApi.kube.scale(context, kind, r.namespace, r.name, Number(v.trim())));
  };

  const forward = (r: ResourceRow) => {
    const fk = forwardKind(kind);
    if (!fk) return;
    useInfraUi.getState().openForward({
      spec: forwardTemplate({ name: r.name, context, namespace: r.namespace, target: { kind: fk, name: r.name }, remotePort: r.ports[0]?.port ?? 8080 }),
    });
  };

  const menu = (r: ResourceRow): MenuItemDef[] => {
    const items: MenuItemDef[] = [
      { label: 'Describe', icon: <Info />, onSelect: () => (setSelected(r.uid), setDetail('describe')) },
      { label: 'YAML', icon: <FileCode2 />, onSelect: () => (setSelected(r.uid), setDetail('yaml')) },
    ];
    if (isPod(kind)) {
      items.push({ label: 'Logs', icon: <ScrollText />, onSelect: () => (setSelected(r.uid), setDetail('logs')) });
      items.push({ label: 'Terminal', icon: <SquareTerminal />, onSelect: () => (setSelected(r.uid), setDetail('terminal')) });
    }
    if (forwardKind(kind)) items.push({ label: 'Port-forward…', icon: <Cable />, onSelect: () => forward(r) });
    items.push({ separator: true });
    if (canScale(kind)) items.push({ label: 'Scale…', icon: <Scaling />, onSelect: () => scale(r) });
    if (canRestart(kind)) items.push({ label: isPod(kind) ? 'Restart (delete pod)…' : 'Rollout restart…', icon: <RotateCw />, onSelect: () => restart(r) });
    items.push({ label: 'Delete…', icon: <Trash2 />, danger: true, onSelect: () => del(r) });
    return items;
  };

  return (
    <div className="infra-vsplit">
      <div className="infra-table-wrap">
        <div className="toolbar">
          <span className="strong small">{kind.kind}</span>
          <SearchInput value={query} onChange={setQuery} placeholder={`Filter ${kind.plural}…`} />
          <span className="xs subtle">{rows ? `${shown.length}${query ? ` / ${rows.length}` : ''}` : ''}</span>
          {watchError && (
            <span className="xs text-warning truncate" title={watchError}>
              <AlertTriangle size={12} /> {watchError}
            </span>
          )}
          <span className="grow" />
          {rows && <span className="xs subtle nowrap"><span className="dot ok pulse" /> live</span>}
        </div>
        {!rows ? (
          watchError ? (
            <div className="p-6">
              <Callout kind="danger">{watchError}</Callout>
            </div>
          ) : (
            <div className="empty">
              <Spinner />
            </div>
          )
        ) : rows.length === 0 ? (
          <Empty icon={<Shapes />} title={`No ${kind.plural}`}>
            {namespace ? `in namespace ${namespace}` : ''}
          </Empty>
        ) : (
          <div className="scroll grow">
            <table className="table infra-table">
              <thead>
                <tr>
                  <th style={{ width: 18 }} />
                  <th>Name</th>
                  {showNs && <th>Namespace</th>}
                  {columns.map((c) => (
                    <th key={c}>{c}</th>
                  ))}
                  <th>Age</th>
                  <th style={{ width: 70 }} />
                </tr>
              </thead>
              <tbody>
                {shown.map((r) => (
                  <tr key={r.uid} className={clsx('clickable', selected === r.uid && 'on')} onClick={() => setSelected(r.uid)}>
                    <td>
                      <span className={clsx('dot', r.health === 'ok' ? 'ok' : r.health === 'warn' ? 'warn' : r.health === 'err' ? 'err' : '')} />
                    </td>
                    <td className="strong small nowrap">{r.name}</td>
                    {showNs && <td className="small subtle">{r.namespace}</td>}
                    {r.cells.map((c, i) => (
                      <td key={i} className={clsx('small', columns[i] === 'Message' ? 'cell-msg' : 'nowrap')} title={c}>
                        {c}
                      </td>
                    ))}
                    <td className="xs subtle nowrap">{age(r.created)}</td>
                    <td onClick={(e) => e.stopPropagation()}>
                      <div className="row-actions">
                        {forwardKind(kind) && (
                          <button className="icon-btn sm" title="Port-forward…" aria-label="Port-forward" onClick={() => forward(r)}>
                            <Cable />
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
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
      {sel && (
        <ResourceDetail key={sel.uid} context={context} kind={kind} row={sel} tab={detail} onTab={setDetail} onClose={() => setSelected(null)} />
      )}
    </div>
  );
}

function ResourceDetail({ context, kind, row, tab, onTab, onClose }: { context: string; kind: KindRef; row: ResourceRow; tab: DetailTab; onTab(t: DetailTab): void; onClose(): void }) {
  const tabs: DetailTab[] = isPod(kind) ? ['describe', 'yaml', 'logs', 'terminal'] : ['describe', 'yaml'];
  const [container, setContainer] = useState(row.containers[0] ?? '');
  const current = tabs.includes(tab) ? tab : 'describe';
  const containerPicker =
    row.containers.length > 1 ? (
      <select className="select sm" value={container} onChange={(e) => setContainer(e.target.value)} aria-label="Container">
        {row.containers.map((c) => (
          <option key={c} value={c}>
            {c}
          </option>
        ))}
      </select>
    ) : null;
  return (
    <div className="infra-detail">
      <div className="subtabs">
        {tabs.map((t) => (
          <button key={t} className={clsx('subtab', current === t && 'on')} onClick={() => onTab(t)}>
            {t === 'describe' ? 'Describe' : t === 'yaml' ? 'YAML' : t === 'logs' ? 'Logs' : 'Terminal'}
          </button>
        ))}
        <span className="grow" />
        <span className="small strong truncate detail-title">
          {kind.kind}/{row.name}
        </span>
        <CopyButton text={row.name} title="Copy name" />
        <button className="icon-btn sm" onClick={onClose} title="Close" aria-label="Close details">
          ×
        </button>
      </div>
      <div className="infra-detail-body">
        {current === 'describe' && <DescribePanel context={context} kind={kind} row={row} />}
        {current === 'yaml' && <YamlPanel context={context} kind={kind} row={row} />}
        {current === 'logs' && row.namespace && (
          <LogPanel
            title={`${row.namespace}/${row.name}${container ? ` · ${container}` : ''}`}
            sourceKey={`${row.uid}|${container}`}
            allowPrevious
            extra={containerPicker}
            start={(streamId, o, onMsg) =>
              infraApi.kube.logs(streamId, context, row.namespace!, row.name, { container: container || undefined, follow: o.follow, tail: o.tail ?? undefined, timestamps: o.timestamps, previous: o.previous }, onMsg)
            }
          />
        )}
        {current === 'terminal' && row.namespace && (
          <div className="term-wrap">
            {containerPicker && <div className="toolbar">{containerPicker}</div>}
            <ExecTerminal
              key={`${row.uid}|${container}`}
              title={`${row.name}${container ? ` · ${container}` : ''}`}
              start={(execId, onEvent) => infraApi.kube.exec(execId, context, row.namespace!, row.name, container || undefined, onEvent)}
            />
          </div>
        )}
      </div>
    </div>
  );
}

function DescribePanel({ context, kind, row }: { context: string; kind: KindRef; row: ResourceRow }) {
  const [d, setD] = useState<Describe | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    let live = true;
    infraApi.kube.describe(context, kind, row.namespace, row.name).then(
      (x) => live && setD(x),
      (e) => live && setError(errorMessage(e)),
    );
    return () => {
      live = false;
    };
  }, [context, kind, row.namespace, row.name, n]);
  if (error) return <div className="p-5"><Callout kind="danger">{error}</Callout></div>;
  if (!d)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  return (
    <div className="scroll describe p-5">
      <div className="row-sm">
        <span className="grow" />
        <button className="icon-btn sm" onClick={() => setN((x) => x + 1)} title="Refresh" aria-label="Refresh">
          <RefreshCw />
        </button>
      </div>
      <dl className="kv">
        <dt>Name</dt>
        <dd className="mono">{d.name}</dd>
        {d.namespace && (
          <>
            <dt>Namespace</dt>
            <dd>{d.namespace}</dd>
          </>
        )}
        <dt>Created</dt>
        <dd>{d.created ? `${d.created} (${age(d.created)} ago)` : '—'}</dd>
        {d.owners.length > 0 && (
          <>
            <dt>Controlled by</dt>
            <dd>{d.owners.join(', ')}</dd>
          </>
        )}
        {d.fields.map(([k, v]) => (
          <FragmentKV key={k} k={k} v={v} />
        ))}
      </dl>
      {d.containers.length > 0 && (
        <section className="mt-5">
          <div className="section-title">Containers</div>
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Image</th>
                <th>State</th>
                <th>Ready</th>
                <th>Restarts</th>
              </tr>
            </thead>
            <tbody>
              {d.containers.map((c) => (
                <tr key={`${c.init}-${c.name}`}>
                  <td className="small strong">
                    {c.name}
                    {c.init && <span className="badge">init</span>}
                  </td>
                  <td className="mono xs">{c.image}</td>
                  <td className="small" title={c.message}>
                    {c.state}
                    {c.reason ? ` (${c.reason})` : ''}
                  </td>
                  <td className="small">{c.ready ? 'yes' : 'no'}</td>
                  <td className="small">{c.restarts}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
      {d.conditions.length > 0 && (
        <section className="mt-5">
          <div className="section-title">Conditions</div>
          <table className="table">
            <tbody>
              {d.conditions.map((c) => (
                <tr key={c.type}>
                  <td className="small strong nowrap">{c.type}</td>
                  <td className="small">
                    <span className={clsx('dot', c.status === 'True' ? 'ok' : c.status === 'False' ? 'warn' : '')} /> {c.status}
                  </td>
                  <td className="small">{c.reason}</td>
                  <td className="small subtle">{c.message}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
      <section className="mt-5">
        <div className="section-title">Events</div>
        {d.events.length === 0 ? (
          <div className="small subtle">No recent events.</div>
        ) : (
          <table className="table">
            <tbody>
              {d.events.map((e, i) => (
                <tr key={i}>
                  <td className={clsx('small nowrap', e.type === 'Warning' && 'text-warning')}>{e.type}</td>
                  <td className="small strong nowrap">{e.reason}</td>
                  <td className="small">{e.message}</td>
                  <td className="xs subtle nowrap">
                    {e.count > 1 ? `×${e.count} · ` : ''}
                    {age(e.lastSeen)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
      {Object.keys(d.labels).length > 0 && (
        <section className="mt-5">
          <div className="section-title">Labels</div>
          <div className="label-chips">
            {Object.entries(d.labels).map(([k, v]) => (
              <span key={k} className="tag mono">
                {k}={v}
              </span>
            ))}
          </div>
        </section>
      )}
      {Object.keys(d.annotations).length > 0 && (
        <section className="mt-5">
          <div className="section-title">Annotations</div>
          <dl className="kv mono">
            {Object.entries(d.annotations).map(([k, v]) => (
              <FragmentKV key={k} k={k} v={v} />
            ))}
          </dl>
        </section>
      )}
    </div>
  );
}

function FragmentKV({ k, v }: { k: string; v: string }) {
  return (
    <>
      <dt>{k}</dt>
      <dd>{v}</dd>
    </>
  );
}

function YamlPanel({ context, kind, row }: { context: string; kind: KindRef; row: ResourceRow }) {
  const [reveal, setReveal] = useState(false);
  const [yaml, setYaml] = useState<string | null>(null);
  const [values, setValues] = useState<SecretEntry[] | null>(null);
  useEffect(() => {
    let live = true;
    setYaml(null);
    infraApi.kube.yaml(context, kind, row.namespace, row.name, reveal).then(
      (y) => live && setYaml(y),
      (e) => live && setYaml(`# ${errorMessage(e)}`),
    );
    return () => {
      live = false;
    };
  }, [context, kind, row.namespace, row.name, reveal]);
  const showValues = async () => {
    if (values) return setValues(null);
    try {
      setValues(await infraApi.kube.secretValues(context, row.namespace!, row.name));
    } catch (e) {
      toast.error('Could not read the secret', errorMessage(e));
    }
  };
  return (
    <div className="yaml-panel">
      <div className="toolbar">
        {isSecret(kind) ? (
          <>
            <span className="xs subtle">Secret values are hidden.</span>
            <button className="btn btn-sm" onClick={showValues}>
              {values ? <EyeOff /> : <Eye />} {values ? 'Hide decoded values' : 'Reveal decoded values'}
            </button>
            <Switch checked={reveal} onChange={setReveal} label="Raw base64 in YAML" />
          </>
        ) : (
          <span className="xs subtle">Read-only view · managedFields omitted</span>
        )}
        <span className="grow" />
        {yaml && <CopyButton text={yaml} title="Copy YAML" />}
      </div>
      {values && (
        <div className="secret-values">
          <Callout kind="warning">Decoded secret values are visible below. They are never written to project files.</Callout>
          <table className="table mt-2">
            <tbody>
              {values.map((v) => (
                <tr key={v.key}>
                  <td className="mono xs strong nowrap">{v.key}</td>
                  <td className="mono xs break">{v.value ?? <span className="subtle">&lt;{v.size} bytes of binary data&gt;</span>}</td>
                  <td>{v.value && <CopyButton text={v.value} title="Copy value" />}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="grow min-h-0">
        {yaml === null ? (
          <div className="empty">
            <Spinner />
          </div>
        ) : (
          <CodeEditor value={yaml} language="yaml" readOnly path={`inmemory://infra/${row.uid}${reveal ? '-raw' : ''}.yaml`} />
        )}
      </div>
    </div>
  );
}

function KubeSettings({ open, onClose, onSaved }: { open: boolean; onClose(): void; onSaved(): void }) {
  const [opts, setOpts] = useState<KubeOptions | null>(null);
  const [files, setFiles] = useState('');
  useEffect(() => {
    if (!open) return;
    infraApi.kube.options().then((o) => {
      setOpts(o);
      setFiles(o.kubeconfig.join('\n'));
    }, () => setOpts({ kubeconfig: [], wslFallback: false }));
  }, [open]);
  const save = async () => {
    if (!opts) return;
    try {
      await infraApi.kube.setOptions({ ...opts, kubeconfig: files.split('\n').map((s) => s.trim()).filter(Boolean) });
      onSaved();
      onClose();
    } catch (e) {
      toast.error('Could not save', errorMessage(e));
    }
  };
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Kubernetes settings"
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={save}>
            Apply
          </button>
        </>
      }
    >
      {opts && (
        <div className="col">
          <Field label="Kubeconfig files" hint="One path per line, merged in order. Empty = KUBECONFIG, else ~/.kube/config. Only this session.">
            <textarea className="textarea mono" rows={4} value={files} onChange={(e) => setFiles(e.target.value)} placeholder="~/.kube/config" />
          </Field>
          <Switch checked={opts.wslFallback} onChange={(v) => setOpts({ ...opts, wslFallback: v })} label="Run auth plugins through WSL when they aren't installed on Windows" />
          <div className="xs subtle">
            Exec auth plugins (gke-gcloud-auth-plugin, aws, kubelogin, doctl) are looked up on PATH, your login shell's PATH and the usual install folders (Homebrew, Google Cloud SDK, AWS CLI, WinGet, Scoop…).
          </div>
        </div>
      )}
    </Modal>
  );
}

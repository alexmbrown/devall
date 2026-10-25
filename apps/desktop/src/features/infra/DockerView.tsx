import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import {
  Box,
  ChevronDown,
  ChevronRight,
  Container,
  ExternalLink,
  Link2,
  MoreHorizontal,
  Play,
  Plug,
  RefreshCw,
  RotateCw,
  ScrollText,
  Square,
  SquareTerminal,
  Trash2,
} from 'lucide-react';
import { Callout, Empty, SearchInput, Segmented, Spinner } from '@/components/ui/misc';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { errorMessage } from '@/lib/ipc';
import { uid } from '@/lib/id';
import {
  infraApi,
  type ComposeAction,
  type ComposeProject,
  type ContainerRow,
  type EngineInfo,
  type ImageRow,
  type NetworkRow,
  type SocketCandidate,
  type StatsSample,
  type VolumeRow,
} from '@/lib/api/infra';
import { LogPanel } from './LogPanel';
import { ExecTerminal } from './Terminal';
import { Sparkline } from './Sparkline';
import { age, fmtBytes, forwardTemplate, openHttpRequest, useInfraUi } from './store';
import { reposApi, type ComposeFile } from '@/lib/api/git';
import { useProjectRepos } from '@/store/repos';
import { useWorkspace } from '@/store/workspace';

/**
 * Compose files found in the open project's repositories that aren't running — a multi-repo
 * project's backends can be started from here (`docker compose -f <file> up -d`, confirmed).
 */
function RepoComposeFiles({ running, onStarted }: { running: ComposeProject[] | undefined; onStarted(): void }) {
  const root = useWorkspace((s) => s.project?.root);
  const repos = useProjectRepos();
  const [files, setFiles] = useState<ComposeFile[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [open, setOpen] = useState(true);
  const repoKey = repos.map((r) => `${r.name}:${r.state}`).join('|');
  useEffect(() => {
    if (!root || !repoKey) return setFiles([]);
    reposApi.composeFiles(root).then(setFiles).catch(() => setFiles([]));
  }, [root, repoKey]);
  const norm = (p: string) => p.replace(/[\\/]+/g, '/').toLowerCase();
  const idle = files.filter((f) => !running?.some((p) => p.configFiles.some((c) => norm(c) === norm(f.path))));
  if (!idle.length) return null;
  const up = async (f: ComposeFile) => {
    const dir = f.path.replace(/[\\/][^\\/]+$/, '');
    const ok = await confirmDialog({ title: `Start ${f.repo}/${f.relPath}?`, message: `docker compose -p ${f.project} -f ${f.relPath} up -d — runs the services this file defines (from ${dir}).`, confirmText: 'Compose up' });
    if (!ok) return;
    setBusy(f.path);
    try {
      const out = await infraApi.docker.compose({ name: f.project, workingDir: dir, configFiles: [f.path], services: [], containers: 0, running: 0 }, 'up');
      toast.success(`compose up ${f.project}`, out.trim().split('\n').slice(-3).join('\n'));
      onStarted();
    } catch (e) {
      toast.error('compose up failed', errorMessage(e));
    } finally {
      setBusy(null);
    }
  };
  return (
    <div className="infra-compose-files">
      <div className="list-group-header" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown /> : <ChevronRight />} Compose files in this project's repositories ({idle.length})
      </div>
      {open &&
        idle.map((f) => (
          <div key={f.path} className="list-item" title={f.path}>
            <Box size={14} className="subtle" />
            <span className="small strong">{f.repo}</span>
            <span className="mono xs subtle truncate grow">{f.relPath}</span>
            {f.group && <span className="badge outline">{f.group}</span>}
            {busy === f.path ? (
              <Spinner />
            ) : (
              <button className="btn btn-sm btn-ghost" onClick={() => up(f)} title="docker compose up -d">
                <Play /> Up
              </button>
            )}
          </div>
        ))}
    </div>
  );
}

type Sub = 'containers' | 'images' | 'volumes' | 'networks';
type Detail = 'logs' | 'stats' | 'terminal' | 'inspect';

function stateDot(c: ContainerRow): string {
  if (c.state === 'running') return c.health === 'unhealthy' ? 'err' : c.health === 'starting' ? 'warn pulse' : 'ok';
  if (c.state === 'restarting' || c.state === 'paused' || c.state === 'created') return 'warn';
  if (c.state === 'dead') return 'err';
  return '';
}

export function DockerView() {
  const dockerHost = useInfraUi((s) => s.dockerHost);
  const setUi = useInfraUi((s) => s.set);
  const [info, setInfo] = useState<EngineInfo | null>(null);
  const [connError, setConnError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [candidates, setCandidates] = useState<SocketCandidate[]>([]);
  const [sub, setSub] = useState<Sub>('containers');

  const connect = useCallback(
    async (host?: string) => {
      setConnecting(true);
      try {
        const i = host !== undefined ? await infraApi.docker.connect(host || undefined) : await infraApi.docker.info();
        setInfo(i);
        setConnError(null);
      } catch (e) {
        setInfo(null);
        setConnError(errorMessage(e));
      } finally {
        setConnecting(false);
      }
    },
    [],
  );

  useEffect(() => {
    infraApi.docker.candidates().then(setCandidates, () => {});
    void connect(dockerHost ?? '');
    // Connect once on mount with the remembered endpoint.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const pick = (host: string) => {
    setUi({ dockerHost: host || undefined });
    void connect(host);
  };

  return (
    <div className="infra-view">
      <div className="toolbar">
        <span className={clsx('dot', info ? 'ok' : connecting ? 'warn pulse' : 'err')} aria-hidden />
        <select className="select sm engine-select" value={dockerHost ?? ''} onChange={(e) => pick(e.target.value)} aria-label="Container engine">
          <option value="">Auto-detect engine</option>
          {candidates.map((c) => (
            <option key={c.host} value={c.host} disabled={!c.exists}>
              {c.source} — {c.host}
              {c.exists ? '' : ' (not running)'}
            </option>
          ))}
        </select>
        {info && (
          <span className="small muted truncate" title={info.host}>
            {info.engine} {info.version} · {info.os}/{info.arch} · <span className="subtle">{info.source}</span>
          </span>
        )}
        <span className="grow" />
        <Segmented
          value={sub}
          onChange={setSub}
          options={[
            { value: 'containers', label: 'Containers' },
            { value: 'images', label: 'Images' },
            { value: 'volumes', label: 'Volumes' },
            { value: 'networks', label: 'Networks' },
          ]}
        />
        <button className="icon-btn sm" title="Reconnect" aria-label="Reconnect" onClick={() => connect(dockerHost ?? '')}>
          <Plug />
        </button>
      </div>
      {connError ? (
        <div className="p-6">
          <Callout kind="warning">
            <div className="strong">No container engine reachable</div>
            <div className="small mt-2 break">{connError}</div>
            <div className="small mt-2">/dev/all doesn't install a runtime — it connects to Docker Desktop, Podman, OrbStack, colima, Rancher Desktop or any DOCKER_HOST.</div>
            <button className="btn btn-sm mt-4" onClick={() => connect(dockerHost ?? '')} disabled={connecting}>
              {connecting ? <Spinner /> : <RefreshCw />} Retry
            </button>
          </Callout>
        </div>
      ) : !info ? (
        <div className="empty">
          <Spinner />
        </div>
      ) : sub === 'containers' ? (
        <ContainersTab />
      ) : sub === 'images' ? (
        <ImagesTab />
      ) : sub === 'volumes' ? (
        <VolumesTab />
      ) : (
        <NetworksTab />
      )}
    </div>
  );
}

/** Polls a list while mounted (Docker has an events API; polling keeps this simple and cheap). */
function usePolled<T>(load: () => Promise<T>, everyMs: number): { data: T | null; error: string | null; reload(): void } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => {
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => {
      try {
        const d = await loadRef.current();
        if (live) {
          setData(d);
          setError(null);
        }
      } catch (e) {
        if (live) setError(errorMessage(e));
      }
      if (live && everyMs > 0) timer = setTimeout(tick, document.hidden ? everyMs * 4 : everyMs);
    };
    void tick();
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [n, everyMs]);
  return { data, error, reload: () => setN((x) => x + 1) };
}

function ContainersTab() {
  const { data, error, reload } = usePolled(() => Promise.all([infraApi.docker.containers(true), infraApi.docker.composeProjects()]), 3000);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<Detail>('logs');
  const [query, setQuery] = useState('');
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const containers = data?.[0];
  const projects = data?.[1];

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = (containers ?? []).filter((c) => !q || `${c.name} ${c.image} ${c.composeProject ?? ''} ${c.composeService ?? ''}`.toLowerCase().includes(q));
    const map = new Map<string, ContainerRow[]>();
    for (const c of list) {
      const k = c.composeProject ?? '';
      map.set(k, [...(map.get(k) ?? []), c]);
    }
    return [...map.entries()].sort(([a], [b]) => (a === '' ? 1 : b === '' ? -1 : a.localeCompare(b)));
  }, [containers, query]);

  const sel = containers?.find((c) => c.id === selected);

  const act = async (label: string, key: string, fn: () => Promise<unknown>) => {
    setBusy(key);
    try {
      await fn();
      reload();
    } catch (e) {
      toast.error(`${label} failed`, errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const remove = async (c: ContainerRow) => {
    const running = c.state === 'running';
    const ok = await confirmDialog({
      title: `Remove container ${c.name}?`,
      message: running ? 'It is running and will be stopped first (force). Its anonymous volumes are kept.' : 'Its anonymous volumes are kept.',
      confirmText: 'Remove',
      danger: true,
    });
    if (!ok) return;
    await act('Remove', c.id, () => infraApi.docker.remove(c.id, running));
    if (selected === c.id) setSelected(null);
  };

  const compose = async (p: ComposeProject, action: ComposeAction) => {
    if (action === 'down') {
      const ok = await confirmDialog({
        title: `docker compose down "${p.name}"?`,
        message: `Stops and removes the project's ${p.containers} container(s) and networks. Named volumes are kept.`,
        confirmText: 'Compose down',
        danger: true,
      });
      if (!ok) return;
    }
    setBusy(`compose:${p.name}`);
    try {
      const out = await infraApi.docker.compose(p, action);
      toast.success(`compose ${action} ${p.name}`, out.trim().split('\n').slice(-3).join('\n'));
      reload();
    } catch (e) {
      toast.error(`compose ${action} failed`, errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const bindPort = (c: ContainerRow, privatePort: number) => {
    useInfraUi.getState().openForward({
      spec: forwardTemplate({ name: `${c.composeService ?? c.name} (docker)`, source: 'docker', target: { kind: 'container', name: c.name }, remotePort: privatePort }),
    });
  };

  const menu = (c: ContainerRow): MenuItemDef[] => [
    { label: 'Logs', icon: <ScrollText />, onSelect: () => (setSelected(c.id), setDetail('logs')) },
    { label: 'Stats', icon: <RefreshCw />, onSelect: () => (setSelected(c.id), setDetail('stats')), disabled: c.state !== 'running' },
    { label: 'Terminal', icon: <SquareTerminal />, onSelect: () => (setSelected(c.id), setDetail('terminal')), disabled: c.state !== 'running' },
    { label: 'Inspect', icon: <Box />, onSelect: () => (setSelected(c.id), setDetail('inspect')) },
    { separator: true },
    ...c.ports
      .filter((p) => p.publicPort)
      .map((p) => ({ label: `Bind :${p.publicPort} to an environment variable…`, icon: <Link2 />, onSelect: () => bindPort(c, p.privatePort) })),
    { label: 'Copy id', onSelect: () => navigator.clipboard.writeText(c.id).then(() => toast.success('Copied')) },
    { separator: true },
    { label: 'Remove…', icon: <Trash2 />, danger: true, onSelect: () => remove(c) },
  ];

  if (error && !containers) return <div className="p-6"><Callout kind="danger">{error}</Callout></div>;
  if (!containers)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );

  return (
    <div className="infra-vsplit">
      <div className="infra-table-wrap">
        <div className="toolbar">
          <SearchInput value={query} onChange={setQuery} placeholder="Filter containers…" />
          <span className="xs subtle">
            {containers.filter((c) => c.state === 'running').length} running · {containers.length} total
          </span>
          <span className="grow" />
          <button className="icon-btn sm" title="Refresh" aria-label="Refresh" onClick={reload}>
            <RefreshCw />
          </button>
        </div>
        {containers.length === 0 ? (
          <Empty icon={<Container />} title="No containers">
            Start something with <code className="code-inline">docker run</code> or <code className="code-inline">docker compose up</code>.
          </Empty>
        ) : (
          <div className="scroll grow">
            <table className="table infra-table">
              <thead>
                <tr>
                  <th style={{ width: 18 }} />
                  <th>Name</th>
                  <th>Image</th>
                  <th>Status</th>
                  <th>Ports</th>
                  <th>Age</th>
                  <th style={{ width: 120 }} />
                </tr>
              </thead>
              <tbody>
                {groups.map(([project, rows]) => {
                  const p = projects?.find((x) => x.name === project);
                  const isCollapsed = !!collapsed[project];
                  return (
                    <Fragment key={project || '__none'}>
                      {project && (
                        <tr className="group-row">
                          <td colSpan={7}>
                            <div className="row-sm">
                              <button className="icon-btn sm" onClick={() => setCollapsed({ ...collapsed, [project]: !isCollapsed })} aria-label={isCollapsed ? 'Expand' : 'Collapse'}>
                                {isCollapsed ? <ChevronRight /> : <ChevronDown />}
                              </button>
                              <span className="strong small">{project}</span>
                              <span className="xs subtle">
                                compose · {p ? `${p.running}/${p.containers} running` : `${rows.length} containers`}
                                {p?.workingDir ? ` · ${p.workingDir}` : ''}
                              </span>
                              <span className="grow" />
                              {p && (
                                <>
                                  {busy === `compose:${p.name}` && <Spinner />}
                                  <button className="btn btn-sm btn-ghost" onClick={() => compose(p, 'up')} title="docker compose up -d">
                                    <Play /> Up
                                  </button>
                                  <button className="btn btn-sm btn-ghost" onClick={() => compose(p, 'restart')}>
                                    <RotateCw /> Restart
                                  </button>
                                  <button className="btn btn-sm btn-ghost" onClick={() => compose(p, 'stop')}>
                                    <Square /> Stop
                                  </button>
                                  <button className="btn btn-sm btn-danger-ghost" onClick={() => compose(p, 'down')} title="docker compose down">
                                    <Trash2 /> Down
                                  </button>
                                </>
                              )}
                            </div>
                          </td>
                        </tr>
                      )}
                      {!isCollapsed &&
                        rows.map((c) => (
                          <tr key={c.id} className={clsx('clickable', selected === c.id && 'on')} onClick={() => setSelected(c.id)}>
                            <td>
                              <span className={clsx('dot', stateDot(c))} title={c.health ? `${c.state} (${c.health})` : c.state} />
                            </td>
                            <td className="nowrap">
                              <span className="strong">{c.composeService ?? c.name}</span>
                              {c.composeService && <span className="xs subtle"> {c.name}</span>}
                            </td>
                            <td className="mono xs truncate cell-max" title={c.image}>
                              {c.image}
                            </td>
                            <td className="small nowrap">{c.status}</td>
                            <td>
                              <div className="port-list">
                                {c.ports.map((p, i) =>
                                  p.url ? (
                                    <button
                                      key={i}
                                      className="port-chip link"
                                      title={`New HTTP request to ${p.url}`}
                                      onClick={(e) => {
                                        e.stopPropagation();
                                        void openHttpRequest(p.url!, `${c.composeService ?? c.name} :${p.publicPort}`);
                                      }}
                                    >
                                      {p.publicPort}→{p.privatePort}
                                      <ExternalLink />
                                    </button>
                                  ) : (
                                    <span key={i} className="port-chip" title="Not published">
                                      {p.privatePort}/{p.protocol}
                                    </span>
                                  ),
                                )}
                              </div>
                            </td>
                            <td className="xs subtle nowrap">{age(c.created)}</td>
                            <td onClick={(e) => e.stopPropagation()}>
                              <div className="row-actions">
                                {busy === c.id ? (
                                  <Spinner />
                                ) : c.state === 'running' ? (
                                  <button className="icon-btn sm" title="Stop" aria-label="Stop" onClick={() => act('Stop', c.id, () => infraApi.docker.stop(c.id))}>
                                    <Square />
                                  </button>
                                ) : (
                                  <button className="icon-btn sm" title="Start" aria-label="Start" onClick={() => act('Start', c.id, () => infraApi.docker.start(c.id))}>
                                    <Play />
                                  </button>
                                )}
                                <button className="icon-btn sm" title="Restart" aria-label="Restart" onClick={() => act('Restart', c.id, () => infraApi.docker.restart(c.id))}>
                                  <RotateCw />
                                </button>
                                <button className="icon-btn sm" title="Terminal" aria-label="Terminal" disabled={c.state !== 'running'} onClick={() => (setSelected(c.id), setDetail('terminal'))}>
                                  <SquareTerminal />
                                </button>
                                <Menu
                                  align="right"
                                  items={() => menu(c)}
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
                    </Fragment>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
        <RepoComposeFiles running={projects} onStarted={reload} />
      </div>
      {sel && (
        <div className="infra-detail">
          <div className="subtabs">
            {(['logs', 'stats', 'terminal', 'inspect'] as Detail[]).map((d) => (
              <button key={d} className={clsx('subtab', detail === d && 'on')} onClick={() => setDetail(d)} disabled={(d === 'terminal' || d === 'stats') && sel.state !== 'running'}>
                {d === 'logs' ? 'Logs' : d === 'stats' ? 'Stats' : d === 'terminal' ? 'Terminal' : 'Inspect'}
              </button>
            ))}
            <span className="grow" />
            <span className="small strong truncate detail-title">{sel.name}</span>
            <button className="icon-btn sm" onClick={() => setSelected(null)} title="Close" aria-label="Close details">
              ×
            </button>
          </div>
          <div className="infra-detail-body">
            {detail === 'logs' && (
              <LogPanel
                key={sel.id}
                title={`${sel.name} (container)`}
                sourceKey={sel.id}
                start={(streamId, o, onMsg) => infraApi.docker.logs(streamId, sel.id, { follow: o.follow, tail: o.tail ?? undefined, timestamps: o.timestamps }, onMsg)}
              />
            )}
            {detail === 'stats' && <StatsPanel key={sel.id} id={sel.id} />}
            {detail === 'terminal' && sel.state === 'running' && (
              <ExecTerminal key={sel.id} title={`${sel.name} · shell`} start={(execId, onEvent) => infraApi.docker.exec(execId, sel.id, onEvent)} />
            )}
            {detail === 'inspect' && <InspectPanel key={sel.id} id={sel.id} />}
          </div>
        </div>
      )}
    </div>
  );
}

const HISTORY = 60;

function StatsPanel({ id }: { id: string }) {
  const [samples, setSamples] = useState<StatsSample[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const streamId = `stats-${uid()}`;
    let live = true;
    infraApi.docker
      .stats(streamId, id, (s) => live && setSamples((prev) => [...prev.slice(-(HISTORY - 1)), s]))
      .catch((e) => live && setError(errorMessage(e)));
    return () => {
      live = false;
      void infraApi.streamStop(streamId);
    };
  }, [id]);
  const last = samples[samples.length - 1];
  if (error) return <div className="p-5"><Callout kind="danger">{error}</Callout></div>;
  if (!last)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  return (
    <div className="stats-grid p-5">
      <div className="card">
        <div className="stat">
          <span className="v">{last.cpuPercent.toFixed(1)}%</span>
          <span className="k">CPU · {last.onlineCpus} cores available</span>
        </div>
        <Sparkline values={samples.map((s) => s.cpuPercent)} max={Math.max(100, ...samples.map((s) => s.cpuPercent))} label="CPU history" width={260} />
      </div>
      <div className="card">
        <div className="stat">
          <span className="v">{fmtBytes(last.memUsed)}</span>
          <span className="k">
            Memory · {last.memLimit ? `${last.memPercent.toFixed(1)}% of ${fmtBytes(last.memLimit)}` : 'no limit'}
          </span>
        </div>
        <Sparkline values={samples.map((s) => s.memUsed)} max={last.memLimit || undefined} label="Memory history" width={260} color="var(--chart-3xx)" />
      </div>
      <div className="card">
        <dl className="kv">
          <dt>Network in / out</dt>
          <dd>
            {fmtBytes(last.netRx)} / {fmtBytes(last.netTx)}
          </dd>
          <dt>Block read / write</dt>
          <dd>
            {fmtBytes(last.blockRead)} / {fmtBytes(last.blockWrite)}
          </dd>
          <dt>Processes</dt>
          <dd>{last.pids}</dd>
        </dl>
      </div>
    </div>
  );
}

function InspectPanel({ id }: { id: string }) {
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    infraApi.docker.inspect(id).then(
      (v) => setText(JSON.stringify(v, null, 2)),
      (e) => setText(`// ${errorMessage(e)}`),
    );
  }, [id]);
  if (text === null)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  return <CodeEditor value={text} language="json" readOnly path={`inmemory://infra/inspect-${id}.json`} />;
}

function ImagesTab() {
  const { data, error } = usePolled(infraApi.docker.images, 0);
  const [q, setQ] = useState('');
  if (error) return <div className="p-6"><Callout kind="danger">{error}</Callout></div>;
  if (!data)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  const rows: ImageRow[] = data.filter((i) => !q || `${i.tags.join(' ')} ${i.shortId}`.toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="infra-table-wrap">
      <div className="toolbar">
        <SearchInput value={q} onChange={setQ} placeholder="Filter images…" />
        <span className="xs subtle">{data.length} images</span>
      </div>
      <div className="scroll grow">
        <table className="table infra-table">
          <thead>
            <tr>
              <th>Repository:tag</th>
              <th>Id</th>
              <th>Size</th>
              <th>Created</th>
              <th>Containers</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((i) => (
              <tr key={i.id}>
                <td className="mono xs">{i.tags.length ? i.tags.join(', ') : <span className="subtle">&lt;none&gt;</span>}</td>
                <td className="mono xs subtle">{i.shortId}</td>
                <td className="small nowrap">{fmtBytes(i.size)}</td>
                <td className="xs subtle nowrap">{age(i.created)}</td>
                <td className="small">{i.containers >= 0 ? i.containers : '—'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function VolumesTab() {
  const { data, error } = usePolled(infraApi.docker.volumes, 0);
  if (error) return <div className="p-6"><Callout kind="danger">{error}</Callout></div>;
  if (!data)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  return (
    <div className="scroll grow">
      <table className="table infra-table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Driver</th>
            <th>Compose project</th>
            <th>Mountpoint</th>
          </tr>
        </thead>
        <tbody>
          {data.map((v: VolumeRow) => (
            <tr key={v.name}>
              <td className="mono xs">{v.name}</td>
              <td className="small">{v.driver}</td>
              <td className="small">{v.composeProject ?? ''}</td>
              <td className="mono xs subtle truncate cell-max" title={v.mountpoint}>
                {v.mountpoint}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function NetworksTab() {
  const { data, error } = usePolled(infraApi.docker.networks, 0);
  if (error) return <div className="p-6"><Callout kind="danger">{error}</Callout></div>;
  if (!data)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  return (
    <div className="scroll grow">
      <table className="table infra-table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Driver</th>
            <th>Scope</th>
            <th>Compose project</th>
            <th>Id</th>
          </tr>
        </thead>
        <tbody>
          {data.map((n: NetworkRow) => (
            <tr key={n.id}>
              <td className="small strong">
                {n.name}
                {n.internal && <span className="badge">internal</span>}
              </td>
              <td className="small">{n.driver}</td>
              <td className="small">{n.scope}</td>
              <td className="small">{n.composeProject ?? ''}</td>
              <td className="mono xs subtle">{n.id.slice(0, 12)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

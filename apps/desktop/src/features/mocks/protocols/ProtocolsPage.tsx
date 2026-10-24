import { useCallback, useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { AlertTriangle, Braces, FilePlus2, Network, Play, Plus, Radio, RefreshCw, Save, Send, Square, Trash2, Waypoints, X } from 'lucide-react';
import { CodeEditor, type CodeLanguage } from '@/components/editor/CodeEditor';
import { Menu } from '@/components/ui/Menu';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import { mockApi, type ProtocolFile } from '@/lib/api/mock';
import { Callout, CopyButton, Empty, RelTime, Spinner, Switch } from '@/features/mocks/components/ui';
import { useWm } from '@/features/mocks/lib/queries';
import { useMockTarget } from '@/features/mocks/store/target';
import type { WireMockClient } from '@/features/mocks/lib/api';
import {
  PROTOCOL_DIR,
  describeMessage,
  templateFor,
  validateDefinition,
  type GraphqlStatus,
  type GrpcStatus,
  type LoadError,
  type MessageEvent,
  type MessageProtocol,
  type MqttStatus,
  type ProtocolTab,
  type WebsocketStatus,
} from '@/features/mocks/lib/protocols';
import { definitionModelPath } from './schemas';
import { SidePane } from '@/components/layout/SidePane';

const TITLES: Record<ProtocolTab, { title: string; icon: React.ReactNode; blurb: React.ReactNode }> = {
  graphql: {
    title: 'GraphQL',
    icon: <Braces size={16} className="subtle" />,
    blurb: (
      <>
        Schemas in <code className="code-inline">mocks/graphql/</code> are served on the HTTP port with generated data; stubs (Stubs tab) match on <code className="code-inline">graphql</code>{' '}
        operation / field / variables.
      </>
    ),
  },
  websocket: {
    title: 'WebSocket',
    icon: <Waypoints size={16} className="subtle" />,
    blurb: (
      <>
        Message stubs in <code className="code-inline">mocks/websocket/</code>, served as <code className="code-inline">ws://</code> endpoints on the HTTP port.
      </>
    ),
  },
  grpc: {
    title: 'gRPC',
    icon: <Network size={16} className="subtle" />,
    blurb: (
      <>
        <code className="code-inline">.proto</code> files and stubs in <code className="code-inline">mocks/grpc/</code>, on their own HTTP/2 port with server reflection.
      </>
    ),
  },
  mqtt: {
    title: 'MQTT',
    icon: <Radio size={16} className="subtle" />,
    blurb: (
      <>
        An embedded broker with scripted behaviours from <code className="code-inline">mocks/mqtt/</code>: retained messages, publish-on-subscribe, periodic publishes, request / response.
      </>
    ),
  },
};

function languageOf(rel: string): CodeLanguage {
  if (/\.(graphql|graphqls|gql)$/i.test(rel)) return 'graphql';
  if (/\.json$/i.test(rel)) return 'json';
  return 'plaintext';
}

/** Polls `f` every `ms` while mounted and `enabled`. */
function usePoll<T>(f: (() => Promise<T>) | null, ms: number, deps: unknown[]): [T | null, () => void] {
  const [data, setData] = useState<T | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!f) return setData(null);
    let stop = false;
    const tick = () =>
      f()
        .then((d) => !stop && setData(d))
        .catch(() => !stop && setData(null));
    void tick();
    const t = setInterval(() => !document.hidden && void tick(), ms);
    return () => {
      stop = true;
      clearInterval(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, n]);
  return [data, () => setN((x) => x + 1)];
}

/**
 * GraphQL / WebSocket / gRPC / MQTT tab of the Mock server page: the protocol's definition files
 * (edited in place, validated, reloaded into the running engine) and a live panel.
 */
export default function ProtocolsPage({ protocol }: { protocol: ProtocolTab }) {
  const root = useMockTarget((s) => s.root);
  const status = useMockTarget((s) => s.status);
  const { client, target } = useWm();
  const live = target.kind === 'builtin' && !!status?.running && client ? client : null;
  const [files, setFiles] = useState<ProtocolFile[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  const [text, setText] = useState('');
  const [saved, setSaved] = useState('');

  const loadFiles = useCallback(async () => {
    if (!root) return;
    try {
      const [, all] = await mockApi.protocolFiles(root);
      setFiles(all.filter((f) => f.protocol === protocol));
    } catch (e) {
      toast.error('Could not list files', errorMessage(e));
    }
  }, [root, protocol]);

  useEffect(() => {
    setSel(null);
    void loadFiles();
  }, [loadFiles]);

  const current = files.find((f) => f.rel === sel) ?? null;
  // Re-read only when another file is picked (refreshing the list must not drop unsaved edits).
  const currentPath = current?.path ?? null;
  useEffect(() => {
    if (!currentPath) {
      setText('');
      setSaved('');
      return;
    }
    let stale = false;
    api
      .readTextFile(currentPath)
      .then((t) => {
        if (stale) return;
        setText(t);
        setSaved(t);
      })
      .catch((e) => toast.error('Could not read the file', errorMessage(e)));
    return () => {
      stale = true;
    };
  }, [currentPath]);

  const loadErrors: LoadError[] = useMemo(() => (status?.loadErrors ?? []).filter((e) => e.file.startsWith(PROTOCOL_DIR[protocol])), [status, protocol]);
  const problems = useMemo(() => (current && protocol !== 'graphql' && current.rel.endsWith('.json') ? validateDefinition(protocol, text) : []), [current, protocol, text]);

  const applyToEngine = async () => {
    if (!live) return;
    try {
      useMockTarget.getState().markLocalWrite();
      await live.devallReload();
      await useMockTarget.getState().refreshStatus();
    } catch (e) {
      toast.error('Saved, but the running server could not reload', errorMessage(e));
    }
  };

  const save = async () => {
    if (!root || !current) return;
    try {
      await mockApi.protocolWrite(root, current.rel, text);
      setSaved(text);
      await applyToEngine();
      toast.success(`Saved ${current.rel}`, live ? 'Reloaded into the running server' : undefined);
    } catch (e) {
      toast.error('Could not save', errorMessage(e));
    }
  };

  const create = async (kind: 'stub' | 'schema' | 'proto') => {
    if (!root) return;
    const t = templateFor(protocol, kind);
    const name = await promptDialog({ title: 'New file', label: `File name in ${PROTOCOL_DIR[protocol]}/`, initial: t.name, validate: (v) => (!v.trim() || /(^|\/)\.\.(\/|$)/.test(v) ? 'Enter a file name' : undefined) });
    if (!name) return;
    const rel = `${PROTOCOL_DIR[protocol]}/${name.trim().replace(/^\/+/, '')}`;
    if (files.some((f) => f.rel === rel)) return toast.error('That file already exists');
    try {
      await mockApi.protocolWrite(root, rel, t.content);
      await loadFiles();
      setSel(rel);
      await applyToEngine();
    } catch (e) {
      toast.error('Could not create the file', errorMessage(e));
    }
  };

  const remove = async () => {
    if (!root || !current) return;
    if (!(await confirmDialog({ title: `Delete ${current.rel}?`, message: 'The file moves to the OS trash.', confirmText: 'Delete', danger: true }))) return;
    try {
      await mockApi.protocolDelete(root, current.rel);
      setSel(null);
      await loadFiles();
      await applyToEngine();
    } catch (e) {
      toast.error('Could not delete', errorMessage(e));
    }
  };

  const t = TITLES[protocol];
  const dirty = text !== saved;
  const newItems = [
    ...(protocol === 'graphql' ? [{ label: 'SDL schema (.graphql)', onSelect: () => void create('schema') }] : []),
    ...(protocol === 'grpc' ? [{ label: '.proto file', onSelect: () => void create('proto') }] : []),
    ...(protocol !== 'graphql' ? [{ label: protocol === 'mqtt' ? 'Behaviour (.json)' : 'Stub (.json)', onSelect: () => void create('stub') }] : []),
  ];

  if (!root) return <Empty title="Open a project" />;
  return (
    <div className="page">
      <div className="page-header">
        {t.icon}
        <h1>{t.title}</h1>
        <span className="subtle small truncate">{t.blurb}</span>
      </div>
      {loadErrors.length > 0 && (
        <div className="mocks-banner">
          <Callout kind="warning">
            <ul className="mocks-load-errors">
              {loadErrors.slice(0, 8).map((e, i) => (
                <li key={i}>
                  <code className="code-inline">{e.file}</code> — {e.error}
                </li>
              ))}
            </ul>
          </Callout>
        </div>
      )}
      <div className="split">
        <SidePane
          title="Files"
          actions={
            <>
              <Menu
                items={() => newItems}
                trigger={(p) => (
                  <button className="btn btn-sm" {...p} title="New file">
                    <FilePlus2 /> New
                  </button>
                )}
              />
              <button className="icon-btn sm" title="Refresh" onClick={() => void loadFiles()}>
                <RefreshCw />
              </button>
            </>
          }
        >
          <div className="list">
            {files.length === 0 && <div className="subtle xs p-4">No files in {PROTOCOL_DIR[protocol]}/ yet.</div>}
            {files.map((f) => (
              <div key={f.rel} className={clsx('list-item', f.rel === sel && 'on')} role="button" tabIndex={0} onClick={() => setSel(f.rel)} onKeyDown={(e) => e.key === 'Enter' && setSel(f.rel)}>
                <span className="truncate mono xs">{f.rel.slice(PROTOCOL_DIR[protocol].length + 1)}</span>
                {loadErrors.some((e) => e.file === f.rel) && <AlertTriangle size={12} className="text-warning" />}
              </div>
            ))}
          </div>
        </SidePane>
        <div className="pane fill">
          {current ? (
            <>
              <div className="toolbar">
                <span className="mono small truncate grow">
                  {current.rel}
                  {dirty && ' •'}
                </span>
                <button className="btn btn-sm btn-primary" disabled={!dirty} onClick={() => void save()} title="Save (Ctrl+S) and reload the running server">
                  <Save /> Save
                </button>
                <button className="icon-btn sm danger" title="Delete" onClick={() => void remove()}>
                  <Trash2 />
                </button>
              </div>
              {problems.length > 0 && (
                <div className="p-2">
                  <Callout kind="warning">
                    {problems.slice(0, 5).map((p, i) => (
                      <div key={i}>{p}</div>
                    ))}
                  </Callout>
                </div>
              )}
              <div className="editor-shell fill">
                <CodeEditor
                  value={text}
                  onChange={setText}
                  language={languageOf(current.rel)}
                  onSave={() => void save()}
                  path={protocol !== 'graphql' && current.rel.endsWith('.json') ? definitionModelPath(protocol as MessageProtocol, current.rel) : `proto-${current.rel}`}
                />
              </div>
            </>
          ) : (
            <Empty icon={t.icon} title={`${t.title} definitions`}>
              Pick a file on the left or create one. Changes are saved to the project and reloaded into the running server.
            </Empty>
          )}
        </div>
        <div className="pane side" style={{ width: 400, borderLeft: '1px solid var(--border)', borderRight: 0 }}>
          {!live ? (
            <Empty title="Server stopped">Start the built-in mock server to see live {t.title} traffic.</Empty>
          ) : protocol === 'graphql' ? (
            <GraphqlPanel client={live} />
          ) : (
            <LivePanel protocol={protocol} client={live} />
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- GraphQL

function GraphqlPanel({ client }: { client: WireMockClient }) {
  const [info] = usePoll<GraphqlStatus>(() => client.graphql(), 5000, [client]);
  const [schema, setSchema] = useState<string>('');
  const [query, setQuery] = useState('{\n  __typename\n}\n');
  const [vars, setVars] = useState('{}');
  const [result, setResult] = useState('');
  const [busy, setBusy] = useState(false);
  const schemas = info?.schemas ?? [];
  const sel = schemas.find((s) => s.name === schema) ?? schemas[0];
  const run = async () => {
    setBusy(true);
    try {
      let variables: Record<string, unknown> = {};
      if (vars.trim()) variables = JSON.parse(vars);
      setResult(JSON.stringify(await client.graphqlExecute({ schema: sel?.name, query, variables }), null, 2));
    } catch (e) {
      setResult(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="col p-4 fill-scroll" style={{ gap: 10 }}>
      {schemas.length === 0 ? (
        <p className="subtle small">No schema loaded. Add an SDL file (or an introspection JSON) to mocks/graphql/.</p>
      ) : (
        <>
          <div className="row">
            <select className="select sm grow" value={sel?.name ?? ''} onChange={(e) => setSchema(e.target.value)} aria-label="Schema">
              {schemas.map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name} — {s.endpoint}
                </option>
              ))}
            </select>
            {sel && <CopyButton text={`${client.base}${sel.endpoint}`} title="Copy the endpoint URL" />}
          </div>
          {sel && (
            <div className="subtle xs">
              {sel.summary.types} types · {sel.summary.query.length} query fields · {sel.summary.mutation.length} mutations · seed {sel.seed} · lists of {sel.listLength}
            </div>
          )}
          <div className="label">Try a query (auto-mocked, no stubs)</div>
          <div style={{ height: 160 }} className="editor-shell">
            <CodeEditor value={query} onChange={setQuery} language="graphql" minimal path="graphql-try-query.graphql" onSave={() => void run()} />
          </div>
          <div style={{ height: 70 }} className="editor-shell">
            <CodeEditor value={vars} onChange={setVars} language="json" minimal path="graphql-try-vars.json" />
          </div>
          <button className="btn btn-sm btn-primary" onClick={() => void run()} disabled={busy}>
            {busy ? <Spinner /> : <Play />} Run
          </button>
          {result && (
            <div style={{ height: 220 }} className="editor-shell">
              <CodeEditor value={result} language="json" readOnly minimal path="graphql-try-result.json" />
            </div>
          )}
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- WebSocket / gRPC / MQTT

function LivePanel({ protocol, client }: { protocol: MessageProtocol; client: WireMockClient }) {
  const [tab, setTab] = useState<'live' | 'journal'>('live');
  return (
    <div className="col h-full" style={{ gap: 0, minHeight: 0 }}>
      <div className="subtabs">
        <button className={clsx('subtab', tab === 'live' && 'on')} onClick={() => setTab('live')}>
          {protocol === 'websocket' ? 'Connections' : protocol === 'grpc' ? 'Listener' : 'Broker'}
        </button>
        <button className={clsx('subtab', tab === 'journal' && 'on')} onClick={() => setTab('journal')}>
          Messages
        </button>
      </div>
      <div className="fill-scroll" style={{ flex: 1, minHeight: 0 }}>
        {tab === 'journal' ? (
          <Journal protocol={protocol} client={client} />
        ) : protocol === 'websocket' ? (
          <WebsocketLive client={client} />
        ) : protocol === 'grpc' ? (
          <GrpcLive client={client} />
        ) : (
          <MqttLive client={client} />
        )}
      </div>
    </div>
  );
}

function Journal({ protocol, client }: { protocol: MessageProtocol; client: WireMockClient }) {
  const [paused, setPaused] = useState(false);
  const [data, refresh] = usePoll<{ messages: MessageEvent[] }>(paused ? null : () => client.messages(protocol, 300), 1500, [client, protocol, paused]);
  const [open, setOpen] = useState<number | null>(null);
  const msgs = data?.messages ?? [];
  return (
    <div className="col p-2" style={{ gap: 6 }}>
      <div className="row">
        <Switch checked={!paused} onChange={(v) => setPaused(!v)} label={<span className="xs">Live</span>} />
        <span className="subtle xs grow">{msgs.length} messages</span>
        <button
          className="btn btn-sm btn-ghost"
          onClick={() =>
            void client
              .clearMessages(protocol)
              .then(refresh)
              .catch((e) => toast.error('Could not clear', errorMessage(e)))
          }
        >
          <X /> Clear
        </button>
      </div>
      {msgs.length === 0 && <div className="subtle xs p-2">Nothing yet.</div>}
      {msgs.map((m) => (
        <div key={m.seq} className={clsx('proto-msg', m.direction)} onClick={() => setOpen(open === m.seq ? null : m.seq)} role="button" tabIndex={0}>
          <div className="row-sm">
            <span className={clsx('badge', m.direction === 'in' ? 'info' : m.direction === 'out' ? 'success' : 'outline')}>{m.direction === 'in' ? '→ in' : m.direction === 'out' ? '← out' : '·'}</span>
            <span className="mono xs truncate grow" title={m.channel}>
              {m.channel || m.connection}
            </span>
            <span className="subtle xs nowrap">
              <RelTime ts={m.at} />
            </span>
          </div>
          <div className={clsx('mono xs', open === m.seq ? 'break' : 'truncate')}>{describeMessage(m)}</div>
          {open === m.seq && (
            <div className="subtle xs">
              {m.connection}
              {m.stub ? ` · ${m.stub}` : ''}
              {m.meta && Object.keys(m.meta).length > 0 ? ` · ${JSON.stringify(m.meta)}` : ''}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

function WebsocketLive({ client }: { client: WireMockClient }) {
  const [st, refresh] = usePoll<WebsocketStatus>(() => client.websocket(), 2000, [client]);
  const [to, setTo] = useState<string>('');
  const [msg, setMsg] = useState('');
  const send = async () => {
    try {
      const target = to.startsWith('/') ? { path: to } : to ? { connectionId: to } : {};
      const r = await client.websocketSend({ ...target, text: msg });
      toast.success(`Sent to ${r.sent} connection${r.sent === 1 ? '' : 's'}`);
    } catch (e) {
      toast.error('Could not send', errorMessage(e));
    }
  };
  const conns = st?.connections ?? [];
  const paths = [...new Set((st?.stubs ?? []).map((s) => ((s.definition as { request?: { urlPath?: string } })?.request?.urlPath ?? '')).filter(Boolean))];
  return (
    <div className="col p-4" style={{ gap: 10 }}>
      <div className="subtle xs">
        {st?.stubs.length ?? 0} stub{st?.stubs.length === 1 ? '' : 's'} · endpoints {paths.length ? paths.map((p) => `${client.base.replace(/^http/, 'ws')}${p}`).join(', ') : '—'}
      </div>
      <div className="label">Open connections ({conns.length})</div>
      {conns.length === 0 && <div className="subtle xs">No client connected.</div>}
      {conns.map((c) => (
        <div key={c.id} className="row-sm">
          <span className="mono xs">{c.id}</span>
          <span className="mono xs truncate grow">{c.path}</span>
          <span className="subtle xs">
            <RelTime ts={c.opened} />
          </span>
          <button
            className="icon-btn sm"
            title="Close this connection"
            onClick={() =>
              void client
                .websocketClose({ connectionId: c.id, code: 1000, reason: 'closed from /dev/all' })
                .then(refresh)
                .catch((e) => toast.error('Could not close', errorMessage(e)))
            }
          >
            <X />
          </button>
        </div>
      ))}
      <div className="label">Push a message</div>
      <select className="select sm" value={to} onChange={(e) => setTo(e.target.value)} aria-label="Send to">
        <option value="">Every connection</option>
        {paths.map((p) => (
          <option key={p} value={p}>
            Connections on {p}
          </option>
        ))}
        {conns.map((c) => (
          <option key={c.id} value={c.id}>
            {c.id} ({c.path})
          </option>
        ))}
      </select>
      <textarea className="textarea mono" rows={4} value={msg} onChange={(e) => setMsg(e.target.value)} placeholder='{"type": "notice", "text": "hello"}' />
      <button className="btn btn-sm btn-primary" onClick={() => void send()} disabled={!msg}>
        <Send /> Send
      </button>
    </div>
  );
}

function PortControls({ running, port, onStart, onStop }: { running: boolean; port: number | null; onStart(port?: number): Promise<void>; onStop(): Promise<void> }) {
  const [want, setWant] = useState('');
  const [busy, setBusy] = useState(false);
  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="row-sm">
      <span className={clsx('dot', running ? 'ok' : '')} />
      <span className="small grow">{running ? <>Listening on port {port}</> : 'Stopped'}</span>
      <input className="input sm" style={{ width: 80 }} inputMode="numeric" placeholder="any port" value={want} onChange={(e) => setWant(e.target.value.replace(/\D/g, ''))} aria-label="Port" />
      <button className="btn btn-sm" disabled={busy} onClick={() => void run(() => onStart(want ? Number(want) : undefined))}>
        {busy ? <Spinner /> : <Play />} {running ? 'Move' : 'Start'}
      </button>
      {running && (
        <button className="btn btn-sm btn-ghost" disabled={busy} onClick={() => void run(onStop)}>
          <Square /> Stop
        </button>
      )}
    </div>
  );
}

function useListenerActions(kind: 'grpc' | 'mqtt', refresh: () => void) {
  const root = useMockTarget((s) => s.root);
  return {
    start: async (port?: number) => {
      if (!root) return;
      try {
        await (kind === 'grpc' ? mockApi.grpcStart(root, port) : mockApi.mqttStart(root, port));
        await useMockTarget.getState().refreshStatus();
        refresh();
      } catch (e) {
        toast.error(`Could not start the ${kind === 'grpc' ? 'gRPC listener' : 'MQTT broker'}`, errorMessage(e));
      }
    },
    stop: async () => {
      if (!root) return;
      try {
        await (kind === 'grpc' ? mockApi.grpcStop(root) : mockApi.mqttStop(root));
        await useMockTarget.getState().refreshStatus();
        refresh();
      } catch (e) {
        toast.error('Could not stop', errorMessage(e));
      }
    },
  };
}

function GrpcLive({ client }: { client: WireMockClient }) {
  const [st, refresh] = usePoll<GrpcStatus>(() => client.grpc(), 3000, [client]);
  const actions = useListenerActions('grpc', refresh);
  if (!st) return <Spinner />;
  const host = client.base.replace(/^https?:\/\//, '').replace(/:\d+$/, '');
  return (
    <div className="col p-4" style={{ gap: 10 }}>
      <PortControls running={st.running} port={st.port} onStart={actions.start} onStop={actions.stop} />
      {st.running && st.port && (
        <div className="row-sm">
          <code className="code-inline grow truncate">grpcurl -plaintext {host}:{st.port} list</code>
          <CopyButton text={`grpcurl -plaintext ${host}:${st.port} list`} />
        </div>
      )}
      {!st.hasProtos && <Callout kind="info">Add .proto files to mocks/grpc/ to describe the services to mock.</Callout>}
      <div className="label">Methods ({st.methods.length})</div>
      {st.methods.map((m) => {
        const stubs = st.stubs.filter((s) => `/${s.method}` === m.path).length;
        return (
          <div key={m.path} className="col-sm">
            <div className="row-sm">
              <span className="mono xs truncate grow" title={`${m.inputType} → ${m.outputType}`}>
                {m.path}
              </span>
              {(m.clientStreaming || m.serverStreaming) && <span className="badge outline">{m.clientStreaming && m.serverStreaming ? 'bidi' : m.clientStreaming ? 'client stream' : 'server stream'}</span>}
              <span className={clsx('badge', stubs ? 'success' : 'outline')}>
                {stubs} stub{stubs === 1 ? '' : 's'}
              </span>
              <CopyButton text={m.template} title="Copy an example request (JSON)" />
            </div>
          </div>
        );
      })}
    </div>
  );
}

function MqttLive({ client }: { client: WireMockClient }) {
  const [st, refresh] = usePoll<MqttStatus>(() => client.mqtt(), 2000, [client]);
  const actions = useListenerActions('mqtt', refresh);
  const [topic, setTopic] = useState('');
  const [payload, setPayload] = useState('');
  const [qos, setQos] = useState(0);
  const [retain, setRetain] = useState(false);
  if (!st) return <Spinner />;
  const host = client.base.replace(/^https?:\/\//, '').replace(/:\d+$/, '');
  const publish = async () => {
    try {
      const r = await client.mqttPublish({ topic, payload, qos, retain });
      toast.success(`Delivered to ${r.delivered} subscriber${r.delivered === 1 ? '' : 's'}`);
      refresh();
    } catch (e) {
      toast.error('Could not publish', errorMessage(e));
    }
  };
  return (
    <div className="col p-4" style={{ gap: 10 }}>
      <PortControls running={st.running} port={st.port} onStart={actions.start} onStop={actions.stop} />
      {st.running && st.port && (
        <div className="row-sm">
          <code className="code-inline grow truncate">mqtt://{host}:{st.port}</code>
          <CopyButton text={`mqtt://${host}:${st.port}`} />
        </div>
      )}
      <div className="label">Clients ({st.clients.length})</div>
      {st.clients.length === 0 && <div className="subtle xs">No client connected.</div>}
      {st.clients.map((c) => (
        <div key={c.clientId} className="col-sm">
          <div className="row-sm">
            <span className="mono xs strong">{c.clientId}</span>
            <span className="subtle xs grow">{c.peer}</span>
          </div>
          {c.subscriptions.length > 0 && <div className="mono xs subtle">{c.subscriptions.join(' · ')}</div>}
        </div>
      ))}
      <div className="label">Retained ({st.retained.length})</div>
      {st.retained.map((r) => (
        <div key={r.topic} className="row-sm">
          <span className="mono xs">{r.topic}</span>
          <span className="mono xs subtle truncate grow">{r.binary ? '<binary>' : r.payload}</span>
        </div>
      ))}
      {st.running && (
        <>
          <div className="label">Publish</div>
          <input className="input sm mono" value={topic} onChange={(e) => setTopic(e.target.value)} placeholder="devices/1/state" aria-label="Topic" />
          <textarea className="textarea mono" rows={3} value={payload} onChange={(e) => setPayload(e.target.value)} placeholder="payload" />
          <div className="row-sm">
            <select className="select sm" value={qos} onChange={(e) => setQos(Number(e.target.value))} aria-label="QoS">
              <option value={0}>QoS 0</option>
              <option value={1}>QoS 1</option>
              <option value={2}>QoS 2</option>
            </select>
            <Switch checked={retain} onChange={setRetain} label={<span className="xs">Retain</span>} />
            <span className="grow" />
            <button className="btn btn-sm btn-primary" disabled={!topic || /[+#]/.test(topic)} onClick={() => void publish()}>
              <Plus /> Publish
            </button>
          </div>
        </>
      )}
    </div>
  );
}

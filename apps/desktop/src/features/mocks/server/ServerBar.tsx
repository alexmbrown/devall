import { useEffect, useState } from 'react';
import clsx from 'clsx';
import { Check, ChevronDown, Container, Flame, Globe2, Play, Plus, RefreshCw, Server, Settings2, Square, HardDrive, AlertTriangle } from 'lucide-react';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { CopyButton, Spinner, Switch, Callout } from '@/features/mocks/components/ui';
import { useCapabilities, useHealth, useRecordingStatus, useWm, invalidateTarget, clientFor } from '@/features/mocks/lib/queries';
import { builtinPrefs, setBuiltinPrefs, useMockTarget, useTargets, ENV_LABEL } from '@/features/mocks/store/target';
import { useNav } from '@/features/mocks/store/nav';
import { ApiError } from '@/features/mocks/lib/api';
import { ChaosDialog } from './ChaosDialog';
import { CiDialog } from './CiDialog';
import { PublishMockDialog } from '@/features/tunnels/PublishMockDialog';
import type { ProtocolsInfo } from '@/lib/api/mock';

/** The other listeners of the built-in engine (GraphQL endpoints, WebSocket stubs, gRPC, MQTT). */
function ProtocolChips({ p }: { p: ProtocolsInfo }) {
  const go = useNav((s) => s.go);
  const chip = (key: string, label: string, title: string, section: 'graphql' | 'websocket' | 'grpc' | 'mqtt') => (
    <button key={key} className="badge accent proto-chip" title={title} onClick={() => go(section)}>
      {label}
    </button>
  );
  return (
    <>
      {p.graphql.length > 0 && chip('gql', p.graphql.length === 1 ? `GraphQL ${p.graphql[0]}` : `GraphQL ×${p.graphql.length}`, `GraphQL endpoints: ${p.graphql.join(', ')}`, 'graphql')}
      {p.websocketStubs > 0 && chip('ws', `WS ${p.websocketConnections}/${p.websocketStubs}`, `${p.websocketConnections} open WebSocket connection(s), ${p.websocketStubs} stub(s)`, 'websocket')}
      {p.grpcPort !== null && chip('grpc', `gRPC :${p.grpcPort}`, `gRPC (h2c, reflection) on port ${p.grpcPort} — ${p.grpcMethods} method(s)`, 'grpc')}
      {p.mqttPort !== null && chip('mqtt', `MQTT :${p.mqttPort}`, `MQTT broker on port ${p.mqttPort} — ${p.mqttClients} client(s)`, 'mqtt')}
    </>
  );
}

/** Asks before listening on every interface (the admin API has no authentication). */
export async function confirmBindAll(): Promise<boolean> {
  return confirmDialog({
    title: 'Listen on all network interfaces?',
    message: (
      <>
        Other machines on your network can then call your stubs <b>and the admin API</b>, which has no authentication: anyone who can reach the port can read and change stubs
        (they are files in this project). Only do this on a network you trust, e.g. to test from a phone or a VM.
      </>
    ),
    confirmText: 'Listen on all interfaces',
    danger: true,
  });
}

/** Port field + bind-all switch + Start, used in the bar and on the start screen. */
export function StartControls({ big }: { big?: boolean }) {
  const root = useMockTarget((s) => s.root);
  const starting = useMockTarget((s) => s.starting);
  const start = useMockTarget((s) => s.start);
  const [port, setPort] = useState(() => String(builtinPrefs(root).port ?? ''));
  const [bindAll, setBindAll] = useState(() => !!builtinPrefs(root).bindAll);
  const [auto, setAuto] = useState(() => !!builtinPrefs(root).autoStart);
  useEffect(() => {
    const p = builtinPrefs(root);
    setPort(String(p.port ?? ''));
    setBindAll(!!p.bindAll);
    setAuto(!!p.autoStart);
  }, [root]);
  const n = Number(port);
  const valid = !port || (Number.isInteger(n) && n > 0 && n < 65536);
  const go = () => valid && start(port ? n : undefined, bindAll);
  return (
    <div className={clsx('row-sm', big && 'wrap')} style={big ? { gap: 12 } : undefined}>
      <input
        className={clsx('input', !big && 'sm')}
        style={{ width: big ? 110 : 76 }}
        inputMode="numeric"
        placeholder="any port"
        title="Port (empty = any free port)"
        value={port}
        onChange={(e) => setPort(e.target.value.replace(/\D/g, ''))}
        onKeyDown={(e) => e.key === 'Enter' && go()}
        aria-label="Port"
      />
      <Switch
        checked={bindAll}
        onChange={async (v) => {
          if (v && !(await confirmBindAll())) return;
          setBindAll(v);
        }}
        label={<span className={clsx('xs', bindAll && 'text-warning')}>All interfaces</span>}
        title="Listen on 0.0.0.0 instead of 127.0.0.1"
      />
      {big && (
        <Switch
          checked={auto}
          onChange={(v) => {
            setAuto(v);
            if (root) setBuiltinPrefs(root, { autoStart: v });
          }}
          label={<span className="xs">Start when this page opens</span>}
        />
      )}
      <button className={clsx('btn btn-primary', !big && 'btn-sm')} onClick={go} disabled={starting || !valid || !root}>
        {starting ? <Spinner /> : <Play />} Start
      </button>
    </div>
  );
}

export function ServerBar() {
  const { target, client } = useWm();
  const targets = useTargets();
  const st = useMockTarget();
  const go = useNav((s) => s.go);
  const health = useHealth();
  const caps = useCapabilities();
  const rec = useRecordingStatus();
  const [chaos, setChaos] = useState(false);
  const [ci, setCi] = useState(false);
  const [publish, setPublish] = useState(false);
  const builtin = target.kind === 'builtin';
  const running = builtin && !!st.status?.running;
  const bindAll = builtin && running && !!builtinPrefs(st.root).bindAll;

  const items = (): MenuItemDef[] => [
    { heading: 'Mock server' },
    ...targets.map((t) => ({
      label: (
        <span className="row-sm">
          {t.env && <span className={`env-chip env-${t.env}`}>{ENV_LABEL[t.env]}</span>}
          <span className="truncate">{t.kind === 'builtin' ? 'This project (built-in)' : t.name}</span>
          <span className="subtle xs truncate">{t.kind === 'builtin' ? (t.url ? t.url.replace(/^https?:\/\//, '') : 'stopped') : t.url}</span>
        </span>
      ),
      hint: t.id === target.id ? <Check size={13} /> : undefined,
      onSelect: () => st.setActive(t.id),
    })),
    { separator: true },
    { label: 'Add remote WireMock…', icon: <Plus />, onSelect: () => go('servers', 'new') },
    { label: 'Manage servers', icon: <Settings2 />, onSelect: () => go('servers') },
  ];

  const reload = async () => {
    const c = clientFor(target);
    if (!c) return;
    try {
      st.markLocalWrite();
      const r = await c.devallReload();
      st.setDiskChanged(false);
      await st.refreshStatus();
      await invalidateTarget(target);
      toast.success(`Reloaded ${r.stubCount} stub${r.stubCount === 1 ? '' : 's'} from mocks/mappings`, r.loadErrors.length ? `${r.loadErrors.length} file(s) could not be loaded` : undefined);
    } catch (e) {
      toast.error('Reload failed', e instanceof ApiError ? e.message : String(e));
    }
  };

  const status = builtin ? (running ? 'ok' : 'off') : health.isError ? 'err' : health.data ? 'ok' : 'pending';
  const errMsg = health.error instanceof ApiError ? health.error.message : health.error ? String(health.error) : '';

  return (
    <>
      <div className="toolbar mocks-bar">
        <Menu
          items={items}
          trigger={(p) => (
            <button className="switcher" {...p} title={errMsg || client?.base || target.name}>
              <Server />
              <span className={`dot ${status === 'ok' ? 'ok' : status === 'err' ? 'err' : status === 'off' ? '' : 'pulse'}`} />
              {target.env && !builtin && <span className={`env-chip env-${target.env}`}>{ENV_LABEL[target.env]}</span>}
              <span className="truncate strong">{builtin ? 'Built-in mock server' : target.name}</span>
              <ChevronDown />
            </button>
          )}
        />
        {builtin ? (
          running ? (
            <>
              <span className="mono small nowrap">{st.status?.url}</span>
              <CopyButton text={st.status?.url ?? ''} title="Copy base URL" />
              {bindAll && (
                <span className="badge warning" title="Listening on all interfaces: reachable from your network, admin API included">
                  <AlertTriangle /> LAN
                </span>
              )}
              <span className="badge outline" title="Stubs loaded (files in mocks/mappings plus in-memory stubs)">
                <HardDrive /> {st.status?.stubCount ?? 0} stubs
              </span>
              {st.status?.protocols && <ProtocolChips p={st.status.protocols} />}
              <button className="btn btn-sm btn-ghost" onClick={() => st.stop()} title="Stop the mock server">
                <Square /> Stop
              </button>
              <button className="icon-btn sm" onClick={reload} title="Reload stubs from mocks/mappings (drops in-memory stubs)">
                <RefreshCw />
              </button>
              <button className={clsx('btn btn-sm', chaos && 'active')} onClick={() => setChaos(true)} title="Chaos: added latency, random errors and faults for every mock request">
                <Flame /> Chaos
              </button>
            </>
          ) : st.statusLoaded ? (
            <StartControls />
          ) : (
            <Spinner />
          )
        ) : (
          <>
            <span className="mono small subtle truncate" style={{ maxWidth: 360 }}>
              {client?.base ?? target.url}
            </span>
            {status === 'ok' && (
              <span className="subtle xs nowrap" title="WireMock version · admin API latency">
                {health.data?.version ? `v${health.data.version}` : 'WireMock 2.x'} · {health.data?.latencyMs} ms
              </span>
            )}
            {status === 'err' && (
              <span className="badge danger" title={errMsg}>
                unreachable
              </span>
            )}
            {caps.devall && (
              <button className="btn btn-sm" onClick={() => setChaos(true)} title="This server runs the /dev/all engine: chaos controls are available">
                <Flame /> Chaos
              </button>
            )}
          </>
        )}
        {builtin && (
          <button className="btn btn-sm btn-ghost" onClick={() => setCi(true)} title="Serve these mocks in CI: devall mock serve, Docker, GitHub Actions, docker-compose">
            <Container /> Run in CI
          </button>
        )}
        {builtin && st.root && (
          <button className="btn btn-sm btn-ghost" onClick={() => setPublish(true)} title="Serve these stubs from the hosted relay so they keep answering while this machine is off">
            <Globe2 /> Publish to the web
          </button>
        )}
        {target.readOnly && <span className="badge outline">READ-ONLY</span>}
        {target.protected && !target.readOnly && <span className="badge warning">PROTECTED</span>}
        {rec.data?.status === 'Recording' && (
          <button className="badge danger" style={{ cursor: 'pointer', border: 0 }} onClick={() => go('recording')}>
            <span className="dot err pulse" /> REC
          </button>
        )}
      </div>
      {builtin && (st.status?.loadErrors.length ?? 0) > 0 && (
        <div className="mocks-banner">
          <Callout kind="warning">
            <b>{st.status!.loadErrors.length} mapping file{st.status!.loadErrors.length === 1 ? '' : 's'} could not be loaded</b>
            <ul className="mocks-load-errors">
              {st.status!.loadErrors.slice(0, 6).map((e) => (
                <li key={e.file}>
                  <code className="code-inline">{e.file}</code> — {e.error}
                </li>
              ))}
            </ul>
          </Callout>
        </div>
      )}
      {builtin && running && st.diskChanged && (
        <div className="mocks-banner">
          <Callout kind="info">
            <div className="row">
              <span className="grow">Stub files in mocks/mappings changed on disk (git pull, another tool, the traffic inspector…). The running server still serves the old set.</span>
              <button className="btn btn-sm btn-primary" onClick={reload}>
                <RefreshCw /> Reload
              </button>
              <button className="btn btn-sm btn-ghost" onClick={() => st.setDiskChanged(false)}>
                Dismiss
              </button>
            </div>
          </Callout>
        </div>
      )}
      {chaos && <ChaosDialog onClose={() => setChaos(false)} />}
      {ci && <CiDialog onClose={() => setCi(false)} />}
      {publish && st.root && <PublishMockDialog root={st.root} onClose={() => setPublish(false)} />}
    </>
  );
}


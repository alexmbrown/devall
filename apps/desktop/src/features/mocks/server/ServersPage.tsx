import { useEffect, useState } from 'react';
import clsx from 'clsx';
import { Plus, Trash2, Copy, ArrowUp, ArrowDown, PlugZap, Server, RefreshCw, KeyRound, HardDrive, FileCode2 } from 'lucide-react';
import { useNav } from '@/features/mocks/store/nav';
import { useMockTarget, BUILTIN_ID, ENV_LABEL, type EnvKind, type RemoteServer } from '@/features/mocks/store/target';
import { WireMockClient, ApiError, normalizeBaseUrl, normalizeContextPath, type AuthType } from '@/features/mocks/lib/api';
import { proxyStatus } from '@/features/mocks/lib/transport';
import { secretNameFor, uniqueId, SECRET_HEADER, SERVERS_FILE } from '@/features/mocks/lib/servers';
import { Field, Callout, Spinner, Switch, Segmented } from '@/features/mocks/components/ui';
import { confirmDialog } from '@/components/ui/dialogs';
import { StringMapEditor } from '@/features/mocks/stubs/form/PatternEditors';
import { ConnectionDoctor } from './ConnectionDoctor';

/** Remote WireMock servers of this project (`mocks/servers.yaml`) plus the built-in engine. */
export default function ServersPage() {
  const st = useMockTarget();
  const param = useNav((s) => s.param);
  const [editing, setEditing] = useState<string | 'new' | null>(param === 'new' ? 'new' : param && st.servers.some((s) => s.id === param) ? param : null);

  useEffect(() => {
    if (param === 'new') setEditing('new');
    else if (param && st.servers.some((s) => s.id === param)) setEditing(param);
  }, [param, st.servers]);

  const save = (servers: RemoteServer[]) => st.saveServers(servers);
  const move = (id: string, dir: -1 | 1) => {
    const list = [...st.servers];
    const i = list.findIndex((s) => s.id === id);
    const j = i + dir;
    if (i < 0 || j < 0 || j >= list.length) return;
    [list[i], list[j]] = [list[j], list[i]];
    void save(list);
  };

  return (
    <div className="page">
      <div className="page-header">
        <Server size={16} className="subtle" />
        <h1>Servers</h1>
        <span className="subtle small">
          Remote servers are shared with your team in <code className="code-inline">{SERVERS_FILE}</code>; credentials stay in each person's OS keychain.
        </span>
        <div className="spacer" />
        <button className="icon-btn" onClick={() => st.reloadServers()} title="Reload servers.yaml">
          <RefreshCw />
        </button>
        <button className="btn btn-sm btn-primary" onClick={() => setEditing('new')}>
          <Plus /> Add server
        </button>
      </div>
      <div className="page-body">
        <div className="col-lg" style={{ maxWidth: 920 }}>
          {st.serversWarnings.map((w, i) => (
            <Callout key={i} kind="warning">
              {w}
            </Callout>
          ))}
          {proxyStatus() === false && (
            <Callout kind="info">
              Remote calls go straight from the webview because the <code className="code-inline">wiremock_admin</code> command is not registered in this build. Keychain credentials
              then can't be used (enter them per session), and the app's content security policy must allow each server.
            </Callout>
          )}

          <div className="card row" style={{ padding: '8px 12px' }}>
            <span className="env-chip env-local">{ENV_LABEL.local}</span>
            <div className="grow" style={{ minWidth: 0 }}>
              <div className="row-sm">
                <HardDrive size={14} className="subtle" />
                <span className="strong">This project (built-in)</span>
              </div>
              <div className="mono xs subtle truncate">{st.status?.running ? st.status.url : 'stopped'} · stubs in mocks/mappings/</div>
            </div>
            <button className={clsx('btn btn-sm', st.activeId === BUILTIN_ID && 'active')} onClick={() => st.setActive(BUILTIN_ID)}>
              {st.activeId === BUILTIN_ID ? 'Connected' : 'Connect'}
            </button>
          </div>

          {editing === 'new' && <ServerForm onDone={() => setEditing(null)} />}
          {st.serversLoaded && st.servers.length === 0 && editing !== 'new' && (
            <div className="subtle small">
              No remote servers yet. Add your team's shared WireMock (dev / staging) to browse its stubs, compare them with this project and copy them in either direction.
            </div>
          )}
          {st.servers.map((s, i) =>
            editing === s.id ? (
              <ServerForm key={s.id} server={s} onDone={() => setEditing(null)} />
            ) : (
              <ServerRow
                key={s.id}
                server={s}
                first={i === 0}
                last={i === st.servers.length - 1}
                onEdit={() => setEditing(s.id)}
                onMove={(d) => move(s.id, d)}
                onDuplicate={() => save([...st.servers, { ...structuredClone(s), id: uniqueId(`${s.name} copy`, st.servers.map((x) => x.id)), name: `${s.name} (copy)` }])}
                onRemove={async () => {
                  if (!(await confirmDialog({ title: `Remove "${s.name}"?`, message: `Removes it from ${SERVERS_FILE} (and its keychain entry on this machine). Nothing on the server changes.`, danger: true, confirmText: 'Remove' }))) return;
                  if (await save(st.servers.filter((x) => x.id !== s.id))) {
                    if (s.auth) await st.deleteSecret(s.id);
                    st.setSessionSecret(s.id, undefined);
                  }
                }}
              />
            ),
          )}
          <div className="subtle xs row-sm">
            <FileCode2 size={13} /> The file is plain YAML — review it in Source control before committing.
          </div>
        </div>
      </div>
    </div>
  );
}

function credentialState(id: string): 'keychain' | 'session' | 'missing' | 'unknown' {
  const st = useMockTarget.getState();
  if (st.sessionSecrets[id] !== undefined) return 'session';
  const p = st.secretPresent[id];
  return p === true ? 'keychain' : p === false ? 'missing' : 'unknown';
}

function ServerRow({ server, first, last, onEdit, onMove, onDuplicate, onRemove }: { server: RemoteServer; first: boolean; last: boolean; onEdit(): void; onMove(d: -1 | 1): void; onDuplicate(): void; onRemove(): void }) {
  const st = useMockTarget();
  const isActive = st.activeId === server.id;
  const [ping, setPing] = useState<{ ok: boolean; msg: string } | null>(null);
  const cred = server.auth ? credentialState(server.id) : undefined;
  const test = async () => {
    setPing(null);
    try {
      const r = await clientForForm(server, st.sessionSecrets[server.id]).ping();
      setPing({ ok: true, msg: `${r.version ? `v${r.version}` : 'WireMock 2.x'} · ${r.latencyMs} ms` });
    } catch (e) {
      setPing({ ok: false, msg: (e as Error).message });
    }
  };
  return (
    <div className="card row" style={{ padding: '8px 12px' }}>
      {server.env && <span className={`env-chip env-${server.env}`}>{ENV_LABEL[server.env]}</span>}
      <div className="grow" style={{ minWidth: 0 }}>
        <div className="row-sm">
          <span className="strong truncate">{server.name}</span>
          {server.readOnly && <span className="badge outline">read-only</span>}
          {server.protected && <span className="badge warning">protected</span>}
          {server.auth && (
            <span className={clsx('badge', cred === 'missing' ? 'warning' : 'outline')} title={cred === 'keychain' ? 'Credential in the OS keychain' : cred === 'session' ? 'Credential for this session only' : 'No credential on this machine yet'}>
              <KeyRound /> {server.auth.type}
              {cred === 'missing' ? ' · not set' : cred === 'session' ? ' · session' : ''}
            </span>
          )}
        </div>
        <div className="mono xs subtle truncate">
          {server.url}
          {server.contextPath ? ` · ${server.contextPath}` : ''}
        </div>
        {ping && <div className={clsx('xs', ping.ok ? 'text-success' : 'text-danger')}>{ping.msg}</div>}
      </div>
      <button className="btn btn-sm" onClick={test}>
        <PlugZap /> Test
      </button>
      <button className={clsx('btn btn-sm', isActive && 'active')} onClick={() => st.setActive(server.id)}>
        {isActive ? 'Connected' : 'Connect'}
      </button>
      <button className="btn btn-sm" onClick={onEdit}>
        Edit
      </button>
      <button className="icon-btn sm" disabled={first} onClick={() => onMove(-1)} title="Move up">
        <ArrowUp />
      </button>
      <button className="icon-btn sm" disabled={last} onClick={() => onMove(1)} title="Move down">
        <ArrowDown />
      </button>
      <button className="icon-btn sm" onClick={onDuplicate} title="Duplicate">
        <Copy />
      </button>
      <button className="icon-btn sm danger" onClick={onRemove} title="Remove">
        <Trash2 />
      </button>
    </div>
  );
}

function clientForForm(s: Omit<RemoteServer, 'id'> & { id: string }, value?: string) {
  const root = useMockTarget.getState().root ?? undefined;
  return new WireMockClient({
    url: s.url,
    contextPath: s.contextPath,
    headers: s.headers,
    insecureTls: s.insecureTls,
    root,
    auth: s.auth && s.auth.type !== 'none' ? { ...s.auth, secret: secretNameFor(s.id), value } : undefined,
    timeoutMs: 10_000,
  });
}

type CredMode = 'keychain' | 'session';

function ServerForm({ server, onDone }: { server?: RemoteServer; onDone(): void }) {
  const st = useMockTarget();
  const [s, setS] = useState<RemoteServer>(server ?? { id: '', name: '', url: 'http://localhost:8080', env: 'dev' });
  const [secret, setSecret] = useState('');
  const [mode, setMode] = useState<CredMode>('keychain');
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: true; msg: string } | { ok: false; err: ApiError } | null>(null);
  const authType: AuthType = s.auth?.type ?? 'none';
  const setAuth = (patch: Partial<NonNullable<RemoteServer['auth']>>) => setS({ ...s, auth: { type: authType, ...s.auth, ...patch } });
  const valid = s.url.trim().length > 0;
  const id = server?.id ?? uniqueId(s.name.trim() || hostOf(s.url), st.servers.map((x) => x.id));
  const cred = server?.auth ? credentialState(server.id) : undefined;
  const headerProblems = (s.headers ?? []).filter((h) => SECRET_HEADER.test(h.key) && h.value);
  const secretLabel = authType === 'basic' ? 'Password' : authType === 'bearer' ? 'Token' : 'Header value';

  const clean = (): RemoteServer => ({
    ...s,
    id,
    url: normalizeBaseUrl(s.url),
    name: s.name.trim() || hostOf(s.url),
    contextPath: normalizeContextPath(s.contextPath) || undefined,
    auth: authType === 'none' ? undefined : { type: authType, username: authType === 'basic' ? s.auth?.username : undefined, headerName: authType === 'header' ? s.auth?.headerName : undefined },
  });

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      const r = await clientForForm(clean(), secret || st.sessionSecrets[id]).ping();
      setResult({ ok: true, msg: `Connected — ${r.version ? `WireMock ${r.version}` : 'WireMock (2.x, no /version)'} in ${r.latencyMs} ms` });
    } catch (e) {
      setResult({ ok: false, err: e instanceof ApiError ? e : new ApiError(String(e), 'network') });
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    const next = clean();
    const list = server ? st.servers.map((x) => (x.id === server.id ? next : x)) : [...st.servers, next];
    if (!(await st.saveServers(list))) return;
    if (next.auth && secret) {
      if (mode === 'keychain') {
        if (await st.storeSecret(id, secret)) st.setSessionSecret(id, undefined);
      } else st.setSessionSecret(id, secret);
    }
    if (!next.auth) st.setSessionSecret(id, undefined);
    if (!server) st.setActive(id);
    onDone();
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <h3 className="grow">{server ? `Edit ${server.name}` : 'Add a remote WireMock'}</h3>
        <span className="subtle xs mono">id: {id}</span>
      </div>
      <div className="panel-body col">
        <div className="grid-3" style={{ gridTemplateColumns: '1fr 2fr 130px' }}>
          <Field label="Name">
            <input className="input" autoFocus value={s.name} placeholder={hostOf(s.url)} onChange={(e) => setS({ ...s, name: e.target.value })} />
          </Field>
          <Field label="Base URL" hint="Where WireMock listens, e.g. https://wiremock.dev.example.com">
            <input className="input mono" value={s.url} onChange={(e) => setS({ ...s, url: e.target.value })} onKeyDown={(e) => e.key === 'Enter' && test()} />
          </Field>
          <Field label="Environment">
            <select className="select" value={s.env ?? 'other'} onChange={(e) => setS({ ...s, env: e.target.value as EnvKind })}>
              {(Object.keys(ENV_LABEL) as EnvKind[]).map((k) => (
                <option key={k} value={k}>
                  {ENV_LABEL[k]}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <div className="grid-2">
          <Field label="Context path" hint="Only if WireMock runs under a path, e.g. /mocks → /mocks/__admin">
            <input className="input mono" value={s.contextPath ?? ''} placeholder="(none)" onChange={(e) => setS({ ...s, contextPath: e.target.value || undefined })} />
          </Field>
          <Field label="Authentication">
            <select className="select" value={authType} onChange={(e) => (e.target.value === 'none' ? setS({ ...s, auth: undefined }) : setAuth({ type: e.target.value as AuthType }))}>
              <option value="none">None</option>
              <option value="basic">Basic (--admin-api-basic-auth)</option>
              <option value="bearer">Bearer token</option>
              <option value="header">Custom header</option>
            </select>
          </Field>
        </div>
        {authType !== 'none' && (
          <div className="fieldset" style={{ padding: 10 }}>
            <div className="col-sm">
              <div className="grid-2">
                {authType === 'basic' && (
                  <Field label="Username" hint="Stored in servers.yaml">
                    <input className="input" autoComplete="off" value={s.auth?.username ?? ''} onChange={(e) => setAuth({ username: e.target.value })} />
                  </Field>
                )}
                {authType === 'header' && (
                  <Field label="Header name" hint="Stored in servers.yaml">
                    <input className="input mono" value={s.auth?.headerName ?? ''} placeholder="X-Api-Key" onChange={(e) => setAuth({ headerName: e.target.value })} />
                  </Field>
                )}
                <Field
                  label={secretLabel}
                  hint={
                    cred === 'keychain'
                      ? 'Saved in the OS keychain — leave empty to keep it'
                      : cred === 'session'
                        ? 'Set for this session — leave empty to keep it'
                        : 'Never written to project files'
                  }
                >
                  <input className="input mono" type="password" autoComplete="new-password" value={secret} onChange={(e) => setSecret(e.target.value)} />
                </Field>
              </div>
              <div className="row wrap">
                <Segmented<CredMode>
                  value={mode}
                  onChange={setMode}
                  options={[
                    { value: 'keychain', label: 'Save in OS keychain', title: `Stored as ${secretNameFor(id)} (all environments of this project)` },
                    { value: 'session', label: 'This session only' },
                  ]}
                />
                <span className="subtle xs grow">
                  Keychain name <code className="code-inline">{secretNameFor(id)}</code>
                </span>
                {cred === 'keychain' && server && (
                  <button className="btn btn-sm btn-danger-ghost" onClick={() => st.deleteSecret(server.id)}>
                    Remove from keychain
                  </button>
                )}
              </div>
            </div>
          </div>
        )}
        <details className="fieldset">
          <summary>Advanced</summary>
          <div className="fieldset-body">
            <Field label="Extra request headers" hint="Sent with every admin call (e.g. a gateway tenant header). Stored in servers.yaml — no credentials here.">
              <StringMapEditor
                value={Object.fromEntries((s.headers ?? []).map((h) => [h.key, h.value]))}
                onChange={(v) => setS({ ...s, headers: Object.entries(v ?? {}).map(([key, value]) => ({ key, value: String(value) })) })}
              />
            </Field>
            {headerProblems.length > 0 && (
              <Callout kind="danger">
                {headerProblems.map((h) => h.key).join(', ')} look like credentials. Use Authentication → Custom header so the value goes to the keychain.
              </Callout>
            )}
            <Switch checked={!!s.protected} onChange={(v) => setS({ ...s, protected: v || undefined })} label="Protected — confirm every change (shared / staging servers)" />
            <Switch checked={!!s.readOnly} onChange={(v) => setS({ ...s, readOnly: v || undefined })} label="Read-only — never write from this app (production observation)" />
            <Switch checked={!!s.insecureTls} onChange={(v) => setS({ ...s, insecureTls: v || undefined })} label="Accept invalid TLS certificates (self-signed test environments)" />
            <Field label="Notes">
              <textarea className="textarea" rows={2} value={s.notes ?? ''} onChange={(e) => setS({ ...s, notes: e.target.value || undefined })} />
            </Field>
          </div>
        </details>
        {result?.ok === true && <Callout kind="success">{result.msg}</Callout>}
        {result?.ok === false && <ConnectionDoctor error={result.err} url={s.url} hasAuth={authType !== 'none'} />}
        <div className="row">
          <button className="btn" onClick={test} disabled={!valid || testing}>
            {testing ? <Spinner /> : <PlugZap />} Test connection
          </button>
          <span className="spacer" />
          <button className="btn btn-ghost" onClick={onDone}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!valid || headerProblems.length > 0} onClick={save}>
            {server ? 'Save' : 'Add & connect'}
          </button>
        </div>
      </div>
    </div>
  );
}

function hostOf(u: string) {
  try {
    return new URL(normalizeBaseUrl(u)).host;
  } catch {
    return u;
  }
}

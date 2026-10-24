import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { CheckCircle2, FolderOpen, KeyRound, Plug, Plus, ShieldAlert, X, XCircle } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { VarInput } from '@/components/VarInput';
import { Callout, Field, Segmented, Spinner, Switch } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import { useVariables } from '@/store/variables';
import { useWorkspace } from '@/store/workspace';
import { DB_KINDS, dbApi, type ConnectionFile, type DbKind, type HostKeyCheck, type TestOutcome, type TlsMode } from '@/lib/api/db';

export const COLORS = ['red', 'orange', 'yellow', 'green', 'teal', 'blue', 'purple', 'gray'];

const OPTION_HINTS: Record<DbKind, string[]> = {
  postgres: ['connectTimeout', 'applicationName', 'statementTimeout'],
  mysql: ['connectTimeout', 'statementTimeout'],
  sqlite: ['create'],
  redis: ['connectTimeout', 'sentinelMaster', 'sentinelPassword'],
  mongodb: ['connectTimeout', 'authSource', 'replicaSet', 'directConnection', 'readPreference'],
  sqlserver: ['connectTimeout', 'applicationName', 'statementTimeout', 'instance', 'applicationIntent', 'encrypt'],
};

function starter(kind: DbKind, name = ''): ConnectionFile {
  const port = DB_KINDS.find((k) => k.kind === kind)?.port;
  const base: ConnectionFile = { name, kind, readOnly: true };
  switch (kind) {
    case 'postgres':
      return { ...base, host: 'localhost', port, database: 'postgres', user: 'postgres', password: '{{secret.DB_PASSWORD}}' };
    case 'mysql':
      return { ...base, host: 'localhost', port, user: 'root', password: '{{secret.DB_PASSWORD}}' };
    case 'sqlite':
      return { ...base, database: 'data.db' };
    case 'sqlserver':
      return { ...base, host: 'localhost', port, database: 'master', user: 'sa', password: '{{secret.DB_PASSWORD}}' };
    default:
      return { ...base, host: 'localhost', port };
  }
}

/** Picks `{{secret.NAME}}` from the project's declared secrets, or creates a new one. */
function SecretPicker({ value, onChange, placeholder }: { value?: string; onChange(v: string | undefined): void; placeholder?: string }) {
  const project = useWorkspace((s) => s.project);
  const activeEnv = useWorkspace((s) => s.activeEnv());
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState('');
  const [secret, setSecret] = useState('');
  const [scope, setScope] = useState<string>('*');
  const names = useMemo(() => [...new Set((project?.environments ?? []).flatMap((e) => e.file.secrets ?? []))].sort(), [project]);
  const m = /^\{\{\s*secret\.([^}\s]+)\s*\}\}$/.exec(value ?? '');
  const current = m?.[1];
  const custom = !!value && !current;

  const create = async () => {
    if (!project || !name.trim()) return;
    const n = name.trim().replace(/[^A-Za-z0-9_.-]/g, '_');
    try {
      await api.setSecret(project.root, scope === '*' ? '*' : scope, n, secret);
      // Declare it on the environment(s) so it shows up on the Environments page.
      const envs = scope === '*' ? project.environments : project.environments.filter((e) => e.id === scope);
      for (const e of envs) {
        if (!(e.file.secrets ?? []).includes(n)) await api.saveEnvironment(project.root, e.id, { ...e.file, secrets: [...(e.file.secrets ?? []), n] });
      }
      await useWorkspace.getState().refresh();
      onChange(`{{secret.${n}}}`);
      setCreating(false);
      setSecret('');
      toast.success(`Secret ${n} saved in the OS keychain`, scope === '*' ? 'for every environment' : `for ${scope}`);
    } catch (e) {
      toast.error('Could not save the secret', errorMessage(e));
    }
  };

  if (creating)
    return (
      <div className="db-secret-new">
        <input className="input sm mono" value={name} onChange={(e) => setName(e.target.value)} placeholder="DB_PASSWORD" autoFocus />
        <input className="input sm" type="password" value={secret} onChange={(e) => setSecret(e.target.value)} placeholder="value (stored in the keychain)" autoComplete="new-password" />
        <select className="select sm" style={{ width: 'auto' }} value={scope} onChange={(e) => setScope(e.target.value)} title="Which environments use this value">
          <option value="*">All environments</option>
          {project?.environments.map((e) => (
            <option key={e.id} value={e.id}>
              {e.file.name}
            </option>
          ))}
        </select>
        <button className="btn btn-sm btn-primary" onClick={() => void create()} disabled={!name.trim()}>
          Save
        </button>
        <button className="icon-btn sm" onClick={() => setCreating(false)} aria-label="Cancel">
          <X />
        </button>
      </div>
    );

  return (
    <div className="row-sm">
      <KeyRound size={14} className="subtle" />
      <select
        className="select sm grow"
        value={custom ? '__custom' : (current ?? '')}
        onChange={(e) => {
          const v = e.target.value;
          if (v === '__new') {
            setScope(activeEnv?.id ?? '*');
            setName(placeholder ?? 'DB_PASSWORD');
            setCreating(true);
          } else if (v === '') onChange(undefined);
          else if (v !== '__custom') onChange(`{{secret.${v}}}`);
        }}
      >
        <option value="">No password</option>
        {current && !names.includes(current) && <option value={current}>{`{{secret.${current}}}`} (not declared)</option>}
        {names.map((n) => (
          <option key={n} value={n}>{`{{secret.${n}}}`}</option>
        ))}
        {custom && <option value="__custom">{value}</option>}
        <option value="__new">New secret…</option>
      </select>
      {activeEnv && <span className="subtle small nowrap" title="Secrets resolve with the active environment">env: {activeEnv.file.name}</span>}
    </div>
  );
}

interface Props {
  open: boolean;
  /** Existing connection path, or undefined to create. */
  path?: string;
  initialKind?: DbKind;
  onClose(): void;
  onSaved(path: string): void;
}

export function ConnectionDialog({ open, path, initialKind = 'postgres', onClose, onSaved }: Props) {
  const project = useWorkspace((s) => s.project);
  const env = useWorkspace((s) => s.activeEnv());
  const vars = useVariables('');
  const [draft, setDraft] = useState<ConnectionFile>(() => starter(initialKind));
  const [loading, setLoading] = useState(false);
  const [useUrl, setUseUrl] = useState(false);
  const [test, setTest] = useState<TestOutcome | null>(null);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [options, setOptions] = useState<[string, string][]>([]);

  const root = project?.root;
  useEffect(() => {
    if (!open || !root) return;
    setTest(null);
    if (!path) {
      setDraft(starter(initialKind));
      setUseUrl(false);
      setOptions(initialKind === 'sqlite' ? [['create', 'true']] : []);
      return;
    }
    setLoading(true);
    dbApi
      .read(root, path)
      .then((c) => {
        setDraft(c);
        setUseUrl(!!c.url);
        setOptions(Object.entries(c.options ?? {}));
      })
      .catch((e) => toast.error('Could not read the connection', errorMessage(e)))
      .finally(() => setLoading(false));
  }, [open, path, root, initialKind]);

  const set = <K extends keyof ConnectionFile>(k: K, v: ConnectionFile[K]) => setDraft((d) => ({ ...d, [k]: v }));
  const kind = draft.kind;
  const isFile = kind === 'sqlite';

  const finalDraft = (): ConnectionFile => {
    const d: ConnectionFile = { ...draft, options: Object.fromEntries(options.filter(([k]) => k.trim())) };
    if (useUrl && !isFile) {
      delete d.host;
      delete d.port;
    } else delete d.url;
    if (!d.options || !Object.keys(d.options).length) delete d.options;
    if (d.tls && !d.tls.mode && !d.tls.caCert) delete d.tls;
    return d;
  };

  const runTest = async (trust?: string) => {
    if (!project) return;
    setTesting(true);
    setTest(null);
    try {
      const r = await dbApi.test(project.root, finalDraft(), env?.id, trust);
      if (r.hostKey && !trust) {
        const ok = await confirmDialog({
          title: 'Trust this SSH host?',
          message: (
            <div className="col-sm">
              <div>
                {r.hostKey.host}:{r.hostKey.port} is not in your known_hosts. Compare the fingerprint with the server's before trusting it.
              </div>
              <code className="code-inline">
                {r.hostKey.keyType} {r.hostKey.fingerprint}
              </code>
            </div>
          ),
          confirmText: 'Trust and continue',
        });
        if (ok) return await runTest(r.hostKey.fingerprint);
      }
      setTest(r);
    } catch (e) {
      setTest({ ok: false, message: errorMessage(e), elapsedMs: 0 });
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    if (!project) return;
    const d = finalDraft();
    if (!d.name.trim()) return toast.error('Give the connection a name');
    setSaving(true);
    try {
      if (path) {
        await dbApi.save(project.root, path, d);
        onSaved(path);
      } else {
        onSaved(await dbApi.create(project.root, d.name, d.kind, d));
      }
    } catch (e) {
      toast.error('Could not save the connection', errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const browse = async (field: 'database' | 'caCert' | 'privateKey') => {
    const picked = await openDialog({ multiple: false, directory: false, defaultPath: project?.root });
    if (typeof picked !== 'string') return;
    let p = picked;
    if (project && p.toLowerCase().startsWith(project.root.toLowerCase())) p = p.slice(project.root.length).replace(/^[\\/]+/, '').replace(/\\/g, '/');
    if (field === 'database') set('database', p);
    else if (field === 'caCert') set('tls', { ...(draft.tls ?? {}), caCert: p });
    else set('ssh', { ...(draft.ssh ?? { host: '', user: '' }), privateKey: p });
  };

  const ssh = draft.ssh;
  const sshAuth = ssh?.privateKey !== undefined ? 'key' : ssh?.password !== undefined ? 'password' : 'agent';

  return (
    <Modal
      open={open}
      onClose={onClose}
      size="lg"
      title={path ? `Edit ${draft.name || 'connection'}` : 'New connection'}
      bodyClassName="db-conn-dialog"
      footer={
        <>
          <button className="btn" onClick={() => void runTest()} disabled={testing || loading}>
            {testing ? <Spinner /> : <Plug />} Test connection
          </button>
          {test && (
            <span className={clsx('small truncate grow', test.ok ? 'text-success' : 'text-danger')} title={test.message}>
              {test.ok ? <CheckCircle2 size={13} /> : <XCircle size={13} />} {test.message}
              {test.ok && ` · ${Math.round(test.elapsedMs)} ms`}
            </span>
          )}
          {!test && <span className="grow" />}
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void save()} disabled={saving || loading}>
            {path ? 'Save' : 'Create'}
          </button>
        </>
      }
    >
      {loading ? (
        <div className="empty">
          <Spinner />
        </div>
      ) : (
        <div className="col">
          <div className="grid-2">
            <Field label="Name">
              <input className="input" value={draft.name} onChange={(e) => set('name', e.target.value)} placeholder="Orders (staging)" autoFocus />
            </Field>
            <Field label="Engine">
              <select
                className="select"
                value={kind}
                onChange={(e) => {
                  const k = e.target.value as DbKind;
                  setDraft((d) => ({ ...starter(k, d.name), readOnly: d.readOnly, protected: d.protected, color: d.color, ssh: k === 'sqlite' ? undefined : d.ssh }));
                  setOptions(k === 'sqlite' ? [['create', 'true']] : []);
                }}
              >
                {DB_KINDS.map((k) => (
                  <option key={k.kind} value={k.kind}>
                    {k.label}
                  </option>
                ))}
              </select>
            </Field>
          </div>

          {!isFile && (
            <Segmented
              value={useUrl ? 'url' : 'fields'}
              onChange={(v) => setUseUrl(v === 'url')}
              options={[
                { value: 'fields', label: 'Host & port' },
                { value: 'url', label: 'Connection URL' },
              ]}
            />
          )}

          {isFile ? (
            <Field label="Database file" hint="Project-relative, absolute or ~/…  — read-only sessions never create the file.">
              <div className="row-sm">
                <VarInput value={draft.database ?? ''} onChange={(v) => set('database', v)} vars={vars} placeholder="data/app.db" />
                <button className="btn btn-sm" onClick={() => void browse('database')}>
                  <FolderOpen /> Browse
                </button>
              </div>
            </Field>
          ) : useUrl ? (
            <Field
              label="URL"
              hint={
                kind === 'mongodb'
                  ? 'mongodb:// or mongodb+srv:// — put the password as {{secret.X}}'
                  : kind === 'sqlserver'
                    ? 'sqlserver://, mssql:// or jdbc:sqlserver://host:1433;databaseName=app;encrypt=true;trustServerCertificate=true — password as {{secret.X}}'
                    : 'Templates allowed; put the password as {{secret.X}}, never inline.'
              }
            >
              <VarInput
                value={draft.url ?? ''}
                onChange={(v) => set('url', v)}
                vars={vars}
                placeholder={kind === 'sqlserver' ? 'sqlserver://sa:{{secret.DB_PASSWORD}}@{{dbHost}}:1433/app?trustServerCertificate=true' : `${kind === 'mongodb' ? 'mongodb' : kind}://user:{{secret.DB_PASSWORD}}@{{dbHost}}/app`}
              />
            </Field>
          ) : (
            <div className="db-conn-grid">
              <Field label="Host" hint={kind === 'sqlserver' ? 'host\\INSTANCE for a named instance (port then comes from SQL Browser)' : undefined}>
                <VarInput value={draft.host ?? ''} onChange={(v) => set('host', v)} vars={vars} placeholder={kind === 'sqlserver' ? 'localhost or host\\SQLEXPRESS' : 'localhost or {{dbHost}}'} />
              </Field>
              <Field label="Port">
                <input
                  className="input mono"
                  value={draft.port ?? ''}
                  onChange={(e) => set('port', /^\d+$/.test(e.target.value) ? Number(e.target.value) : e.target.value || undefined)}
                  placeholder={String(DB_KINDS.find((k) => k.kind === kind)?.port ?? '')}
                />
              </Field>
              <Field label={kind === 'redis' ? 'DB index' : 'Database'}>
                <VarInput value={draft.database ?? ''} onChange={(v) => set('database', v || undefined)} vars={vars} placeholder={kind === 'redis' ? '0' : kind === 'mysql' ? '(optional)' : kind === 'sqlserver' ? 'master' : 'app'} />
              </Field>
            </div>
          )}

          {!isFile && !useUrl && (
            <div className="grid-2">
              <Field label={kind === 'redis' ? 'User (ACL, optional)' : 'User'}>
                <VarInput value={draft.user ?? ''} onChange={(v) => set('user', v || undefined)} vars={vars} placeholder={kind === 'postgres' ? 'postgres' : kind === 'mysql' ? 'root' : kind === 'sqlserver' ? 'sa (SQL authentication)' : ''} />
              </Field>
              <Field label="Password" hint="Stored in the OS keychain — the file only holds the reference.">
                <SecretPicker value={draft.password} onChange={(v) => set('password', v)} placeholder="DB_PASSWORD" />
              </Field>
            </div>
          )}

          {!isFile && (
            <div className="grid-2">
              <Field label="TLS">
                <select className="select" value={draft.tls?.mode ?? ''} onChange={(e) => set('tls', e.target.value ? { ...(draft.tls ?? {}), mode: e.target.value as TlsMode } : draft.tls?.caCert ? { caCert: draft.tls.caCert } : undefined)}>
                  <option value="">Default ({kind === 'postgres' || kind === 'mysql' || kind === 'sqlserver' ? 'prefer' : kind === 'redis' ? 'off' : 'from URL'})</option>
                  <option value="disable">Disable</option>
                  {kind !== 'redis' && <option value="prefer">Prefer (encrypt if offered, no checks)</option>}
                  <option value="require">{kind === 'sqlserver' ? 'Require (encrypt, trust server certificate)' : 'Require (encrypt, no certificate checks)'}</option>
                  <option value="verify-full">Verify full (trusted certificate + host name)</option>
                </select>
              </Field>
              {draft.tls?.mode === 'verify-full' && (
                <Field label="CA certificate (optional)" hint="PEM; empty = the OS trust store">
                  <div className="row-sm">
                    <input className="input mono" value={draft.tls?.caCert ?? ''} onChange={(e) => set('tls', { ...(draft.tls ?? {}), caCert: e.target.value || undefined })} placeholder="certs/ca.pem" />
                    <button className="icon-btn" onClick={() => void browse('caCert')} aria-label="Browse">
                      <FolderOpen />
                    </button>
                  </div>
                </Field>
              )}
            </div>
          )}

          {!isFile && (
            <div className="fieldset db-ssh">
              <Switch checked={!!ssh} onChange={(v) => set('ssh', v ? { host: '', user: '', knownHostsCheck: 'ask' } : undefined)} label="Connect through an SSH tunnel" />
              {ssh && (
                <div className="col mt-2">
                  <div className="db-conn-grid">
                    <Field label="SSH host">
                      <VarInput value={ssh.host} onChange={(v) => set('ssh', { ...ssh, host: v })} vars={vars} placeholder="bastion.example.com" />
                    </Field>
                    <Field label="Port">
                      <input className="input mono" value={ssh.port ?? ''} onChange={(e) => set('ssh', { ...ssh, port: /^\d+$/.test(e.target.value) ? Number(e.target.value) : e.target.value || undefined })} placeholder="22" />
                    </Field>
                    <Field label="User">
                      <VarInput value={ssh.user} onChange={(v) => set('ssh', { ...ssh, user: v })} vars={vars} placeholder="deploy" />
                    </Field>
                  </div>
                  <div className="grid-2">
                    <Field label="Authentication">
                      <Segmented
                        value={sshAuth}
                        onChange={(v) => {
                          const { privateKey: _k, passphrase: _p, password: _w, ...rest } = ssh;
                          void _k;
                          void _p;
                          void _w;
                          set('ssh', v === 'key' ? { ...rest, privateKey: '~/.ssh/id_ed25519' } : v === 'password' ? { ...rest, password: '{{secret.SSH_PASSWORD}}' } : rest);
                        }}
                        options={[
                          { value: 'agent', label: 'Agent' },
                          { value: 'key', label: 'Key file' },
                          { value: 'password', label: 'Password' },
                        ]}
                      />
                    </Field>
                    <Field label="Host key check" hint="Unknown hosts are confirmed once, then added to ~/.ssh/known_hosts">
                      <select className="select" value={ssh.knownHostsCheck ?? 'ask'} onChange={(e) => set('ssh', { ...ssh, knownHostsCheck: e.target.value as HostKeyCheck })}>
                        <option value="ask">Ask for unknown hosts (recommended)</option>
                        <option value="strict">Strict (known_hosts only)</option>
                        <option value="off">Off (insecure)</option>
                      </select>
                    </Field>
                  </div>
                  {sshAuth === 'key' && (
                    <div className="grid-2">
                      <Field label="Private key (OpenSSH)">
                        <div className="row-sm">
                          <input className="input mono" value={ssh.privateKey ?? ''} onChange={(e) => set('ssh', { ...ssh, privateKey: e.target.value })} placeholder="~/.ssh/id_ed25519" />
                          <button className="icon-btn" onClick={() => void browse('privateKey')} aria-label="Browse">
                            <FolderOpen />
                          </button>
                        </div>
                      </Field>
                      <Field label="Key passphrase">
                        <SecretPicker value={ssh.passphrase} onChange={(v) => set('ssh', { ...ssh, passphrase: v })} placeholder="SSH_KEY_PASSPHRASE" />
                      </Field>
                    </div>
                  )}
                  {sshAuth === 'password' && (
                    <Field label="SSH password">
                      <SecretPicker value={ssh.password} onChange={(v) => set('ssh', { ...ssh, password: v ?? '' })} placeholder="SSH_PASSWORD" />
                    </Field>
                  )}
                  {ssh.knownHostsCheck === 'off' && (
                    <Callout kind="warning">
                      <ShieldAlert size={14} /> Host keys are not verified — anyone on the network path could impersonate the server.
                    </Callout>
                  )}
                </div>
              )}
            </div>
          )}

          <div className="grid-2">
            <div className="col-sm">
              <Switch checked={draft.readOnly} onChange={(v) => set('readOnly', v)} label="Read-only by default" title="Sessions start read-only: writes are rejected and the database session itself is read-only" />
              <Switch checked={!!draft.protected} onChange={(v) => set('protected', v || undefined)} label="Protected (typing the name is required to allow writes)" />
            </div>
            <Field label="Colour">
              <div className="row-sm db-colors">
                <button className={clsx('db-color none', !draft.color && 'on')} onClick={() => set('color', undefined)} title="No colour" aria-label="No colour" />
                {COLORS.map((c) => (
                  <button key={c} className={clsx('db-color', `c-${c}`, draft.color === c && 'on')} onClick={() => set('color', c)} title={c} aria-label={c} />
                ))}
              </div>
            </Field>
          </div>

          <details className="db-options">
            <summary className="small strong">Options {options.length ? `(${options.length})` : ''}</summary>
            <div className="col-sm mt-2">
              {options.map(([k, v], i) => (
                <div key={i} className="row-sm">
                  <input className="input sm mono" list={`db-opt-${kind}`} value={k} onChange={(e) => setOptions((o) => o.map((x, j) => (j === i ? [e.target.value, x[1]] : x)))} placeholder="name" style={{ width: 180 }} />
                  <input className="input sm mono grow" value={v} onChange={(e) => setOptions((o) => o.map((x, j) => (j === i ? [x[0], e.target.value] : x)))} placeholder="value" />
                  <button className="icon-btn sm" onClick={() => setOptions((o) => o.filter((_, j) => j !== i))} aria-label="Remove option">
                    <X />
                  </button>
                </div>
              ))}
              <datalist id={`db-opt-${kind}`}>
                {OPTION_HINTS[kind].map((o) => (
                  <option key={o} value={o} />
                ))}
              </datalist>
              <button className="btn btn-sm btn-ghost" style={{ alignSelf: 'flex-start' }} onClick={() => setOptions((o) => [...o, ['', '']])}>
                <Plus /> Option
              </button>
              <span className="subtle small">
                {OPTION_HINTS[kind].join(' · ')} — connectTimeout in seconds, statementTimeout in ms.
              </span>
            </div>
          </details>
          {kind === 'sqlserver' && (
            <Callout kind="info">
              SQL authentication (user + password). SQL Server has no read-only session flag: read-only sessions refuse writes before they are sent (EXEC and dynamic SQL count as writes) and run at READ COMMITTED.
              For an availability-group secondary, add the option <code className="code-inline">applicationIntent: ReadOnly</code>; <code className="code-inline">encrypt: strict</code> uses TDS 8.0.
            </Callout>
          )}
          {test && !test.ok && (
            <Callout kind="danger">
              <pre className="db-error">{test.message}</pre>
            </Callout>
          )}
          {test?.warnings?.map((w, i) => (
            <Callout key={i} kind="warning">
              {w}
            </Callout>
          ))}
        </div>
      )}
    </Modal>
  );
}

import { useCallback, useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { AlertTriangle, Database, DatabaseZap, Dices, FolderOpen, History, Lock, LockOpen, Network, Pencil, Plug, PlugZap, Plus, RotateCw, Shield, Trash2, Unplug } from 'lucide-react';
import { openLocalHistory } from '@/features/history/LocalHistoryDialog';
import { Menu, useContextMenu } from '@/components/ui/Menu';
import { Callout, Empty, Kbd, Spinner } from '@/components/ui/misc';
import { SidePane } from '@/components/layout/SidePane';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { useNav, useViewLabel } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { DB_KINDS, dbApi, type ConnectionEntry, type DbKind, type SavedQueryEntry, type SessionInfo } from '@/lib/api/db';
import { connKey, useDb } from './store';
import { ConnectionDialog } from './ConnectionDialog';
import { SqlWorkspace } from './SqlWorkspace';
import { RedisWorkspace } from './RedisWorkspace';
import { MongoWorkspace } from './MongoWorkspace';
import { SavedQueries } from './SavedQueries';
import { SeedDialog } from './SeedDialog';
import { FromDatabaseDialog, type FromDatabaseTarget } from '@/features/data/FromDatabaseDialog';
import './db.css';

const KIND_LABEL: Record<DbKind, string> = { postgres: 'PG', mysql: 'MY', sqlite: 'LITE', redis: 'RDS', mongodb: 'MGO', sqlserver: 'MS' };

function ColorChip({ color }: { color?: string }) {
  return <span className={clsx('db-chip', color ? `c-${color}` : 'none')} />;
}

type OpenReq = { path: string; text: string; nonce: number };

/** Database clients: Postgres, MySQL/MariaDB, SQLite, SQL Server, Redis and MongoDB. Connections live in
 * `connections/*.conn.yaml`, saved queries in `queries/`. */
export default function DbPage() {
  const project = useWorkspace((s) => s.project);
  const env = useWorkspace((s) => s.activeEnv());
  const param = useNav((s) => s.param);
  const go = useNav((s) => s.go);
  const sessions = useDb((s) => s.sessions);
  const sessionEnv = useDb((s) => s.sessionEnv);
  const connecting = useDb((s) => s.connecting);
  const [conns, setConns] = useState<ConnectionEntry[] | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [dialog, setDialog] = useState<{ path?: string; kind?: DbKind } | null>(null);
  const [seed, setSeed] = useState<{ schema?: string; table?: string } | null>(null);
  const [fakeTarget, setFakeTarget] = useState<FromDatabaseTarget | null>(null);
  const [openReq, setOpenReq] = useState<Record<string, OpenReq>>({});
  const ctx = useContextMenu();
  const root = project?.root;

  const load = useCallback(async () => {
    if (!root) return;
    try {
      setConns(await dbApi.list(root));
    } catch (e) {
      setConns([]);
      toast.error('Could not list connections', errorMessage(e));
    }
  }, [root]);

  useEffect(() => {
    setConns(null);
    void load();
    if (root) void useDb.getState().syncSessions(root);
  }, [load, root]);

  useEffect(() => {
    const on = () => void load();
    window.addEventListener('focus', on);
    return () => window.removeEventListener('focus', on);
  }, [load]);

  const selected = useMemo(() => (param && conns?.some((c) => c.path === param) ? conns.find((c) => c.path === param) : conns?.find((c) => !c.error)), [param, conns]);
  useViewLabel(conns ? (selected?.name ?? null) : undefined);
  const key = root && selected ? connKey(root, selected.path) : '';
  const session: SessionInfo | undefined = key ? sessions[key] : undefined;

  const connect = useCallback(
    async (c: ConnectionEntry, trust?: string) => {
      if (!root) return;
      const k = connKey(root, c.path);
      useDb.getState().setConnecting(k, true);
      setErrors((e) => ({ ...e, [k]: '' }));
      try {
        const out = await dbApi.connect(root, c.path, env?.id, trust);
        if (out.type === 'hostKey') {
          const ok = await confirmDialog({
            title: 'Trust this SSH host?',
            message: (
              <div className="col-sm">
                <div>
                  {out.host}:{out.port} is not in your known_hosts. Compare the fingerprint with the one your server administrator gives you before trusting it.
                </div>
                <code className="code-inline">
                  {out.keyType} {out.fingerprint}
                </code>
              </div>
            ),
            confirmText: 'Trust and connect',
          });
          if (ok) return await connect(c, out.fingerprint);
          return;
        }
        useDb.getState().setSession(k, out.session, env?.id);
        for (const w of out.session.warnings ?? []) toast.info('Heads up', w);
      } catch (e) {
        setErrors((x) => ({ ...x, [k]: errorMessage(e) }));
      } finally {
        useDb.getState().setConnecting(k, false);
      }
    },
    [root, env?.id],
  );

  const disconnect = async (c: ConnectionEntry) => {
    if (!root) return;
    const k = connKey(root, c.path);
    const s = useDb.getState().sessions[k];
    if (s) await dbApi.disconnect(s.id).catch(() => false);
    useDb.getState().setSession(k, undefined);
  };

  const requestWrite = useCallback(async () => {
    if (!session || !key) return;
    const ok = await confirmDialog({
      title: `Allow writes to “${session.name}”?`,
      message: session.protected
        ? 'This connection is protected (production-like). Statements that modify data will run for real.'
        : 'Statements that modify data (INSERT, UPDATE, DELETE, DDL, Redis writes, Mongo writes) will run. You can switch back any time.',
      confirmText: 'Allow writes',
      danger: true,
      typeToConfirm: session.protected ? session.name : undefined,
    });
    if (!ok) return;
    try {
      const info = await dbApi.setReadOnly(session.id, false, session.protected ? session.name : undefined);
      useDb.getState().setSession(key, info, useDb.getState().sessionEnv[key]);
    } catch (e) {
      toast.error('Could not switch to read-write', errorMessage(e));
    }
  }, [session, key]);

  const makeReadOnly = async () => {
    if (!session || !key) return;
    try {
      const info = await dbApi.setReadOnly(session.id, true);
      useDb.getState().setSession(key, info, useDb.getState().sessionEnv[key]);
    } catch (e) {
      toast.error('Could not switch to read-only', errorMessage(e));
    }
  };

  const remove = async (c: ConnectionEntry) => {
    if (!root) return;
    if (!(await confirmDialog({ title: `Delete “${c.name}”?`, message: `connections/${c.path} moves to your system trash. Saved queries are kept.`, confirmText: 'Move to trash', danger: true }))) return;
    try {
      await disconnect(c);
      await dbApi.remove(root, c.path);
      if (c.path === selected?.path) go('data');
      await load();
    } catch (e) {
      toast.error('Delete failed', errorMessage(e));
    }
  };

  const openSaved = async (q: Pick<SavedQueryEntry, 'path' | 'kind'>) => {
    if (!root || !conns) return;
    try {
      const full = await dbApi.readQuery(root, q.path);
      const conn = (full.connection && conns.find((c) => c.slug === full.connection)) || selected;
      if (!conn) return toast.error('No connection for this query', full.connection ? `connections/${full.connection}.conn.yaml was not found` : 'Select a connection first');
      const want = conn.kind === 'redis' ? 'redis' : conn.kind === 'mongodb' ? 'mongo' : 'sql';
      if (want !== q.kind) return toast.error(`This is a ${q.kind} query`, `${conn.name} is a ${conn.kind} connection`);
      go('data', conn.path);
      const k = connKey(root, conn.path);
      setOpenReq((r) => ({ ...r, [k]: { path: q.path, text: full.text, nonce: Date.now() } }));
      if (!useDb.getState().sessions[k]) void connect(conn);
    } catch (e) {
      toast.error('Could not open the query', errorMessage(e));
    }
  };

  // Deep link `query:<path>` (e.g. from a note) opens a saved query on its connection.
  useEffect(() => {
    if (!root || !conns || !param?.startsWith('query:')) return;
    const path = param.slice('query:'.length);
    dbApi
      .readQuery(root, path)
      .then((q) => openSaved({ path, kind: q.kind }))
      .catch((e) => toast.error('Could not open the query', errorMessage(e)));
    // openSaved changes every render; the param and loaded connections drive this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root, conns, param]);

  if (!project || !root)
    return (
      <Empty
        icon={<Database />}
        title="Databases need a project"
        action={
          <button className="btn btn-sm btn-primary" onClick={() => go('projects')}>
            Open a project
          </button>
        }
      >
        Connections are saved in the project's <code className="code-inline">connections/</code> folder (passwords stay in your OS keychain) so the team shares them through git.
      </Empty>
    );

  const newMenu = DB_KINDS.map((k) => ({ label: k.label, onSelect: () => setDialog({ kind: k.kind }) }));
  const envMismatch = session && (sessionEnv[key] ?? '') !== (env?.id ?? '');

  return (
    <div className="split">
      <SidePane
        title="Connections"
        className="db-side"
        actions={
          <Menu
            align="right"
            trigger={(p) => (
              <button className="icon-btn" {...p} title="New connection" aria-label="New connection">
                <Plus />
              </button>
            )}
            items={newMenu}
          />
        }
      >
        <div className="list db-conns">
          {conns === null ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : conns.length === 0 ? (
            <Empty title="No connections" action={<Menu trigger={(p) => <button className="btn btn-sm btn-primary" {...p}><Plus /> New connection</button>} items={newMenu} />}>
              Postgres, MySQL/MariaDB, SQLite, Redis and MongoDB — read-only by default.
            </Empty>
          ) : (
            conns.map((c) => {
              const k = connKey(root, c.path);
              const s = sessions[k];
              const busy = connecting[k];
              return (
                <div
                  key={c.path}
                  className={clsx('list-item db-conn', c.path === selected?.path && 'on')}
                  onClick={() => go('data', c.path)}
                  onDoubleClick={() => !s && !c.error && void connect(c)}
                  tabIndex={0}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      go('data', c.path);
                      if (!s && !c.error) void connect(c);
                    }
                  }}
                  onContextMenu={(e) =>
                    ctx.open(e, [
                      s ? { label: 'Disconnect', icon: <Unplug />, onSelect: () => void disconnect(c) } : { label: 'Connect', icon: <Plug />, disabled: !!c.error, onSelect: () => void connect(c) },
                      { label: 'Edit…', icon: <Pencil />, onSelect: () => setDialog({ path: c.path }) },
                      { label: 'Reveal file', icon: <FolderOpen />, onSelect: () => void revealItemInDir(`${root}/connections/${c.path}`).catch((err) => toast.error('Could not reveal', errorMessage(err))) },
                      { label: 'Local history…', icon: <History />, onSelect: () => openLocalHistory({ root, path: `connections/${c.path}`, title: c.name }) },
                      { separator: true },
                      { label: 'Delete', icon: <Trash2 />, danger: true, onSelect: () => void remove(c) },
                    ])
                  }
                  title={c.error ?? `${c.target}${c.ssh ? ' (via SSH)' : ''} — connections/${c.path}`}
                >
                  {c.error ? <AlertTriangle size={14} className="text-warning" /> : <ColorChip color={c.color} />}
                  <div className="grow" style={{ minWidth: 0 }}>
                    <div className="row-sm" style={{ gap: 4 }}>
                      <span className="truncate">{c.name}</span>
                      {(s ? s.readOnly : c.readOnly) && <span className="badge db-ro" title="Read-only">RO</span>}
                      {s && !s.readOnly && <span className="badge danger" title="Writes allowed">RW</span>}
                      {c.protected && (
                        <span className="badge warning" title="Protected: allowing writes requires typing the connection name">
                          PROT
                        </span>
                      )}
                    </div>
                    <div className="meta truncate">
                      {c.kind ? KIND_LABEL[c.kind] : '?'} · {c.error ? 'cannot be parsed' : c.target}
                      {c.ssh ? ' · ssh' : ''}
                    </div>
                  </div>
                  {busy ? <Spinner /> : s ? <span className="dot ok" title={`Connected · ${s.serverVersion}`} /> : null}
                </div>
              );
            })
          )}
        </div>
        <SavedQueries root={root} selected={selected} onOpen={(q) => void openSaved(q)} />
      </SidePane>
      <div className="pane fill db-main">
        {!selected ? (
          conns?.some((c) => c.error) ? (
            <Empty icon={<AlertTriangle />} title="Connections need fixing">
              {conns.find((c) => c.error)?.error}
            </Empty>
          ) : (
            <Empty icon={<Database />} title="No connection selected">
              Create a connection — it's saved to <code className="code-inline">connections/</code> with secrets referenced as <code className="code-inline">{'{{secret.DB_PASSWORD}}'}</code>.
            </Empty>
          )
        ) : (
          <>
            <div className={clsx('page-header db-header', session && !session.readOnly && 'db-writable')}>
              <ColorChip color={selected.color} />
              <h1 className="truncate">{selected.name}</h1>
              {session ? (
                <>
                  <span className="subtle small truncate" title={session.target}>
                    {session.serverVersion} · {session.target}
                  </span>
                  {session.tunnel && (
                    <span className="badge info" title={`SSH tunnel via ${session.tunnel}`}>
                      <Network size={11} /> SSH
                    </span>
                  )}
                  {envMismatch && (
                    <button className="badge warning" onClick={() => void disconnect(selected).then(() => connect(selected))} title="Connected with another environment — click to reconnect with the active one">
                      <RotateCw size={11} /> env: {sessionEnv[key] || 'none'}
                    </button>
                  )}
                  <span className="grow" />
                  {session.readOnly ? (
                    <button className="btn btn-sm db-ro-toggle" onClick={() => void requestWrite()} title="Read-only: writes are rejected. Click to allow writes.">
                      <Lock /> Read-only
                    </button>
                  ) : (
                    <button className="btn btn-sm btn-danger db-ro-toggle" onClick={() => void makeReadOnly()} title="Writes are allowed. Click to go back to read-only.">
                      <LockOpen /> Writes allowed
                    </button>
                  )}
                  {session.kind !== 'redis' && (
                    <button className="icon-btn" onClick={() => setFakeTarget({ session: session.id })} title="Create a fake-data schema from this database's tables…" aria-label="Create fake-data schema">
                      <DatabaseZap />
                    </button>
                  )}
                  <button className="btn btn-sm btn-ghost" onClick={() => setSeed({})} title="Generate rows with the fake-data module and insert them">
                    <Dices /> Seed…
                  </button>
                  <button className="icon-btn" onClick={() => setDialog({ path: selected.path })} title="Edit connection" aria-label="Edit connection">
                    <Pencil />
                  </button>
                  <button className="icon-btn" onClick={() => void disconnect(selected)} title="Disconnect" aria-label="Disconnect">
                    <Unplug />
                  </button>
                </>
              ) : (
                <>
                  <span className="subtle small truncate">{selected.target}</span>
                  <span className="grow" />
                  <button className="icon-btn" onClick={() => setDialog({ path: selected.path })} title="Edit connection" aria-label="Edit connection">
                    <Pencil />
                  </button>
                </>
              )}
            </div>
            {session ? (
              <div className="db-workspace" key={session.id}>
                {session.kind === 'redis' ? (
                  <RedisWorkspace session={session} onRequestWrite={() => void requestWrite()} openRequest={openReq[key]} />
                ) : session.kind === 'mongodb' ? (
                  <MongoWorkspace
                    root={root}
                    slug={selected.slug}
                    session={session}
                    onRequestWrite={() => void requestWrite()}
                    onFakeSchema={(database, collections) => setFakeTarget({ session: session.id, database, collections })}
                    openRequest={openReq[key]}
                  />
                ) : (
                  <SqlWorkspace root={root} connKey={key} slug={selected.slug} session={session} onRequestWrite={() => void requestWrite()} onSeed={(t) => setSeed(t)}
                    onFakeSchema={(tables) => setFakeTarget({ session: session.id, tables })}
                    openRequest={openReq[key]}
                  />
                )}
              </div>
            ) : (
              <div className="db-connect">
                <div className="card db-connect-card">
                  <div className="row-sm">
                    <ColorChip color={selected.color} />
                    <span className="strong">{selected.name}</span>
                    {selected.readOnly && <span className="badge db-ro">READ-ONLY</span>}
                    {selected.protected && (
                      <span className="badge warning">
                        <Shield size={11} /> PROTECTED
                      </span>
                    )}
                  </div>
                  <dl className="kv mt-4">
                    <dt>Engine</dt>
                    <dd>{DB_KINDS.find((k) => k.kind === selected.kind)?.label ?? '?'}</dd>
                    <dt>Target</dt>
                    <dd className="mono">{selected.target || '—'}</dd>
                    <dt>Via</dt>
                    <dd>{selected.ssh ? 'SSH tunnel' : 'direct'}</dd>
                    <dt>Environment</dt>
                    <dd>{env?.file.name ?? 'none (project variables only)'}</dd>
                  </dl>
                  {errors[key] && (
                    <Callout kind="danger" className="mt-4">
                      <pre className="db-error">{errors[key]}</pre>
                    </Callout>
                  )}
                  <div className="row-sm mt-4">
                    <button className="btn btn-primary" onClick={() => void connect(selected)} disabled={!!connecting[key] || !!selected.error} autoFocus>
                      {connecting[key] ? <Spinner /> : <PlugZap />} Connect
                    </button>
                    <button className="btn" onClick={() => setDialog({ path: selected.path })}>
                      <Pencil /> Edit
                    </button>
                    <span className="subtle small">
                      <Kbd>Enter</Kbd> on a connection connects
                    </span>
                  </div>
                </div>
              </div>
            )}
          </>
        )}
      </div>
      {ctx.node}
      <ConnectionDialog
        open={!!dialog}
        path={dialog?.path}
        initialKind={dialog?.kind}
        onClose={() => setDialog(null)}
        onSaved={(path) => {
          setDialog(null);
          void load();
          go('data', path);
          // Settings changed: the next connect uses them.
          const k = connKey(root, path);
          const s = useDb.getState().sessions[k];
          if (s) toast.info('Saved', 'Reconnect to apply the new settings');
        }}
      />
      {fakeTarget && <FromDatabaseDialog open root={root} target={fakeTarget} onClose={() => setFakeTarget(null)} />}
      {session && seed && (
        <SeedDialog
          open
          root={root}
          session={session}
          target={seed}
          onClose={() => setSeed(null)}
          onRequestWrite={() => void requestWrite()}
        />
      )}
    </div>
  );
}

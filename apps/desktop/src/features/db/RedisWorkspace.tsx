import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { Clock, KeyRound, Pencil, Play, Plus, Radio, RefreshCw, Save, Terminal, Trash2, X } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Modal } from '@/components/ui/Modal';
import { Callout, CopyButton, Empty, Field, Resizer, Segmented, Spinner, formatBytes } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { uid } from '@/lib/id';
import { errorMessage } from '@/lib/ipc';
import { dbApi, type CommandReply, type InfoSection, type KeyInfo, type KeyValue, type PubSubMessage, type RedisEdit, type RedisKey, type SessionInfo, type SlowlogEntry } from '@/lib/api/db';
import { fmtMs, fmtTtl, hexDump, tryPrettyJson } from './format';

const TYPES = ['string', 'hash', 'list', 'set', 'zset', 'stream', 'ReJSON-RL'];
const TYPE_LABEL: Record<string, string> = { string: 'STR', hash: 'HASH', list: 'LIST', set: 'SET', zset: 'ZSET', stream: 'STRM', 'ReJSON-RL': 'JSON' };
const PAGE = 200;

const keyRef = (k: KeyInfo): RedisKey => (k.keyB64 ? { b64: k.keyB64 } : k.key);

function TypeBadge({ type }: { type: string }) {
  return <span className={clsx('badge db-rtype', `rt-${type.replace(/[^a-z]/gi, '').toLowerCase()}`)}>{TYPE_LABEL[type] ?? type.slice(0, 5).toUpperCase()}</span>;
}

interface Props {
  session: SessionInfo;
  onRequestWrite(): void;
  /** Saved `.redis` script to open in the console. */
  openRequest?: { path: string; text: string; nonce: number };
}

export function RedisWorkspace({ session, onRequestWrite, openRequest }: Props) {
  const [sideW, setSideW] = useState(320);
  const [pattern, setPattern] = useState('*');
  const [typeFilter, setTypeFilter] = useState('');
  const [keys, setKeys] = useState<KeyInfo[]>([]);
  const [cursor, setCursor] = useState('0');
  const [done, setDone] = useState(false);
  const [dbSize, setDbSize] = useState(0);
  const [scanning, setScanning] = useState(false);
  const [scanError, setScanError] = useState<string>();
  const [selected, setSelected] = useState<KeyInfo | null>(null);
  const [tab, setTab] = useState<'value' | 'console' | 'pubsub' | 'info' | 'slowlog'>('value');
  const [creating, setCreating] = useState(false);
  const ro = session.readOnly;

  const scan = useCallback(
    async (reset: boolean) => {
      setScanning(true);
      setScanError(undefined);
      try {
        const page = await dbApi.redisScan(session.id, { cursor: reset ? '0' : cursor, pattern, typeFilter: typeFilter || undefined, count: 500 });
        setKeys((k) => {
          const merged = reset ? page.keys : k.concat(page.keys);
          const seen = new Set<string>();
          return merged.filter((x) => (seen.has(x.key) ? false : (seen.add(x.key), true)));
        });
        setCursor(page.cursor);
        setDone(page.done);
        setDbSize(page.dbSize);
      } catch (e) {
        setScanError(errorMessage(e));
      } finally {
        setScanning(false);
      }
    },
    [session.id, cursor, pattern, typeFilter],
  );

  useEffect(() => {
    void scan(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session.id, typeFilter]);

  useEffect(() => {
    if (openRequest) setTab('console');
  }, [openRequest?.nonce]); // eslint-disable-line react-hooks/exhaustive-deps

  const guard = (fn: () => void) => () => (ro ? onRequestWrite() : fn());

  const deleteKeys = async (ks: KeyInfo[]) => {
    if (!(await confirmDialog({ title: ks.length === 1 ? `Delete “${ks[0].key}”?` : `Delete ${ks.length} keys?`, message: 'This cannot be undone.', confirmText: 'Delete', danger: true }))) return;
    try {
      await dbApi.redisDelete(session.id, ks.map(keyRef));
      setKeys((all) => all.filter((k) => !ks.some((x) => x.key === k.key)));
      if (selected && ks.some((x) => x.key === selected.key)) setSelected(null);
    } catch (e) {
      toast.error('Delete failed', errorMessage(e));
    }
  };

  return (
    <div className="split">
      <div className="pane side" style={{ width: sideW }}>
        <div className="toolbar">
          <input
            className="input sm mono grow"
            value={pattern}
            onChange={(e) => setPattern(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && void scan(true)}
            placeholder="Pattern, e.g. user:*"
            title="SCAN MATCH pattern — Enter to search"
            style={{ width: 0 }}
          />
          <select className="select sm" style={{ width: 'auto' }} value={typeFilter} onChange={(e) => setTypeFilter(e.target.value)} title="Type">
            <option value="">All types</option>
            {TYPES.map((t) => (
              <option key={t} value={t}>
                {TYPE_LABEL[t]}
              </option>
            ))}
          </select>
          <button className="icon-btn sm" onClick={() => void scan(true)} title="Scan again" aria-label="Scan again" disabled={scanning}>
            {scanning ? <Spinner /> : <RefreshCw />}
          </button>
          <button className="icon-btn sm" onClick={guard(() => setCreating(true))} title={ro ? 'Read-only' : 'New key'} aria-label="New key">
            <Plus />
          </button>
        </div>
        <div className="list db-keys">
          {scanError && <Callout kind="danger" className="m-3">{scanError}</Callout>}
          {keys.length === 0 && !scanning && !scanError && <Empty icon={<KeyRound />} title="No keys">{pattern !== '*' ? 'Nothing matches this pattern (yet — SCAN is incremental).' : 'This database is empty.'}</Empty>}
          {keys.map((k) => (
            <div
              key={k.key}
              className={clsx('list-item db-key', selected?.key === k.key && 'on')}
              onClick={() => {
                setSelected(k);
                setTab('value');
              }}
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.key === 'Enter') setSelected(k);
                if (e.key === 'Delete' && !ro) void deleteKeys([k]);
              }}
              title={k.key}
            >
              <TypeBadge type={k.type} />
              <span className="mono small truncate grow">{k.key}</span>
              {k.ttlMs >= 0 && (
                <span className="subtle small nowrap" title="Time to live">
                  <Clock size={10} /> {fmtTtl(k.ttlMs)}
                </span>
              )}
            </div>
          ))}
          {!done && keys.length > 0 && (
            <div className="p-2">
              <button className="btn btn-sm w-full" onClick={() => void scan(false)} disabled={scanning}>
                Scan more
              </button>
            </div>
          )}
        </div>
        <div className="statusbar small subtle db-key-status">
          {keys.length.toLocaleString()} loaded{done ? '' : '+'} · {dbSize.toLocaleString()} keys in db
        </div>
      </div>
      <Resizer width={sideW} onChange={setSideW} min={220} max={640} />
      <div className="pane fill">
        <div className="subtabs">
          {(
            [
              ['value', 'Value', <KeyRound key="v" size={13} />],
              ['console', 'Console', <Terminal key="c" size={13} />],
              ['pubsub', 'Pub/Sub', <Radio key="p" size={13} />],
              ['info', 'Info', null],
              ['slowlog', 'Slowlog', null],
            ] as const
          ).map(([id, label, icon]) => (
            <button key={id} className={clsx('subtab', tab === id && 'on')} onClick={() => setTab(id)}>
              {icon} {label}
            </button>
          ))}
        </div>
        <div className="db-redis-body">
          {tab === 'value' &&
            (selected ? (
              <KeyView
                key={selected.key}
                session={session}
                info={selected}
                onRequestWrite={onRequestWrite}
                onDeleted={() => {
                  setKeys((all) => all.filter((k) => k.key !== selected.key));
                  setSelected(null);
                }}
                onRenamed={(newKey) => {
                  const next = { ...selected, key: newKey, keyB64: undefined };
                  setKeys((all) => all.map((k) => (k.key === selected.key ? next : k)));
                  setSelected(next);
                }}
                onDelete={() => void deleteKeys([selected])}
              />
            ) : (
              <Empty icon={<KeyRound />} title="Select a key">
                Keys are listed with SCAN (never KEYS), so even huge databases stay responsive.
              </Empty>
            ))}
          {tab === 'console' && <Console session={session} openRequest={openRequest} onRequestWrite={onRequestWrite} />}
          {tab === 'pubsub' && <PubSub session={session} onRequestWrite={onRequestWrite} />}
          {tab === 'info' && <InfoView session={session} />}
          {tab === 'slowlog' && <SlowlogView session={session} />}
        </div>
      </div>
      <NewKeyDialog
        open={creating}
        session={session}
        onClose={() => setCreating(false)}
        onCreated={(k) => {
          setCreating(false);
          const info: KeyInfo = { key: k.key, type: k.type, ttlMs: -1 };
          setKeys((all) => [info, ...all.filter((x) => x.key !== k.key)]);
          setSelected(info);
          setTab('value');
        }}
      />
    </div>
  );
}

// ---------- value view ----------

function KeyView({ session, info, onRequestWrite, onDeleted, onRenamed, onDelete }: { session: SessionInfo; info: KeyInfo; onRequestWrite(): void; onDeleted(): void; onRenamed(k: string): void; onDelete(): void }) {
  const [kv, setKv] = useState<KeyValue | null>(null);
  const [error, setError] = useState<string>();
  const [memory, setMemory] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const ro = session.readOnly;
  const ref = keyRef(info);

  const load = useCallback(
    async (page: { cursor?: string; offset?: number } = {}, append = false) => {
      setError(undefined);
      try {
        const v = await dbApi.redisGet(session.id, ref, { ...page, count: PAGE });
        setKv((old) => {
          if (!append || !old || old.value.type !== v.value.type) return v;
          const a = old.value;
          const b = v.value;
          if (a.type === 'hash' && b.type === 'hash') return { ...v, value: { ...b, fields: a.fields.concat(b.fields) } };
          if (a.type === 'set' && b.type === 'set') return { ...v, value: { ...b, members: a.members.concat(b.members) } };
          if (a.type === 'list' && b.type === 'list') return { ...v, value: { ...b, items: a.items.concat(b.items), offset: a.offset } };
          if (a.type === 'zset' && b.type === 'zset') return { ...v, value: { ...b, members: a.members.concat(b.members), offset: a.offset } };
          if (a.type === 'stream' && b.type === 'stream') return { ...v, value: { ...b, entries: a.entries.concat(b.entries) } };
          return v;
        });
      } catch (e) {
        setError(errorMessage(e));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [session.id, info.key, info.keyB64],
  );

  useEffect(() => {
    void load();
    dbApi.redisMemory(session.id, ref).then(setMemory, () => setMemory(null));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load]);

  const edit = async (e: RedisEdit, msg?: string) => {
    if (ro) return onRequestWrite();
    setBusy(true);
    try {
      await dbApi.redisSet(session.id, e);
      if (msg) toast.success(msg);
      await load();
    } catch (err) {
      toast.error('Redis rejected the change', errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const setTtl = async () => {
    const v = await promptDialog({ title: 'Time to live', label: 'Seconds (empty = no expiry)', initial: kv && kv.ttlMs > 0 ? String(Math.round(kv.ttlMs / 1000)) : '', validate: (s) => (s.trim() === '' || /^\d+$/.test(s.trim()) ? undefined : 'A whole number of seconds') });
    if (v === null) return;
    await edit({ type: 'expire', key: ref, ttlMs: v.trim() ? Number(v) * 1000 : null }, v.trim() ? 'TTL set' : 'Expiry removed');
  };

  const rename = async () => {
    const v = await promptDialog({ title: 'Rename key', label: 'New name', initial: info.key, validate: (s) => (s.trim() ? undefined : 'Required') });
    if (!v || v === info.key) return;
    if (ro) return onRequestWrite();
    try {
      await dbApi.redisSet(session.id, { type: 'rename', key: ref, newKey: v });
      onRenamed(v);
    } catch (e) {
      toast.error('Rename failed', errorMessage(e));
    }
  };

  const value = kv?.value;
  return (
    <div className="db-keyview">
      <div className="toolbar">
        {kv && <TypeBadge type={kv.kind} />}
        <span className="mono strong truncate" title={info.key}>
          {info.key}
        </span>
        <CopyButton text={info.key} title="Copy key" />
        <span className="grow" />
        {kv && (
          <button className="btn btn-sm btn-ghost" onClick={() => void setTtl()} title="Set or remove the TTL">
            <Clock /> {fmtTtl(kv.ttlMs)}
          </button>
        )}
        {memory !== null && <span className="subtle small" title="MEMORY USAGE">{formatBytes(memory)}</span>}
        <button className="icon-btn sm" onClick={() => void load()} title="Reload" aria-label="Reload">
          <RefreshCw />
        </button>
        <button className="icon-btn sm" onClick={() => void rename()} title="Rename" aria-label="Rename">
          <Pencil />
        </button>
        <button className="icon-btn sm danger" onClick={ro ? onRequestWrite : onDelete} title="Delete key" aria-label="Delete key">
          <Trash2 />
        </button>
      </div>
      {error && <Callout kind="danger" className="m-3">{error}</Callout>}
      {!value && !error && (
        <div className="empty">
          <Spinner />
        </div>
      )}
      {value?.type === 'none' && (
        <Empty title="The key no longer exists">
          <button className="btn btn-sm" onClick={onDeleted}>
            Remove from list
          </button>
        </Empty>
      )}
      {value?.type === 'string' && <StringValue value={value} ro={ro} busy={busy} onSave={(v) => void edit({ type: 'setString', key: ref, value: v }, 'Saved')} />}
      {value?.type === 'json' && <JsonValue value={value.value} ro={ro} busy={busy} onSave={(v) => void edit({ type: 'jsonSet', key: ref, value: v }, 'Saved')} />}
      {value?.type === 'hash' && (
        <PairTable
          title={`${value.len.toLocaleString()} fields`}
          head={['Field', 'Value']}
          rows={value.fields.map((f) => [f.field, f.value])}
          ro={ro}
          onEdit={(row, v) => void edit({ type: 'hashSet', key: ref, field: row[0], value: v })}
          onDelete={(row) => void edit({ type: 'hashDelete', key: ref, field: row[0] })}
          onAdd={async () => {
            const f = await promptDialog({ title: 'Add field', label: 'Field', validate: (s) => (s ? undefined : 'Required') });
            if (f === null) return;
            const v = await promptDialog({ title: `Value of ${f}`, label: 'Value' });
            if (v !== null) void edit({ type: 'hashSet', key: ref, field: f, value: v });
          }}
          more={value.cursor !== '0'}
          onMore={() => void load({ cursor: value.cursor }, true)}
          editCol={1}
        />
      )}
      {value?.type === 'list' && (
        <PairTable
          title={`${value.len.toLocaleString()} items`}
          head={['Index', 'Value']}
          rows={value.items.map((v, i) => [String(value.offset + i), v])}
          ro={ro}
          onEdit={(row, v) => void edit({ type: 'listSet', key: ref, index: Number(row[0]), value: v })}
          onDelete={(row) => void edit({ type: 'listRemove', key: ref, value: row[1], count: 1 })}
          onAdd={async () => {
            const v = await promptDialog({ title: 'Push to the tail', label: 'Value' });
            if (v !== null) void edit({ type: 'listPush', key: ref, value: v });
          }}
          more={value.offset + value.items.length < value.len}
          onMore={() => void load({ offset: value.offset + value.items.length }, true)}
          editCol={1}
        />
      )}
      {value?.type === 'set' && (
        <PairTable
          title={`${value.len.toLocaleString()} members`}
          head={['Member']}
          rows={value.members.map((m) => [m])}
          ro={ro}
          onDelete={(row) => void edit({ type: 'setRemove', key: ref, member: row[0] })}
          onAdd={async () => {
            const v = await promptDialog({ title: 'Add member', label: 'Member' });
            if (v !== null) void edit({ type: 'setAdd', key: ref, member: v });
          }}
          more={value.cursor !== '0'}
          onMore={() => void load({ cursor: value.cursor }, true)}
        />
      )}
      {value?.type === 'zset' && (
        <PairTable
          title={`${value.len.toLocaleString()} members`}
          head={['Member', 'Score']}
          rows={value.members.map((m) => [m.member, String(m.score)])}
          ro={ro}
          editCol={1}
          onEdit={(row, v) => (Number.isFinite(Number(v)) ? void edit({ type: 'zAdd', key: ref, member: row[0], score: Number(v) }) : toast.error('The score must be a number'))}
          onDelete={(row) => void edit({ type: 'zRemove', key: ref, member: row[0] })}
          onAdd={async () => {
            const m = await promptDialog({ title: 'Add member', label: 'Member' });
            if (m === null) return;
            const s = await promptDialog({ title: `Score of ${m}`, label: 'Score', initial: '0', validate: (x) => (Number.isFinite(Number(x)) ? undefined : 'A number') });
            if (s !== null) void edit({ type: 'zAdd', key: ref, member: m, score: Number(s) });
          }}
          more={value.offset + value.members.length < value.len}
          onMore={() => void load({ offset: value.offset + value.members.length }, true)}
        />
      )}
      {value?.type === 'stream' && (
        <PairTable
          title={`${value.len.toLocaleString()} entries (newest first)`}
          head={['ID', 'Fields']}
          rows={value.entries.map((e) => [e.id, e.fields.map((f) => `${f.field}=${f.value}`).join('  ')])}
          ro={ro}
          onDelete={(row) => void edit({ type: 'streamDelete', key: ref, id: row[0] })}
          onAdd={async () => {
            const t = await promptDialog({ title: 'Add entry (XADD *)', label: 'Fields as JSON object', initial: '{"field": "value"}', validate: (s) => {
              try {
                const o = JSON.parse(s);
                return o && typeof o === 'object' && !Array.isArray(o) && Object.keys(o).length ? undefined : 'An object with at least one field';
              } catch {
                return 'Invalid JSON';
              }
            } });
            if (t === null) return;
            const o = JSON.parse(t) as Record<string, unknown>;
            void edit({ type: 'streamAdd', key: ref, fields: Object.entries(o).map(([k, v]) => [k, typeof v === 'string' ? v : JSON.stringify(v)]) });
          }}
          more={!!value.next}
          onMore={() => void load({ cursor: value.next ?? undefined }, true)}
          footer={value.groups && Array.isArray(value.groups) && value.groups.length > 0 ? <pre className="db-detail-value p-2">{JSON.stringify(value.groups, null, 2)}</pre> : undefined}
        />
      )}
      {value?.type === 'unsupported' && (
        <Empty title={`${value.typeName} keys have no viewer yet`}>Use the console (e.g. TS.RANGE, BF.INFO, VINFO).</Empty>
      )}
    </div>
  );
}

function StringValue({ value, ro, busy, onSave }: { value: { value: string; base64?: string; size: number; truncated: boolean }; ro: boolean; busy: boolean; onSave(v: string): void }) {
  const pretty = useMemo(() => tryPrettyJson(value.value), [value.value]);
  const [mode, setMode] = useState<'text' | 'json' | 'hex'>(value.base64 ? 'hex' : pretty ? 'json' : 'text');
  const [draft, setDraft] = useState(pretty ?? value.value);
  useEffect(() => setDraft(mode === 'json' ? (pretty ?? value.value) : value.value), [value.value, pretty, mode]);
  const original = mode === 'json' ? (pretty ?? value.value) : value.value;
  const dirty = draft !== original;
  const save = () => {
    if (mode === 'json') {
      try {
        onSave(JSON.stringify(JSON.parse(draft)));
      } catch {
        toast.error('Not valid JSON');
      }
    } else onSave(draft);
  };
  return (
    <div className="db-value col">
      <div className="row-sm">
        <Segmented
          value={mode}
          onChange={setMode}
          options={[
            { value: 'text', label: 'Text' },
            { value: 'json', label: 'JSON' },
            ...(value.base64 ? [{ value: 'hex' as const, label: 'Hex' }] : []),
          ]}
        />
        <span className="subtle small grow">{formatBytes(value.size)}{value.truncated ? ' · showing the first 1 MB' : ''}</span>
        <CopyButton text={value.value} label="Copy" />
        {!value.truncated && !value.base64 && mode !== 'hex' && (
          <button className="btn btn-sm btn-primary" disabled={ro || !dirty || busy} onClick={save} title={ro ? 'Read-only' : 'SET (keeps the TTL)'}>
            <Save /> Save
          </button>
        )}
      </div>
      <div className="grow" style={{ minHeight: 0 }}>
        {mode === 'hex' && value.base64 ? (
          <pre className="db-hex">{hexDump(value.base64)}</pre>
        ) : (
          <CodeEditor value={draft} onChange={setDraft} language={mode === 'json' ? 'json' : 'plaintext'} readOnly={ro || value.truncated || !!value.base64} onSave={save} />
        )}
      </div>
    </div>
  );
}

function JsonValue({ value, ro, busy, onSave }: { value: unknown; ro: boolean; busy: boolean; onSave(v: string): void }) {
  const text = useMemo(() => JSON.stringify(value, null, 2), [value]);
  const [draft, setDraft] = useState(text);
  useEffect(() => setDraft(text), [text]);
  const save = () => {
    try {
      onSave(JSON.stringify(JSON.parse(draft)));
    } catch {
      toast.error('Not valid JSON');
    }
  };
  return (
    <div className="db-value col">
      <div className="row-sm">
        <span className="subtle small grow">RedisJSON document</span>
        <CopyButton text={draft} label="Copy" />
        <button className="btn btn-sm btn-primary" disabled={ro || draft === text || busy} onClick={save}>
          <Save /> Save
        </button>
      </div>
      <div className="grow" style={{ minHeight: 0 }}>
        <CodeEditor value={draft} onChange={setDraft} language="json" readOnly={ro} onSave={save} />
      </div>
    </div>
  );
}

function PairTable(p: {
  title: string;
  head: string[];
  rows: string[][];
  ro: boolean;
  editCol?: number;
  onEdit?(row: string[], value: string): void;
  onDelete?(row: string[]): void;
  onAdd?(): void;
  more: boolean;
  onMore(): void;
  footer?: React.ReactNode;
}) {
  const [filter, setFilter] = useState('');
  const [editing, setEditing] = useState<{ i: number; text: string } | null>(null);
  const shown = useMemo(() => {
    const q = filter.toLowerCase();
    return p.rows.map((r, i) => ({ r, i })).filter(({ r }) => !q || r.some((c) => c.toLowerCase().includes(q)));
  }, [p.rows, filter]);
  return (
    <div className="db-value col">
      <div className="row-sm">
        <span className="small strong">{p.title}</span>
        <input className="input sm grow" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter loaded items…" />
        {p.onAdd && (
          <button className="btn btn-sm" onClick={p.onAdd} disabled={p.ro} title={p.ro ? 'Read-only' : 'Add'}>
            <Plus /> Add
          </button>
        )}
      </div>
      <div className="grow scroll" style={{ minHeight: 0 }}>
        <table className="table db-pairs">
          <thead>
            <tr>
              {p.head.map((h) => (
                <th key={h}>{h}</th>
              ))}
              <th style={{ width: 64 }} />
            </tr>
          </thead>
          <tbody>
            {shown.map(({ r, i }) => (
              <tr key={i}>
                {r.map((c, ci) =>
                  editing?.i === i && ci === p.editCol ? (
                    <td key={ci}>
                      <input
                        className="input sm mono"
                        autoFocus
                        value={editing.text}
                        onChange={(e) => setEditing({ i, text: e.target.value })}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter') {
                            p.onEdit?.(r, editing.text);
                            setEditing(null);
                          }
                          if (e.key === 'Escape') setEditing(null);
                        }}
                        onBlur={() => setEditing(null)}
                      />
                    </td>
                  ) : (
                    <td key={ci} className="mono small db-pair-cell" onDoubleClick={() => !p.ro && ci === p.editCol && p.onEdit && setEditing({ i, text: c })} title={c}>
                      {c}
                    </td>
                  ),
                )}
                <td className="nowrap">
                  {p.onEdit && p.editCol !== undefined && (
                    <button className="icon-btn sm" disabled={p.ro} onClick={() => setEditing({ i, text: r[p.editCol ?? 0] })} aria-label="Edit">
                      <Pencil />
                    </button>
                  )}
                  {p.onDelete && (
                    <button className="icon-btn sm danger" disabled={p.ro} onClick={() => p.onDelete?.(r)} aria-label="Remove">
                      <X />
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {p.more && (
          <div className="p-2">
            <button className="btn btn-sm w-full" onClick={p.onMore}>
              Load more
            </button>
          </div>
        )}
        {p.footer}
      </div>
    </div>
  );
}

// ---------- console ----------

interface ConsoleLine {
  id: string;
  command: string;
  reply?: CommandReply;
  error?: string;
}

function Console({ session, openRequest, onRequestWrite }: { session: SessionInfo; openRequest?: { path: string; text: string; nonce: number }; onRequestWrite(): void }) {
  const [lines, setLines] = useState<ConsoleLine[]>([]);
  const [input, setInput] = useState('');
  const [hist, setHist] = useState<string[]>([]);
  const [hi, setHi] = useState(-1);
  const [running, setRunning] = useState(false);
  const [script, setScript] = useState<string | null>(null);
  const outRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (openRequest) setScript(openRequest.text);
  }, [openRequest?.nonce]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    outRef.current?.scrollTo({ top: outRef.current.scrollHeight });
  }, [lines]);

  const exec = async (command: string) => {
    if (!command.trim()) return;
    setHist((h) => [command, ...h.filter((x) => x !== command)].slice(0, 200));
    setHi(-1);
    const id = uid();
    setLines((l) => [...l, { id, command }]);
    setRunning(true);
    try {
      const reply = await dbApi.redisCommand(session.id, command);
      setLines((l) => l.map((x) => (x.id === id ? { ...x, reply } : x)));
    } catch (e) {
      setLines((l) => l.map((x) => (x.id === id ? { ...x, error: errorMessage(e) } : x)));
    } finally {
      setRunning(false);
    }
  };

  const runScript = async (text: string) => {
    setRunning(true);
    try {
      const replies = await dbApi.redisScript(session.id, text);
      setLines((l) => [...l, ...replies.map((r) => ({ id: uid(), command: r.command, reply: r }))]);
    } catch (e) {
      setLines((l) => [...l, { id: uid(), command: '(script)', error: errorMessage(e) }]);
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="db-console">
      {script !== null && (
        <div className="db-console-script">
          <div className="toolbar">
            <span className="small strong grow">Script ({openRequest?.path})</span>
            <button className="btn btn-sm btn-primary" onClick={() => void runScript(script)} disabled={running}>
              <Play /> Run all
            </button>
            <button className="icon-btn sm" onClick={() => setScript(null)} aria-label="Close script">
              <X />
            </button>
          </div>
          <div style={{ height: 160 }}>
            <CodeEditor value={script} onChange={setScript} language="shell" />
          </div>
        </div>
      )}
      <div ref={outRef} className="db-console-out mono">
        {lines.length === 0 && <div className="subtle small">redis-cli style console. Write commands are blocked while the session is read-only. Try: INFO keyspace · SCAN 0 MATCH user:* · TYPE key</div>}
        {lines.map((l) => (
          <div key={l.id} className="db-console-entry">
            <div className="db-console-cmd">
              <span className="subtle">{session.name}&gt;</span> {l.command}
              {l.reply && <span className="subtle small"> · {fmtMs(l.reply.elapsedMs)}</span>}
            </div>
            {l.reply && <pre className={clsx('db-console-reply', l.reply.error && 'text-danger')}>{l.reply.text}</pre>}
            {l.error && (
              <pre className={clsx('db-console-reply', l.error.startsWith('Read-only') ? 'text-warning' : 'text-danger')}>
                {l.error}
                {l.error.startsWith('Read-only') && (
                  <>
                    {' '}
                    <button className="btn btn-sm" onClick={onRequestWrite}>
                      Allow writes…
                    </button>
                  </>
                )}
              </pre>
            )}
          </div>
        ))}
      </div>
      <div className="db-console-input">
        <span className="mono subtle">&gt;</span>
        <input
          className="input mono grow"
          value={input}
          autoFocus
          placeholder="GET key"
          onChange={(e) => setInput(e.target.value)}
          onPaste={(e) => {
            const t = e.clipboardData.getData('text');
            if (t.includes('\n')) {
              e.preventDefault();
              setScript(t);
            }
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !running) {
              void exec(input);
              setInput('');
            } else if (e.key === 'ArrowUp') {
              e.preventDefault();
              const n = Math.min(hist.length - 1, hi + 1);
              if (n >= 0) {
                setHi(n);
                setInput(hist[n]);
              }
            } else if (e.key === 'ArrowDown') {
              e.preventDefault();
              const n = hi - 1;
              setHi(n);
              setInput(n >= 0 ? hist[n] : '');
            } else if (e.key === 'l' && e.ctrlKey) {
              e.preventDefault();
              setLines([]);
            }
          }}
        />
        {running && <Spinner />}
      </div>
    </div>
  );
}

// ---------- pub/sub ----------

const MAX_MESSAGES = 2000;

function PubSub({ session, onRequestWrite }: { session: SessionInfo; onRequestWrite(): void }) {
  const [channels, setChannels] = useState('');
  const [subId, setSubId] = useState<string | null>(null);
  const [messages, setMessages] = useState<PubSubMessage[]>([]);
  const [pubChannel, setPubChannel] = useState('');
  const [pubMessage, setPubMessage] = useState('');
  const [filter, setFilter] = useState('');

  useEffect(() => () => void (subId && dbApi.redisUnsubscribe(subId)), [subId]);

  const subscribe = async () => {
    const parts = channels.split(/[\s,]+/).filter(Boolean);
    const patterns = parts.filter((p) => /[*?[]/.test(p));
    const plain = parts.filter((p) => !/[*?[]/.test(p));
    const id = uid();
    try {
      await dbApi.redisSubscribe(session.id, id, plain, patterns, (m) => setMessages((all) => (all.length >= MAX_MESSAGES ? all.slice(-MAX_MESSAGES + 1) : all).concat(m)));
      setSubId(id);
    } catch (e) {
      toast.error('Subscribe failed', errorMessage(e));
    }
  };

  const publish = async () => {
    if (session.readOnly) return onRequestWrite();
    try {
      const n = await dbApi.redisPublish(session.id, pubChannel, pubMessage);
      toast.success(`Delivered to ${n} subscriber${n === 1 ? '' : 's'}`);
    } catch (e) {
      toast.error('Publish failed', errorMessage(e));
    }
  };

  const shown = filter ? messages.filter((m) => m.channel.includes(filter) || m.payload.includes(filter)) : messages;
  return (
    <div className="db-pubsub col">
      <div className="row-sm">
        <input className="input sm mono grow" value={channels} onChange={(e) => setChannels(e.target.value)} placeholder="Channels or patterns (orders, events:*)" disabled={!!subId} onKeyDown={(e) => e.key === 'Enter' && !subId && void subscribe()} />
        {subId ? (
          <button
            className="btn btn-sm"
            onClick={() => {
              void dbApi.redisUnsubscribe(subId);
              setSubId(null);
            }}
          >
            <span className="dot ok pulse" /> Stop
          </button>
        ) : (
          <button className="btn btn-sm btn-primary" onClick={() => void subscribe()} disabled={!channels.trim()}>
            <Radio /> Subscribe
          </button>
        )}
        <input className="input sm" style={{ width: 160 }} value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter…" />
        <button className="btn btn-sm btn-ghost" onClick={() => setMessages([])}>
          Clear
        </button>
      </div>
      <div className="grow scroll db-messages" style={{ minHeight: 0 }}>
        {shown.length === 0 ? (
          <Empty icon={<Radio />} title={subId ? 'Waiting for messages…' : 'Not subscribed'} />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th style={{ width: 110 }}>Time</th>
                <th style={{ width: 180 }}>Channel</th>
                <th>Payload</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((m, i) => (
                <tr key={i}>
                  <td className="mono small subtle">{new Date(m.at).toLocaleTimeString([], { hour12: false })}</td>
                  <td className="mono small" title={m.pattern ? `via ${m.pattern}` : undefined}>
                    {m.channel}
                  </td>
                  <td className="mono small db-pair-cell" title={m.payload}>
                    {m.payload}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      <div className="row-sm">
        <input className="input sm mono" style={{ width: 180 }} value={pubChannel} onChange={(e) => setPubChannel(e.target.value)} placeholder="Channel" />
        <input className="input sm mono grow" value={pubMessage} onChange={(e) => setPubMessage(e.target.value)} placeholder="Message" onKeyDown={(e) => e.key === 'Enter' && pubChannel && void publish()} />
        <button className="btn btn-sm" onClick={() => void publish()} disabled={!pubChannel} title={session.readOnly ? 'Read-only: PUBLISH is blocked' : 'PUBLISH'}>
          Publish
        </button>
      </div>
    </div>
  );
}

// ---------- info / slowlog ----------

function InfoView({ session }: { session: SessionInfo }) {
  const [sections, setSections] = useState<InfoSection[] | null>(null);
  const [error, setError] = useState<string>();
  const [q, setQ] = useState('');
  const load = useCallback(() => {
    setError(undefined);
    dbApi.redisInfo(session.id).then(setSections, (e) => setError(errorMessage(e)));
  }, [session.id]);
  useEffect(load, [load]);
  return (
    <div className="db-value col">
      <div className="row-sm">
        <input className="input sm grow" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Filter (memory, clients, keyspace…)" />
        <button className="icon-btn sm" onClick={load} aria-label="Reload">
          <RefreshCw />
        </button>
      </div>
      {error && <Callout kind="danger">{error}</Callout>}
      <div className="grow scroll db-info" style={{ minHeight: 0 }}>
        {sections?.map((s) => {
          const entries = s.entries.filter(([k, v]) => !q || k.includes(q) || v.includes(q) || s.name.toLowerCase().includes(q.toLowerCase()));
          if (!entries.length) return null;
          return (
            <div key={s.name} className="mb-4">
              <div className="section-title">{s.name}</div>
              <dl className="kv mono">
                {entries.map(([k, v]) => (
                  <div key={k} style={{ display: 'contents' }}>
                    <dt>{k}</dt>
                    <dd>{v}</dd>
                  </div>
                ))}
              </dl>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function SlowlogView({ session }: { session: SessionInfo }) {
  const [entries, setEntries] = useState<SlowlogEntry[] | null>(null);
  const [error, setError] = useState<string>();
  const load = useCallback(() => {
    setError(undefined);
    dbApi.redisSlowlog(session.id, 128).then(setEntries, (e) => setError(errorMessage(e)));
  }, [session.id]);
  useEffect(load, [load]);
  return (
    <div className="db-value col">
      <div className="row-sm">
        <span className="small subtle grow">Slowest recent commands (SLOWLOG GET)</span>
        <button className="icon-btn sm" onClick={load} aria-label="Reload">
          <RefreshCw />
        </button>
      </div>
      {error && <Callout kind="danger">{error}</Callout>}
      <div className="grow scroll" style={{ minHeight: 0 }}>
        {entries && entries.length === 0 && <Empty title="The slowlog is empty" />}
        {entries && entries.length > 0 && (
          <table className="table">
            <thead>
              <tr>
                <th>When</th>
                <th>Duration</th>
                <th>Command</th>
                <th>Client</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((e) => (
                <tr key={e.id}>
                  <td className="small nowrap">{new Date(e.timestamp * 1000).toLocaleString()}</td>
                  <td className="mono small nowrap">{fmtMs(e.durationUs / 1000)}</td>
                  <td className="mono small db-pair-cell" title={e.command}>
                    {e.command}
                  </td>
                  <td className="mono small subtle">{e.client}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

// ---------- new key ----------

function NewKeyDialog({ open, session, onClose, onCreated }: { open: boolean; session: SessionInfo; onClose(): void; onCreated(k: { key: string; type: string }): void }) {
  const [key, setKey] = useState('');
  const [type, setType] = useState('string');
  const [field, setField] = useState('');
  const [value, setValue] = useState('');
  const [ttl, setTtl] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (open) {
      setKey('');
      setField('');
      setValue('');
      setTtl('');
    }
  }, [open]);
  const create = async () => {
    const edit: RedisEdit | null =
      type === 'string'
        ? { type: 'setString', key, value, ttlMs: ttl ? Number(ttl) * 1000 : null }
        : type === 'hash'
          ? { type: 'hashSet', key, field, value }
          : type === 'list'
            ? { type: 'listPush', key, value }
            : type === 'set'
              ? { type: 'setAdd', key, member: value }
              : type === 'zset'
                ? { type: 'zAdd', key, member: value, score: Number(field) || 0 }
                : type === 'stream'
                  ? { type: 'streamAdd', key, fields: [[field || 'field', value]] }
                  : type === 'ReJSON-RL'
                    ? { type: 'jsonSet', key, value: value || '{}' }
                    : null;
    if (!edit) return;
    setBusy(true);
    try {
      await dbApi.redisSet(session.id, edit);
      if (ttl && type !== 'string') await dbApi.redisSet(session.id, { type: 'expire', key, ttlMs: Number(ttl) * 1000 });
      onCreated({ key, type });
    } catch (e) {
      toast.error('Could not create the key', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const needsField = type === 'hash' || type === 'zset' || type === 'stream';
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="New key"
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void create()} disabled={!key || busy || (type === 'zset' && !Number.isFinite(Number(field)))}>
            Create
          </button>
        </>
      }
    >
      <div className="col">
        <div className="row-sm">
          <Field label="Key" className="grow">
            <input className="input mono" value={key} onChange={(e) => setKey(e.target.value)} autoFocus placeholder="user:42" />
          </Field>
          <Field label="Type">
            <select className="select" value={type} onChange={(e) => setType(e.target.value)}>
              {TYPES.map((t) => (
                <option key={t} value={t}>
                  {t === 'ReJSON-RL' ? 'JSON (RedisJSON)' : t}
                </option>
              ))}
            </select>
          </Field>
        </div>
        {needsField && (
          <Field label={type === 'zset' ? 'Score' : 'Field'}>
            <input className="input mono" value={field} onChange={(e) => setField(e.target.value)} placeholder={type === 'zset' ? '0' : 'name'} />
          </Field>
        )}
        <Field label={type === 'set' || type === 'zset' ? 'Member' : 'Value'}>
          <textarea className="textarea mono" rows={4} value={value} onChange={(e) => setValue(e.target.value)} placeholder={type === 'ReJSON-RL' ? '{"name": "Ada"}' : ''} />
        </Field>
        <Field label="TTL (seconds, optional)">
          <input className="input" value={ttl} onChange={(e) => setTtl(e.target.value.replace(/\D/g, ''))} placeholder="no expiry" />
        </Field>
      </div>
    </Modal>
  );
}

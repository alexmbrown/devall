import { useEffect, useMemo, useState } from 'react';
import { ArrowLeft, ArrowRight, Database, Link2, Wand2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Field, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { dataApi, type DataField } from '@/lib/api/data';
import { dbApi, type FromDbResult, type OpenSession, type SchemaTree, type TableRef } from '@/lib/api/db';
import { useNav } from '@/store/nav';

export interface FromDatabaseTarget {
  /** Preselected open session. */
  session?: string;
  /** SQL tables to start with (default: every table of the default schema). */
  tables?: TableRef[];
  /** MongoDB database / collections to start with. */
  database?: string;
  collections?: string[];
}

interface Props {
  open: boolean;
  root: string;
  target?: FromDatabaseTarget;
  onClose(): void;
  /** Called with the new file's path (relative to `data/`); default: open it on the Fake data page. */
  onCreated?(path: string): void;
}

const key = (t: TableRef) => `${t.schema ?? ''}\u0000${t.name}`;

/** One line describing a field's generator, e.g. `ref → users.id`, `enum (3)`. */
export function describeField(f: DataField): string {
  switch (f.type) {
    case 'ref':
      return `→ ${f.entity}.${f.field}`;
    case 'enum':
      return `enum (${f.values.length})`;
    case 'regex':
      return `regex ${f.pattern}`;
    case 'object':
      return f.fields.some((x) => x.name === '$oid') ? 'ObjectId' : f.fields.some((x) => x.name === '$date') ? 'date' : 'object';
    case 'json':
      return f.value === null ? 'null' : 'json';
    default:
      return f.type;
  }
}

/**
 * "Fake-data schema from a database": pick an open connection and its tables (SQL) or
 * collections (MongoDB, sampled), review the generated entities, create `data/<name>.data.yaml`.
 * Only the schema is read for SQL; MongoDB documents are sampled and reduced to field types.
 */
export function FromDatabaseDialog({ open, root, target, onClose, onCreated }: Props) {
  const go = useNav((s) => s.go);
  const [sessions, setSessions] = useState<OpenSession[] | null>(null);
  const [sessionId, setSessionId] = useState('');
  const [tree, setTree] = useState<SchemaTree | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [databases, setDatabases] = useState<string[]>([]);
  const [database, setDatabase] = useState('');
  const [collections, setCollections] = useState<string[]>([]);
  const [pickedColls, setPickedColls] = useState<Set<string>>(new Set());
  const [count, setCount] = useState('25');
  const [nullPct, setNullPct] = useState('10');
  const [sample, setSample] = useState('50');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<FromDbResult | null>(null);
  const [name, setName] = useState('');

  const session = sessions?.find((s) => s.info.id === sessionId)?.info;

  useEffect(() => {
    if (!open) return;
    setResult(null);
    setError(null);
    setSessions(null);
    dbApi
      .sessions()
      .then((all) => {
        const usable = all.filter((s) => s.info.kind !== 'redis');
        setSessions(usable);
        const pick = usable.find((s) => s.info.id === target?.session) ?? usable.find((s) => s.root === root) ?? usable[0];
        setSessionId(pick?.info.id ?? '');
      })
      .catch((e) => {
        setSessions([]);
        setError(errorMessage(e));
      });
  }, [open, root, target?.session]);

  // Load what can be picked for the chosen session.
  useEffect(() => {
    if (!open || !session) return;
    setTree(null);
    setCollections([]);
    setName(`${session.name} data`);
    if (session.kind === 'mongodb') {
      dbApi
        .mongoDatabases(session.id)
        .then((dbs) => {
          const names = dbs.map((d) => d.name).filter((n) => !['admin', 'local', 'config'].includes(n));
          setDatabases(names);
          setDatabase(target?.database ?? session.database ?? names[0] ?? '');
        })
        .catch((e) => setError(errorMessage(e)));
    } else {
      dbApi
        .dbSchema(session.id)
        .then((t) => {
          setTree(t);
          const initial = target?.tables?.length
            ? target.tables.map((x) => ({ schema: x.schema ?? t.defaultSchema, name: x.name }))
            : (t.schemas.find((s) => s.name === t.defaultSchema) ?? t.schemas[0])?.tables.filter((x) => x.kind === 'table').map((x) => ({ schema: t.defaultSchema ?? undefined, name: x.name })) ?? [];
          setPicked(new Set(initial.map((x) => key({ schema: x.schema ?? undefined, name: x.name }))));
        })
        .catch((e) => setError(errorMessage(e)));
    }
  }, [open, session, target]);

  useEffect(() => {
    if (!open || !session || session.kind !== 'mongodb' || !database) return;
    dbApi
      .mongoCollections(session.id, database)
      .then((cs) => {
        const names = cs.filter((c) => c.kind === 'collection' && !c.name.startsWith('system.')).map((c) => c.name);
        setCollections(names);
        setPickedColls(new Set(target?.collections?.length ? target.collections.filter((c) => names.includes(c)) : names));
      })
      .catch((e) => setError(errorMessage(e)));
  }, [open, session, database, target?.collections]);

  const tables = useMemo(() => tree?.schemas.flatMap((s) => s.tables.filter((t) => t.kind === 'table').map((t) => ({ schema: s.name, name: t.name, columns: t.columns.length }))) ?? [], [tree]);
  const multiSchema = (tree?.schemas.filter((s) => s.tables.some((t) => t.kind === 'table')).length ?? 0) > 1;

  const build = async () => {
    if (!session) return;
    setBusy(true);
    setError(null);
    try {
      const options = { count: Math.max(1, Number(count) || 25), nullPercent: Math.min(100, Math.max(0, Number(nullPct) || 0)) };
      const r =
        session.kind === 'mongodb'
          ? await dbApi.fakeSchema(session.id, { name, database, collections: [...pickedColls], sample: Number(sample) || 50, options })
          : await dbApi.fakeSchema(session.id, { name, tables: tables.filter((t) => picked.has(key(t))).map((t) => ({ schema: t.schema, name: t.name })), options });
      setResult(r);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const create = async () => {
    if (!result || !name.trim()) return;
    setBusy(true);
    try {
      const path = await dataApi.create(root, name.trim(), result.schema);
      toast.success('Fake-data schema created', `data/${path}`);
      onClose();
      if (onCreated) onCreated(path);
      else go('fakedata', path);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const nothingPicked = session?.kind === 'mongodb' ? pickedColls.size === 0 : picked.size === 0;
  const toggle = (set: Set<string>, k: string, on: boolean) => {
    const next = new Set(set);
    if (on) next.add(k);
    else next.delete(k);
    return next;
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      size="lg"
      title="Fake-data schema from a database"
      footer={
        result ? (
          <>
            <button className="btn" onClick={() => setResult(null)} disabled={busy}>
              <ArrowLeft /> Back
            </button>
            <span className="spacer" />
            <button className="btn btn-primary" onClick={() => void create()} disabled={busy || !name.trim()}>
              {busy ? <Spinner /> : <Wand2 />} Create schema
            </button>
          </>
        ) : (
          <>
            <span className="subtle xs grow">{session?.kind === 'mongodb' ? 'Documents are sampled and reduced to field names and types.' : 'Only the schema is read — no rows.'}</span>
            <button className="btn" onClick={onClose}>
              Cancel
            </button>
            <button className="btn btn-primary" onClick={() => void build()} disabled={busy || !session || nothingPicked}>
              {busy ? <Spinner /> : <ArrowRight />} Build schema
            </button>
          </>
        )
      }
    >
      <div className="col">
        {error && <Callout kind="danger">{error}</Callout>}
        {sessions === null ? (
          <Spinner />
        ) : sessions.length === 0 ? (
          <Callout>
            Connect to a SQL or MongoDB database first.{' '}
            <button
              className="btn btn-sm"
              onClick={() => {
                onClose();
                go('data');
              }}
            >
              <Database /> Open Databases
            </button>
          </Callout>
        ) : result ? (
          <ResultView result={result} name={name} onName={setName} />
        ) : (
          <>
            <div className="grid-2">
              <Field label="Connection">
                <select className="select" value={sessionId} onChange={(e) => setSessionId(e.target.value)}>
                  {sessions.map((s) => (
                    <option key={s.info.id} value={s.info.id}>
                      {s.info.name} · {s.info.target}
                    </option>
                  ))}
                </select>
              </Field>
              {session?.kind === 'mongodb' && (
                <Field label="Database">
                  <select className="select" value={database} onChange={(e) => setDatabase(e.target.value)}>
                    {databases.map((d) => (
                      <option key={d} value={d}>
                        {d}
                      </option>
                    ))}
                  </select>
                </Field>
              )}
            </div>
            {session?.kind === 'mongodb' ? (
              <Field label={`Collections (${pickedColls.size}/${collections.length})`}>
                <PickList items={collections.map((c) => ({ key: c, label: c }))} picked={pickedColls} onToggle={(k, on) => setPickedColls((s) => toggle(s, k, on))} onAll={(on) => setPickedColls(new Set(on ? collections : []))} />
              </Field>
            ) : (
              <Field label={`Tables (${picked.size}/${tables.length})`} hint="Foreign keys to tables you leave out get plain values instead of references.">
                {tree ? (
                  <PickList
                    items={tables.map((t) => ({ key: key(t), label: multiSchema ? `${t.schema}.${t.name}` : t.name, hint: `${t.columns} columns` }))}
                    picked={picked}
                    onToggle={(k, on) => setPicked((s) => toggle(s, k, on))}
                    onAll={(on) => setPicked(new Set(on ? tables.map(key) : []))}
                  />
                ) : (
                  <Spinner />
                )}
              </Field>
            )}
            <div className="row wrap">
              <Field label="Rows per table">
                <input className="input" style={{ width: 120 }} value={count} onChange={(e) => setCount(e.target.value.replace(/\D/g, ''))} />
              </Field>
              <Field label="Null rate for nullable columns (%)">
                <input className="input" style={{ width: 120 }} value={nullPct} onChange={(e) => setNullPct(e.target.value.replace(/[^\d.]/g, ''))} />
              </Field>
              {session?.kind === 'mongodb' && (
                <Field label="Documents sampled per collection">
                  <input className="input" style={{ width: 120 }} value={sample} onChange={(e) => setSample(e.target.value.replace(/\D/g, ''))} />
                </Field>
              )}
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}

function PickList({ items, picked, onToggle, onAll }: { items: { key: string; label: string; hint?: string }[]; picked: Set<string>; onToggle(k: string, on: boolean): void; onAll(on: boolean): void }) {
  const all = items.length > 0 && items.every((i) => picked.has(i.key));
  return (
    <div className="panel">
      <div className="list-item">
        <Checkbox checked={all} indeterminate={!all && picked.size > 0} onChange={onAll} label={<span className="strong small">All</span>} />
      </div>
      <div className="move-list" style={{ maxHeight: 240 }}>
        {items.length === 0 && <div className="subtle small p-4">Nothing here</div>}
        {items.map((i) => (
          <div key={i.key} className="list-item" onClick={() => onToggle(i.key, !picked.has(i.key))}>
            <Checkbox checked={picked.has(i.key)} onChange={(on) => onToggle(i.key, on)} label={<span className="mono small">{i.label}</span>} />
            <span className="spacer" />
            {i.hint && <span className="subtle xs">{i.hint}</span>}
          </div>
        ))}
      </div>
    </div>
  );
}

function ResultView({ result, name, onName }: { result: FromDbResult; name: string; onName(v: string): void }) {
  return (
    <>
      <Field label="Schema name" hint="Saved as data/<name>.data.yaml — review and tweak generators on the Fake data page.">
        <input className="input" value={name} onChange={(e) => onName(e.target.value)} autoFocus />
      </Field>
      {result.warnings.length > 0 && (
        <Callout kind="warning">
          <ul className="small" style={{ margin: 0, paddingLeft: 18 }}>
            {result.warnings.map((w, i) => (
              <li key={i}>{w}</li>
            ))}
          </ul>
        </Callout>
      )}
      <div className="panel" style={{ maxHeight: 340, overflow: 'auto' }}>
        <table className="table">
          <thead>
            <tr>
              <th>Entity</th>
              <th style={{ width: 70 }}>Rows</th>
              <th>Fields</th>
            </tr>
          </thead>
          <tbody>
            {result.schema.entities.map((e) => (
              <tr key={e.name}>
                <td className="mono small" style={{ verticalAlign: 'top' }}>
                  {e.name}
                </td>
                <td className="small" style={{ verticalAlign: 'top' }}>
                  {e.count}
                </td>
                <td className="xs">
                  {e.fields.map((f) => (
                    <span key={f.name} className="row-sm" style={{ display: 'inline-flex', marginRight: 10 }}>
                      <span className="mono">{f.name}</span>
                      <span className={f.type === 'ref' ? 'text-accent' : 'subtle'}>
                        {f.type === 'ref' && <Link2 size={10} />} {describeField(f)}
                        {f.unique ? ' · unique' : ''}
                        {f.nullPercent ? ` · ${f.nullPercent}% null` : ''}
                      </span>
                    </span>
                  ))}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}

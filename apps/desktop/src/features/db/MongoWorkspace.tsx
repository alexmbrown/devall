import { useCallback, useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { ChevronDown, ChevronRight, Database, DatabaseZap, Eye, FileJson, Hash, Layers, Play, Plus, RefreshCw, Save, Search, Trash2, Pencil } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Modal } from '@/components/ui/Modal';
import { useContextMenu } from '@/components/ui/Menu';
import { Callout, Empty, Field, Kbd, MOD, Resizer, SearchInput, Segmented, Spinner, Switch, formatBytes } from '@/components/ui/misc';
import { promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { dbApi, type ColumnMeta, type Documents, type MongoSavedQuery, type MongoWrite, type SessionInfo } from '@/lib/api/db';
import { ResultsGrid } from './ResultsGrid';
import { AiAssistButton } from '@/features/ai';
import { mongoFields } from '@/features/ai/nl';
import { fmtCount, fmtMs } from './format';

interface Props {
  root: string;
  slug: string;
  session: SessionInfo;
  onRequestWrite(): void;
  /** "Create fake-data schema…" from sampled collections (empty = all of the database). */
  onFakeSchema?(database: string, collections: string[]): void;
  openRequest?: { path: string; text: string; nonce: number };
}

type Mode = 'find' | 'aggregate' | 'indexes' | 'command';

/** `{"$oid": "…"}` → `ObjectId("…")`-like display values for the table view. */
function simplify(v: unknown): unknown {
  if (v && typeof v === 'object' && !Array.isArray(v)) {
    const o = v as Record<string, unknown>;
    const keys = Object.keys(o);
    if (keys.length === 1) {
      const k = keys[0];
      if (k === '$oid' || k === '$numberDecimal' || k === '$numberLong' || k === '$uuid') return String(o[k]);
      if (k === '$date') return typeof o[k] === 'string' ? o[k] : JSON.stringify(o[k]);
    }
  }
  return v;
}

function docsToGrid(docs: Record<string, unknown>[]): { columns: ColumnMeta[]; rows: unknown[][] } {
  const keys: string[] = [];
  for (const d of docs.slice(0, 500)) for (const k of Object.keys(d)) if (!keys.includes(k)) keys.push(k);
  const idIdx = keys.indexOf('_id');
  if (idIdx > 0) {
    keys.splice(idIdx, 1);
    keys.unshift('_id');
  }
  const columns: ColumnMeta[] = keys.map((k) => ({ name: k, typeName: '', kind: 'json' }));
  const rows = docs.map((d) => keys.map((k) => (k in d ? simplify(d[k]) : null)));
  return { columns, rows };
}

const editorHeight = (text: string, min = 1, max = 12) => Math.min(max, Math.max(min, text.split('\n').length)) * 18 + 14;

export function MongoWorkspace({ root, slug, session, onRequestWrite, onFakeSchema, openRequest }: Props) {
  const ctx = useContextMenu();
  const [sideW, setSideW] = useState(260);
  const [dbs, setDbs] = useState<{ name: string; sizeOnDisk: number }[] | null>(null);
  const [colls, setColls] = useState<Record<string, { name: string; kind: string }[]>>({});
  const [openDbs, setOpenDbs] = useState<Record<string, boolean>>({});
  const [treeError, setTreeError] = useState<string>();
  const [filterTree, setFilterTree] = useState('');
  const [target, setTarget] = useState<{ db: string; coll: string } | null>(null);
  const [mode, setMode] = useState<Mode>('find');
  const [filter, setFilter] = useState('{}');
  const [projection, setProjection] = useState('');
  const [sort, setSort] = useState('');
  const [limit, setLimit] = useState(50);
  const [skip, setSkip] = useState(0);
  const [pipeline, setPipeline] = useState('[\n  { $match: {} },\n  { $limit: 20 }\n]');
  const [command, setCommand] = useState('{ dbStats: 1 }');
  const [result, setResult] = useState<Documents | null>(null);
  const [extra, setExtra] = useState<unknown>(null);
  const [view, setView] = useState<'table' | 'json'>('table');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [count, setCount] = useState<number | null>(null);
  const [savedPath, setSavedPath] = useState<string>();
  const [write, setWrite] = useState<MongoWrite['type'] | null>(null);

  const loadDbs = useCallback(async () => {
    setTreeError(undefined);
    try {
      const list = await dbApi.mongoDatabases(session.id);
      setDbs(list);
      const def = session.database ?? list.find((d) => !['admin', 'config', 'local'].includes(d.name))?.name;
      if (def) setOpenDbs((o) => ({ [def]: true, ...o }));
    } catch (e) {
      setTreeError(errorMessage(e));
    }
  }, [session.id, session.database]);

  useEffect(() => {
    void loadDbs();
  }, [loadDbs]);

  const loadColls = useCallback(
    async (db: string) => {
      try {
        const list = await dbApi.mongoCollections(session.id, db);
        setColls((c) => ({ ...c, [db]: list }));
      } catch (e) {
        toast.error(`Could not list collections of ${db}`, errorMessage(e));
      }
    },
    [session.id],
  );

  useEffect(() => {
    for (const [db, open] of Object.entries(openDbs)) if (open && !colls[db]) void loadColls(db);
  }, [openDbs, colls, loadColls]);

  // Saved .mongo.json query → fill the editors.
  useEffect(() => {
    if (!openRequest) return;
    try {
      const q = JSON.parse(openRequest.text) as MongoSavedQuery;
      if (q.database && q.collection) setTarget({ db: q.database, coll: q.collection });
      const js = (v: unknown, fallback: string) => (v === undefined ? fallback : typeof v === 'string' ? v : JSON.stringify(v, null, 2));
      if (q.type === 'aggregate') {
        setMode('aggregate');
        setPipeline(js(q.pipeline, '[]'));
      } else {
        setMode('find');
        setFilter(js(q.filter, '{}'));
        setProjection(q.projection === undefined ? '' : js(q.projection, ''));
        setSort(q.sort === undefined ? '' : js(q.sort, ''));
        if (q.limit) setLimit(q.limit);
      }
      setSavedPath(openRequest.path);
    } catch (e) {
      toast.error('Invalid saved query', errorMessage(e));
    }
  }, [openRequest?.nonce]); // eslint-disable-line react-hooks/exhaustive-deps

  const run = async (opts: { skip?: number; append?: boolean } = {}) => {
    if (!target && mode !== 'command') return;
    setBusy(true);
    setError(undefined);
    try {
      if (mode === 'find' && target) {
        const s = opts.skip ?? 0;
        const r = await dbApi.mongoFind(session.id, { database: target.db, collection: target.coll, filter, projection: projection || undefined, sort: sort || undefined, skip: s, limit });
        setSkip(s);
        setResult((old) => (opts.append && old ? { ...r, docs: old.docs.concat(r.docs) } : r));
        setExtra(null);
        setCount(null);
      } else if (mode === 'aggregate' && target) {
        setResult(await dbApi.mongoAggregate(session.id, { database: target.db, collection: target.coll, pipeline, limit: Math.max(limit, 100) }));
        setExtra(null);
      } else if (mode === 'indexes' && target) {
        setExtra(await dbApi.mongoIndexes(session.id, target.db, target.coll));
        setResult(null);
      } else if (mode === 'command') {
        setExtra(await dbApi.mongoCommand(session.id, target?.db ?? session.database ?? 'admin', command));
        setResult(null);
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const explain = async () => {
    if (!target) return;
    setBusy(true);
    setError(undefined);
    try {
      const plan =
        mode === 'aggregate'
          ? await dbApi.mongoExplain(session.id, { type: 'aggregate', request: { database: target.db, collection: target.coll, pipeline } }, false)
          : await dbApi.mongoExplain(session.id, { type: 'find', request: { database: target.db, collection: target.coll, filter, projection: projection || undefined, sort: sort || undefined, limit } }, false);
      setExtra(plan);
      setResult(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const doCount = async () => {
    if (!target) return;
    try {
      setCount(await dbApi.mongoCount(session.id, target.db, target.coll, filter));
    } catch (e) {
      toast.error('Count failed', errorMessage(e));
    }
  };

  const save = async () => {
    if (!target || (mode !== 'find' && mode !== 'aggregate')) return;
    const parse = (t: string) => {
      try {
        return JSON.parse(t);
      } catch {
        return t; // shell syntax is kept as text
      }
    };
    const body: MongoSavedQuery = mode === 'find'
      ? { type: 'find', database: target.db, collection: target.coll, filter: parse(filter), ...(projection ? { projection: parse(projection) } : {}), ...(sort ? { sort: parse(sort) } : {}), limit }
      : { type: 'aggregate', database: target.db, collection: target.coll, pipeline: parse(pipeline) as unknown[] };
    try {
      if (savedPath) {
        await dbApi.saveQuery(root, savedPath, JSON.stringify({ connection: slug, ...body }, null, 2) + '\n');
        toast.success('Saved', `queries/${savedPath}`);
      } else {
        const name = await promptDialog({ title: 'Save query', label: 'Name', placeholder: 'Recent orders', validate: (v) => (v.trim() ? undefined : 'Required') });
        if (!name) return;
        const path = await dbApi.createQuery(root, name, 'mongo', { connection: slug, text: JSON.stringify(body) });
        setSavedPath(path);
        toast.success('Saved', `queries/${path}`);
        window.dispatchEvent(new Event('devall:db-queries-changed'));
      }
    } catch (e) {
      toast.error('Could not save', errorMessage(e));
    }
  };

  const grid = useMemo(() => (result ? docsToGrid(result.docs) : null), [result]);
  const q = filterTree.toLowerCase();
  const onKey = (e: React.KeyboardEvent) => {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
      // Capture phase: runs before Monaco's own Ctrl+Enter (insert line).
      e.preventDefault();
      e.stopPropagation();
      void run();
    }
  };

  return (
    <div className="split">
      <div className="pane side" style={{ width: sideW }}>
        <div className="toolbar">
          <SearchInput value={filterTree} onChange={setFilterTree} placeholder="Filter collections…" className="grow" />
          <button className="icon-btn sm" onClick={() => void loadDbs()} title="Reload" aria-label="Reload">
            <RefreshCw />
          </button>
        </div>
        <div className="tree">
          {treeError && <Callout kind="danger" className="m-3">{treeError}</Callout>}
          {!dbs && !treeError && (
            <div className="empty">
              <Spinner />
            </div>
          )}
          {dbs?.map((d) => {
            const open = openDbs[d.name] ?? false;
            const list = (colls[d.name] ?? []).filter((c) => !q || c.name.toLowerCase().includes(q));
            return (
              <div key={d.name}>
                <div
                  className="tree-row"
                  style={{ paddingLeft: 6 }}
                  onClick={() => setOpenDbs((o) => ({ ...o, [d.name]: !open }))}
                  tabIndex={0}
                  onKeyDown={(e) => e.key === 'Enter' && setOpenDbs((o) => ({ ...o, [d.name]: !open }))}
                  onContextMenu={(e) => onFakeSchema && ctx.open(e, [{ label: 'Create fake-data schema from all collections…', icon: <DatabaseZap />, onSelect: () => onFakeSchema(d.name, []) }])}
                >
                  {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  <Database size={13} className="subtle" />
                  <span className="truncate grow">{d.name}</span>
                  {d.sizeOnDisk > 0 && <span className="subtle small">{formatBytes(d.sizeOnDisk)}</span>}
                </div>
                {open &&
                  list.map((c) => (
                    <div
                      key={c.name}
                      className={clsx('tree-row', target?.db === d.name && target.coll === c.name && 'on')}
                      style={{ paddingLeft: 26 }}
                      tabIndex={0}
                      onClick={() => {
                        setTarget({ db: d.name, coll: c.name });
                        setSavedPath(undefined);
                        setResult(null);
                        setExtra(null);
                        setCount(null);
                      }}
                      onDoubleClick={() => void run()}
                      onKeyDown={(e) => e.key === 'Enter' && setTarget({ db: d.name, coll: c.name })}
                      onContextMenu={(e) =>
                        onFakeSchema && c.kind === 'collection' && ctx.open(e, [{ label: 'Create fake-data schema…', icon: <DatabaseZap />, onSelect: () => onFakeSchema(d.name, [c.name]) }])
                      }
                      title={c.kind}
                    >
                      {c.kind === 'view' ? <Eye size={13} className="subtle" /> : <Layers size={13} className="text-accent" />}
                      <span className="truncate">{c.name}</span>
                    </div>
                  ))}
              </div>
            );
          })}
        </div>
      </div>
      <Resizer width={sideW} onChange={setSideW} min={180} max={520} />
      {ctx.node}
      <div className="pane fill db-mongo" onKeyDownCapture={onKey}>
        {!target && mode !== 'command' ? (
          <Empty icon={<Layers />} title="Pick a collection">
            Filters accept Extended JSON or mongo-shell syntax: <code className="code-inline">{'{ age: { $gt: 30 }, _id: ObjectId("…") }'}</code>
          </Empty>
        ) : (
          <>
            <div className="toolbar">
              <span className="mono strong">{target ? `${target.db}.${target.coll}` : 'runCommand'}</span>
              <Segmented
                value={mode}
                onChange={(m) => {
                  setMode(m);
                  setResult(null);
                  setExtra(null);
                  setError(undefined);
                }}
                options={[
                  { value: 'find', label: 'Find' },
                  { value: 'aggregate', label: 'Aggregate' },
                  { value: 'indexes', label: 'Indexes' },
                  { value: 'command', label: 'Command' },
                ]}
              />
              {target && (mode === 'find' || mode === 'aggregate') && (
                <AiAssistButton
                  feature="nlMongo"
                  title="Write a find or aggregate from a question — sends sampled field names and types (never values) and checks the result with explain"
                  applyLabel="Use query"
                  input={async (q) => ({
                    question: q,
                    database: target.db,
                    collection: target.coll,
                    fields: await dbApi.mongoFields(session.id, target.db, target.coll),
                    session: session.id,
                  })}
                  onResult={(r) => {
                    if (r.type === 'aggregate') {
                      setMode('aggregate');
                      setPipeline(r.pipeline);
                    } else {
                      const f = mongoFields(r);
                      setMode('find');
                      setFilter(f.filter);
                      setProjection(f.projection);
                      setSort(f.sort);
                      if (f.limit) setLimit(f.limit);
                    }
                    setResult(null);
                    setExtra(null);
                  }}
                />
              )}
              <span className="grow" />
              {(mode === 'find' || mode === 'aggregate') && (
                <>
                  <button className="btn btn-sm btn-ghost" onClick={() => (session.readOnly ? onRequestWrite() : setWrite('insert'))} title={session.readOnly ? 'Read-only' : 'insertMany'}>
                    <Plus /> Insert
                  </button>
                  <button className="btn btn-sm btn-ghost" onClick={() => (session.readOnly ? onRequestWrite() : setWrite('update'))} title={session.readOnly ? 'Read-only' : 'updateOne / updateMany'}>
                    <Pencil /> Update
                  </button>
                  <button className="btn btn-sm btn-ghost" onClick={() => (session.readOnly ? onRequestWrite() : setWrite('delete'))} title={session.readOnly ? 'Read-only' : 'deleteOne / deleteMany'}>
                    <Trash2 /> Delete
                  </button>
                  <button className="btn btn-sm" onClick={() => void save()} title="Save to queries/*.mongo.json">
                    <Save /> {savedPath ? 'Save' : 'Save as…'}
                  </button>
                </>
              )}
            </div>
            <div className="db-mongo-editors">
              {mode === 'find' && (
                <>
                  <div className="db-mongo-row">
                    <span className="db-mongo-label">Filter</span>
                    <div className="grow" style={{ height: editorHeight(filter) }}>
                      <CodeEditor value={filter} onChange={setFilter} language="javascript" minimal lineNumbers={false} path={`mongo-filter-${session.id}.js`} />
                    </div>
                  </div>
                  <div className="db-mongo-row">
                    <span className="db-mongo-label">Project</span>
                    <input className="input sm mono grow" value={projection} onChange={(e) => setProjection(e.target.value)} placeholder="{ name: 1, email: 1 }" />
                    <span className="db-mongo-label">Sort</span>
                    <input className="input sm mono grow" value={sort} onChange={(e) => setSort(e.target.value)} placeholder="{ createdAt: -1 }" />
                    <span className="db-mongo-label">Limit</span>
                    <input className="input sm" style={{ width: 70 }} value={limit} onChange={(e) => setLimit(Math.max(1, Math.min(10000, Number(e.target.value.replace(/\D/g, '')) || 1)))} />
                  </div>
                </>
              )}
              {mode === 'aggregate' && (
                <div style={{ height: editorHeight(pipeline, 4, 16) }}>
                  <CodeEditor value={pipeline} onChange={setPipeline} language="javascript" path={`mongo-pipeline-${session.id}.js`} />
                </div>
              )}
              {mode === 'command' && (
                <div style={{ height: editorHeight(command, 3, 12) }}>
                  <CodeEditor value={command} onChange={setCommand} language="javascript" path={`mongo-command-${session.id}.js`} />
                </div>
              )}
              <div className="row-sm">
                <button className="btn btn-sm btn-primary" onClick={() => void run()} disabled={busy} title={`${MOD}+Enter`}>
                  {busy ? <Spinner /> : <Play />} {mode === 'indexes' ? 'List indexes' : 'Run'}
                </button>
                {(mode === 'find' || mode === 'aggregate') && (
                  <button className="btn btn-sm" onClick={() => void explain()} disabled={busy}>
                    <Search /> Explain
                  </button>
                )}
                {mode === 'find' && (
                  <button className="btn btn-sm btn-ghost" onClick={() => void doCount()} title="countDocuments(filter)">
                    <Hash /> Count{count !== null ? `: ${fmtCount(count)}` : ''}
                  </button>
                )}
                <span className="subtle small">
                  <Kbd>{MOD}</Kbd> <Kbd>Enter</Kbd> runs
                </span>
                <span className="grow" />
                {result && (
                  <Segmented
                    value={view}
                    onChange={setView}
                    options={[
                      { value: 'table', label: 'Table' },
                      { value: 'json', label: <><FileJson size={12} /> JSON</> },
                    ]}
                  />
                )}
              </div>
            </div>
            <div className="db-results">
              {error ? (
                <div className="p-4">
                  <Callout kind={error.startsWith('Read-only') ? 'warning' : 'danger'}>
                    <pre className="db-error">{error}</pre>
                    {error.startsWith('Read-only') && (
                      <button className="btn btn-sm mt-2" onClick={onRequestWrite}>
                        Allow writes…
                      </button>
                    )}
                  </Callout>
                </div>
              ) : result && grid ? (
                view === 'table' ? (
                  <ResultsGrid
                    key={`${result.elapsedMs}:${result.docs.length}`}
                    columns={grid.columns}
                    rows={grid.rows}
                    status={`${fmtCount(result.docs.length)}${result.more ? '+' : ''} documents · ${fmtMs(result.elapsedMs)}`}
                    more={result.more && mode === 'find'}
                    onLoadMore={() => void run({ skip: skip + limit, append: true })}
                    loadingMore={busy}
                  />
                ) : (
                  <CodeEditor value={JSON.stringify(result.docs, null, 2)} language="json" readOnly />
                )
              ) : extra !== null ? (
                <CodeEditor value={JSON.stringify(extra, null, 2)} language="json" readOnly />
              ) : (
                <Empty title={mode === 'indexes' ? 'List the indexes' : 'Run a query'} />
              )}
            </div>
          </>
        )}
      </div>
      {write && target && (
        <WriteDialog
          kind={write}
          session={session}
          target={target}
          initialFilter={filter}
          onClose={() => setWrite(null)}
          onDone={() => {
            setWrite(null);
            void run();
          }}
        />
      )}
    </div>
  );
}

function WriteDialog({ kind, session, target, initialFilter, onClose, onDone }: { kind: MongoWrite['type']; session: SessionInfo; target: { db: string; coll: string }; initialFilter: string; onClose(): void; onDone(): void }) {
  const [filter, setFilter] = useState(kind === 'insert' ? '' : initialFilter);
  const [body, setBody] = useState(kind === 'insert' ? '[\n  { "name": "Ada" }\n]' : kind === 'update' ? '{ $set: { status: "active" } }' : '');
  const [many, setMany] = useState(false);
  const [upsert, setUpsert] = useState(false);
  const [busy, setBusy] = useState(false);
  const go = async () => {
    const base = { database: target.db, collection: target.coll };
    const op: MongoWrite =
      kind === 'insert'
        ? { type: 'insert', ...base, documents: body }
        : kind === 'update'
          ? { type: 'update', ...base, filter, update: body, many, upsert }
          : kind === 'replace'
            ? { type: 'replace', ...base, filter, replacement: body }
            : { type: 'delete', ...base, filter, many };
    setBusy(true);
    try {
      const r = await dbApi.mongoWrite(session.id, op);
      toast.success('Done', JSON.stringify(r));
      onDone();
    } catch (e) {
      toast.error('Write failed', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const emptyFilterDelete = kind === 'delete' && many && /^\s*(\{\s*\})?\s*$/.test(filter);
  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      title={`${kind[0].toUpperCase()}${kind.slice(1)} — ${target.db}.${target.coll}`}
      footer={
        <>
          {emptyFilterDelete && <span className="text-danger small grow">An empty filter with “many” deletes every document.</span>}
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className={kind === 'delete' ? 'btn btn-danger' : 'btn btn-primary'} onClick={() => void go()} disabled={busy}>
            {busy && <Spinner />} {kind === 'insert' ? 'Insert' : kind === 'update' ? 'Update' : kind === 'delete' ? 'Delete' : 'Replace'}
          </button>
        </>
      }
    >
      <div className="col">
        {kind !== 'insert' && (
          <Field label="Filter">
            <div style={{ height: 70 }}>
              <CodeEditor value={filter} onChange={setFilter} language="javascript" minimal />
            </div>
          </Field>
        )}
        {kind !== 'delete' && (
          <Field label={kind === 'insert' ? 'Documents (object or array)' : kind === 'update' ? 'Update (operators or pipeline)' : 'Replacement'}>
            <div style={{ height: 200 }}>
              <CodeEditor value={body} onChange={setBody} language="javascript" />
            </div>
          </Field>
        )}
        {(kind === 'update' || kind === 'delete') && (
          <div className="row">
            <Switch checked={many} onChange={setMany} label={kind === 'update' ? 'updateMany' : 'deleteMany'} />
            {kind === 'update' && <Switch checked={upsert} onChange={setUpsert} label="upsert" />}
          </div>
        )}
      </div>
    </Modal>
  );
}

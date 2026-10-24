import { useEffect, useState } from 'react';
import { Dices } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Field, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { dataApi, type DataFileEntry } from '@/lib/api/data';
import { dbApi, type SessionInfo } from '@/lib/api/db';
import { useNav } from '@/store/nav';

interface Props {
  open: boolean;
  root: string;
  session: SessionInfo;
  target?: { schema?: string; table?: string };
  onClose(): void;
  onRequestWrite(): void;
  onSeeded?(): void;
}

const ALL = '__all__';

/** "Seed from fake data…": generate an entity of `data/*.data.yaml` and insert it (or every
 * entity, parents first, so foreign keys line up). */
export function SeedDialog({ open, root, session, target, onClose, onRequestWrite, onSeeded }: Props) {
  const go = useNav((s) => s.go);
  const [files, setFiles] = useState<DataFileEntry[] | null>(null);
  const [path, setPath] = useState('');
  const [entity, setEntity] = useState('');
  const [table, setTable] = useState('');
  const [schema, setSchema] = useState('');
  const [count, setCount] = useState('');
  const [busy, setBusy] = useState(false);
  const kind = session.kind;

  useEffect(() => {
    if (!open) return;
    setFiles(null);
    setTable(target?.table ?? '');
    setSchema(target?.schema ?? (kind === 'mongodb' ? (session.database ?? '') : ''));
    dataApi
      .list(root)
      .then((f) => {
        const ok = f.filter((x) => !x.error && x.entities.length);
        setFiles(ok);
        const first = ok[0];
        if (first) {
          setPath(first.path);
          const e = (target?.table && first.entities.find((n) => n.toLowerCase() === target.table?.toLowerCase())) || first.entities[0];
          setEntity(e);
          if (!target?.table) setTable(kind === 'redis' ? `${e}:` : e);
        }
      })
      .catch((e) => {
        setFiles([]);
        toast.error('Could not list data schemas', errorMessage(e));
      });
  }, [open, root, target, kind, session.database]);

  const file = files?.find((f) => f.path === path);
  const all = entity === ALL;
  const seed = async () => {
    setBusy(true);
    try {
      if (all) {
        const rs = await dbApi.seedAll(session.id, root, path, schema || undefined, count ? Number(count) : undefined);
        const rows = rs.reduce((n, r) => n + r.rows, 0);
        toast.success(`Inserted ${rows.toLocaleString()} rows into ${rs.length} tables`, rs.map((r) => `${r.target}: ${r.rows}`).join(' · '));
        onSeeded?.();
        onClose();
        return;
      }
      const r = await dbApi.seed(session.id, root, path, entity, table, schema || undefined, count ? Number(count) : undefined);
      toast.success(`Inserted ${r.rows.toLocaleString()} rows`, `${r.target} · ${Math.round(r.elapsedMs)} ms`);
      onSeeded?.();
      onClose();
    } catch (e) {
      toast.error('Seeding failed', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const tableLabel = kind === 'mongodb' ? 'Collection' : kind === 'redis' ? 'Key prefix (hash per row)' : 'Table';
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Seed from fake data"
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void seed()} disabled={busy || !path || !entity || (!table && !all) || session.readOnly}>
            {busy ? <Spinner /> : <Dices />} Generate & insert
          </button>
        </>
      }
    >
      <div className="col">
        {session.readOnly && (
          <Callout kind="warning">
            Seeding writes data and this session is read-only.{' '}
            <button className="btn btn-sm" onClick={onRequestWrite}>
              Allow writes…
            </button>
          </Callout>
        )}
        {files === null ? (
          <Spinner />
        ) : files.length === 0 ? (
          <Callout>
            No generator schemas in <code className="code-inline">data/</code> yet.{' '}
            <button
              className="btn btn-sm"
              onClick={() => {
                onClose();
                go('fakedata');
              }}
            >
              Open Fake data
            </button>
          </Callout>
        ) : (
          <>
            <div className="grid-2">
              <Field label="Schema file">
                <select
                  className="select"
                  value={path}
                  onChange={(e) => {
                    setPath(e.target.value);
                    const f = files.find((x) => x.path === e.target.value);
                    setEntity(f?.entities[0] ?? '');
                  }}
                >
                  {files.map((f) => (
                    <option key={f.path} value={f.path}>
                      {f.name} (data/{f.path})
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Entity">
                <select className="select" value={entity} onChange={(e) => setEntity(e.target.value)}>
                  {(file?.entities.length ?? 0) > 1 && kind !== 'redis' && <option value={ALL}>All entities (parents first)</option>}
                  {file?.entities.map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              </Field>
            </div>
            <div className="grid-2">
              {kind !== 'redis' && kind !== 'sqlite' && (
                <Field label={kind === 'mongodb' ? 'Database' : 'Schema (optional)'}>
                  <input className="input mono" value={schema} onChange={(e) => setSchema(e.target.value)} placeholder={kind === 'postgres' ? 'public' : kind === 'sqlserver' ? 'dbo' : ''} />
                </Field>
              )}
              {!all && (
                <Field label={tableLabel}>
                  <input className="input mono" value={table} onChange={(e) => setTable(e.target.value)} />
                </Field>
              )}
            </div>
            <Field
              label="Rows"
              hint={
                all
                  ? "Empty = each entity's count. Every entity goes into its source table (or the table named like it), parents first, so references point at inserted rows."
                  : 'Empty = the count in the schema. Fields map to columns by name; parents of ref fields are generated but not inserted.'
              }
            >
              <input className="input" value={count} onChange={(e) => setCount(e.target.value.replace(/\D/g, ''))} placeholder="schema default" style={{ width: 160 }} />
            </Field>
          </>
        )}
      </div>
    </Modal>
  );
}

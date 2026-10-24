import { useEffect, useState } from 'react';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { Download, FileJson, Wand2 } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Modal } from '@/components/ui/Modal';
import { Callout, Field, Segmented, Spinner, Switch, formatBytes } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { dataApi, type DataEntity, type DataSchema, type OutputFormat, type SqlDialect } from '@/lib/api/data';

export type MergeMode = 'add' | 'replace';

const INFER_PLACEHOLDER = `[
  { "id": 1, "firstName": "Ada", "email": "ada@example.com", "status": "active", "createdAt": "2024-05-01T10:20:30Z" },
  { "id": 2, "firstName": "Alan", "email": "alan@example.com", "status": "disabled", "createdAt": "2024-06-01T08:00:00Z" }
]`;

/** Paste sample JSON → one inferred entity. */
export function InferDialog({ open, onClose, onApply, defaultName, current }: { open: boolean; onClose(): void; onApply(entity: DataEntity, mode: MergeMode): void; defaultName: string; current?: string }) {
  const [text, setText] = useState('');
  const [name, setName] = useState(defaultName);
  const [mode, setMode] = useState<MergeMode>('add');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (open) {
      setName(defaultName);
      setError(null);
    }
  }, [open, defaultName]);

  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      const schema = await dataApi.infer(text, mode === 'replace' && current ? current : name);
      onApply(schema.entities[0], mode);
      setText('');
      onClose();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Infer from JSON"
      size="lg"
      footer={
        <>
          <span className="subtle xs grow">Types are guessed from key names and values — review them after.</span>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!text.trim() || busy || (mode === 'add' && !name.trim())} onClick={apply}>
            {busy ? <Spinner /> : <Wand2 />} Infer
          </button>
        </>
      }
    >
      <div className="data-dialog-body">
        <div className="row wrap">
          <Segmented<MergeMode>
            value={mode}
            onChange={setMode}
            options={[
              { value: 'add', label: 'Add as new entity' },
              { value: 'replace', label: current ? `Replace “${current}”` : 'Replace current entity' },
            ]}
          />
          {mode === 'add' && (
            <Field className="grow">
              <input className="input sm" value={name} onChange={(e) => setName(e.target.value)} placeholder="Entity name" aria-label="Entity name" />
            </Field>
          )}
        </div>
        <div className="editor-shell data-dialog-editor" style={{ height: 320 }}>
          <CodeEditor value={text} onChange={setText} language="json" path="data-infer.json" />
        </div>
        {!text && (
          <p className="subtle xs">
            Paste a response body or a sample record, e.g. <code className="code-inline mono">{INFER_PLACEHOLDER.split('\n')[1].trim().slice(0, 70)}…</code>
          </p>
        )}
        {error && <Callout kind="danger">{error}</Callout>}
      </div>
    </Modal>
  );
}

/** Paste JSON Schema / OpenAPI (JSON or YAML) → entities. */
export function JsonSchemaDialog({ open, onClose, onApply }: { open: boolean; onClose(): void; onApply(schema: DataSchema, mode: MergeMode): void }) {
  const [text, setText] = useState('');
  const [mode, setMode] = useState<MergeMode>('add');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const yaml = !!text.trim() && !/^\s*[{[]/.test(text);

  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      onApply(await dataApi.fromJsonSchema(text), mode);
      setText('');
      onClose();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Import JSON Schema or OpenAPI"
      size="lg"
      footer={
        <>
          <span className="subtle xs grow">OpenAPI: one entity per object in components.schemas. $ref, allOf, oneOf, enum, formats, min/max and patterns are honoured.</span>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!text.trim() || busy} onClick={apply}>
            {busy ? <Spinner /> : <FileJson />} Import
          </button>
        </>
      }
    >
      <div className="data-dialog-body">
        <Segmented<MergeMode>
          value={mode}
          onChange={setMode}
          options={[
            { value: 'add', label: 'Add entities' },
            { value: 'replace', label: 'Replace all entities' },
          ]}
        />
        <div className="editor-shell data-dialog-editor" style={{ height: 340 }}>
          <CodeEditor value={text} onChange={setText} language={yaml ? 'yaml' : 'json'} path="data-import-schema.json" />
        </div>
        {error && <Callout kind="danger">{error}</Callout>}
      </div>
    </Modal>
  );
}

type FormatId = OutputFormat['type'];

export function buildFormat(id: FormatId, dialect: SqlDialect, createTable: boolean): OutputFormat {
  switch (id) {
    case 'sql':
      return { type: 'sql', dialect, createTable };
    case 'json':
      return { type: 'json', pretty: true };
    case 'csv':
      return { type: 'csv', header: true };
    default:
      return { type: id };
  }
}

const slug = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '') || 'data';

/** Format + scope, then a native save dialog. Exports read the saved file, so it saves first. */
export function ExportDialog({
  open,
  onClose,
  root,
  path,
  schema,
  entity,
  ensureSaved,
}: {
  open: boolean;
  onClose(): void;
  root: string;
  path: string;
  schema: DataSchema;
  entity?: string;
  ensureSaved(): Promise<boolean>;
}) {
  const [format, setFormat] = useState<FormatId>('json');
  const [dialect, setDialect] = useState<SqlDialect>('postgres');
  const [createTable, setCreateTable] = useState(true);
  const [scope, setScope] = useState<string>('');
  const [busy, setBusy] = useState(false);
  const multi = schema.entities.length > 1 && !scope;
  const ext = format === 'yaml' ? 'yaml' : format;
  const total = schema.entities.filter((e) => !scope || e.name === scope).reduce((a, e) => a + (e.count ?? 0), 0);

  const run = async () => {
    if (!(await ensureSaved())) return;
    const base = slug(schema.name || path.replace(/\.data\.yaml$/, ''));
    const dest = await saveDialog({
      title: 'Export generated data',
      defaultPath: `${base}${scope ? `.${slug(scope)}` : ''}.${ext}`,
      filters: [{ name: ext.toUpperCase(), extensions: [ext] }],
    });
    if (!dest) return;
    setBusy(true);
    try {
      const res = await dataApi.exportFile(root, path, buildFormat(format, dialect, createTable), dest, scope || undefined);
      toast.success(`Exported ${res.rows.toLocaleString()} rows`, `${res.files.length > 1 ? `${res.files.length} files, ` : ''}${formatBytes(res.bytes)} · seed ${res.seed}`, {
        label: 'Show in folder',
        run: () => void revealItemInDir(res.files[0]).catch((e) => toast.error('Could not reveal', errorMessage(e))),
      });
      onClose();
    } catch (e) {
      toast.error('Export failed', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Export data"
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy || schema.entities.length === 0} onClick={run}>
            {busy ? <Spinner /> : <Download />} Export…
          </button>
        </>
      }
    >
      <div className="col">
        <Field label="Format">
          <Segmented<FormatId>
            value={format}
            onChange={setFormat}
            options={[
              { value: 'json', label: 'JSON' },
              { value: 'ndjson', label: 'NDJSON' },
              { value: 'csv', label: 'CSV' },
              { value: 'sql', label: 'SQL' },
              { value: 'yaml', label: 'YAML' },
            ]}
          />
        </Field>
        {format === 'sql' && (
          <div className="row wrap">
            <Field label="Dialect">
              <select className="select sm" value={dialect} onChange={(e) => setDialect(e.target.value as SqlDialect)}>
                <option value="postgres">PostgreSQL</option>
                <option value="mysql">MySQL / MariaDB</option>
                <option value="sqlite">SQLite</option>
                <option value="sqlserver">SQL Server</option>
              </select>
            </Field>
            <Switch checked={createTable} onChange={setCreateTable} label="CREATE TABLE statements (types, keys, foreign keys)" />
          </div>
        )}
        <Field label="Entities" hint={multi && (format === 'csv' || format === 'ndjson') ? 'One file per entity: <name>.<entity>.' + ext : undefined}>
          <select className="select sm" value={scope} onChange={(e) => setScope(e.target.value)}>
            <option value="">All entities ({schema.entities.length})</option>
            {schema.entities.map((e) => (
              <option key={e.name} value={e.name}>
                {e.name}
                {e.name === entity ? ' (current)' : ''}
              </option>
            ))}
          </select>
        </Field>
        <Callout>
          {total.toLocaleString()} rows{scope ? '' : ' in total'}, generated from {schema.seed !== undefined ? `seed ${schema.seed} (reproducible)` : 'a random seed (shown after export)'}. Parents are written before children, so SQL loads with foreign keys on.
        </Callout>
      </div>
    </Modal>
  );
}

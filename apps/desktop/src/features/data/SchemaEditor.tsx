import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { Database, Dices, Download, Eye, EyeOff, FileJson, FolderOpen, MoreHorizontal, Pencil, Plus, Save, Trash2, Wand2, X, CopyPlus } from 'lucide-react';
import { CodeEditor, type CodeLanguage } from '@/components/editor/CodeEditor';
import { Menu } from '@/components/ui/Menu';
import { Callout, CopyButton, Empty, Segmented, Spinner, MOD } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { DATA_LOCALES, dataApi, type DataEntity, type DataField, type DataSchema, type SqlDialect } from '@/lib/api/data';
import { FieldTable } from './FieldTable';
import { ExportDialog, InferDialog, JsonSchemaDialog, buildFormat, type MergeMode } from './dialogs';
import { newEntity, uniqueName } from './fieldTypes';
import { useActiveViewRef } from '@/store/nav';

type PreviewFormat = 'json' | 'csv' | 'sql' | 'yaml';
const PREVIEW_LANG: Record<PreviewFormat, CodeLanguage> = { json: 'json', csv: 'plaintext', sql: 'plaintext', yaml: 'yaml' };
const randomSeed = () => Math.floor(Math.random() * 1_000_000_000) + 1;

/** Renames `ref` targets when an entity is renamed. */
function renameRefs(fields: DataField[], from: string, to: string): DataField[] {
  return fields.map((f) => {
    if (f.type === 'ref' && f.entity === from) return { ...f, entity: to };
    if (f.type === 'object') return { ...f, fields: renameRefs(f.fields, from, to) };
    if (f.type === 'array' && f.of.type === 'object') return { ...f, of: { ...f.of, fields: renameRefs(f.of.fields, from, to) } };
    return f;
  });
}

export function SchemaEditor({ root, path, onSaved, onDeleted, onDirtyChange }: { root: string; path: string; onSaved(): void; onDeleted(): void; onDirtyChange(dirty: boolean): void }) {
  const [draft, setDraft] = useState<DataSchema | null>(null);
  const [saved, setSaved] = useState('');
  const [loadError, setLoadError] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const [dialog, setDialog] = useState<'infer' | 'schema' | 'export' | null>(null);

  // Preview
  const [showPreview, setShowPreview] = useState(true);
  const [format, setFormat] = useState<PreviewFormat>('json');
  const [dialect, setDialect] = useState<SqlDialect>('postgres');
  const [scope, setScope] = useState<'entity' | 'all'>('entity');
  const [rows, setRows] = useState(20);
  const [sessionSeed] = useState(randomSeed);
  const [preview, setPreview] = useState('');
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [generating, setGenerating] = useState(false);
  const seq = useRef(0);

  useEffect(() => {
    let alive = true;
    dataApi
      .read(root, path)
      .then((s) => {
        if (!alive) return;
        const schema = { ...s, entities: s.entities ?? [] };
        setDraft(schema);
        setSaved(JSON.stringify(schema));
        setActive(0);
      })
      .catch((e) => alive && setLoadError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, [root, path]);

  const dirty = !!draft && JSON.stringify(draft) !== saved;
  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange]);

  const save = useCallback(async () => {
    if (!draft) return false;
    try {
      await dataApi.save(root, path, draft);
      setSaved(JSON.stringify(draft));
      onSaved();
      return true;
    } catch (e) {
      toast.error('Save failed', errorMessage(e));
      return false;
    }
  }, [draft, root, path, onSaved]);

  const activeView = useActiveViewRef();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!activeView.current) return;
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
        e.preventDefault();
        void save();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [save]);

  const entity: DataEntity | undefined = draft?.entities[Math.min(active, (draft?.entities.length ?? 1) - 1)];
  const activeIndex = draft ? Math.min(active, draft.entities.length - 1) : 0;

  // Live preview, debounced. Unseeded schemas use a per-session seed so the preview stays put.
  const previewKey = useMemo(() => (draft ? JSON.stringify(draft) : ''), [draft]);
  useEffect(() => {
    if (!draft || !showPreview) return;
    const n = ++seq.current;
    if (draft.entities.length === 0) {
      setPreview('');
      setPreviewError(null);
      return;
    }
    setGenerating(true);
    const t = setTimeout(() => {
      dataApi
        .generate({
          root,
          schema: { ...draft, seed: draft.seed ?? sessionSeed },
          entity: scope === 'entity' ? entity?.name : undefined,
          limit: rows,
          format: buildFormat(format, dialect, false),
        })
        .then((text) => n === seq.current && (setPreview(text), setPreviewError(null)))
        .catch((e) => n === seq.current && setPreviewError(errorMessage(e)))
        .finally(() => n === seq.current && setGenerating(false));
    }, 250);
    return () => clearTimeout(t);
  }, [previewKey, showPreview, format, dialect, scope, rows, entity?.name, sessionSeed, root]);

  if (loadError)
    return (
      <Empty icon={<Database />} title="Could not open this schema">
        {loadError}
      </Empty>
    );
  if (!draft)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );

  const patch = (p: Partial<DataSchema>) => setDraft({ ...draft, ...p });
  const setEntities = (entities: DataEntity[]) => patch({ entities });
  const patchEntity = (i: number, e: DataEntity) => setEntities(draft.entities.map((x, j) => (j === i ? e : x)));
  const names = draft.entities.map((e) => e.name);

  const addEntity = async () => {
    const name = await promptDialog({
      title: 'New entity',
      label: 'Name (table / collection)',
      initial: uniqueName(draft.entities.length ? 'items' : 'users', names),
      validate: (v) => (!v.trim() ? 'Required' : names.includes(v.trim()) ? 'Already exists' : undefined),
    });
    if (!name) return;
    setEntities([...draft.entities, newEntity(name.trim())]);
    setActive(draft.entities.length);
  };

  const renameEntity = async (i: number) => {
    const old = draft.entities[i].name;
    const name = await promptDialog({
      title: 'Rename entity',
      label: 'References to it are updated too',
      initial: old,
      validate: (v) => (!v.trim() ? 'Required' : v.trim() !== old && names.includes(v.trim()) ? 'Already exists' : undefined),
    });
    if (!name || name.trim() === old) return;
    const to = name.trim();
    setEntities(draft.entities.map((e, j) => ({ ...(j === i ? { ...e, name: to } : e), fields: renameRefs(e.fields, old, to) })));
  };

  const duplicateEntity = (i: number) => {
    const copy = { ...structuredClone(draft.entities[i]), name: uniqueName(`${draft.entities[i].name}_copy`, names) };
    setEntities([...draft.entities.slice(0, i + 1), copy, ...draft.entities.slice(i + 1)]);
    setActive(i + 1);
  };

  const removeEntity = async (i: number) => {
    const e = draft.entities[i];
    const referenced = draft.entities.some((x) => x.name !== e.name && JSON.stringify(x.fields).includes(`"entity":${JSON.stringify(e.name)}`));
    const ok = await confirmDialog({
      title: `Remove entity “${e.name}”?`,
      message: referenced ? 'Other entities reference it — their ref fields will need a new target.' : `Its ${e.fields.length} field definitions are removed from this schema (save to apply).`,
      confirmText: 'Remove',
      danger: true,
    });
    if (!ok) return;
    setEntities(draft.entities.filter((_, j) => j !== i));
    setActive(Math.max(0, i - 1));
  };

  const applyInferred = (inferred: DataEntity, mode: MergeMode) => {
    if (mode === 'replace' && entity) {
      patchEntity(activeIndex, { ...entity, fields: inferred.fields });
    } else {
      setEntities([...draft.entities, { ...inferred, name: uniqueName(inferred.name, names) }]);
      setActive(draft.entities.length);
    }
  };

  const applyImported = (imported: DataSchema, mode: MergeMode) => {
    if (mode === 'replace') {
      setDraft({ ...draft, entities: imported.entities });
      setActive(0);
      return;
    }
    const taken = [...names];
    const added = imported.entities.map((e) => {
      const name = uniqueName(e.name, taken);
      taken.push(name);
      return { ...e, name };
    });
    setEntities([...draft.entities, ...added]);
    setActive(draft.entities.length);
  };

  const remove = async () => {
    const ok = await confirmDialog({ title: `Delete “${draft.name || path}”?`, message: `data/${path} moves to your system trash.`, confirmText: 'Move to trash', danger: true });
    if (!ok) return;
    try {
      await dataApi.remove(root, path);
      onDeleted();
    } catch (e) {
      toast.error('Delete failed', errorMessage(e));
    }
  };

  const ensureSaved = async () => (dirty ? save() : true);

  return (
    <div className="data-editor">
      <div className="page-header">
        <input className="name-input" value={draft.name} placeholder="Schema name" onChange={(e) => patch({ name: e.target.value })} aria-label="Schema name" />
        <span className="subtle xs mono truncate">data/{path}</span>
        <span className="spacer" />
        <div className="row-sm" title="Same seed + same schema = same data. Empty = a new random seed every run.">
          <span className="subtle xs">Seed</span>
          <input
            className="input sm data-seed mono"
            type="number"
            min={0}
            value={draft.seed ?? ''}
            placeholder="random"
            onChange={(e) => {
              const v = e.target.value === '' ? undefined : Math.max(0, Math.min(Number.MAX_SAFE_INTEGER, Math.floor(Number(e.target.value))));
              const next = { ...draft };
              if (v === undefined || Number.isNaN(v)) delete next.seed;
              else next.seed = v;
              setDraft(next);
            }}
            aria-label="Seed"
          />
          <button className="icon-btn sm" onClick={() => patch({ seed: randomSeed() })} title="New random seed" aria-label="New random seed">
            <Dices />
          </button>
        </div>
        <select className="select sm" value={draft.locale ?? 'en'} onChange={(e) => {
          const next = { ...draft };
          if (e.target.value === 'en') delete next.locale;
          else next.locale = e.target.value;
          setDraft(next);
        }} aria-label="Locale" title="Locale for names, addresses, phone numbers, …">
          {DATA_LOCALES.map(([code, label]) => (
            <option key={code} value={code}>
              {label}
            </option>
          ))}
        </select>
        <button className="btn btn-sm" onClick={() => setDialog('export')} disabled={draft.entities.length === 0}>
          <Download /> Export
        </button>
        <button className="btn btn-sm btn-primary" disabled={!dirty} onClick={() => void save()} title={`Save (${MOD}+S)`}>
          <Save /> Save
        </button>
        <Menu
          align="right"
          trigger={(p) => (
            <button className="icon-btn" {...p} aria-label="More">
              <MoreHorizontal />
            </button>
          )}
          items={[
            { label: 'Reveal file', icon: <FolderOpen />, onSelect: () => void revealItemInDir(`${root}/data/${path}`).catch((e) => toast.error('Could not reveal', errorMessage(e))) },
            { separator: true },
            { label: 'Delete schema', icon: <Trash2 />, danger: true, onSelect: () => void remove() },
          ]}
        />
      </div>

      <div className="tabbar" role="tablist">
        {draft.entities.map((e, i) => (
          <div
            key={i}
            className={clsx('etab data-entity-tab', i === activeIndex && 'on')}
            onClick={() => setActive(i)}
            onDoubleClick={() => void renameEntity(i)}
            role="tab"
            aria-selected={i === activeIndex}
            title={`${e.name} — double-click to rename`}
          >
            <span className="truncate grow mono">{e.name}</span>
            <span className="count">{(e.count ?? 0).toLocaleString()}</span>
            <button
              className="close"
              onClick={(ev) => {
                ev.stopPropagation();
                void removeEntity(i);
              }}
              aria-label={`Remove ${e.name}`}
            >
              <X />
            </button>
          </div>
        ))}
        <button className="icon-btn" style={{ alignSelf: 'center', marginLeft: 4 }} onClick={() => void addEntity()} title="New entity" aria-label="New entity">
          <Plus />
        </button>
      </div>

      {entity ? (
        <>
          <div className="toolbar">
            <span className="subtle xs">Rows</span>
            <input
              className="input sm data-count"
              type="number"
              min={0}
              value={entity.count}
              onChange={(e) => patchEntity(activeIndex, { ...entity, count: Math.max(0, Math.floor(Number(e.target.value) || 0)) })}
              aria-label="Row count"
            />
            <button className="btn btn-sm btn-ghost" onClick={() => void renameEntity(activeIndex)}>
              <Pencil /> Rename
            </button>
            <button className="btn btn-sm btn-ghost" onClick={() => duplicateEntity(activeIndex)}>
              <CopyPlus /> Duplicate
            </button>
            <span className="spacer" />
            <button className="btn btn-sm" onClick={() => setDialog('infer')} title="Guess fields from a sample JSON body">
              <Wand2 /> Infer from JSON…
            </button>
            <button className="btn btn-sm" onClick={() => setDialog('schema')} title="Entities from JSON Schema or OpenAPI">
              <FileJson /> Import JSON Schema…
            </button>
            <button className={clsx('icon-btn', showPreview && 'active')} onClick={() => setShowPreview(!showPreview)} title={showPreview ? 'Hide preview' : 'Show preview'} aria-label="Toggle preview">
              {showPreview ? <Eye /> : <EyeOff />}
            </button>
          </div>
          <div className={clsx('data-main', !showPreview && 'no-preview')}>
            <div className="data-fields">
              <FieldTable fields={entity.fields} onChange={(fields) => patchEntity(activeIndex, { ...entity, fields })} ctx={{ schema: draft, entity: entity.name }} />
            </div>
            {showPreview && (
              <div className="data-preview">
                <div className="toolbar">
                  <span className="strong small">Preview</span>
                  <Segmented<PreviewFormat>
                    value={format}
                    onChange={setFormat}
                    options={[
                      { value: 'json', label: 'JSON' },
                      { value: 'csv', label: 'CSV' },
                      { value: 'sql', label: 'SQL' },
                      { value: 'yaml', label: 'YAML' },
                    ]}
                  />
                  {format === 'sql' && (
                    <select className="select sm" value={dialect} onChange={(e) => setDialect(e.target.value as SqlDialect)} aria-label="SQL dialect">
                      <option value="postgres">PostgreSQL</option>
                      <option value="mysql">MySQL</option>
                      <option value="sqlite">SQLite</option>
                      <option value="sqlserver">SQL Server</option>
                    </select>
                  )}
                  <select className="select sm" value={scope} onChange={(e) => setScope(e.target.value as 'entity' | 'all')} aria-label="Preview scope">
                    <option value="entity">{entity.name}</option>
                    <option value="all">All entities</option>
                  </select>
                  <select className="select sm" value={rows} onChange={(e) => setRows(Number(e.target.value))} aria-label="Preview rows">
                    {[5, 20, 100, 1000].map((n) => (
                      <option key={n} value={n}>
                        first {n}
                      </option>
                    ))}
                  </select>
                  {generating && <Spinner />}
                  <span className="spacer" />
                  {draft.seed === undefined && <span className="subtle xs">random seed per run</span>}
                  <CopyButton text={preview} label="Copy" title="Copy the preview" />
                </div>
                <div className="data-preview-body">
                  {previewError ? (
                    <Callout kind="danger">{previewError}</Callout>
                  ) : (
                    <div className="editor-shell fill">
                      <CodeEditor value={preview} language={PREVIEW_LANG[format]} readOnly minimal path={`data-preview.${format}`} />
                    </div>
                  )}
                </div>
              </div>
            )}
          </div>
        </>
      ) : (
        <Empty
          icon={<Database />}
          title="No entities yet"
          action={
            <div className="row-sm">
              <button className="btn btn-sm btn-primary" onClick={() => void addEntity()}>
                <Plus /> Add entity
              </button>
              <button className="btn btn-sm" onClick={() => setDialog('infer')}>
                <Wand2 /> Infer from JSON…
              </button>
              <button className="btn btn-sm" onClick={() => setDialog('schema')}>
                <FileJson /> Import JSON Schema…
              </button>
            </div>
          }
        >
          An entity is a table or collection: a row count and a list of fields. Reference fields link entities, and the generator fills parents first.
        </Empty>
      )}

      <InferDialog open={dialog === 'infer'} onClose={() => setDialog(null)} onApply={applyInferred} defaultName={uniqueName('items', names)} current={entity?.name} />
      <JsonSchemaDialog open={dialog === 'schema'} onClose={() => setDialog(null)} onApply={applyImported} />
      <ExportDialog open={dialog === 'export'} onClose={() => setDialog(null)} root={root} path={path} schema={draft} entity={entity?.name} ensureSaved={ensureSaved} />
    </div>
  );
}

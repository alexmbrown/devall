import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { History, KeyRound, Trash2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Callout, CopyButton, Empty, RelTime, SearchInput, Spinner, Segmented } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import type { EditPreview, TableDetails } from '@/lib/api/db';
import { clearHistory, readHistory, type HistoryItem } from './store';
import { fmtMs } from './format';

export function TableInfoModal({ open, onClose, details, loading, error }: { open: boolean; onClose(): void; details?: TableDetails; loading: boolean; error?: string }) {
  const [tab, setTab] = useState<'columns' | 'indexes' | 'keys' | 'ddl'>('columns');
  return (
    <Modal open={open} onClose={onClose} size="xl" title={details ? `${details.schema ? `${details.schema}.` : ''}${details.name}` : 'Table'} bodyClassName="db-modal-body">
      {loading ? (
        <div className="empty">
          <Spinner />
        </div>
      ) : error ? (
        <Callout kind="danger">{error}</Callout>
      ) : details ? (
        <div className="col h-full" style={{ minHeight: 0 }}>
          <Segmented
            value={tab}
            onChange={setTab}
            options={[
              { value: 'columns', label: `Columns (${details.columns.length})` },
              { value: 'indexes', label: `Indexes (${details.indexes.length})` },
              { value: 'keys', label: `Foreign keys (${details.foreignKeys.length})` },
              { value: 'ddl', label: 'DDL' },
            ]}
          />
          <div className="grow scroll" style={{ minHeight: 0 }}>
            {tab === 'columns' && (
              <table className="table">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Type</th>
                    <th>Null</th>
                    <th>Default</th>
                  </tr>
                </thead>
                <tbody>
                  {details.columns.map((c) => (
                    <tr key={c.name}>
                      <td className="mono">
                        {c.primaryKey && <KeyRound size={11} className="text-warning" />} {c.name}
                      </td>
                      <td className="mono subtle">{c.dataType}</td>
                      <td>{c.nullable ? 'yes' : <span className="strong">NOT NULL</span>}</td>
                      <td className="mono subtle">{c.default ?? ''}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {tab === 'indexes' &&
              (details.indexes.length ? (
                <table className="table">
                  <thead>
                    <tr>
                      <th>Name</th>
                      <th>Columns</th>
                      <th>Kind</th>
                      <th>Definition</th>
                    </tr>
                  </thead>
                  <tbody>
                    {details.indexes.map((i) => (
                      <tr key={i.name}>
                        <td className="mono">{i.name}</td>
                        <td className="mono">{i.columns.join(', ')}</td>
                        <td>
                          {i.primary ? <span className="badge warning">PRIMARY</span> : i.unique ? <span className="badge accent">UNIQUE</span> : <span className="badge">INDEX</span>} {i.method && <span className="subtle small">{i.method}</span>}
                        </td>
                        <td className="mono subtle small">{i.definition ?? ''}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <Empty title="No indexes" />
              ))}
            {tab === 'keys' &&
              (details.foreignKeys.length ? (
                <table className="table">
                  <thead>
                    <tr>
                      <th>Name</th>
                      <th>Columns</th>
                      <th>References</th>
                      <th>On update / delete</th>
                    </tr>
                  </thead>
                  <tbody>
                    {details.foreignKeys.map((f) => (
                      <tr key={f.name}>
                        <td className="mono">{f.name}</td>
                        <td className="mono">{f.columns.join(', ')}</td>
                        <td className="mono">
                          {f.refSchema ? `${f.refSchema}.` : ''}
                          {f.refTable} ({f.refColumns.join(', ')})
                        </td>
                        <td className="small">
                          {f.onUpdate ?? '—'} / {f.onDelete ?? '—'}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <Empty title="No foreign keys" />
              ))}
            {tab === 'ddl' && (
              <div className="col h-full">
                <div className="row-sm">
                  <span className="grow subtle small">{details.ddl ? 'Generated from the catalog' : 'Not available'}</span>
                  {details.ddl && <CopyButton text={details.ddl} label="Copy" />}
                </div>
                <div style={{ height: 380 }}>
                  <CodeEditor value={details.ddl ?? ''} language="sql" readOnly />
                </div>
              </div>
            )}
          </div>
        </div>
      ) : null}
    </Modal>
  );
}

export function EditReviewModal({ preview, applying, onApply, onClose }: { preview: EditPreview | null; applying: boolean; onApply(): void; onClose(): void }) {
  return (
    <Modal
      open={!!preview}
      onClose={onClose}
      size="lg"
      title="Review changes"
      footer={
        <>
          <span className="grow subtle small">Runs in one transaction; any update or delete that doesn't hit exactly one row rolls everything back.</span>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={onApply} disabled={applying} autoFocus>
            {applying ? <Spinner /> : null} Apply {preview?.statements.length ?? 0} change{preview?.statements.length === 1 ? '' : 's'}
          </button>
        </>
      }
    >
      <div style={{ height: 320 }}>{preview && <CodeEditor value={preview.script} language="sql" readOnly />}</div>
    </Modal>
  );
}

export function HistoryModal({ open, root, connection, onPick, onClose }: { open: boolean; root: string; connection: string; onPick(sql: string): void; onClose(): void }) {
  const [items, setItems] = useState<HistoryItem[]>([]);
  const [q, setQ] = useState('');
  const [all, setAll] = useState(false);
  useEffect(() => {
    if (open) setItems(readHistory(root));
  }, [open, root]);
  const shown = useMemo(
    () => items.filter((h) => (all || h.connection === connection) && (!q || h.sql.toLowerCase().includes(q.toLowerCase()))),
    [items, all, connection, q],
  );
  return (
    <Modal
      open={open}
      onClose={onClose}
      size="lg"
      title="Query history"
      headerExtra={
        <button
          className="btn btn-sm btn-danger-ghost"
          onClick={async () => {
            if (await confirmDialog({ title: 'Clear query history?', message: 'History is stored on this computer only.', confirmText: 'Clear', danger: true })) {
              clearHistory(root);
              setItems([]);
            }
          }}
        >
          <Trash2 /> Clear
        </button>
      }
    >
      <div className="col" style={{ minHeight: 0, height: 440 }}>
        <div className="row-sm">
          <SearchInput value={q} onChange={setQ} placeholder="Search history…" className="grow" autoFocus />
          <Segmented
            value={all ? 'all' : 'this'}
            onChange={(v) => setAll(v === 'all')}
            options={[
              { value: 'this', label: 'This connection' },
              { value: 'all', label: 'All' },
            ]}
          />
        </div>
        <div className="list grow" style={{ minHeight: 0 }}>
          {shown.length === 0 ? (
            <Empty icon={<History />} title="No queries yet">
              Queries you run are kept here (on this computer, never in the project).
            </Empty>
          ) : (
            shown.map((h, i) => (
              <div
                key={i}
                className="list-item db-history-item"
                onClick={() => {
                  onPick(h.sql);
                  onClose();
                }}
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    onPick(h.sql);
                    onClose();
                  }
                }}
              >
                <span className={clsx('dot', h.ok ? 'ok' : 'err')} />
                <div className="grow" style={{ minWidth: 0 }}>
                  <div className="mono small truncate">{h.sql.replace(/\s+/g, ' ')}</div>
                  <div className="meta">
                    <RelTime ts={h.at} /> · {fmtMs(h.ms)}
                    {h.rows !== undefined ? ` · ${h.rows.toLocaleString()} rows` : ''}
                    {all ? ` · ${h.connection}` : ''}
                  </div>
                </div>
              </div>
            ))
          )}
        </div>
      </div>
    </Modal>
  );
}

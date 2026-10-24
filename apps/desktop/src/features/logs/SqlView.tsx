import { useCallback, useEffect, useRef, useState } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { Database, FilePlus2, Play, X } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Callout, CopyButton, Empty, Kbd, MOD, Spinner } from '@/components/ui/misc';
import { promptDialog } from '@/components/ui/dialogs';
import { errorMessage } from '@/lib/ipc';
import { logsApi, type SqlResult, type SqlSource, type TableInfo } from '@/lib/api/logs';
import type { DataApi } from './source';

interface Props {
  api: DataApi;
  sql: string;
  onSql(sql: string): void;
}

function cell(v: unknown): string {
  if (v == null) return '';
  if (typeof v === 'object') return JSON.stringify(v);
  return String(v);
}

function toCsv(r: SqlResult): string {
  const q = (s: string) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);
  return [r.columns.map(q).join(','), ...r.rows.map((row) => row.map((v) => q(cell(v))).join(','))].join('\n');
}

/** SQL over `logs` / `events` (DuckDB, or SQLite fallback) plus attached CSV/JSON/Parquet files. */
export function SqlView({ api, sql, onSql }: Props) {
  const [result, setResult] = useState<SqlResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [tables, setTables] = useState<TableInfo[] | null>(null);
  const [backend, setBackend] = useState('');
  const [extra, setExtra] = useState<SqlSource[]>([]);
  const sqlRef = useRef(sql);
  sqlRef.current = sql;

  useEffect(() => {
    let alive = true;
    setTables(null);
    logsApi
      .sqlTables(api.sqlTarget)
      .then(([b, t]) => {
        if (!alive) return;
        setBackend(b);
        setTables(t);
      })
      .catch((e) => alive && setError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, [api]);

  const run = useCallback(async () => {
    setRunning(true);
    setError(null);
    try {
      const r = await logsApi.sql(api.sqlTarget, sqlRef.current, extra, 2000);
      setResult(r);
      setBackend(r.backend);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRunning(false);
    }
  }, [api, extra]);

  const attach = async () => {
    const picked = await openDialog({ multiple: false, filters: [{ name: 'Data', extensions: ['csv', 'tsv', 'json', 'jsonl', 'ndjson', 'parquet'] }] });
    if (typeof picked !== 'string') return;
    const base = (picked.split(/[\\/]/).pop() ?? 'data').replace(/\.[^.]+$/, '').replace(/[^A-Za-z0-9_]/g, '_').replace(/^(\d)/, '_$1') || 'data';
    const name = await promptDialog({ title: 'Attach as table', label: 'Table name', initial: base, validate: (v) => (/^[A-Za-z_][A-Za-z0-9_]*$/.test(v) ? undefined : 'Letters, digits and _ only') });
    if (!name) return;
    setExtra((x) => [...x.filter((s) => s.name !== name), { type: 'file', name, path: picked }]);
  };

  return (
    <div
      className="logs-sql"
      onKeyDownCapture={(e) => {
        if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
          e.preventDefault();
          e.stopPropagation();
          void run();
        }
      }}
    >
      <div className="logs-sql-top">
        <div className="logs-sql-editor">
          <CodeEditor value={sql} onChange={onSql} language="sql" path={`logs-${api.key}.sql`} minimal lineNumbers />
        </div>
        <aside className="logs-sql-tables">
          <div className="toolbar">
            <Database size={14} />
            <span className="strong small grow">Tables</span>
            {backend && <span className="tag" title="SQL engine">{backend === 'duckdb' ? 'DuckDB' : 'SQLite'}</span>}
          </div>
          <div className="logs-sql-tables-body">
            {!tables ? (
              <Spinner />
            ) : (
              tables.map((t) => (
                <details key={t.name} open={t.name === 'logs' || t.name === 'events'}>
                  <summary className="mono">{t.name}</summary>
                  {t.columns.map(([c, ty]) => (
                    <div key={c} className="col mono small">
                      <span>{c}</span>
                      <span className="subtle">{ty.toLowerCase()}</span>
                    </div>
                  ))}
                </details>
              ))
            )}
            {extra.map((s) => (
              <div key={s.name} className="row-sm small">
                <span className="mono grow truncate" title={s.type === 'file' ? s.path : s.name}>
                  {s.name}
                </span>
                <button className="icon-btn sm" onClick={() => setExtra((x) => x.filter((y) => y.name !== s.name))} aria-label={`Detach ${s.name}`}>
                  <X />
                </button>
              </div>
            ))}
            <p className="subtle xs">
              JSON columns: <code>fields-&gt;&gt;'$.status'</code> or <code>json_extract(fields, '$.user.id')</code>.
              {backend === 'duckdb' && (
                <>
                  {' '}
                  Read files directly: <code>read_csv_auto('path')</code>, <code>read_parquet</code>, <code>read_json_auto</code>.
                </>
              )}
            </p>
          </div>
        </aside>
      </div>
      <div className="toolbar">
        <button className="btn btn-sm btn-primary" onClick={run} disabled={running}>
          {running ? <Spinner /> : <Play />} Run
        </button>
        <span className="small subtle">
          <Kbd>{MOD}</Kbd>+<Kbd>Enter</Kbd>
        </span>
        <button className="btn btn-sm btn-ghost" onClick={attach} title="Attach a CSV/JSON/Parquet file as a table">
          <FilePlus2 /> Attach file…
        </button>
        <span className="grow" />
        {result && (
          <>
            <span className="small subtle">
              {result.rows.length}
              {result.truncated ? '+' : ''} rows · {result.elapsedMs.toFixed(0)} ms
            </span>
            <CopyButton text={() => toCsv(result)} label="CSV" title="Copy results as CSV" />
          </>
        )}
      </div>
      <div className="logs-sql-results">
        {error ? (
          <Callout kind="danger" className="m-3">
            <pre className="logs-sql-error">{error}</pre>
          </Callout>
        ) : !result ? (
          <Empty icon={<Database />} title="SQL over your logs">
            Query the parsed entries as a table. Run with {MOD}+Enter.
          </Empty>
        ) : result.columns.length === 0 ? (
          <div className="p-4 small subtle">Statement executed ({result.statements}).</div>
        ) : (
          <table className="table logs-grid">
            <thead>
              <tr>
                <th className="num">#</th>
                {result.columns.map((c, i) => (
                  <th key={i}>{c}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {result.rows.map((row, i) => (
                <tr key={i}>
                  <td className="num subtle">{i + 1}</td>
                  {row.map((v, j) => (
                    <td key={j} className={typeof v === 'number' ? 'num' : undefined} title={cell(v).length > 80 ? cell(v) : undefined}>
                      {v == null ? <span className="subtle">null</span> : cell(v)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

/**
 * "Data source lookup": pick a CSV / JSON / SQLite source from `mocks/data/` and either insert a
 * `{{dataSource …}}` helper into the body or make it the stub's lookup
 * (`transformerParameters.dataSource` → `{{row.x}}` / `{{#each data}}` in the template).
 * Rendered by the /dev/all engine only (stock WireMock leaves the helpers unrendered).
 */
import { useEffect, useMemo, useState } from 'react';
import { Table2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Empty, Field, Segmented, Spinner } from '@/components/ui/misc';
import { mockApi, type DataSourceInfo } from '@/lib/api/mock';
import { useWorkspace } from '@/store/workspace';
import { dataSourceSnippet, lookupConfig, type DataLookup } from '@/features/mocks/lib/dataSource';

const VALUE_EXPRESSIONS = ['request.path.id', 'request.pathSegments.[1]', 'request.query.id', "request.headers.X-User-Id", "(jsonPath request.body '$.id')"];

export function DataSourceButton({ onInsert, onLookup }: { onInsert(snippet: string): void; onLookup(config: DataLookup): void }) {
  const root = useWorkspace((s) => s.project?.root ?? null);
  const [open, setOpen] = useState(false);
  const [sources, setSources] = useState<DataSourceInfo[] | null>(null);
  const [name, setName] = useState('');
  const [column, setColumn] = useState('');
  const [expr, setExpr] = useState(VALUE_EXPRESSIONS[0]);
  const [mode, setMode] = useState<'lookup' | 'helper'>('lookup');
  const [notFound, setNotFound] = useState(true);

  useEffect(() => {
    if (!open || !root) return;
    setSources(null);
    mockApi
      .dataSources(root)
      .then((s) => {
        setSources(s);
        const first = s.find((x) => !x.error);
        if (first && !s.some((x) => x.name === name)) {
          setName(first.name);
          setColumn(first.columns[0] ?? '');
        }
      })
      .catch(() => setSources([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, root]);

  const source = useMemo(() => sources?.find((s) => s.name === name), [sources, name]);
  const snippet = dataSourceSnippet(name, column, expr);
  const config = lookupConfig(name, column, expr, notFound);

  const apply = () => {
    if (!name) return;
    if (mode === 'helper') onInsert(snippet);
    else onLookup(config);
    setOpen(false);
  };

  return (
    <>
      <button className="btn btn-sm btn-ghost" onClick={() => setOpen(true)} title="Look up rows from a CSV / JSON / SQLite file in mocks/data (turns templating on)">
        <Table2 /> Data source
      </button>
      {open && (
        <Modal
          open
          onClose={() => setOpen(false)}
          title="Data source lookup"
          size="lg"
          footer={
            <>
              <button className="btn" onClick={() => setOpen(false)}>
                Cancel
              </button>
              <button className="btn btn-primary" disabled={!name || !column} onClick={apply}>
                {mode === 'helper' ? 'Insert helper' : 'Use as stub lookup'}
              </button>
            </>
          }
        >
          {!sources ? (
            <Spinner />
          ) : !sources.length ? (
            <Empty icon={<Table2 />} title="No data sources yet">
              Put a <code className="code-inline">.csv</code> (header row), <code className="code-inline">.json</code> (array of objects) or SQLite{' '}
              <code className="code-inline">.db</code> file in <code className="code-inline">mocks/data/</code> — e.g. <code className="code-inline">mocks/data/users.csv</code> — then look rows up
              by a request value.
            </Empty>
          ) : (
            <div className="col">
              <div className="grid-2">
                <Field label="Source" hint={source ? `${source.file} · ${source.rows} rows` : undefined} error={source?.error}>
                  <select
                    className="select"
                    value={name}
                    onChange={(e) => {
                      setName(e.target.value);
                      setColumn(sources.find((s) => s.name === e.target.value)?.columns[0] ?? '');
                    }}
                  >
                    {sources.map((s) => (
                      <option key={s.name} value={s.name} disabled={!!s.error}>
                        {s.name} ({s.kind})
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Match column">
                  <select className="select" value={column} onChange={(e) => setColumn(e.target.value)}>
                    {(source?.columns ?? []).map((c) => (
                      <option key={c} value={c}>
                        {c}
                      </option>
                    ))}
                  </select>
                </Field>
              </div>
              <Field label="…equal to (template expression)">
                <input className="input mono" list="ds-exprs" value={expr} onChange={(e) => setExpr(e.target.value)} />
                <datalist id="ds-exprs">
                  {VALUE_EXPRESSIONS.map((v) => (
                    <option key={v} value={v} />
                  ))}
                </datalist>
              </Field>
              <Segmented
                value={mode}
                onChange={setMode}
                options={[
                  { value: 'lookup', label: 'Stub lookup (row / data)' },
                  { value: 'helper', label: 'Inline helper' },
                ]}
              />
              {mode === 'lookup' ? (
                <>
                  <Checkbox checked={notFound} onChange={setNotFound} label="Answer 404 when no row matches" />
                  <div className="subtle xs">
                    Sets <code className="code-inline">transformerParameters.dataSource</code>; the body can use <code className="code-inline">{'{{row.' + (source?.columns[1] ?? 'name') + '}}'}</code>,{' '}
                    <code className="code-inline">{'{{#if row}}…{{else}}…{{/if}}'}</code> and <code className="code-inline">{'{{#each data}}'}</code> (every match).
                  </div>
                  <pre className="code">{JSON.stringify({ dataSource: config }, null, 2)}</pre>
                </>
              ) : (
                <>
                  <div className="subtle xs">Inserted at the end of the body; renders the matching row as JSON. As a sub-expression it is an object.</div>
                  <pre className="code">{snippet}</pre>
                </>
              )}
              <Callout kind="info">Data sources are a /dev/all engine feature (like WireMock Cloud's); a stock WireMock server leaves these helpers unrendered.</Callout>
            </div>
          )}
        </Modal>
      )}
    </>
  );
}

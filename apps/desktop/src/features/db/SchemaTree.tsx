import { useMemo, useState } from 'react';
import clsx from 'clsx';
import { ChevronDown, ChevronRight, Copy, DatabaseZap, Dices, Eye, FileCode2, Hash, Info, KeyRound, Layers, RefreshCw, Table2 } from 'lucide-react';
import { useContextMenu } from '@/components/ui/Menu';
import { Callout, SearchInput, Spinner, copyText } from '@/components/ui/misc';
import type { Dialect, SchemaTree as Tree, TableNode } from '@/lib/api/db';
import { quoteIdent, selectSql } from './format';

interface Props {
  tree: Tree | null;
  loading: boolean;
  error?: string;
  dialect: Dialect;
  onRefresh(): void;
  onOpenTable(schema: string, table: TableNode): void;
  onShowTable(schema: string, table: TableNode): void;
  onInsert(text: string): void;
  onSeed(schema: string, table: TableNode): void;
  /** A table, or a whole schema when `table` is omitted. */
  onFakeSchema(schema: string, table?: TableNode): void;
}

/** Schemas → tables/views → columns, with a filter box. */
export function SchemaTree({ tree, loading, error, dialect, onRefresh, onOpenTable, onShowTable, onInsert, onSeed, onFakeSchema }: Props) {
  const [filter, setFilter] = useState('');
  const [openSchemas, setOpenSchemas] = useState<Record<string, boolean>>({});
  const [openTables, setOpenTables] = useState<Record<string, boolean>>({});
  const ctx = useContextMenu();

  const q = filter.trim().toLowerCase();
  const schemas = useMemo(() => {
    if (!tree) return [];
    return tree.schemas
      .map((s) => ({ ...s, tables: q ? s.tables.filter((t) => t.name.toLowerCase().includes(q) || t.columns.some((c) => c.name.toLowerCase().includes(q))) : s.tables }))
      .filter((s) => !q || s.tables.length > 0);
  }, [tree, q]);

  const isOpen = (s: string) => openSchemas[s] ?? (q !== '' || s === tree?.defaultSchema || (tree?.schemas.length ?? 0) <= 2);
  const qualified = (schema: string, t: string) => (schema === tree?.defaultSchema ? quoteIdent(t, dialect) : `${quoteIdent(schema, dialect)}.${quoteIdent(t, dialect)}`);

  const tableMenu = (e: React.MouseEvent, schema: string, t: TableNode) =>
    ctx.open(e, [
      { label: 'Open data', icon: <Table2 />, hint: 'Enter', onSelect: () => onOpenTable(schema, t) },
      { label: 'Structure, indexes & DDL', icon: <Info />, onSelect: () => onShowTable(schema, t) },
      { separator: true },
      { label: 'Insert SELECT', icon: <FileCode2 />, onSelect: () => onInsert(selectSql(t.columns.map((c) => quoteIdent(c.name, dialect)).join(', ') || '*', qualified(schema, t.name), 100, dialect)) },
      { label: 'Insert name', icon: <Hash />, onSelect: () => onInsert(qualified(schema, t.name)) },
      { label: 'Copy name', icon: <Copy />, onSelect: () => void copyText(qualified(schema, t.name), 'Copied') },
      ...(t.kind === 'table'
        ? [
            { separator: true },
            { label: 'Seed from fake data…', icon: <Dices />, onSelect: () => onSeed(schema, t) },
            { label: 'Create fake-data schema…', icon: <DatabaseZap />, onSelect: () => onFakeSchema(schema, t) },
          ]
        : []),
    ]);

  const schemaMenu = (e: React.MouseEvent, schema: string) =>
    ctx.open(e, [{ label: 'Create fake-data schema from all tables…', icon: <DatabaseZap />, onSelect: () => onFakeSchema(schema) }]);

  return (
    <div className="db-schema">
      <div className="toolbar">
        <SearchInput value={filter} onChange={setFilter} placeholder="Filter tables…" className="grow" />
        <button className="icon-btn sm" onClick={onRefresh} title="Reload schema" aria-label="Reload schema" disabled={loading}>
          {loading ? <Spinner /> : <RefreshCw />}
        </button>
      </div>
      <div className="tree db-tree" role="tree">
        {error && (
          <Callout kind="danger" className="m-3">
            {error}
          </Callout>
        )}
        {!tree && loading && (
          <div className="empty">
            <Spinner />
          </div>
        )}
        {tree && schemas.length === 0 && <div className="subtle small p-4">{q ? 'No matches' : 'No tables'}</div>}
        {schemas.map((s) => (
          <div key={s.name} role="group">
            <div className="tree-row" style={{ paddingLeft: 6 }} onClick={() => setOpenSchemas((o) => ({ ...o, [s.name]: !isOpen(s.name) }))} role="treeitem" aria-expanded={isOpen(s.name)} tabIndex={0} onKeyDown={(e) => e.key === 'Enter' && setOpenSchemas((o) => ({ ...o, [s.name]: !isOpen(s.name) }))} onContextMenu={(e) => schemaMenu(e, s.name)}>
              {isOpen(s.name) ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
              <Layers size={13} className="subtle" />
              <span className={clsx('truncate grow', s.name === tree?.defaultSchema && 'strong')}>{s.name}</span>
              <span className="count subtle small">{s.tables.length}</span>
            </div>
            {isOpen(s.name) &&
              s.tables.map((t) => {
                const key = `${s.name}.${t.name}`;
                const open = openTables[key] ?? false;
                return (
                  <div key={key}>
                    <div
                      className="tree-row"
                      style={{ paddingLeft: 20 }}
                      role="treeitem"
                      tabIndex={0}
                      aria-expanded={open}
                      title={`${t.kind}${t.rowsEstimate !== undefined ? ` · ~${t.rowsEstimate.toLocaleString()} rows` : ''} — double-click to open`}
                      onClick={() => setOpenTables((o) => ({ ...o, [key]: !open }))}
                      onDoubleClick={() => onOpenTable(s.name, t)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') onOpenTable(s.name, t);
                        if (e.key === 'ArrowRight') setOpenTables((o) => ({ ...o, [key]: true }));
                        if (e.key === 'ArrowLeft') setOpenTables((o) => ({ ...o, [key]: false }));
                      }}
                      onContextMenu={(e) => tableMenu(e, s.name, t)}
                    >
                      {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                      {t.kind === 'table' ? <Table2 size={13} className="text-accent" /> : <Eye size={13} className="subtle" />}
                      <span className="truncate grow">{t.name}</span>
                      {t.rowsEstimate !== undefined && t.rowsEstimate > 0 && <span className="subtle small nowrap">{compact(t.rowsEstimate)}</span>}
                    </div>
                    {open &&
                      t.columns.map((c) => (
                        <div
                          key={c.name}
                          className="tree-row db-col-row"
                          style={{ paddingLeft: 44 }}
                          title={`${c.name} ${c.dataType}${c.nullable ? '' : ' NOT NULL'}${c.default ? ` DEFAULT ${c.default}` : ''}`}
                          onDoubleClick={() => onInsert(quoteIdent(c.name, dialect))}
                        >
                          {c.primaryKey ? <KeyRound size={11} className="text-warning" /> : <span className="db-col-dot" />}
                          <span className="truncate grow">{c.name}</span>
                          <span className="db-type">{c.dataType}</span>
                        </div>
                      ))}
                  </div>
                );
              })}
          </div>
        ))}
      </div>
      {ctx.node}
    </div>
  );
}

function compact(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

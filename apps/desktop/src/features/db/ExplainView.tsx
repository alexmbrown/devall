import { useMemo, useState } from 'react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Callout, Segmented } from '@/components/ui/misc';
import type { ExplainOutput } from '@/lib/api/db';
import { fmtMs } from './format';

type PgNode = Record<string, unknown> & { Plans?: PgNode[] };

interface Flat {
  depth: number;
  node: PgNode;
}

function flatten(node: PgNode, depth: number, out: Flat[]) {
  out.push({ depth, node });
  for (const c of node.Plans ?? []) flatten(c, depth + 1, out);
}

const num = (v: unknown) => (typeof v === 'number' ? v : undefined);

/** Postgres JSON plans as an indented tree with cost/time bars; SQLite plans as a tree; MySQL
 * plans (and everything else) as raw JSON/text. */
export function ExplainView({ explain }: { explain: ExplainOutput }) {
  const [view, setView] = useState<'tree' | 'raw'>('tree');
  const raw = explain.text ?? JSON.stringify(explain.plan, null, 2);
  const pg = useMemo(() => {
    if ((explain.format !== 'postgres' && explain.format !== 'sqlserver') || !Array.isArray(explain.plan)) return null;
    const top = explain.plan[0] as Record<string, unknown> | undefined;
    const plan = top?.Plan as PgNode | undefined;
    if (!plan) return null;
    const rows: Flat[] = [];
    flatten(plan, 0, rows);
    return { rows, planning: num(top?.['Planning Time']), execution: num(top?.['Execution Time']) };
  }, [explain]);

  const analyzed = !!pg?.rows.some((r) => r.node['Actual Total Time'] !== undefined);
  const maxMetric = pg ? Math.max(1e-9, ...pg.rows.map((r) => num(analyzed ? r.node['Actual Total Time'] : r.node['Total Cost']) ?? 0)) : 1;

  return (
    <div className="db-explain">
      <div className="toolbar">
        <span className="strong small">Plan</span>
        {pg?.planning !== undefined && <span className="subtle small">planning {fmtMs(pg.planning)}</span>}
        {pg?.execution !== undefined && <span className="subtle small">· execution {fmtMs(pg.execution)}</span>}
        {explain.rolledBack && <span className="badge info" title="EXPLAIN ANALYZE ran inside a transaction that was rolled back">rolled back</span>}
        <span className="grow" />
        {(pg || explain.format === 'sqlite') && (
          <Segmented
            value={view}
            onChange={setView}
            options={[
              { value: 'tree', label: 'Tree' },
              { value: 'raw', label: 'Raw' },
            ]}
          />
        )}
      </div>
      {view === 'tree' && pg ? (
        <div className="db-explain-tree">
          {pg.rows.map(({ depth, node }, i) => {
            const metric = num(analyzed ? node['Actual Total Time'] : node['Total Cost']) ?? 0;
            const rel = [node['Relation Name'], node['Index Name'] && `using ${node['Index Name']}`, node['Join Type'] && `${node['Join Type']} join`].filter(Boolean).join(' ');
            const estRows = num(node['Plan Rows']);
            const actRows = num(node['Actual Rows']);
            const misestimate = analyzed && estRows !== undefined && actRows !== undefined && estRows > 0 && (actRows / estRows > 10 || estRows / Math.max(actRows, 1) > 10);
            return (
              <div key={i} className="db-plan-node" style={{ paddingLeft: 8 + depth * 18 }}>
                <div className="db-plan-bar" style={{ width: `${Math.max(1, (metric / maxMetric) * 100)}%` }} />
                <span className="strong">{String(node['Node Type'] ?? '?')}</span>
                {rel && <span className="mono small">{rel}</span>}
                <span className="grow" />
                {node['Filter'] !== undefined && <span className="subtle small mono truncate" title={String(node['Filter'])}>filter: {String(node['Filter'])}</span>}
                <span className="subtle small nowrap">cost {node['Startup Cost'] !== undefined ? `${String(node['Startup Cost'])}..` : ''}{String(node['Total Cost'] ?? '')}</span>
                <span className="subtle small nowrap">
                  rows {estRows ?? '?'}
                  {analyzed && <> → <span className={misestimate ? 'text-warning strong' : undefined}>{actRows ?? '?'}</span></>}
                </span>
                {analyzed && <span className="small nowrap">{fmtMs(num(node['Actual Total Time']) ?? 0)}{(num(node['Actual Loops']) ?? 1) > 1 ? ` ×${node['Actual Loops']}` : ''}</span>}
              </div>
            );
          })}
          {analyzed && <div className="subtle small p-4">Times are per loop, inclusive of children. Highlighted row counts are off by more than 10×.</div>}
        </div>
      ) : view === 'tree' && explain.format === 'sqlite' ? (
        <div className="db-explain-tree">
          <SqliteTree nodes={explain.plan as SqliteNode[]} depth={0} />
        </div>
      ) : explain.format === 'sqlserver' ? (
        <div className="grow" style={{ minHeight: 0 }}>
          <CodeEditor value={explain.text ?? ''} language="xml" readOnly />
        </div>
      ) : explain.format === 'text' ? (
        <pre className="db-explain-text">{explain.text}</pre>
      ) : (
        <div className="grow" style={{ minHeight: 0 }}>
          {explain.format === 'mysql' && <Callout className="m-3">MySQL JSON plan — look for full scans (<code className="code-inline">access_type: ALL</code>) and high <code className="code-inline">rows_examined_per_scan</code>.</Callout>}
          <CodeEditor value={raw} language="json" readOnly />
        </div>
      )}
    </div>
  );
}

interface SqliteNode {
  id: number;
  detail: string;
  children: SqliteNode[];
}

function SqliteTree({ nodes, depth }: { nodes: SqliteNode[]; depth: number }) {
  return (
    <>
      {nodes.map((n) => (
        <div key={n.id}>
          <div className="db-plan-node" style={{ paddingLeft: 8 + depth * 18 }}>
            <span className={/SCAN/.test(n.detail) && !/USING/.test(n.detail) ? 'text-warning' : undefined}>{n.detail}</span>
          </div>
          <SqliteTree nodes={n.children} depth={depth + 1} />
        </div>
      ))}
    </>
  );
}

import { useMemo, useState, type KeyboardEvent } from 'react';
import clsx from 'clsx';
import { ChevronRight, CornerDownLeft, Network, RefreshCw } from 'lucide-react';
import { Callout, Empty, SearchInput, Spinner } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { protoApi, type GqlField, type GqlOperation, type GqlOperationKind, type GqlType, type GraphqlSchema } from '@/lib/api/proto';
import type { HttpRequestFile } from '@/lib/types';
import { usePrefs } from '@/store/prefs';
import './proto.css';

/** Schemas loaded this session, per project + request URL. */
const cache = new Map<string, GraphqlSchema>();

export interface GraphqlInsert {
  query: string;
  variables: string;
  operationName: string;
}

interface Props {
  root: string;
  /** The HTTP request's path (for folder headers/auth/variables); '' for unsaved drafts. */
  path: string;
  /** The request as edited (URL, headers, auth and settings are used for introspection). */
  request: HttpRequestFile;
  /** Called with a generated operation when a field is picked. */
  onInsert(op: GraphqlInsert): void;
}

const GROUPS: { kind: GqlOperationKind; label: string }[] = [
  { kind: 'query', label: 'Query' },
  { kind: 'mutation', label: 'Mutation' },
  { kind: 'subscription', label: 'Subscription' },
];

const opName = (op: GqlOperation) => op.query.split(/[\s(]/)[1] ?? '';

/**
 * Schema explorer for a GraphQL request: loads the schema by introspection (only when asked —
 * it's a network call), lists Query/Mutation/Subscription fields as a tree and inserts a
 * generated operation (with variables) for the picked field.
 */
export function GraphqlSchemaPanel({ root, path, request, onInsert }: Props) {
  const env = usePrefs((s) => s.activeEnv[root]);
  const cacheKey = `${root}|${path}|${request.url}`;
  const [schema, setSchema] = useState<GraphqlSchema | null>(() => cache.get(cacheKey) ?? null);
  const [shownKey, setShownKey] = useState(cacheKey);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const [filter, setFilter] = useState('');
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [collapsed, setCollapsed] = useState<Set<GqlOperationKind>>(() => new Set());

  if (shownKey !== cacheKey) {
    setShownKey(cacheKey);
    setSchema(cache.get(cacheKey) ?? null);
    setOpen(new Set());
  }

  const types = useMemo(() => new Map((schema?.types ?? []).map((t) => [t.name, t])), [schema]);
  const ops = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return (schema?.operations ?? []).filter((o) => !q || o.field.toLowerCase().includes(q) || (o.description ?? '').toLowerCase().includes(q));
  }, [schema, filter]);

  const load = async () => {
    setLoading(true);
    setError(undefined);
    try {
      const s = await protoApi.graphqlIntrospect(root, path, request, env);
      cache.set(cacheKey, s);
      setSchema(s);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  };

  const toggle = (k: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(k)) n.delete(k);
      else n.add(k);
      return n;
    });
  const insert = (op: GqlOperation) => onInsert({ query: op.query, variables: op.variables, operationName: opName(op) });

  return (
    <div className="gql-panel">
      <div className="toolbar">
        <Network size={14} />
        <span className="strong small">Schema</span>
        {schema && <SearchInput value={filter} onChange={setFilter} placeholder="Find a field" className="grow" />}
        {!schema && <span className="spacer" />}
        <button className="icon-btn sm" onClick={load} disabled={loading} title={schema ? 'Reload the schema' : 'Load the schema (introspection)'} aria-label="Load schema">
          {loading ? <Spinner /> : <RefreshCw />}
        </button>
      </div>
      {error && (
        <Callout kind="danger" className="m-3">
          <div className="strong">Introspection failed</div>
          <div className="mono xs break">{error}</div>
        </Callout>
      )}
      {!schema ? (
        <Empty
          icon={<Network />}
          title="GraphQL schema"
          action={
            <button className="btn btn-sm btn-primary" onClick={load} disabled={loading}>
              {loading ? <Spinner /> : <RefreshCw />} Load schema
            </button>
          }
        >
          Runs an introspection query against this request’s URL with its headers and auth, then lets you insert ready-made queries.
        </Empty>
      ) : (
        <div className="gql-tree" role="tree" aria-label="GraphQL schema">
          {GROUPS.map(({ kind, label }) => {
            const list = ops.filter((o) => o.kind === kind);
            if (!list.length) return null;
            const isCollapsed = collapsed.has(kind);
            return (
              <div key={kind} role="group">
                <div
                  className="gql-group"
                  onClick={() =>
                    setCollapsed((s) => {
                      const n = new Set(s);
                      if (n.has(kind)) n.delete(kind);
                      else n.add(kind);
                      return n;
                    })
                  }
                >
                  <ChevronRight className={clsx('chev', !isCollapsed && 'open')} size={12} />
                  {label} <span className="subtle">({list.length})</span>
                </div>
                {!isCollapsed &&
                  list.map((op) => {
                    const k = `${kind}.${op.field}`;
                    const rootType = types.get(kind === 'query' ? (schema.queryType ?? '') : kind === 'mutation' ? (schema.mutationType ?? '') : (schema.subscriptionType ?? ''));
                    const field = rootType?.fields?.find((f) => f.name === op.field);
                    return (
                      <div key={k}>
                        <Row
                          depth={0}
                          name={op.field}
                          args={field?.args?.length ?? 0}
                          typeRef={op.typeRef}
                          deprecated={!!op.deprecated}
                          expandable
                          open={open.has(k)}
                          onToggle={() => toggle(k)}
                          onActivate={() => insert(op)}
                          title={op.description ?? `Insert ${op.kind} ${op.field}`}
                          insertable
                        />
                        {open.has(k) && field && <FieldDetail field={field} types={types} path={k} depth={1} open={open} toggle={toggle} />}
                      </div>
                    );
                  })}
              </div>
            );
          })}
          {ops.length === 0 && <div className="plog-more">No fields match.</div>}
        </div>
      )}
    </div>
  );
}

function Row({
  depth,
  name,
  args,
  typeRef,
  deprecated,
  expandable,
  open,
  onToggle,
  onActivate,
  title,
  insertable,
}: {
  depth: number;
  name: string;
  args: number;
  typeRef: string;
  deprecated: boolean;
  expandable: boolean;
  open: boolean;
  onToggle(): void;
  onActivate?(): void;
  title?: string;
  insertable?: boolean;
}) {
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      (onActivate ?? onToggle)();
    } else if (e.key === ' ' || (e.key === 'ArrowRight' && !open) || (e.key === 'ArrowLeft' && open)) {
      if (expandable) {
        e.preventDefault();
        onToggle();
      }
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const rows = Array.from(e.currentTarget.closest('.gql-tree')?.querySelectorAll<HTMLElement>('.gql-row') ?? []);
      const i = rows.indexOf(e.currentTarget);
      rows[i + (e.key === 'ArrowDown' ? 1 : -1)]?.focus();
    }
  };
  return (
    <div
      className={clsx('gql-row', deprecated && 'deprecated')}
      style={{ paddingLeft: 8 + depth * 14 }}
      tabIndex={0}
      role="treeitem"
      aria-expanded={expandable ? open : undefined}
      onClick={insertable ? onActivate : onToggle}
      onKeyDown={onKeyDown}
      title={title}
    >
      <ChevronRight
        className={clsx('chev', open && 'open', !expandable && 'none')}
        onClick={(e) => {
          e.stopPropagation();
          if (expandable) onToggle();
        }}
      />
      <span className="gql-name">
        {name}
        {args > 0 && <span className="subtle">({args})</span>}
      </span>
      {insertable && (
        <button
          className="icon-btn sm gql-insert"
          tabIndex={-1}
          onClick={(e) => {
            e.stopPropagation();
            onActivate?.();
          }}
          title="Insert query"
          aria-label={`Insert ${name}`}
        >
          <CornerDownLeft />
        </button>
      )}
      <span className="gql-type">{typeRef}</span>
    </div>
  );
}

/** Description, arguments and (expandable) fields of a field's return type. */
function FieldDetail({ field, types, path, depth, open, toggle }: { field: GqlField; types: Map<string, GqlType>; path: string; depth: number; open: Set<string>; toggle(k: string): void }) {
  const t = types.get(field.namedType);
  const children: GqlField[] = t?.fields ?? [];
  return (
    <>
      {field.description && (
        <div className="gql-desc" style={{ paddingLeft: 8 + depth * 14 + 18 }}>
          {field.description}
        </div>
      )}
      {(field.args ?? []).map((a) => (
        <div key={`arg-${a.name}`} className="gql-desc mono" style={{ paddingLeft: 8 + depth * 14 + 18 }}>
          ${a.name}: <span className="text-accent">{a.typeRef}</span>
          {a.defaultValue !== undefined && <span className="subtle"> = {a.defaultValue}</span>}
        </div>
      ))}
      {t?.kind === 'ENUM' && (
        <div className="gql-desc mono" style={{ paddingLeft: 8 + depth * 14 + 18 }}>
          {(t.enumValues ?? []).map((v) => v.name).join(' | ')}
        </div>
      )}
      {t?.kind === 'UNION' && (
        <div className="gql-desc mono" style={{ paddingLeft: 8 + depth * 14 + 18 }}>
          {(t.possibleTypes ?? []).join(' | ')}
        </div>
      )}
      {children.map((f) => {
        const k = `${path}.${f.name}`;
        const ft = types.get(f.namedType);
        const expandable = (!!ft && ['OBJECT', 'INTERFACE', 'UNION', 'ENUM'].includes(ft.kind)) || !!f.args?.length || !!f.description;
        return (
          <div key={k}>
            <Row depth={depth} name={f.name} args={f.args?.length ?? 0} typeRef={f.typeRef} deprecated={!!f.deprecated} expandable={expandable && depth < 8} open={open.has(k)} onToggle={() => toggle(k)} title={f.description ?? f.deprecationReason} />
            {open.has(k) && depth < 8 && <FieldDetail field={f} types={types} path={k} depth={depth + 1} open={open} toggle={toggle} />}
          </div>
        );
      })}
    </>
  );
}

import { Fragment, useEffect, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, CopyPlus, MoreHorizontal, Plus, Trash2, X } from 'lucide-react';
import { Menu } from '@/components/ui/Menu';
import { Checkbox } from '@/components/ui/misc';
import type { DataField, DataSchema, FieldKind, FieldType } from '@/lib/api/data';
import { FIELD_TYPES, TYPE_CATEGORIES, defaultKind, formatLooseValue, guessType, kindOf, parseLooseValue, typeInfo, uniqueName, withKind } from './fieldTypes';

export interface FieldCtx {
  schema: DataSchema;
  /** The entity being edited (for self-references and ref defaults). */
  entity: string;
}

/** Fields added in this session whose type still follows their name (`email` → e-mail). */
const fresh = new WeakSet<object>();

const hasNested = (k: FieldKind) => k.type === 'object' || (k.type === 'array' && k.of.type === 'object');

export function FieldTable({ fields, onChange, ctx, depth = 0 }: { fields: DataField[]; onChange(fields: DataField[]): void; ctx: FieldCtx; depth?: number }) {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});

  const update = (i: number, next: DataField) => {
    if (fresh.has(fields[i])) fresh.add(next);
    onChange(fields.map((f, j) => (j === i ? next : f)));
  };
  const remove = (i: number) => onChange(fields.filter((_, j) => j !== i));
  const move = (i: number, d: -1 | 1) => {
    const j = i + d;
    if (j < 0 || j >= fields.length) return;
    const next = [...fields];
    [next[i], next[j]] = [next[j], next[i]];
    onChange(next);
  };
  const duplicate = (i: number) => {
    const copy = { ...structuredClone(fields[i]), name: uniqueName(`${fields[i].name}Copy`, fields.map((f) => f.name)) };
    onChange([...fields.slice(0, i + 1), copy, ...fields.slice(i + 1)]);
  };
  const add = () => {
    const f: DataField = { name: uniqueName('field', fields.map((x) => x.name)), type: 'word' };
    fresh.add(f);
    onChange([...fields, f]);
  };

  const rename = (i: number, name: string) => {
    const f = fields[i];
    const guess = fresh.has(f) ? guessType(name) : undefined;
    update(i, guess && guess !== f.type ? withKind({ ...f, name }, defaultKind(guess, ctx.schema, ctx.entity)) : { ...f, name });
  };

  const dupNames = new Set(fields.map((f) => f.name).filter((n, i, all) => all.indexOf(n) !== i));

  return (
    <table className={clsx('table data-fields-table', depth > 0 && 'nested')}>
      <colgroup>
        <col style={{ width: 26 }} />
        <col style={{ width: '17%' }} />
        <col style={{ width: 170 }} />
        <col />
        <col style={{ width: 76 }} />
        <col style={{ width: 58 }} />
        <col style={{ width: 38 }} />
      </colgroup>
      {depth === 0 && (
        <thead>
          <tr>
            <th />
            <th>Field</th>
            <th>Type</th>
            <th>Options</th>
            <th title="Share of rows where the value is null">Null %</th>
            <th title="Every non-null value is distinct">Unique</th>
            <th />
          </tr>
        </thead>
      )}
      <tbody>
        {fields.map((f, i) => {
          const kind = kindOf(f);
          const nested = hasNested(kind);
          const key = `${i}:${f.name}`;
          const open = nested && !collapsed[key];
          return (
            <Fragment key={i}>
              <tr>
                <td className="data-cell-toggle">
                  {nested && (
                    <button className="icon-btn sm" onClick={() => setCollapsed({ ...collapsed, [key]: open })} aria-label={open ? 'Collapse' : 'Expand'}>
                      {open ? <ChevronDown /> : <ChevronRight />}
                    </button>
                  )}
                </td>
                <td>
                  <input
                    className={clsx('input sm mono w-full', (!f.name || dupNames.has(f.name)) && 'invalid')}
                    value={f.name}
                    placeholder="name"
                    spellCheck={false}
                    onChange={(e) => rename(i, e.target.value)}
                    aria-label="Field name"
                    title={dupNames.has(f.name) ? 'Duplicate name' : undefined}
                  />
                </td>
                <td>
                  <TypePicker
                    value={kind.type}
                    onChange={(t) => {
                      fresh.delete(f);
                      update(i, withKind(f, defaultKind(t, ctx.schema, ctx.entity)));
                    }}
                  />
                </td>
                <td>
                  <ParamsEditor kind={kind} onChange={(k) => update(i, withKind(f, k))} ctx={ctx} depth={depth} />
                </td>
                <td>
                  <input
                    className="input sm w-full"
                    type="number"
                    min={0}
                    max={100}
                    value={f.nullPercent ?? ''}
                    placeholder="0"
                    onChange={(e) => {
                      const v = e.target.value === '' ? undefined : Math.min(100, Math.max(0, Number(e.target.value)));
                      const next = { ...f };
                      if (v) next.nullPercent = v;
                      else delete next.nullPercent;
                      update(i, next);
                    }}
                    aria-label="Null percent"
                  />
                </td>
                <td style={{ textAlign: 'center' }}>
                  <Checkbox
                    checked={!!f.unique}
                    onChange={(v) => {
                      const next = { ...f };
                      if (v) next.unique = true;
                      else delete next.unique;
                      update(i, next);
                    }}
                    title="Unique"
                  />
                </td>
                <td>
                  <Menu
                    align="right"
                    trigger={(p) => (
                      <button className="icon-btn sm" {...p} aria-label="Field actions">
                        <MoreHorizontal />
                      </button>
                    )}
                    items={[
                      { label: 'Move up', icon: <ArrowUp />, disabled: i === 0, onSelect: () => move(i, -1) },
                      { label: 'Move down', icon: <ArrowDown />, disabled: i === fields.length - 1, onSelect: () => move(i, 1) },
                      { label: 'Duplicate', icon: <CopyPlus />, onSelect: () => duplicate(i) },
                      { separator: true },
                      { label: 'Delete', icon: <Trash2 />, danger: true, onSelect: () => remove(i) },
                    ]}
                  />
                </td>
              </tr>
              {open && (
                <tr className="data-nested-row">
                  <td />
                  <td colSpan={6}>
                    <div className="data-nested">
                      <div className="subtle xs">{kind.type === 'array' ? 'Each array item is an object with:' : 'Object fields:'}</div>
                      <FieldTable
                        fields={kind.type === 'object' ? kind.fields : kind.type === 'array' && kind.of.type === 'object' ? kind.of.fields : []}
                        onChange={(nf) =>
                          update(i, withKind(f, kind.type === 'object' ? { ...kind, fields: nf } : kind.type === 'array' ? { ...kind, of: { type: 'object', fields: nf } } : kind))
                        }
                        ctx={ctx}
                        depth={depth + 1}
                      />
                    </div>
                  </td>
                </tr>
              )}
            </Fragment>
          );
        })}
        <tr>
          <td />
          <td colSpan={6}>
            <button className="btn btn-sm btn-ghost" onClick={add}>
              <Plus /> Add field
            </button>
          </td>
        </tr>
      </tbody>
    </table>
  );
}

export function TypePicker({ value, onChange, exclude = [] }: { value: FieldType; onChange(t: FieldType): void; exclude?: FieldType[] }) {
  return (
    <select className="select sm w-full" value={value} onChange={(e) => onChange(e.target.value as FieldType)} title={typeInfo(value)?.hint} aria-label="Field type">
      {TYPE_CATEGORIES.map((cat) => (
        <optgroup key={cat} label={cat}>
          {FIELD_TYPES.filter((t) => t.category === cat && !exclude.includes(t.type)).map((t) => (
            <option key={t.type} value={t.type}>
              {t.label}
            </option>
          ))}
        </optgroup>
      ))}
    </select>
  );
}

// ---------- parameters ----------

function Param({ label, children, title }: { label: string; children: ReactNode; title?: string }) {
  return (
    <label className="data-param" title={title}>
      <span>{label}</span>
      {children}
    </label>
  );
}

function Num({ value, onChange, placeholder, step, min, max, width = 64 }: { value?: number; onChange(v?: number): void; placeholder?: string; step?: number; min?: number; max?: number; width?: number }) {
  return (
    <input
      className="input sm"
      type="number"
      style={{ width }}
      value={value ?? ''}
      placeholder={placeholder}
      step={step}
      min={min}
      max={max}
      onChange={(e) => {
        if (e.target.value === '') return onChange(undefined);
        const v = Number(e.target.value);
        if (Number.isNaN(v)) return;
        // Most parameters are integers on the Rust side; only fractional steps allow decimals.
        onChange(step !== undefined && step < 1 ? v : Math.trunc(v));
      }}
    />
  );
}

function Text({ value, onChange, placeholder, mono, width, grow }: { value?: string; onChange(v?: string): void; placeholder?: string; mono?: boolean; width?: number; grow?: boolean }) {
  return (
    <input
      className={clsx('input sm', mono && 'mono', grow && 'grow')}
      style={width ? { width } : undefined}
      value={value ?? ''}
      placeholder={placeholder}
      spellCheck={false}
      onChange={(e) => onChange(e.target.value === '' ? undefined : e.target.value)}
    />
  );
}

/** Sets or clears an optional parameter. */
function set<K extends FieldKind>(kind: K, key: string, v: unknown): K {
  const next = { ...kind } as Record<string, unknown>;
  if (v === undefined || v === '') delete next[key];
  else next[key] = v;
  return next as K;
}

export function ParamsEditor({ kind, onChange, ctx, depth = 0 }: { kind: FieldKind; onChange(k: FieldKind): void; ctx: FieldCtx; depth?: number }) {
  const p = <K extends FieldKind>(k: K, key: string) => (v: unknown) => onChange(set(k, key, v));
  let body: ReactNode = null;
  switch (kind.type) {
    case 'uuid':
      body = (
        <Param label="version">
          <select className="select sm" value={kind.version ?? 4} onChange={(e) => onChange(set(kind, 'version', Number(e.target.value) === 4 ? undefined : 7))}>
            <option value={4}>v4 random</option>
            <option value={7}>v7 time-ordered</option>
          </select>
        </Param>
      );
      break;
    case 'sequence':
      body = (
        <>
          <Param label="start">
            <Num value={kind.start} onChange={p(kind, 'start')} placeholder="1" />
          </Param>
          <Param label="step">
            <Num value={kind.step} onChange={p(kind, 'step')} placeholder="1" />
          </Param>
        </>
      );
      break;
    case 'email':
      body = (
        <Param label="domain" title="Defaults to example.com / .net / .org, which never reach a real inbox">
          <Text value={kind.domain} onChange={p(kind, 'domain')} placeholder="example.com" width={150} />
        </Param>
      );
      break;
    case 'password':
      body = (
        <>
          <Param label="length">
            <Num value={kind.minLength} onChange={p(kind, 'minLength')} placeholder="10" min={1} width={56} />
          </Param>
          <Param label="to">
            <Num value={kind.maxLength} onChange={p(kind, 'maxLength')} placeholder="16" min={1} width={56} />
          </Param>
        </>
      );
      break;
    case 'price':
    case 'float':
      body = (
        <>
          <Param label="min">
            <Num value={kind.min} onChange={p(kind, 'min')} placeholder={kind.type === 'price' ? '1' : '0'} step={0.01} width={80} />
          </Param>
          <Param label="max">
            <Num value={kind.max} onChange={p(kind, 'max')} placeholder={kind.type === 'price' ? '1000' : '1'} step={0.01} width={80} />
          </Param>
          <Param label="decimals">
            <Num value={kind.decimals} onChange={p(kind, 'decimals')} placeholder="2" min={0} max={12} width={52} />
          </Param>
        </>
      );
      break;
    case 'int':
      body = (
        <>
          <Param label="min">
            <Num value={kind.min} onChange={p(kind, 'min')} placeholder="0" width={80} />
          </Param>
          <Param label="max">
            <Num value={kind.max} onChange={p(kind, 'max')} placeholder="1000" width={80} />
          </Param>
        </>
      );
      break;
    case 'bool':
      body = (
        <Param label="true %">
          <Num value={kind.truePercent} onChange={p(kind, 'truePercent')} placeholder="50" min={0} max={100} width={56} />
        </Param>
      );
      break;
    case 'sentence':
      body = (
        <>
          <Param label="words">
            <Num value={kind.minWords} onChange={p(kind, 'minWords')} placeholder="4" min={1} width={52} />
          </Param>
          <Param label="to">
            <Num value={kind.maxWords} onChange={p(kind, 'maxWords')} placeholder="10" min={1} width={52} />
          </Param>
        </>
      );
      break;
    case 'paragraph':
      body = (
        <Param label="sentences">
          <Num value={kind.sentences} onChange={p(kind, 'sentences')} placeholder="3" min={1} width={52} />
        </Param>
      );
      break;
    case 'lorem':
      body = (
        <Param label="words">
          <Num value={kind.words} onChange={p(kind, 'words')} placeholder="5" min={1} width={52} />
        </Param>
      );
      break;
    case 'date':
    case 'datetime':
    case 'timestamp':
      body = (
        <>
          <Param label="from">
            <Text value={kind.from} onChange={p(kind, 'from')} placeholder="2020-01-01" width={96} mono />
          </Param>
          <Param label="to">
            <Text value={kind.to} onChange={p(kind, 'to')} placeholder="2025-12-31" width={96} mono />
          </Param>
          {kind.type === 'timestamp' ? (
            <Param label="unit">
              <select className="select sm" value={kind.unit ?? 's'} onChange={(e) => onChange(set(kind, 'unit', e.target.value === 's' ? undefined : 'ms'))}>
                <option value="s">seconds</option>
                <option value="ms">milliseconds</option>
              </select>
            </Param>
          ) : (
            <Param label="format" title="strftime, e.g. %Y-%m-%d or %d/%m/%Y %H:%M">
              <Text value={kind.format} onChange={p(kind, 'format')} placeholder={kind.type === 'date' ? '%Y-%m-%d' : 'RFC 3339'} width={110} mono />
            </Param>
          )}
        </>
      );
      break;
    case 'enum':
      body = <EnumEditor kind={kind} onChange={onChange} />;
      break;
    case 'regex':
      body = <Text value={kind.pattern} onChange={(v) => onChange({ ...kind, pattern: v ?? '' })} placeholder="[A-Z]{3}-[0-9]{4}" mono grow />;
      break;
    case 'template':
      body = <Text value={kind.value} onChange={(v) => onChange({ ...kind, value: v ?? '' })} placeholder="{{firstName|lower}}.{{lastName|lower}}@acme.test" mono grow />;
      break;
    case 'ref': {
      const target = ctx.schema.entities.find((e) => e.name === kind.entity);
      body = (
        <>
          <select className={clsx('select sm', !target && 'invalid')} value={kind.entity} onChange={(e) => {
            const t = ctx.schema.entities.find((x) => x.name === e.target.value);
            onChange({ ...kind, entity: e.target.value, field: t?.fields.find((x) => x.name === 'id')?.name ?? t?.fields[0]?.name ?? '' });
          }} aria-label="Referenced entity">
            {!target && <option value={kind.entity}>{kind.entity || '(choose)'}</option>}
            {ctx.schema.entities.map((e) => (
              <option key={e.name} value={e.name}>
                {e.name}
                {e.name === ctx.entity ? ' (self)' : ''}
              </option>
            ))}
          </select>
          <span className="subtle xs">.</span>
          <select className="select sm" value={kind.field} onChange={(e) => onChange({ ...kind, field: e.target.value })} aria-label="Referenced field">
            {!target?.fields.some((f) => f.name === kind.field) && <option value={kind.field}>{kind.field || '(choose)'}</option>}
            {target?.fields.map((f) => (
              <option key={f.name} value={f.name}>
                {f.name}
              </option>
            ))}
          </select>
          {kind.entity === ctx.entity && <span className="subtle xs">picks an earlier row (null for the first)</span>}
        </>
      );
      break;
    }
    case 'object':
      body = <span className="subtle xs">{kind.fields.length} field{kind.fields.length === 1 ? '' : 's'}</span>;
      break;
    case 'array':
      body = (
        <>
          <Param label="items">
            <Num value={kind.min} onChange={p(kind, 'min')} placeholder="1" min={0} width={52} />
          </Param>
          <Param label="to">
            <Num value={kind.max} onChange={p(kind, 'max')} placeholder="3" min={0} width={52} />
          </Param>
          <Param label="of">
            <div style={{ width: 150 }}>
              <TypePicker value={kind.of.type} onChange={(t) => onChange({ ...kind, of: defaultKind(t, ctx.schema, ctx.entity) })} />
            </div>
          </Param>
          {kind.of.type !== 'object' && depth < 6 && <ParamsEditor kind={kind.of} onChange={(of) => onChange({ ...kind, of })} ctx={ctx} depth={depth + 1} />}
        </>
      );
      break;
    case 'json':
      body = <JsonValue value={kind.value} onChange={(value) => onChange({ ...kind, value })} />;
      break;
    default:
      body = <span className="subtle xs truncate">{typeInfo(kind.type)?.hint}</span>;
  }
  return <div className="data-params">{body}</div>;
}

function EnumEditor({ kind, onChange }: { kind: Extract<FieldKind, { type: 'enum' }>; onChange(k: FieldKind): void }) {
  const [draft, setDraft] = useState('');
  const weighted = !!kind.weights?.length;
  const addValue = () => {
    if (!draft.trim()) return;
    const values = [...kind.values, parseLooseValue(draft)];
    onChange({ ...kind, values, ...(weighted ? { weights: [...(kind.weights ?? []), 1] } : {}) });
    setDraft('');
  };
  const removeAt = (i: number) => {
    const next: FieldKind = { ...kind, values: kind.values.filter((_, j) => j !== i) };
    if (weighted) next.weights = kind.weights!.filter((_, j) => j !== i);
    onChange(next);
  };
  return (
    <>
      {kind.values.map((v, i) => (
        <span key={i} className="tag data-enum-tag" title={typeof v === 'string' ? 'text' : `JSON ${typeof v}`}>
          <span className={clsx('mono', typeof v !== 'string' && 'text-accent')}>{formatLooseValue(v)}</span>
          {weighted && (
            <input
              className="data-weight"
              type="number"
              min={0}
              value={kind.weights?.[i] ?? 1}
              onChange={(e) => onChange({ ...kind, weights: kind.values.map((_, j) => (j === i ? Number(e.target.value) : (kind.weights?.[j] ?? 1))) })}
              aria-label="Weight"
              title="Relative weight"
            />
          )}
          <button onClick={() => removeAt(i)} aria-label="Remove value">
            <X />
          </button>
        </span>
      ))}
      <input
        className="input sm mono"
        style={{ width: 110 }}
        value={draft}
        placeholder="add value ⏎"
        title='Press Enter to add. Numbers, true/false and null are JSON; quote ("42") to keep text.'
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            addValue();
          }
        }}
        onBlur={addValue}
      />
      <Checkbox
        checked={weighted}
        onChange={(on) => {
          const next: FieldKind = { ...kind };
          if (on) next.weights = kind.values.map(() => 1);
          else delete next.weights;
          onChange(next);
        }}
        label={<span className="xs">weighted</span>}
      />
    </>
  );
}

function JsonValue({ value, onChange }: { value: unknown; onChange(v: unknown): void }) {
  const [text, setText] = useState(() => JSON.stringify(value));
  const [error, setError] = useState(false);
  useEffect(() => {
    setText(JSON.stringify(value));
    setError(false);
  }, [value]);
  return (
    <input
      className={clsx('input sm mono grow', error && 'invalid')}
      value={text}
      spellCheck={false}
      placeholder='null, 42, "text", {"a": 1}'
      onChange={(e) => {
        setText(e.target.value);
        try {
          JSON.parse(e.target.value);
          setError(false);
        } catch {
          setError(true);
        }
      }}
      onBlur={() => {
        try {
          onChange(JSON.parse(text));
        } catch {
          setError(true);
        }
      }}
      title={error ? 'Not valid JSON' : 'Any JSON value'}
    />
  );
}

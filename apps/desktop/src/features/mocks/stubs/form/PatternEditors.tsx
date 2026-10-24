import { useEffect, useRef, useState } from 'react';
import { Plus, X, CaseSensitive, Braces } from 'lucide-react';
import clsx from 'clsx';
import type { ContentPattern, MethodPattern } from '@/features/mocks/lib/types';
import {
  BODY_OPS,
  OP_LABEL,
  SIMPLE_OPS,
  SIMPLE_OPS_V4,
  NUMBER_OPS,
  METHODS,
  METHODS_V4,
  isNumberOp,
  methodSetOf,
  opOf,
  patternValue,
  tryParseJson,
  prettyJson,
} from '@/features/mocks/lib/stub-utils';
import { CodeEditor, type CodeLanguage } from '@/components/editor/CodeEditor';
import { Checkbox } from '@/features/mocks/components/ui';
import { useCapabilities } from '@/features/mocks/lib/queries';

const ADVANCED = '__json';

/** Build a pattern for `op` carrying over a previous value where it makes sense. */
export function patternFor(op: string, value: string, prev?: ContentPattern): ContentPattern {
  if (op === 'absent') return { absent: true };
  if (isNumberOp(op)) {
    const n = Number(value);
    return { [op]: value.trim() !== '' && Number.isFinite(n) ? n : 0 };
  }
  if (op === 'equalToJson' || op === 'matchesJsonSchema') {
    const parsed = tryParseJson(value);
    const p: ContentPattern = { [op]: parsed.ok ? parsed.value : value || {} };
    if (op === 'equalToJson' && prev) {
      if (prev.ignoreArrayOrder) p.ignoreArrayOrder = true;
      if (prev.ignoreExtraElements) p.ignoreExtraElements = true;
    }
    return p;
  }
  if (op === 'matchesJsonPath' || op === 'matchesXPath') return { [op]: value || (op === 'matchesJsonPath' ? '$.id' : '//id') };
  const p: ContentPattern = { [op]: value };
  if (op === 'equalTo' && prev?.caseInsensitive) p.caseInsensitive = true;
  return p;
}

function isSimple(p: ContentPattern | undefined): boolean {
  if (!p) return true;
  const op = opOf(p);
  // Number matchers (WireMock 4) get the inline editor too; on 3.x it warns instead of hiding them.
  if (!(SIMPLE_OPS_V4 as string[]).includes(op)) return false;
  const extra = Object.keys(p).filter((k) => k !== op && k !== 'caseInsensitive');
  return extra.length === 0 && (typeof p[op] === 'string' || typeof p[op] === 'number' || typeof p[op] === 'boolean');
}

/**
 * Matchers offered for a server: WireMock 4 adds the number matchers. A number
 * matcher already in the stub stays visible on 3.x so it can be changed.
 */
export function useSimpleOps(current?: string): string[] {
  const caps = useCapabilities();
  const ops: string[] = caps.numberMatchers ? SIMPLE_OPS_V4 : SIMPLE_OPS;
  return current && isNumberOp(current) && !ops.includes(current) ? [...ops, current] : ops;
}

/** Numeric operand with a local draft, so "1." or "-" can be typed before it is a number. */
function NumberOperand({ value, onChange, className = 'input sm mono grow' }: { value: unknown; onChange(n: number): void; className?: string }) {
  const fmt = (v: unknown) => (v === undefined || v === null ? '' : String(v));
  const [text, setText] = useState(() => fmt(value));
  useEffect(() => {
    if (Number(text) !== Number(value) || text.trim() === '') setText(fmt(value));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);
  const valid = text.trim() !== '' && Number.isFinite(Number(text));
  return (
    <input
      className={clsx(className, !valid && 'invalid')}
      inputMode="decimal"
      value={text}
      placeholder="number, e.g. 18 or 2.5"
      title="Compared as a number (Java double): 18, 18.0 and 1.8e1 are equal"
      onChange={(e) => {
        setText(e.target.value);
        const n = Number(e.target.value);
        if (e.target.value.trim() !== '' && Number.isFinite(n)) onChange(n);
      }}
    />
  );
}

/** Single-line pattern editor used in key/value matcher rows. */
export function InlinePattern({ value, onChange, ops: opsProp }: { value?: ContentPattern; onChange(p: ContentPattern): void; ops?: string[] }) {
  const simple = isSimple(value);
  const defaultOps = useSimpleOps(value ? opOf(value) : undefined);
  const ops = opsProp ?? defaultOps;
  const caps = useCapabilities();
  const [raw, setRaw] = useState(() => (simple ? '' : prettyJson(value)));
  const [err, setErr] = useState(false);
  const op = simple ? opOf(value) : ADVANCED;
  useEffect(() => {
    if (!simple) setRaw(prettyJson(value));
  }, [simple, value]);

  return (
    <>
      <select
        className="select sm"
        value={op}
        onChange={(e) => {
          const next = e.target.value;
          if (next === ADVANCED) onChange(value ?? { equalTo: '' });
          else onChange(patternFor(next, simple ? patternValue(value) : '', value));
          if (next === ADVANCED) setRaw(prettyJson(value ?? { equalTo: '' }));
        }}
      >
        {ops.map((o) => (
          <option key={o} value={o}>
            {OP_LABEL[o] ?? o}
          </option>
        ))}
        <option value={ADVANCED}>advanced (JSON)…</option>
      </select>
      {op === ADVANCED ? (
        <input
          className={clsx('input sm mono', err && 'invalid')}
          value={raw.replace(/\s*\n\s*/g, ' ')}
          title="Any WireMock matcher as JSON, e.g. {&quot;hasExactly&quot;:[{&quot;equalTo&quot;:&quot;a&quot;}]}"
          onChange={(e) => {
            setRaw(e.target.value);
            const r = tryParseJson<ContentPattern>(e.target.value);
            setErr(!r.ok);
            if (r.ok && r.value && typeof r.value === 'object') onChange(r.value);
          }}
        />
      ) : op === 'absent' ? (
        <span className="subtle xs">must not be present</span>
      ) : isNumberOp(op) ? (
        <div className="row-sm">
          <NumberOperand value={value?.[op]} onChange={(n) => onChange({ [op]: n })} />
          {caps.loaded && !caps.numberMatchers && (
            <span className="text-warning xs nowrap" title="Number matchers were added in WireMock 4; this server rejects them with 422.">
              needs WireMock 4
            </span>
          )}
        </div>
      ) : (
        <div className="row-sm">
          <input
            className="input sm mono grow"
            value={patternValue(value)}
            placeholder={op === 'matches' || op === 'doesNotMatch' ? 'regex, e.g. [0-9]+' : op === 'before' || op === 'after' ? 'now +1 days / 2026-01-01T00:00:00Z' : 'value'}
            onChange={(e) => onChange({ ...patternFor(op, e.target.value, value), ...(value?.caseInsensitive && op === 'equalTo' ? { caseInsensitive: true } : {}) })}
          />
          {op === 'equalTo' && (
            <button
              className={clsx('icon-btn sm', value?.caseInsensitive && 'active')}
              title="Case-insensitive"
              onClick={() => onChange({ ...value!, caseInsensitive: value?.caseInsensitive ? undefined : true })}
            >
              <CaseSensitive />
            </button>
          )}
        </div>
      )}
    </>
  );
}

/**
 * Map of name → pattern (query params, headers, cookies, form params…).
 * Keeps local row order + allows blank keys while typing.
 */
export function PatternMapEditor({
  value,
  onChange,
  keyPlaceholder = 'name',
  keySuggestions,
  fixedKeys,
  addLabel = 'Add',
}: {
  value?: Record<string, ContentPattern>;
  onChange(v: Record<string, ContentPattern> | undefined): void;
  keyPlaceholder?: string;
  keySuggestions?: string[];
  fixedKeys?: boolean;
  addLabel?: string;
}) {
  type R = { id: number; key: string; p: ContentPattern };
  const seq = useRef(0);
  const toRows = (v?: Record<string, ContentPattern>): R[] => Object.entries(v ?? {}).map(([key, p]) => ({ id: seq.current++, key, p }));
  const [rows, setRows] = useState<R[]>(() => toRows(value));
  const lastEmitted = useRef(JSON.stringify(value ?? {}));
  useEffect(() => {
    const s = JSON.stringify(value ?? {});
    if (s !== lastEmitted.current) {
      lastEmitted.current = s;
      setRows(toRows(value));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  const emit = (next: R[]) => {
    setRows(next);
    const obj: Record<string, ContentPattern> = {};
    for (const r of next) if (r.key) obj[r.key] = r.p;
    const out = Object.keys(obj).length ? obj : undefined;
    lastEmitted.current = JSON.stringify(out ?? {});
    onChange(out);
  };
  const listId = useRef(`dl-${Math.random().toString(36).slice(2)}`).current;

  return (
    <div className="col-sm">
      {rows.map((r, i) => (
        <div key={r.id} className="matcher-row">
          <input
            className="input sm mono"
            value={r.key}
            placeholder={keyPlaceholder}
            readOnly={fixedKeys}
            list={keySuggestions ? listId : undefined}
            onChange={(e) => emit(rows.map((x, j) => (j === i ? { ...x, key: e.target.value } : x)))}
          />
          <InlinePattern value={r.p} onChange={(p) => emit(rows.map((x, j) => (j === i ? { ...x, p } : x)))} />
          {!fixedKeys ? (
            <button className="icon-btn sm danger" onClick={() => emit(rows.filter((_, j) => j !== i))} aria-label="Remove">
              <X />
            </button>
          ) : (
            <span />
          )}
        </div>
      ))}
      {!fixedKeys && (
        <div>
          <button className="btn btn-sm btn-ghost" onClick={() => setRows([...rows, { id: seq.current++, key: '', p: { equalTo: '' } }])}>
            <Plus /> {addLabel}
          </button>
        </div>
      )}
      {keySuggestions && (
        <datalist id={listId}>
          {keySuggestions.map((k) => (
            <option key={k} value={k} />
          ))}
        </datalist>
      )}
    </div>
  );
}

/** Simple string → string map (response headers, proxy headers, webhook headers). */
export function StringMapEditor({
  value,
  onChange,
  keySuggestions,
  keyPlaceholder = 'Header',
  valuePlaceholder = 'value',
  addLabel = 'Add header',
}: {
  value?: Record<string, string | string[]>;
  onChange(v: Record<string, string | string[]> | undefined): void;
  keySuggestions?: string[];
  keyPlaceholder?: string;
  valuePlaceholder?: string;
  addLabel?: string;
}) {
  type R = { id: number; key: string; v: string };
  const seq = useRef(0);
  const fmt = (v: string | string[]) => (Array.isArray(v) ? JSON.stringify(v) : String(v));
  const toRows = (v?: Record<string, string | string[]>): R[] => Object.entries(v ?? {}).map(([key, val]) => ({ id: seq.current++, key, v: fmt(val) }));
  const [rows, setRows] = useState<R[]>(() => toRows(value));
  const lastEmitted = useRef(JSON.stringify(value ?? {}));
  useEffect(() => {
    const s = JSON.stringify(value ?? {});
    if (s !== lastEmitted.current) {
      lastEmitted.current = s;
      setRows(toRows(value));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);
  const emit = (next: R[]) => {
    setRows(next);
    const obj: Record<string, string | string[]> = {};
    for (const r of next) {
      if (!r.key) continue;
      const t = r.v.trim();
      if (t.startsWith('[')) {
        const p = tryParseJson<string[]>(t);
        obj[r.key] = p.ok && Array.isArray(p.value) ? p.value : r.v;
      } else obj[r.key] = r.v;
    }
    const out = Object.keys(obj).length ? obj : undefined;
    lastEmitted.current = JSON.stringify(out ?? {});
    onChange(out);
  };
  const listId = useRef(`dl-${Math.random().toString(36).slice(2)}`).current;
  return (
    <div className="col-sm">
      {rows.map((r, i) => (
        <div key={r.id} className="matcher-row" style={{ gridTemplateColumns: 'minmax(120px, 1fr) minmax(140px, 2fr) auto' }}>
          <input className="input sm mono" value={r.key} placeholder={keyPlaceholder} list={keySuggestions ? listId : undefined} onChange={(e) => emit(rows.map((x, j) => (j === i ? { ...x, key: e.target.value } : x)))} />
          <input className="input sm mono" value={r.v} placeholder={valuePlaceholder} onChange={(e) => emit(rows.map((x, j) => (j === i ? { ...x, v: e.target.value } : x)))} />
          <button className="icon-btn sm danger" onClick={() => emit(rows.filter((_, j) => j !== i))} aria-label="Remove">
            <X />
          </button>
        </div>
      ))}
      <div>
        <button className="btn btn-sm btn-ghost" onClick={() => setRows([...rows, { id: seq.current++, key: '', v: '' }])}>
          <Plus /> {addLabel}
        </button>
      </div>
      {keySuggestions && (
        <datalist id={listId}>
          {keySuggestions.map((k) => (
            <option key={k} value={k} />
          ))}
        </datalist>
      )}
    </div>
  );
}

const SUB_OPS = ['', 'equalTo', 'contains', 'doesNotContain', 'matches', 'doesNotMatch', 'absent', 'before', 'after', 'equalToDateTime'];
const SUB_OPS_V4 = [...SUB_OPS, ...NUMBER_OPS];

/** Expression + optional sub-matcher (matchesJsonPath / matchesXPath object form). */
function ExpressionWithMatcher({ p, op, onChange }: { p: ContentPattern; op: 'matchesJsonPath' | 'matchesXPath'; onChange(p: ContentPattern): void }) {
  const v = p[op] as string | { expression: string; [k: string]: unknown };
  const expression = typeof v === 'string' ? v : v?.expression ?? '';
  const sub = typeof v === 'object' && v ? (Object.fromEntries(Object.entries(v).filter(([k]) => k !== 'expression')) as ContentPattern) : undefined;
  const subOp = sub && Object.keys(sub).length ? opOf(sub) : '';
  const caps = useCapabilities();
  const subOps = caps.numberMatchers || isNumberOp(subOp) ? SUB_OPS_V4 : SUB_OPS;
  const set = (expr: string, s?: ContentPattern) => {
    const next: ContentPattern = { ...p };
    next[op] = s && Object.keys(s).length ? { expression: expr, ...s } : expr;
    onChange(next);
  };
  return (
    <div className="col-sm">
      <div className="row-sm">
        <input className="input sm mono grow" value={expression} placeholder={op === 'matchesJsonPath' ? "$.items[?(@.qty > 1)]" : '//order/id/text()'} onChange={(e) => set(e.target.value, sub)} />
      </div>
      <div className="row-sm">
        <span className="subtle xs nowrap">and the result</span>
        <select className="select sm" style={{ width: 150 }} value={subOp} onChange={(e) => set(expression, e.target.value ? patternFor(e.target.value, sub ? patternValue(sub) : '') : undefined)}>
          {subOps.map((o) => (
            <option key={o} value={o}>
              {o ? OP_LABEL[o] : 'exists (no extra check)'}
            </option>
          ))}
        </select>
        {subOp && isNumberOp(subOp) ? (
          <NumberOperand value={sub?.[subOp]} onChange={(n) => set(expression, { [subOp]: n })} />
        ) : (
          subOp && subOp !== 'absent' && <input className="input sm mono grow" value={sub ? patternValue(sub) : ''} onChange={(e) => set(expression, patternFor(subOp, e.target.value, sub))} />
        )}
      </div>
      {op === 'matchesXPath' && (
        <NamespacesInput value={p.xPathNamespaces} onChange={(ns) => onChange({ ...p, xPathNamespaces: ns })} />
      )}
    </div>
  );
}

function NamespacesInput({ value, onChange }: { value?: Record<string, string>; onChange(v?: Record<string, string>): void }) {
  const [open, setOpen] = useState(!!value && Object.keys(value).length > 0);
  if (!open)
    return (
      <div>
        <button className="btn btn-sm btn-ghost" onClick={() => setOpen(true)}>
          <Plus /> XML namespaces
        </button>
      </div>
    );
  return (
    <div className="col-sm">
      <span className="field-label">Namespaces (prefix → URI)</span>
      <StringMapEditor value={value} onChange={(v) => onChange(v as Record<string, string> | undefined)} keyPlaceholder="prefix" valuePlaceholder="http://example.com/ns" addLabel="Add namespace" />
    </div>
  );
}

/** One request-body pattern with a type-appropriate editor. */
export function BodyPatternEditor({ value, onChange, onRemove, contentType }: { value: ContentPattern; onChange(p: ContentPattern): void; onRemove(): void; contentType?: string }) {
  const op = opOf(value);
  const known = [...BODY_OPS, 'and', 'or', 'not'] as string[];
  const [text, setText] = useState(() => editorText(value, op));
  const [err, setErr] = useState<string>();
  useEffect(() => {
    setText(editorText(value, op));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [op]);

  const lang: CodeLanguage = op === 'equalToXml' ? 'xml' : op === 'equalToJson' || op === 'matchesJsonSchema' || op === 'and' || op === 'or' || op === 'not' ? 'json' : contentType?.includes('json') ? 'json' : contentType?.includes('xml') ? 'xml' : 'plaintext';

  const onText = (t: string) => {
    setText(t);
    if (op === 'and' || op === 'or' || op === 'not' || !known.includes(op)) {
      const r = tryParseJson<ContentPattern>(t);
      setErr(r.ok ? undefined : r.error);
      if (r.ok) onChange(r.value);
      return;
    }
    if (op === 'equalToJson' || op === 'matchesJsonSchema') {
      const r = tryParseJson(t);
      setErr(r.ok ? undefined : `Invalid JSON: ${r.error}`);
      onChange({ ...value, [op]: r.ok ? r.value : t });
      return;
    }
    onChange({ ...value, [op]: t });
  };

  const caps = useCapabilities();
  const engineGap = caps.devall && op === 'matchesJsonSchema';

  return (
    <div className="fieldset" style={{ padding: 8 }}>
      <div className="col-sm">
        {engineGap && <div className="text-warning xs">The built-in engine does not evaluate JSON Schema yet: this pattern never matches there (stock WireMock does).</div>}
        <div className="row-sm">
          <select
            className="select sm"
            style={{ width: 190 }}
            value={known.includes(op) ? op : '__other'}
            onChange={(e) => {
              const n = e.target.value;
              let next: ContentPattern;
              if (n === 'and' || n === 'or') next = { [n]: [value] };
              else if (n === 'not') next = { not: value };
              else next = patternFor(n, op === 'absent' ? '' : text, value);
              onChange(next);
              setText(editorText(next, n));
              setErr(undefined);
            }}
          >
            {BODY_OPS.map((o) => (
              <option key={o} value={o}>
                {OP_LABEL[o]}
              </option>
            ))}
            <option value="and">all of (and)…</option>
            <option value="or">any of (or)…</option>
            <option value="not">not…</option>
            {!known.includes(op) && <option value="__other">{op}</option>}
          </select>
          {op === 'equalToJson' && (
            <>
              <Checkbox checked={!!value.ignoreArrayOrder} onChange={(v) => onChange({ ...value, ignoreArrayOrder: v || undefined })} label="ignore array order" />
              <Checkbox checked={!!value.ignoreExtraElements} onChange={(v) => onChange({ ...value, ignoreExtraElements: v || undefined })} label="ignore extra fields" />
            </>
          )}
          {op === 'equalToXml' && <Checkbox checked={!!value.enablePlaceholders} onChange={(v) => onChange({ ...value, enablePlaceholders: v || undefined })} label="placeholders (${xmlunit.ignore})" />}
          {(op === 'equalTo' || op === 'contains') && <Checkbox checked={!!value.caseInsensitive} onChange={(v) => onChange({ ...value, caseInsensitive: v || undefined })} label="case-insensitive" />}
          {op === 'matchesJsonSchema' && (
            <select className="select sm" style={{ width: 110 }} value={value.schemaVersion ?? 'V202012'} onChange={(e) => onChange({ ...value, schemaVersion: e.target.value })} title="JSON Schema version">
              {['V4', 'V6', 'V7', 'V201909', 'V202012'].map((v) => (
                <option key={v}>{v}</option>
              ))}
            </select>
          )}
          <span className="spacer" />
          <button className="icon-btn sm danger" onClick={onRemove} aria-label="Remove body pattern">
            <X />
          </button>
        </div>
        {op === 'matchesJsonPath' || op === 'matchesXPath' ? (
          <ExpressionWithMatcher p={value} op={op} onChange={onChange} />
        ) : op === 'absent' ? (
          <span className="subtle xs">Request must have no body.</span>
        ) : (
          <>
            <div className="editor-shell">
              <CodeEditor value={text} onChange={onText} language={lang} autoHeight={{ min: 60, max: 360 }} minimal />
            </div>
            {err && <div className="text-danger xs">{err}</div>}
            {op === 'equalToJson' && (
              <div className="subtle xs">
                Placeholders: <code className="code-inline">{'${json-unit.any-string}'}</code> <code className="code-inline">{'${json-unit.any-number}'}</code>{' '}
                <code className="code-inline">{'${json-unit.ignore}'}</code> <code className="code-inline">{'${json-unit.regex}[a-z]+'}</code>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}

function editorText(p: ContentPattern, op: string): string {
  if (op === 'and' || op === 'or' || op === 'not' || !([...BODY_OPS] as string[]).includes(op)) return prettyJson(p);
  const v = p[op];
  if (v === undefined || op === 'absent') return '';
  if (typeof v === 'string') return v;
  return prettyJson(v);
}

export function BodyPatternsEditor({ value, onChange, contentType }: { value?: ContentPattern[]; onChange(v?: ContentPattern[]): void; contentType?: string }) {
  const list = value ?? [];
  // Stable keys so each row keeps its own editor state when rows are removed.
  const seq = useRef(0);
  const ids = useRef<number[]>([]);
  while (ids.current.length < list.length) ids.current.push(seq.current++);
  if (ids.current.length > list.length) ids.current.length = list.length;
  return (
    <div className="col-sm">
      {list.map((p, i) => (
        <BodyPatternEditor
          key={ids.current[i]}
          value={p}
          contentType={contentType}
          onChange={(np) => onChange(list.map((x, j) => (j === i ? np : x)))}
          onRemove={() => {
            ids.current.splice(i, 1);
            const n = list.filter((_, j) => j !== i);
            onChange(n.length ? n : undefined);
          }}
        />
      ))}
      <div className="row-sm">
        <button className="btn btn-sm btn-ghost" onClick={() => onChange([...list, { equalToJson: {}, ignoreExtraElements: true }])}>
          <Braces /> JSON body
        </button>
        <button className="btn btn-sm btn-ghost" onClick={() => onChange([...list, { matchesJsonPath: '$.id' }])}>
          <Plus /> JSONPath
        </button>
        <button className="btn btn-sm btn-ghost" onClick={() => onChange([...list, { contains: '' }])}>
          <Plus /> Text
        </button>
        <button className="btn btn-sm btn-ghost" onClick={() => onChange([...list, { matchesXPath: '//id' }])}>
          <Plus /> XPath
        </button>
      </div>
    </div>
  );
}

/**
 * HTTP method picker. On WireMock 4 it also offers `{ oneOf: […] }` and
 * `{ noneOf: […] }`; a set already in the stub stays editable on 3.x (with a warning).
 */
export function MethodSelect({ value, onChange, width = 110 }: { value?: MethodPattern; onChange(m: MethodPattern): void; width?: number }) {
  const caps = useCapabilities();
  const set = methodSetOf(value);
  const methods = caps.v4 ? METHODS_V4 : METHODS;
  const single = typeof value === 'string' && value ? value.toUpperCase() : 'ANY';
  const choices = methods.filter((m) => m !== 'ANY');
  const selected = set ? `__${set.kind}` : single;
  return (
    <div className="col-sm">
      <select
        className="select"
        style={{ width }}
        value={selected}
        onChange={(e) => {
          const v = e.target.value;
          if (v === '__oneOf' || v === '__noneOf') {
            const seed = set?.methods ?? (single !== 'ANY' ? [single] : v === '__oneOf' ? ['GET', 'HEAD'] : ['DELETE']);
            onChange(v === '__oneOf' ? { oneOf: seed } : { noneOf: seed });
          } else onChange(v);
        }}
      >
        {methods.map((m) => (
          <option key={m}>{m}</option>
        ))}
        {!set && !methods.includes(single) && <option>{single}</option>}
        {(caps.methodSets || set) && (
          <>
            <option value="__oneOf">one of…</option>
            <option value="__noneOf">none of…</option>
          </>
        )}
      </select>
      {set && (
        <>
          <div className="segmented" style={{ flexWrap: 'wrap', maxWidth: 260 }} role="group" aria-label={set.kind === 'oneOf' ? 'Methods to match' : 'Methods to exclude'}>
            {choices.map((m) => {
              const on = set.methods.some((x) => x.toUpperCase() === m);
              return (
                <button
                  key={m}
                  type="button"
                  className={clsx(on && 'on')}
                  aria-pressed={on}
                  onClick={() => {
                    const next = on ? set.methods.filter((x) => x.toUpperCase() !== m) : [...set.methods, m];
                    if (next.length) onChange(set.kind === 'oneOf' ? { oneOf: next } : { noneOf: next });
                  }}
                >
                  {m}
                </button>
              );
            })}
          </div>
          {caps.loaded && !caps.methodSets && <span className="text-warning xs">Method sets need WireMock 4; this server rejects them.</span>}
        </>
      )}
    </div>
  );
}

export function TagsInput({ value, onChange, placeholder = 'Add tag…', suggestions }: { value: string[]; onChange(v: string[]): void; placeholder?: string; suggestions?: string[] }) {
  const [draft, setDraft] = useState('');
  const listId = useRef(`dl-${Math.random().toString(36).slice(2)}`).current;
  const add = () => {
    const t = draft.trim().replace(/,$/, '');
    if (t && !value.includes(t)) onChange([...value, t]);
    setDraft('');
  };
  return (
    <div className="row-sm wrap input" style={{ height: 'auto', minHeight: 'var(--control-h)', padding: '3px 6px' }}>
      {value.map((t) => (
        <span key={t} className="tag">
          {t}
          <button onClick={() => onChange(value.filter((x) => x !== t))} aria-label={`Remove ${t}`}>
            <X />
          </button>
        </span>
      ))}
      <input
        style={{ border: 0, outline: 'none', background: 'transparent', flex: 1, minWidth: 80, fontSize: 'var(--fs-sm)' }}
        value={draft}
        list={suggestions ? listId : undefined}
        placeholder={value.length ? '' : placeholder}
        onChange={(e) => (e.target.value.endsWith(',') ? (setDraft(e.target.value), setTimeout(add)) : setDraft(e.target.value))}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            add();
          } else if (e.key === 'Backspace' && !draft && value.length) onChange(value.slice(0, -1));
        }}
        onBlur={add}
      />
      {suggestions && (
        <datalist id={listId}>
          {suggestions.filter((s) => !value.includes(s)).map((s) => (
            <option key={s} value={s} />
          ))}
        </datalist>
      )}
    </div>
  );
}

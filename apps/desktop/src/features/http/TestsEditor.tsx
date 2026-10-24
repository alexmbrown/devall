import { Plus, Trash2 } from 'lucide-react';
import { Checkbox } from '@/components/ui/misc';
import { VarInput } from '@/components/VarInput';
import type { AssertOp, AssertTarget, Assertion, Capture, CaptureSource, VarInfo } from '@/lib/types';
import { AiAssistButton } from '@/features/ai';
import type { ResponseContext } from '@/lib/api/ai';
import { parseIgnore, snapshotPath } from './snapshots';

const TARGETS: { value: AssertTarget['assert']; label: string }[] = [
  { value: 'status', label: 'Status' },
  { value: 'jsonPath', label: 'JSON path' },
  { value: 'header', label: 'Header' },
  { value: 'body', label: 'Body' },
  { value: 'responseTime', label: 'Response time (ms)' },
  { value: 'size', label: 'Size (bytes)' },
  { value: 'contentType', label: 'Content-Type' },
  { value: 'snapshot', label: 'Snapshot (body)' },
];

const OPS: { value: AssertOp; label: string; noValue?: boolean }[] = [
  { value: 'eq', label: 'equals' },
  { value: 'ne', label: 'not equal' },
  { value: 'lt', label: '<' },
  { value: 'lte', label: '≤' },
  { value: 'gt', label: '>' },
  { value: 'gte', label: '≥' },
  { value: 'contains', label: 'contains' },
  { value: 'notContains', label: 'does not contain' },
  { value: 'startsWith', label: 'starts with' },
  { value: 'endsWith', label: 'ends with' },
  { value: 'matches', label: 'matches regex' },
  { value: 'oneOf', label: 'is one of (a, b, …)' },
  { value: 'isType', label: 'is type' },
  { value: 'exists', label: 'exists', noValue: true },
  { value: 'notExists', label: 'does not exist', noValue: true },
  { value: 'isEmpty', label: 'is empty', noValue: true },
  { value: 'isNotEmpty', label: 'is not empty', noValue: true },
];

const SOURCES: { value: CaptureSource['from']; label: string }[] = [
  { value: 'jsonPath', label: 'JSON path' },
  { value: 'header', label: 'Header' },
  { value: 'cookie', label: 'Cookie' },
  { value: 'regex', label: 'Regex on body' },
  { value: 'status', label: 'Status code' },
  { value: 'body', label: 'Whole body' },
];

function targetWith(kind: AssertTarget['assert'], prev: Assertion, requestPath: string): AssertTarget {
  if (kind === 'jsonPath') return { assert: kind, path: 'path' in prev ? prev.path : '$.' };
  if (kind === 'header') return { assert: kind, name: 'name' in prev ? prev.name : '' };
  if (kind === 'snapshot') return { assert: kind, file: snapshotPath(requestPath || 'untitled') };
  return { assert: kind } as AssertTarget;
}

/** Snapshot assertions only compare: “matches” (eq) or “differs” (ne). */
const SNAPSHOT_OPS: { value: AssertOp; label: string }[] = [
  { value: 'eq', label: 'matches' },
  { value: 'ne', label: 'differs from' },
];

function sourceWith(kind: CaptureSource['from'], prev: Capture): CaptureSource {
  switch (kind) {
    case 'jsonPath':
      return { from: kind, path: 'path' in prev ? prev.path : '$.' };
    case 'header':
      return { from: kind, header: prev.from === 'cookie' ? prev.cookie : '' };
    case 'cookie':
      return { from: kind, cookie: prev.from === 'header' ? prev.header : '' };
    case 'regex':
      return { from: kind, pattern: 'pattern' in prev ? prev.pattern : '', group: 1 };
    default:
      return { from: kind } as CaptureSource;
  }
}

export function TestsEditor({
  tests,
  captures,
  onTests,
  onCaptures,
  vars,
  sample,
  path = '',
}: {
  tests: Assertion[];
  captures: Capture[];
  onTests(t: Assertion[]): void;
  onCaptures(c: Capture[]): void;
  vars: VarInfo[];
  /** The last response, for “Suggest tests”. */
  sample?: ResponseContext | null;
  /** Request path (relative to http/), for default snapshot file names. */
  path?: string;
}) {
  const setTest = (i: number, t: Assertion) => onTests(tests.map((x, j) => (j === i ? t : x)));
  const setCap = (i: number, c: Capture) => onCaptures(captures.map((x, j) => (j === i ? c : x)));
  return (
    <div className="col-lg">
      <section className="section">
        <div className="section-head">
          <h3>Assertions</h3>
          <span className="subtle xs">Checked on every send; failures fail <code className="code-inline">devall run</code> in CI.</span>
          <span className="spacer" />
          {sample && (
            <AiAssistButton
              feature="suggestTests"
              align="right"
              title="Suggest assertions from the last response"
              input={(p) => ({ ...sample, notes: p || null })}
              onResult={(r) => onTests([...tests, ...r.tests.map((t) => t.assertion)])}
              applyLabel="Add assertions"
            />
          )}
          <button className="btn btn-sm" onClick={() => onTests([...tests, { assert: 'status', op: 'eq', value: '200' }])}>
            <Plus /> Add assertion
          </button>
        </div>
        {tests.length === 0 && <p className="muted small">No assertions yet. Try “status equals 200” or “JSON path $.id exists”.</p>}
        {tests.map((t, i) => {
          const noValue = OPS.find((o) => o.value === t.op)?.noValue;
          return (
            <div key={i} className={`test-row${t.enabled === false ? ' off' : ''}`}>
              <Checkbox checked={t.enabled !== false} onChange={(v) => setTest(i, { ...t, enabled: v ? undefined : false })} />
              <select
                className="select sm"
                value={t.assert}
                onChange={(e) => {
                  const kind = e.target.value as AssertTarget['assert'];
                  const { op, value, enabled } = t;
                  setTest(i, kind === 'snapshot' ? ({ ...targetWith(kind, t, path), op: 'eq', enabled } as Assertion) : ({ ...targetWith(kind, t, path), op, value, enabled } as Assertion));
                }}
              >
                {TARGETS.map((x) => (
                  <option key={x.value} value={x.value}>
                    {x.label}
                  </option>
                ))}
              </select>
              {t.assert === 'jsonPath' ? (
                <input className="input sm mono" value={t.path} placeholder="$.data[0].id" onChange={(e) => setTest(i, { ...t, path: e.target.value })} />
              ) : t.assert === 'header' ? (
                <input className="input sm mono" value={t.name} placeholder="Header name" onChange={(e) => setTest(i, { ...t, name: e.target.value })} />
              ) : t.assert === 'snapshot' ? (
                <input className="input sm mono" value={t.file} placeholder={snapshotPath(path || 'untitled')} title="Snapshot file, relative to the project root" onChange={(e) => setTest(i, { ...t, file: e.target.value })} />
              ) : (
                <span />
              )}
              {t.assert === 'snapshot' ? (
                <>
                  <select className="select sm" value={t.op === 'ne' ? 'ne' : 'eq'} onChange={(e) => setTest(i, { ...t, op: e.target.value as AssertOp })}>
                    {SNAPSHOT_OPS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                  <input
                    className="input sm mono"
                    defaultValue={(t.ignore ?? []).join(', ')}
                    key={(t.ignore ?? []).join(',')}
                    placeholder="ignore: $..createdAt, $..id"
                    title="JSONPaths masked before comparing (timestamps, generated ids)"
                    onBlur={(e) => {
                      const ignore = parseIgnore(e.target.value);
                      setTest(i, { ...t, ignore: ignore.length ? ignore : undefined });
                    }}
                  />
                </>
              ) : (
                <>
                  <select className="select sm" value={t.op} onChange={(e) => setTest(i, { ...t, op: e.target.value as AssertOp, value: OPS.find((o) => o.value === e.target.value)?.noValue ? undefined : (t.value ?? '') })}>
                    {OPS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                  {noValue ? <span /> : <VarInput value={t.value ?? ''} onChange={(value) => setTest(i, { ...t, value })} vars={vars} placeholder={t.op === 'isType' ? 'string | number | boolean | array | object | null' : 'expected'} className="sm" />}
                </>
              )}
              <button className="icon-btn sm danger" onClick={() => onTests(tests.filter((_, j) => j !== i))} aria-label="Remove assertion">
                <Trash2 />
              </button>
            </div>
          );
        })}
      </section>

      <section className="section">
        <div className="section-head">
          <h3>Captures</h3>
          <span className="subtle xs">Save response values as variables for later requests — e.g. capture a token, then use {'{{token}}'}.</span>
          <span className="spacer" />
          <button className="btn btn-sm" onClick={() => onCaptures([...captures, { name: '', from: 'jsonPath', path: '$.' }])}>
            <Plus /> Add capture
          </button>
        </div>
        {captures.length === 0 && <p className="muted small">No captures. Captured values live for this app session (and for the duration of a collection run).</p>}
        {captures.map((c, i) => (
          <div key={i} className={`capture-row${c.enabled === false ? ' off' : ''}`}>
            <Checkbox checked={c.enabled !== false} onChange={(v) => setCap(i, { ...c, enabled: v ? undefined : false })} />
            <input className="input sm mono" value={c.name} placeholder="variable" onChange={(e) => setCap(i, { ...c, name: e.target.value })} />
            <span className="subtle xs">from</span>
            <select className="select sm" value={c.from} onChange={(e) => setCap(i, { name: c.name, enabled: c.enabled, ...sourceWith(e.target.value as CaptureSource['from'], c) })}>
              {SOURCES.map((s) => (
                <option key={s.value} value={s.value}>
                  {s.label}
                </option>
              ))}
            </select>
            {c.from === 'jsonPath' && <input className="input sm mono" value={c.path} placeholder="$.access_token" onChange={(e) => setCap(i, { ...c, path: e.target.value })} />}
            {c.from === 'header' && <input className="input sm mono" value={c.header} placeholder="Header name" onChange={(e) => setCap(i, { ...c, header: e.target.value })} />}
            {c.from === 'cookie' && <input className="input sm mono" value={c.cookie} placeholder="Cookie name" onChange={(e) => setCap(i, { ...c, cookie: e.target.value })} />}
            {c.from === 'regex' && (
              <div className="row-sm">
                <input className="input sm mono" value={c.pattern} placeholder={'"id":(\\d+)'} onChange={(e) => setCap(i, { ...c, pattern: e.target.value })} />
                <input className="input sm" type="number" min={0} style={{ width: 64 }} value={c.group ?? 0} title="Group" onChange={(e) => setCap(i, { ...c, group: Number(e.target.value) || 0 })} />
              </div>
            )}
            {(c.from === 'status' || c.from === 'body') && <span />}
            <button className="icon-btn sm danger" onClick={() => onCaptures(captures.filter((_, j) => j !== i))} aria-label="Remove capture">
              <Trash2 />
            </button>
          </div>
        ))}
      </section>
    </div>
  );
}

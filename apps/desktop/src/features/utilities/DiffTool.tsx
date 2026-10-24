import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { ArrowLeftRight } from 'lucide-react';
import { CodeDiff, CodeEditor } from '@/components/editor/CodeEditor';
import { Callout, CopyButton, Segmented, Spinner, Switch } from '@/components/ui/misc';
import type { JsonDiffResult, JsonDifference, TextDiffResult } from '@/lib/api/utils';
import { PaneHead, ToolHeader, loadLocal, saveLocal, sizeLabel, useOpRunner, useRecordRun, useSeed, useUtilities, type ToolProps } from './shared';

type Mode = 'text' | 'json';
type Gran = 'line' | 'word' | 'char';
const STORE_KEY = 'devall.utils.diff';

function parseJson<T>(s: string): T | null {
  try {
    return s ? (JSON.parse(s) as T) : null;
  } catch {
    return null;
  }
}

/** Pretty JSON with sorted keys, so the side-by-side view ignores key order like the semantic diff. */
function normalized(text: string): string {
  const sort = (v: unknown): unknown =>
    Array.isArray(v) ? v.map(sort) : v && typeof v === 'object' ? Object.fromEntries(Object.keys(v as object).sort().map((k) => [k, sort((v as Record<string, unknown>)[k])])) : v;
  try {
    return JSON.stringify(sort(JSON.parse(text)), null, 2);
  } catch {
    return text;
  }
}

function short(v: unknown): string {
  if (v === undefined) return '';
  const s = JSON.stringify(v);
  return s.length > 160 ? `${s.slice(0, 160)}…` : s;
}

export function DiffTool({ op, input, setInput, seed }: ToolProps) {
  const { open } = useUtilities();
  const mode: Mode = op.id === 'diff.json' ? 'json' : 'text';
  const [right, setRight] = useState(() => loadLocal(STORE_KEY, ''));
  const [gran, setGran] = useState<Gran>('line');
  const [context, setContext] = useState('3');
  const [unordered, setUnordered] = useState(false);
  const [textView, setTextView] = useState<'split' | 'unified' | 'inline'>('split');
  const [jsonView, setJsonView] = useState<'changes' | 'split' | 'patch'>('changes');

  useEffect(() => saveLocal(STORE_KEY, right), [right]);
  useEffect(() => {
    if (gran !== 'line' && textView === 'unified') setTextView('inline');
    if (gran === 'line' && textView === 'inline') setTextView('unified');
  }, [gran, textView]);
  const applyArgs = (a: Record<string, string>) => {
    if (a.other !== undefined) setRight(a.other);
    if (a.mode === 'line' || a.mode === 'word' || a.mode === 'char') setGran(a.mode);
    if (a.context) setContext(a.context);
    if (a.ignoreArrayOrder) setUnordered(a.ignoreArrayOrder === 'yes');
  };
  useSeed(seed, applyArgs);

  const both = input.length > 0 || right.length > 0;
  const text = useOpRunner('diff.text', input, { other: right, mode: gran, context }, { enabled: mode === 'text' && both });
  const json = useOpRunner('diff.json', input, { other: right, ignoreArrayOrder: unordered ? 'yes' : 'no' }, { enabled: mode === 'json' && input.trim().length > 0 && right.trim().length > 0 });
  const tr = useMemo(() => parseJson<TextDiffResult>(text.output), [text.output]);
  const jr = useMemo(() => parseJson<JsonDiffResult>(json.output), [json.output]);
  useRecordRun(op, input, mode === 'json' ? { other: right, ignoreArrayOrder: unordered ? 'yes' : 'no' } : { other: right, mode: gran, context }, mode === 'json' ? json : text);
  const lang = mode === 'json' ? 'json' : 'plaintext';

  return (
    <div className="page">
      <ToolHeader
        op={op}
        onExample={(inp, a) => {
          setInput(inp);
          applyArgs(a);
        }}
      >
        <Segmented
          value={mode}
          onChange={(m) => open(m === 'json' ? 'diff.json' : 'diff.text')}
          options={[
            { value: 'text', label: 'Text' },
            { value: 'json', label: 'JSON', title: 'Semantic JSON diff: key order ignored, RFC 6902 patch' },
          ]}
        />
      </ToolHeader>
      <div className="util-body util-diff">
        <div className="row wrap util-args">
          {mode === 'text' ? (
            <>
              <Segmented
                value={gran}
                onChange={setGran}
                options={[
                  { value: 'line', label: 'Lines' },
                  { value: 'word', label: 'Words' },
                  { value: 'char', label: 'Characters' },
                ]}
              />
              {gran === 'line' && (
                <label className="row-sm small muted">
                  Context
                  <input className="input sm" type="number" min={0} style={{ width: 64 }} value={context} onChange={(e) => setContext(e.target.value)} />
                </label>
              )}
            </>
          ) : (
            <Switch checked={unordered} onChange={setUnordered} label="Ignore array order" title="Compare arrays as multisets; objects with the same id/key/name are paired" />
          )}
          <span className="spacer" />
          <button
            className="btn btn-sm"
            onClick={() => {
              const l = input;
              setInput(right);
              setRight(l);
            }}
            title="Swap original and changed"
          >
            <ArrowLeftRight /> Swap
          </button>
        </div>
        <div className="util-diff-inputs">
          <div className="util-pane">
            <PaneHead title="Original" meta={input ? sizeLabel(input) : undefined} />
            <div className="editor-shell">
              <CodeEditor value={input} onChange={setInput} language={lang} minimal />
            </div>
          </div>
          <div className="util-pane">
            <PaneHead title="Changed" meta={right ? sizeLabel(right) : undefined} />
            <div className="editor-shell">
              <CodeEditor value={right} onChange={setRight} language={lang} minimal />
            </div>
          </div>
        </div>
        <div className="util-diff-result util-pane">
          {mode === 'text' ? (
            <>
              <PaneHead
                title="Result"
                lead={
                  <>
                    {tr && !text.error ? (
                      tr.identical ? (
                        <span className="badge success">identical</span>
                      ) : (
                        <>
                          <span className="badge success">+{tr.stats.inserted}</span>
                          <span className="badge danger">−{tr.stats.deleted}</span>
                          <span className="subtle xs truncate">
                            {tr.stats.unit} · {Math.round(tr.similarity * 100)}% similar
                          </span>
                        </>
                      )
                    ) : null}
                    {text.running && <Spinner />}
                  </>
                }
              >
                <Segmented
                  value={textView}
                  onChange={setTextView}
                  options={[{ value: 'split', label: 'Side by side' }, gran === 'line' ? { value: 'unified', label: 'Unified' } : { value: 'inline', label: 'Inline' }]}
                />
                <CopyButton text={tr?.unified ?? ''} label={gran === 'line' ? 'Copy patch' : 'Copy'} />
              </PaneHead>
              {text.error ? (
                <Callout kind="danger">{text.error}</Callout>
              ) : textView === 'split' ? (
                <div className="editor-shell">
                  <CodeDiff original={input} modified={right} language="plaintext" />
                </div>
              ) : textView === 'unified' ? (
                <UnifiedView text={tr?.unified ?? ''} />
              ) : (
                <pre className="util-preview util-inline-diff">
                  {(tr?.segments ?? []).map((s, i) => (
                    <span key={i} className={clsx(s.tag === 'insert' && 'ins', s.tag === 'delete' && 'del')}>
                      {s.value}
                    </span>
                  ))}
                </pre>
              )}
            </>
          ) : (
            <>
              <PaneHead
                title="Result"
                lead={
                  <>
                    {jr && !json.error ? (
                      jr.equal ? (
                        <span className="badge success">semantically equal</span>
                      ) : (
                        <>
                          <span className="badge success">{jr.summary.added} added</span>
                          <span className="badge danger">{jr.summary.removed} removed</span>
                          <span className="badge warning">{jr.summary.changed} changed</span>
                        </>
                      )
                    ) : null}
                    {json.running && <Spinner />}
                  </>
                }
              >
                <Segmented
                  value={jsonView}
                  onChange={setJsonView}
                  options={[
                    { value: 'changes', label: 'Changes' },
                    { value: 'split', label: 'Side by side' },
                    { value: 'patch', label: 'JSON Patch' },
                  ]}
                />
                <CopyButton text={jr ? JSON.stringify(jr.patch, null, 2) : ''} label="Copy patch" title="Copy the RFC 6902 JSON Patch" />
              </PaneHead>
              {json.error ? (
                <Callout kind="danger">{json.error}</Callout>
              ) : !input.trim() || !right.trim() ? (
                <span className="subtle xs">Paste two JSON documents. Key order never counts as a difference.</span>
              ) : jsonView === 'changes' ? (
                <ChangeList diffs={jr?.differences ?? []} note={jr?.note} />
              ) : jsonView === 'split' ? (
                <div className="editor-shell">
                  <CodeDiff original={normalized(input)} modified={normalized(right)} language="json" />
                </div>
              ) : (
                <div className="editor-shell">
                  <CodeEditor value={jr ? JSON.stringify(jr.patch, null, 2) : ''} language="json" readOnly minimal />
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function UnifiedView({ text }: { text: string }) {
  const lines = text ? text.replace(/\n$/, '').split('\n') : [];
  return (
    <pre className="util-preview util-unified">
      {lines.map((l, i) => (
        <div key={i} className={clsx(l.startsWith('+') && !l.startsWith('+++') && 'diffline-add', l.startsWith('-') && !l.startsWith('---') && 'diffline-del', l.startsWith('@@') && 'hunk')}>
          {l || ' '}
        </div>
      ))}
    </pre>
  );
}

const OP_BADGE: Record<JsonDifference['op'], string> = { added: 'success', removed: 'danger', changed: 'warning' };

function ChangeList({ diffs, note }: { diffs: JsonDifference[]; note?: string }) {
  if (!diffs.length) return <span className="subtle xs">No differences.</span>;
  return (
    <div className="util-table-wrap">
      {note && <div className="subtle xs p-2">{note}</div>}
      <table className="table">
        <thead>
          <tr>
            <th />
            <th>Path</th>
            <th>Old</th>
            <th>New</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {diffs.map((d, i) => (
            <tr key={i}>
              <td>
                <span className={clsx('badge', OP_BADGE[d.op])}>{d.op}</span>
              </td>
              <td className="mono util-cell" title={d.pointer}>
                {d.path}
              </td>
              <td className="mono util-cell del">{short(d.old)}</td>
              <td className="mono util-cell ins">{short(d.new)}</td>
              <td>
                <CopyButton text={d.pointer} title="Copy JSON Pointer" />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

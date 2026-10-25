import { useEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { CheckCircle2, Eye, Sparkles, XCircle } from 'lucide-react';
import { Callout, CopyButton, Spinner } from '@/components/ui/misc';
import { aiApi, type Feature, type FeatureMap, type Meta, type Preview } from '@/lib/api/ai';
import { errorMessage } from '@/lib/ipc';
import { useNav } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { newCallId, useAi } from './store';
import { SourceList, linkedText } from './sources';
import './ai.css';

const DEFAULT_LABEL: Record<Feature, string> = {
  ask: 'Ask AI',
  nlRegex: 'Regex from description',
  nlJsonPath: 'JSONPath from description',
  nlJq: 'jq from description',
  nlRequest: 'Draft with AI',
  mockData: 'Generate with AI',
  explainResponse: 'Explain this response',
  explainError: 'Explain error',
  explainFailure: 'Explain this failure',
  openapiNames: 'Improve names with AI',
  nlSql: 'SQL from a question',
  nlMongo: 'Query from a question',
  nlKubectl: 'kubectl from a description',
  suggestTests: 'Suggest tests',
  summarizeNotes: 'Summarize',
  todosFromText: 'Extract to-dos',
  commitMessage: 'Suggest message',
};

const PLACEHOLDER: Partial<Record<Feature, string>> = {
  nlRegex: 'e.g. order ids like ORD-1234, or an ISO date',
  nlJsonPath: 'e.g. the ids of all active users',
  nlJq: 'e.g. names and emails of users created this year, sorted by name',
  nlRequest: 'e.g. create a refund for charge {{chargeId}} with an idempotency key',
  mockData: 'Optional hints, e.g. German customers, prices in EUR',
  explainResponse: 'Optional: what you expected instead',
  explainError: 'Optional: what you were doing',
  explainFailure: 'Optional: what you expected instead',
  nlSql: 'e.g. the 10 customers with the most orders this year',
  nlMongo: 'e.g. paid orders per customer, most first',
  nlKubectl: 'e.g. why is the api pod restarting? / scale the worker to 3',
  suggestTests: 'Optional: what matters most in this response',
  todosFromText: 'Optional: focus, e.g. only my action items',
  commitMessage: 'Optional: what it is for, e.g. fixes the login redirect loop',
};

/** Features whose input doesn't need a typed prompt (the button supplies everything). */
const PROMPT_OPTIONAL: Feature[] = ['mockData', 'explainResponse', 'explainError', 'explainFailure', 'suggestTests', 'summarizeNotes', 'todosFromText', 'commitMessage'];

export interface AiAssistButtonProps<F extends Feature> {
  feature: F;
  /** Builds the feature input from the text typed in the popover (called on Generate). */
  input: (prompt: string) => FeatureMap[F]['input'] | Promise<FeatureMap[F]['input']>;
  /** Receives the validated result when the user clicks Apply (omit for read-only features). */
  onResult?: (result: FeatureMap[F]['output']) => void;
  label?: string;
  title?: string;
  placeholder?: string;
  /** Show the prompt box (default: required for NL features, optional otherwise). */
  prompt?: boolean;
  applyLabel?: string;
  /** Project root to use (default: the open project). */
  root?: string;
  align?: 'left' | 'right';
  className?: string;
  /** Icon-only button. */
  compact?: boolean;
  disabled?: boolean;
}

/**
 * A small "✨" button that opens a popover with a prompt box, runs an AI feature against the
 * configured (local-first) model, shows the validated result and hands it to `onResult`.
 *
 * Mount points: Utilities regex tool (`nlRegex`), HTTP client URL bar (`nlRequest`), response
 * panel (`explainResponse`), Tests tab (`suggestTests`), response body filter (`nlJq`, `nlJsonPath`),
 * Fake data (`mockData`), Notes (`summarizeNotes`), To-dos (`todosFromText`), Source control's commit box
 * (`commitMessage`).
 */
export function AiAssistButton<F extends Feature>(props: AiAssistButtonProps<F>) {
  const { feature, onResult, compact, className, disabled } = props;
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const label = props.label ?? DEFAULT_LABEL[feature];

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
    window.addEventListener('mousedown', onDown);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  return (
    <div className={clsx('ai-assist', className)} ref={ref}>
      <button
        className={clsx(compact ? 'icon-btn sm' : 'btn btn-sm btn-ghost', open && 'on')}
        onClick={() => setOpen((o) => !o)}
        title={props.title ?? `${label} (local AI)`}
        aria-label={label}
        aria-expanded={open}
        disabled={disabled}
      >
        <Sparkles />
        {!compact && label}
      </button>
      {open && <AssistPopover {...props} label={label} onApplied={() => setOpen(false)} onResult={onResult} />}
    </div>
  );
}

function AssistPopover<F extends Feature>(props: AiAssistButtonProps<F> & { label: string; onApplied(): void }) {
  const { feature, input, onResult, onApplied, align = 'left' } = props;
  const project = useWorkspace((s) => s.project);
  const root = props.root ?? project?.root;
  const { view, load, selected } = useAi();
  const [text, setText] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<FeatureMap[F]['output'] | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const showPrompt = props.prompt ?? !PROMPT_OPTIONAL.includes(feature);
  const go = useNav((s) => s.go);

  useEffect(() => {
    if (!view) load();
  }, [view, load]);
  const noProvider = view && view.config.providers.length === 0 && !view.managed;

  const run = async () => {
    if (showPrompt && !PROMPT_OPTIONAL.includes(feature) && !text.trim()) return;
    const id = newCallId();
    setBusy(id);
    setError(null);
    setResult(null);
    setPreview(null);
    try {
      const inp = await input(text.trim());
      setResult(await aiApi.run(root, feature, inp, { provider: selected, callId: id }));
    } catch (e) {
      const msg = errorMessage(e);
      if (msg !== 'cancelled') setError(msg);
    } finally {
      setBusy(null);
    }
  };

  const showPreview = async () => {
    setError(null);
    try {
      setPreview(await aiApi.preview(root, feature, await input(text.trim()), selected));
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <div className={clsx('ai-pop', align)} role="dialog" aria-label={props.label}>
      <div className="row">
        <Sparkles size={14} />
        <span className="strong grow">{props.label}</span>
        <span className="subtle xs">runs on your configured model</span>
      </div>
      {noProvider ? (
        <Callout kind="warning">
          No model configured yet.{' '}
          <button className="btn btn-sm btn-ghost" onClick={() => go('ai', 'models')}>
            Set up a local model
          </button>
        </Callout>
      ) : (
        <>
          {showPrompt && (
            <textarea
              className="textarea"
              autoFocus
              value={text}
              placeholder={props.placeholder ?? PLACEHOLDER[feature] ?? 'Describe what you want'}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) run();
              }}
            />
          )}
          <div className="row">
            {busy ? (
              <>
                <Spinner /> <span className="subtle small grow">Thinking…</span>
                <button className="btn btn-sm" onClick={() => aiApi.cancel(busy)}>
                  Cancel
                </button>
              </>
            ) : (
              <>
                <button className="btn btn-sm btn-ghost" onClick={showPreview} title="Show exactly what will be sent (after redaction)">
                  <Eye /> What will be sent
                </button>
                <span className="spacer" />
                <button className="btn btn-sm btn-primary" onClick={run} disabled={showPrompt && !PROMPT_OPTIONAL.includes(feature) && !text.trim()}>
                  <Sparkles /> Generate
                </button>
              </>
            )}
          </div>
        </>
      )}
      {error && <Callout kind="danger">{error}</Callout>}
      {preview && <PreviewView preview={preview} compact />}
      {result && (
        <div className="ai-result">
          <AiResultView feature={feature} result={result} />
          {onResult && (
            <div className="row">
              <span className="spacer" />
              <button
                className="btn btn-sm btn-primary"
                onClick={() => {
                  onResult(result);
                  onApplied();
                }}
              >
                {props.applyLabel ?? 'Apply'}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export function MetaLine({ meta }: { meta: Meta }) {
  return (
    <div className="ai-meta">
      <span className={clsx('badge', meta.remote ? 'warning' : 'success')}>{meta.remote ? 'cloud' : 'local'}</span>
      <span className="truncate">
        {meta.provider} · {meta.model}
      </span>
      {meta.redactions > 0 && <span title="Secret values and credential-like strings were masked before sending">{meta.redactions} masked</span>}
      {meta.repaired && <span title="The first answer didn't validate; the model corrected it">repaired</span>}
    </div>
  );
}

function Check({ ok, children }: { ok: boolean; children: ReactNode }) {
  return (
    <div className="row-sm small">
      {ok ? <CheckCircle2 size={13} className="text-success" /> : <XCircle size={13} className="text-danger" />}
      <span className="break">{children}</span>
    </div>
  );
}

const pretty = (v: unknown) => JSON.stringify(v, null, 2);

/** Compact rendering of any feature's result. */
export function AiResultView<F extends Feature>({ feature, result }: { feature: F; result: FeatureMap[F]['output'] }) {
  const r = result as FeatureMap[Feature]['output'];
  const body = (() => {
    switch (feature) {
      case 'nlRegex': {
        const x = r as FeatureMap['nlRegex']['output'];
        return (
          <>
            <pre className="ai-pre">
              /{x.pattern}/{x.flags}
            </pre>
            <div className="small muted">{x.explanation}</div>
            {!x.compiles && <Callout kind="danger">Doesn't compile with the Rust regex engine.</Callout>}
            {x.checks.map((c, i) => (
              <Check key={i} ok={c.matched === c.shouldMatch}>
                <span className="mono xs">{c.sample}</span> {c.shouldMatch ? 'should match' : 'should not match'}
              </Check>
            ))}
            {x.problem && <Callout kind="warning">{x.problem}</Callout>}
          </>
        );
      }
      case 'nlJsonPath': {
        const x = r as FeatureMap['nlJsonPath']['output'];
        return (
          <>
            <pre className="ai-pre">{x.path}</pre>
            <div className="small muted">{x.explanation}</div>
            <Check ok={x.valid}>
              {x.count} match{x.count === 1 ? '' : 'es'} in the sample
            </Check>
            {x.matches.length > 0 && <pre className="ai-pre">{pretty(x.matches.slice(0, 5))}</pre>}
          </>
        );
      }
      case 'nlJq': {
        const x = r as FeatureMap['nlJq']['output'];
        return (
          <>
            <pre className="ai-pre">{x.filter}</pre>
            <div className="small muted">{x.explanation}</div>
            <Check ok={x.valid}>{x.valid ? 'Runs on the sample' : 'Does not work on the sample'}</Check>
            {x.output && <pre className="ai-pre">{x.output.length > 600 ? `${x.output.slice(0, 600)}…` : x.output}</pre>}
            {x.problem && <Callout kind="warning">{x.problem}</Callout>}
          </>
        );
      }
      case 'nlRequest': {
        const x = r as FeatureMap['nlRequest']['output'];
        return (
          <>
            <div className="row-sm">
              <span className="badge method">{x.request.method}</span>
              <span className="mono xs break">{x.request.url}</span>
            </div>
            <div className="strong small">{x.request.name}</div>
            <div className="small muted">{x.explanation}</div>
            {x.unknownVariables.length > 0 && (
              <Callout kind="warning">
                Undefined variables: <span className="mono xs">{x.unknownVariables.map((v) => `{{${v}}}`).join(', ')}</span>
              </Callout>
            )}
          </>
        );
      }
      case 'mockData': {
        const x = r as FeatureMap['mockData']['output'];
        return (
          <>
            <Check ok={x.valid}>
              {x.items.length} item{x.items.length === 1 ? '' : 's'} {x.valid ? 'valid against the schema' : 'with problems'}
              {x.inferredSchema ? ' (schema inferred from your sample)' : ''}
            </Check>
            {x.errors.slice(0, 5).map((e, i) => (
              <div key={i} className="xs text-danger">
                {e}
              </div>
            ))}
            <pre className="ai-pre">{pretty(x.items.slice(0, 3))}</pre>
          </>
        );
      }
      case 'explainResponse':
      case 'explainError': {
        const x = r as FeatureMap['explainResponse']['output'] & Partial<FeatureMap['explainError']['output']>;
        return (
          <>
            <div className="strong">{x.summary}</div>
            {x.rootCause && <div className="small">{x.rootCause}</div>}
            {x.likelyCauses?.length > 0 && (
              <ul className="small">
                {x.likelyCauses.map((c, i) => (
                  <li key={i}>{c}</li>
                ))}
              </ul>
            )}
            {x.suggestedFixes.map((f, i) => (
              <div key={i} className="small">
                <span className="strong">{f.title}</span> — <span className="muted">{f.detail}</span>
              </div>
            ))}
            {x.relevantLines && x.relevantLines.length > 0 && <pre className="ai-pre">{x.relevantLines.join('\n')}</pre>}
          </>
        );
      }
      case 'explainFailure': {
        const x = r as FeatureMap['explainFailure']['output'];
        return (
          <>
            <div className="strong">{linkedText(x.summary, x.sources)}</div>
            {x.likelyCauses.length > 0 && (
              <ul className="small">
                {x.likelyCauses.map((c, i) => (
                  <li key={i}>{linkedText(c, x.sources)}</li>
                ))}
              </ul>
            )}
            {x.suggestedFixes.map((f, i) => (
              <div key={i} className="small">
                <span className="strong">{f.title}</span> — <span className="muted">{linkedText(f.detail, x.sources)}</span>
              </div>
            ))}
            <SourceList sources={x.sources} cited={x.citations} />
            {x.omitted.length > 0 && <div className="xs subtle">Not sent to the cloud model: {x.omitted.join(', ')}</div>}
          </>
        );
      }
      case 'nlSql': {
        const x = r as FeatureMap['nlSql']['output'];
        return (
          <>
            <div className="ai-snippet">
              <pre className="ai-pre">{x.sql}</pre>
              <CopyButton text={x.sql} />
            </div>
            <div className="small muted">{x.explanation}</div>
            <Check ok={x.validated}>{x.validated ? `The database accepted it (EXPLAIN${x.check?.readOnly === false ? ' — this statement CHANGES data' : ', read-only'})` : 'Not validated by the database'}</Check>
            {x.check?.plan && (
              <details>
                <summary className="xs subtle">Plan</summary>
                <pre className="ai-pre">{x.check.plan}</pre>
              </details>
            )}
            {x.problem && <Callout kind="warning">{x.problem}</Callout>}
          </>
        );
      }
      case 'nlMongo': {
        const x = r as FeatureMap['nlMongo']['output'];
        const text = x.type === 'aggregate' ? x.pipeline : [x.filter && `filter: ${x.filter}`, x.projection && `projection: ${x.projection}`, x.sort && `sort: ${x.sort}`, x.limit ? `limit: ${x.limit}` : ''].filter(Boolean).join('\n');
        return (
          <>
            <div className="row-sm">
              <span className="badge">{x.type}</span>
            </div>
            <pre className="ai-pre">{text || '{}'}</pre>
            <div className="small muted">{x.explanation}</div>
            <Check ok={x.validated}>{x.validated ? 'The server accepted it (explain)' : 'Not validated by the server'}</Check>
            {x.problem && <Callout kind="warning">{x.problem}</Callout>}
          </>
        );
      }
      case 'nlKubectl': {
        const x = r as FeatureMap['nlKubectl']['output'];
        return (
          <>
            <div className="ai-snippet">
              <pre className="ai-pre">{x.command}</pre>
              <CopyButton text={x.command} />
            </div>
            <div className="small muted">{x.explanation}</div>
            {x.check?.mutating && <Callout kind="warning">This command changes the cluster. It was not run.</Callout>}
            <Check ok={x.validated}>{x.validated ? x.check?.dryRun ?? (x.check?.unverified ? 'Parsed; not verifiable on the server' : 'Checked against the cluster') : 'Not validated'}</Check>
            {x.check?.unverified && <div className="xs text-warning">{x.check.unverified}</div>}
            {x.problem && <Callout kind="warning">{x.problem}</Callout>}
          </>
        );
      }
      case 'suggestTests': {
        const x = r as FeatureMap['suggestTests']['output'];
        return (
          <>
            {x.tests.map((t, i) => (
              <div key={i} className="small">
                <span className="mono xs">
                  {t.assertion.assert}
                  {'name' in t.assertion ? ` ${t.assertion.name}` : ''}
                  {'path' in t.assertion ? ` ${t.assertion.path}` : ''} {t.assertion.op} {t.assertion.value ?? ''}
                </span>
                <div className="subtle xs">{t.reason}</div>
              </div>
            ))}
            {x.dropped.length > 0 && <div className="subtle xs">{x.dropped.length} suggestion(s) dropped: they don't fit this response.</div>}
          </>
        );
      }
      case 'todosFromText': {
        const x = r as FeatureMap['todosFromText']['output'];
        return (
          <ul className="small">
            {x.todos.map((t, i) => (
              <li key={i}>
                {t.text}
                {t.due && (
                  <span className="subtle xs">
                    {' '}
                    · {t.due}
                    {t.dueDate && t.dueDate !== t.due ? ` (${t.dueDate})` : ''}
                  </span>
                )}
                {t.priority < 4 && <span className="subtle xs"> · p{t.priority}</span>}
                {t.labels.length > 0 && <span className="subtle xs"> · {t.labels.map((l) => `@${l}`).join(' ')}</span>}
              </li>
            ))}
          </ul>
        );
      }
      case 'summarizeNotes': {
        const x = r as FeatureMap['summarizeNotes']['output'];
        return (
          <>
            <div className="small" style={{ whiteSpace: 'pre-wrap' }}>
              {x.summary}
            </div>
            {x.keyPoints.length > 0 && (
              <ul className="small">
                {x.keyPoints.map((k, i) => (
                  <li key={i}>{k}</li>
                ))}
              </ul>
            )}
          </>
        );
      }
      case 'commitMessage': {
        const x = r as FeatureMap['commitMessage']['output'];
        return (
          <>
            <div className="strong small">{x.subject}</div>
            {x.body && <pre className="ai-pre">{x.body}</pre>}
          </>
        );
      }
      case 'ask': {
        const x = r as FeatureMap['ask']['output'];
        return (
          <div className="small" style={{ whiteSpace: 'pre-wrap' }}>
            {x.answer}
          </div>
        );
      }
      default:
        return <pre className="ai-pre">{pretty(r)}</pre>;
    }
  })();
  return (
    <>
      {body}
      {'meta' in r && <MetaLine meta={r.meta} />}
    </>
  );
}

/** The exact request (after redaction) a feature would send. */
export function PreviewView({ preview, compact }: { preview: Preview; compact?: boolean }) {
  return (
    <div className="ai-preview">
      <Callout kind={preview.remote ? 'warning' : 'info'}>{preview.notice}</Callout>
      <div className="mono xs break">
        {preview.payload.method} {preview.payload.url}
      </div>
      {!compact &&
        preview.payload.headers.map(([k, v]) => (
          <div key={k} className="mono xs subtle">
            {k}: {v}
          </div>
        ))}
      {preview.sources.length > 0 && (
        <div className="xs subtle">
          Context: {preview.sources.map((s) => `${s.id} ${s.source}`).join(' · ')}
        </div>
      )}
      <pre className="ai-pre">{pretty(preview.payload.body)}</pre>
    </div>
  );
}

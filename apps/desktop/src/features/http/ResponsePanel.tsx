import { Fragment, useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { Ban, Camera, CheckCircle2, CircleSlash, Clock, FileText, Filter, History, KeyRound, List, Radio, Send, Terminal, Upload, Wrench, XCircle } from 'lucide-react';
import { CodeEditor, languageFor } from '@/components/editor/CodeEditor';
import { DecodedValues, messageText, useDetections } from '@/components/DecodedValues';
import { ResponsiveTabs, type TabDef } from '@/components/ui/ResponsiveTabs';
import { useViewState } from '@/store/nav';
import { Callout, CopyButton, Empty, Segmented, Spinner, StatusCode, formatBytes } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { AssertionResult, ResponseView, SendResult, SseEvent } from '@/lib/types';
import { useWorkspace } from '@/store/workspace';
import { AiAssistButton } from '@/features/ai';
import { aiApi } from '@/lib/api/ai';
import { openInUtilities } from '@/features/utilities/open';
import { useOpRunner } from '@/features/utilities/shared';
import { MockFromResponseButton } from '@/features/mocks';
import { ScriptLogs, hasScriptOutput, scriptLogCount } from '@/features/scripts';
import { HistoryView, useHistoryFocus } from './HistoryView';
import { responseContext } from './aiContext';
import { upsertSnapshotTest } from './snapshots';

type Sub = 'body' | 'tests' | 'stream' | 'console' | 'headers' | 'timing' | 'sent' | 'inspect' | 'history';

interface Props {
  result?: SendResult;
  sending: boolean;
  onCancel(): void;
  stream?: SseEvent[];
  progress?: number;
  root: string;
  path: string;
}

export function ResponsePanel({ result, sending, onCancel, stream = [], progress = 0, root, path }: Props) {
  const [sub, setSub] = useViewState<Sub>('response.sub', 'body');
  const resp = result?.response ?? null;
  const detections = useDetections(resp ? messageText(resp.headers, resp.bodyText) : null);
  const failed = result?.tests.filter((t) => !t.passed).length ?? 0;

  // Jump to the most useful tab when a stream starts or tests fail.
  useEffect(() => {
    if (sending && stream.length > 0) setSub('stream');
  }, [sending, stream.length]);
  useEffect(() => {
    if (failed > 0) setSub('tests');
  }, [result?.at, failed]);
  useEffect(() => {
    if (result?.scripts?.errors.length) setSub('console');
  }, [result?.at, result?.scripts?.errors.length]);
  // History page → "Open request": show that past response.
  const focusPath = useHistoryFocus((s) => s.path);
  useEffect(() => {
    if (focusPath === path) setSub('history');
  }, [focusPath, path]);

  const tabs: TabDef<Sub>[] = [];
  if (resp) {
    tabs.push({ key: 'body', label: 'Body', icon: <FileText /> });
    if (result!.tests.length || result!.captures.length)
      tabs.push({
        key: 'tests',
        label: 'Tests',
        icon: failed ? <XCircle className="text-danger" /> : <CheckCircle2 className="text-success" />,
        showIcon: true,
        badge: `${result!.tests.length - failed}/${result!.tests.length}`,
      });
  }
  if (stream.length) tabs.push({ key: 'stream', label: 'Stream', icon: <Radio />, showIcon: true, badge: stream.length });
  if (resp) {
    tabs.push({ key: 'headers', label: 'Headers', icon: <List />, badge: resp.headers.length }, { key: 'timing', label: 'Timing', icon: <Clock /> }, { key: 'sent', label: 'Request sent', icon: <Upload /> });
    if (detections.length) tabs.push({ key: 'inspect', label: 'Decoded', icon: <KeyRound />, showIcon: true, badge: detections.length });
  }
  if (hasScriptOutput(result?.scripts)) tabs.push({ key: 'console', label: 'Console', icon: <Terminal />, showIcon: true, badge: scriptLogCount(result!.scripts) });
  tabs.push({ key: 'history', label: 'History', icon: <History /> });
  const current = tabs.some((t) => t.key === sub) ? sub : (tabs[0]?.key ?? 'history');

  return (
    <div className="col min-h-0 h-full" style={{ gap: 0 }}>
      <div className="response-status">
        {sending ? (
          <>
            <Spinner />
            <span className="strong">Sending…</span>
            {progress > 0 && <span className="subtle xs">{formatBytes(progress)} received</span>}
            <span className="spacer" />
            <button className="btn btn-sm" onClick={onCancel}>
              <Ban /> Cancel
            </button>
          </>
        ) : resp ? (
          <>
            <StatusCode status={resp.status} />
            <span className="strong">{resp.statusText}</span>
            <span className="subtle xs" title={`TTFB ${resp.timings.ttfbMs} ms · download ${resp.timings.downloadMs} ms`}>
              <Clock size={12} /> {fmtMs(resp.timings.totalMs)}
            </span>
            <span className="subtle xs">{formatBytes(resp.bodySize)}</span>
            <span className="subtle xs">{resp.httpVersion}</span>
            {resp.redirected && (
              <span className="badge info" title={resp.url}>
                REDIRECTED
              </span>
            )}
            {result!.tests.length > 0 && <span className={clsx('badge', failed ? 'danger' : 'success')}>{failed ? `${failed} FAILED` : 'TESTS PASS'}</span>}
            <span className="spacer" />
            <span className="subtle xs">{new Date(result!.at).toLocaleTimeString()}</span>
            <MockFromResponseButton result={result} compact />
            {resp.status >= 400 || failed > 0 ? (
              <AiAssistButton
                feature="explainFailure"
                compact
                align="right"
                title="Explain this failure — with the mock stub, related events and pod events/logs as cited sources"
                input={(p) => aiApi.failureContext(root, { ...responseContext(result)!, notes: p || null }, null, result!.at)}
              />
            ) : (
              <AiAssistButton feature="explainResponse" compact align="right" title="Explain this response" input={(p) => ({ ...responseContext(result)!, notes: p || null })} />
            )}
          </>
        ) : result?.cancelled ? (
          <span className="muted">Cancelled</span>
        ) : result?.error ? (
          <>
            <span className="text-danger strong">{errorTitle(result.error.kind)}</span>
            <span className="spacer" />
            <AiAssistButton
              feature="explainFailure"
              compact
              align="right"
              title="Explain this error — with related events and pod events/logs when the URL is a port-forward"
              input={(p) =>
                aiApi.failureContext(
                  root,
                  { method: result.sent?.method ?? '', url: result.sent?.url ?? '', requestHeaders: result.sent?.headers, requestBody: result.sent?.body, status: 0, notes: p || null },
                  `${errorTitle(result.error!.kind)}: ${result.error!.message}`,
                  result.at,
                )
              }
            />
          </>
        ) : (
          <span className="subtle">No response yet</span>
        )}
      </div>
      {((result && result.unresolved.length > 0) || (result?.error && !sending)) && (
        <div className="callout-stack">
          {result && result.unresolved.length > 0 && (
            <Callout kind="warning">
              Unresolved: {result.unresolved.map((u) => `{{${u}}}`).join(', ')} — sent literally. Check the active environment.
            </Callout>
          )}
          {result?.error && !sending && (
            <>
              <Callout kind="danger">
                <div className="strong">{errorTitle(result.error.kind)}</div>
                <div className="mono xs break">{result.error.message}</div>
              </Callout>
              <ErrorHints kind={result.error.kind} message={result.error.message} />
            </>
          )}
        </div>
      )}
      <ResponsiveTabs tabs={tabs} value={current} onChange={setSub} label="Response" />
      <div className="response-body">
        {current === 'history' && <HistoryView root={root} path={path} />}
        {current === 'stream' && <StreamView events={stream} />}
        {current === 'console' && <ScriptLogs report={result?.scripts} />}
        {!sending && !result && current !== 'history' && (
          <Empty icon={<Send />} title="No response yet">
            Press <span className="kbd">Ctrl</span> + <span className="kbd">Enter</span> to send. Everything stays on your machine.
          </Empty>
        )}
        {result?.cancelled && current !== 'history' && <Empty icon={<CircleSlash />} title="Request cancelled" />}
        {resp && !sending && (
          <>
            {current === 'body' && <BodyView resp={resp} />}
            {current === 'tests' && <TestsView result={result!} root={root} path={path} />}
            {current === 'headers' && <HeadersView headers={resp.headers} />}
            {current === 'timing' && <TimingView resp={resp} />}
            {current === 'sent' && result!.sent && (
              <div className="p-5 col">
                <div className="mono small break">
                  <span className="strong">{result!.sent.method}</span> {result!.sent.url}
                </div>
                <HeadersView headers={result!.sent.headers} />
                {result!.sent.body && <pre className="code">{result!.sent.body}</pre>}
                <p className="subtle xs">Secret values are masked here, in history and in exports.</p>
              </div>
            )}
            {current === 'inspect' && <DecodedValues items={detections} />}
          </>
        )}
      </div>
    </div>
  );
}

/** `snapshot <file>` rows that failed: their file, so the current body can be accepted. */
function snapshotFile(t: AssertionResult): string | null {
  return !t.passed && t.label.startsWith('snapshot ') ? t.label.slice('snapshot '.length) : null;
}

function TestsView({ result, root, path }: { result: SendResult; root: string; path: string }) {
  const [accepted, setAccepted] = useState<Record<string, boolean>>({});
  const resp = result.response;
  const accept = async (file: string) => {
    if (resp?.bodyText == null) return;
    const tab = useWorkspace.getState().tabs.find((t) => t.kind === 'request' && t.path === path);
    const tests = tab?.kind === 'request' ? tab.draft.tests : undefined;
    const current = tests?.find((t) => t.assert === 'snapshot' && t.file === file);
    const ignore = current?.assert === 'snapshot' ? (current.ignore ?? []) : [];
    try {
      await api.snapshotSave(root, path, resp.bodyText ?? '', resp.contentType, ignore, file);
      if (tab?.kind === 'request' && !current) useWorkspace.getState().updateRequest(tab.id, (d) => ({ ...d, tests: upsertSnapshotTest(d.tests, file, ignore) }));
      setAccepted((a) => ({ ...a, [file]: true }));
      toast.success('Snapshot updated', `${file} now holds this response.`);
    } catch (e) {
      toast.error('Could not update the snapshot', errorMessage(e));
    }
  };
  return (
    <div className="p-5 col-lg">
      {result.tests.length > 0 && (
        <table className="table">
          <tbody>
            {result.tests.map((t, i) => (
              <tr key={i}>
                <td style={{ width: 22 }}>{t.passed ? <CheckCircle2 size={14} className="text-success" /> : <XCircle size={14} className="text-danger" />}</td>
                <td className="mono xs">{t.label}</td>
                <td className="mono xs subtle break">{t.message ?? (t.actual !== null ? `actual: ${t.actual}` : '')}</td>
                <td style={{ width: 1 }}>
                  {snapshotFile(t) && !accepted[snapshotFile(t)!] && resp?.bodyText != null && (
                    <button className="btn btn-sm btn-ghost nowrap" onClick={() => accept(snapshotFile(t)!)} title="Overwrite the snapshot file with this response">
                      <Camera /> Update snapshot
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {result.captures.length > 0 && (
        <section className="section">
          <div className="section-head">
            <h3>Captured into variables</h3>
          </div>
          <dl className="kv mono">
            {result.captures.map((c) => (
              <Fragment key={c.name}>
                <dt>{`{{${c.name}}}`}</dt>
                <dd className={c.error ? 'text-danger' : ''}>{c.error ?? c.value}</dd>
              </Fragment>
            ))}
          </dl>
        </section>
      )}
    </div>
  );
}

function StreamView({ events }: { events: SseEvent[] }) {
  const [filter, setFilter] = useState('');
  const shown = filter ? events.filter((e) => e.event.includes(filter) || e.data.includes(filter)) : events;
  return (
    <div className="col min-h-0 h-full" style={{ gap: 0 }}>
      <div className="toolbar">
        <input className="input sm" style={{ maxWidth: 260 }} placeholder="Filter events" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <span className="spacer" />
        <CopyButton text={() => events.map((e) => e.data).join('\n')} label="Copy data" />
      </div>
      <div className="list stream-list">
        {shown.map((e, i) => (
          <div key={i} className="stream-row">
            <span className="badge accent">{e.event}</span>
            {e.id && <span className="subtle xs mono">#{e.id}</span>}
            <span className="mono xs break grow">{e.data}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function fmtMs(ms: number) {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(2)} s`;
}

function errorTitle(kind?: string) {
  switch (kind) {
    case 'connect':
      return 'Could not connect';
    case 'timeout':
      return 'Timed out';
    case 'invalidUrl':
      return 'Invalid URL';
    case 'invalidRequest':
      return 'Invalid request';
    case 'body':
      return 'Response body failed';
    default:
      return 'Request failed';
  }
}

function ErrorHints({ kind, message }: { kind: string; message: string }) {
  const m = message.toLowerCase();
  const hints: string[] = [];
  if (kind === 'connect' && m.includes('refused')) hints.push('Nothing is listening on that host/port. Is the service (or your mock) running?');
  if (m.includes('dns') || m.includes('resolve')) hints.push('The host name could not be resolved. Check the URL and the active environment’s baseUrl.');
  if (m.includes('certificate')) hints.push('TLS verification failed. For local/self-signed servers, turn off “Verify TLS” in the request’s Settings tab.');
  if (kind === 'timeout') hints.push('Raise the timeout in the request’s Settings tab, or check that the server responds.');
  if (m.includes('{{')) hints.push('The URL still contains {{variables}}. Select an environment that defines them.');
  if (!hints.length) return null;
  return (
    <ul className="muted small" style={{ margin: 0, paddingLeft: 18 }}>
      {hints.map((h) => (
        <li key={h}>{h}</li>
      ))}
    </ul>
  );
}

type FilterLang = 'jq' | 'jsonpath' | 'jmespath';
const FILTER_KEY = 'devall.response.filterLang';
/** Utility op and argument name per filter language (the same engines as the Utilities page). */
const FILTER_OPS: Record<FilterLang, { op: string; arg: string; placeholder: string }> = {
  jq: { op: 'jq', arg: 'filter', placeholder: '.items[] | select(.active) | {id, name}' },
  jsonpath: { op: 'jsonpath', arg: 'path', placeholder: '$.items[?@.active].id' },
  jmespath: { op: 'jmespath', arg: 'expression', placeholder: 'items[?active].{id: id, name: name}' },
};

function loadFilterLang(): FilterLang {
  try {
    const v = localStorage.getItem(FILTER_KEY);
    return v === 'jsonpath' || v === 'jmespath' ? v : 'jq';
  } catch {
    return 'jq';
  }
}

function BodyView({ resp }: { resp: ResponseView }) {
  const [mode, setMode] = useState<'pretty' | 'raw'>('pretty');
  const [filter, setFilter] = useState('');
  const [filterLang, setFilterLang] = useState<FilterLang>(loadFilterLang);
  const lang = languageFor(resp.contentType ?? undefined, resp.bodyText ?? undefined);
  const [pretty, isJson] = useMemo((): [string | null, boolean] => {
    if (resp.bodyText == null) return [null, false];
    if (lang === 'json') {
      try {
        return [JSON.stringify(JSON.parse(resp.bodyText), null, 2), true];
      } catch {
        return [resp.bodyText, false];
      }
    }
    return [resp.bodyText, false];
  }, [resp.bodyText, lang]);
  const spec = FILTER_OPS[filterLang];
  const active = isJson && filter.trim() !== '';
  const filtered = useOpRunner(spec.op, resp.bodyText ?? '', { [spec.arg]: filter }, { enabled: active, delay: 180 });
  const setLang = (l: FilterLang) => {
    setFilterLang(l);
    try {
      localStorage.setItem(FILTER_KEY, l);
    } catch {
      /* storage unavailable */
    }
  };

  if (resp.bodyText == null) {
    const isImage = resp.contentType?.startsWith('image/');
    return (
      <div className="p-5 col">
        {isImage && resp.bodyBase64 && <img className="response-image" src={`data:${resp.contentType};base64,${resp.bodyBase64}`} alt="Response" />}
        <p className="muted small">
          Binary body ({formatBytes(resp.bodySize)}, {resp.contentType ?? 'unknown type'}).
        </p>
      </div>
    );
  }
  if (!resp.bodyText) return <p className="muted small p-5">Empty body.</p>;
  const showFiltered = active && !filtered.error && (filtered.output !== '' || !filtered.running);
  const shown = showFiltered ? filtered.output : mode === 'pretty' ? (pretty ?? '') : resp.bodyText;
  const filteredLang = filtered.output.trimStart().startsWith('{') || filtered.output.trimStart().startsWith('[') ? 'json' : 'plaintext';
  return (
    <div className="col min-h-0 h-full" style={{ gap: 0 }}>
      <div className="toolbar">
        <Segmented
          value={mode}
          onChange={setMode}
          options={[
            { value: 'pretty', label: 'Pretty' },
            { value: 'raw', label: 'Raw' },
          ]}
        />
        {resp.bodyTruncated && <span className="badge warning">TRUNCATED</span>}
        <span className="spacer" />
        <button
          className="btn btn-sm btn-ghost"
          title="Open this body in the Utilities page (with the current filter)"
          onClick={() => (active ? openInUtilities(spec.op, resp.bodyText ?? '', { [spec.arg]: filter }) : openInUtilities(isJson ? 'jq' : 'text.stats', resp.bodyText ?? ''))}
        >
          <Wrench /> Open in Utilities
        </button>
        <CopyButton text={shown} label={showFiltered ? 'Copy result' : 'Copy'} />
      </div>
      {isJson && (
        <div className="toolbar response-filter">
          <Filter size={13} className="subtle" />
          <select className="select sm" value={filterLang} onChange={(e) => setLang(e.target.value as FilterLang)} aria-label="Filter language">
            <option value="jq">jq</option>
            <option value="jsonpath">JSONPath</option>
            <option value="jmespath">JMESPath</option>
          </select>
          <input
            className="input sm mono grow"
            value={filter}
            placeholder={`Filter the body — ${spec.placeholder}`}
            spellCheck={false}
            onChange={(e) => setFilter(e.target.value)}
            onKeyDown={(e) => e.key === 'Escape' && setFilter('')}
            aria-label="Filter expression"
          />
          {filterLang === 'jq' && (
            <AiAssistButton feature="nlJq" compact align="right" title="Write a jq filter from a description" input={(d) => ({ description: d, sampleJson: resp.bodyText ?? '' })} onResult={(r) => setFilter(r.filter)} applyLabel="Use filter" />
          )}
          {filterLang === 'jsonpath' && (
            <AiAssistButton feature="nlJsonPath" compact align="right" title="Write a JSONPath from a description" input={(d) => ({ description: d, sampleJson: resp.bodyText ?? '' })} onResult={(r) => setFilter(r.path)} applyLabel="Use path" />
          )}
          {active && filtered.running && <Spinner />}
          {active && filtered.error && (
            <span className="text-danger xs truncate" style={{ maxWidth: '40%' }} title={filtered.error}>
              {filtered.error}
            </span>
          )}
        </div>
      )}
      <div className="grow min-h-0">
        <CodeEditor value={shown} language={showFiltered ? filteredLang : mode === 'pretty' ? lang : 'plaintext'} readOnly />
      </div>
    </div>
  );
}

function HeadersView({ headers }: { headers: [string, string][] }) {
  return (
    <table className="table">
      <tbody>
        {headers.map(([k, v], i) => (
          <tr key={i}>
            <td className="mono xs strong nowrap" style={{ width: '30%' }}>
              {k}
            </td>
            <td className="mono xs break">{v}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function TimingView({ resp }: { resp: ResponseView }) {
  const t = resp.timings;
  const total = Math.max(t.totalMs, 0.01);
  const rows: [string, number, string][] = [
    ['Waiting (connect + TLS + server)', t.ttfbMs, 'var(--chart-3xx)'],
    ['Content download', t.downloadMs, 'var(--chart-2xx)'],
  ];
  return (
    <div className="p-5 col-lg" style={{ maxWidth: 640 }}>
      {rows.map(([label, ms, color]) => (
        <div key={label} className="col-sm">
          <div className="row between small">
            <span>{label}</span>
            <span className="mono">{fmtMs(ms)}</span>
          </div>
          <div className="progress">
            <div style={{ width: `${Math.max(1, (ms / total) * 100)}%`, background: color }} />
          </div>
        </div>
      ))}
      <dl className="kv">
        <dt>Total</dt>
        <dd className="mono">{fmtMs(t.totalMs)}</dd>
        <dt>Final URL</dt>
        <dd className="mono xs break">{resp.url}</dd>
        {resp.remoteAddr && (
          <>
            <dt>Remote address</dt>
            <dd className="mono xs">{resp.remoteAddr}</dd>
          </>
        )}
        <dt>Protocol</dt>
        <dd className="mono xs">{resp.httpVersion}</dd>
      </dl>
      <p className="subtle xs">DNS / connect / TLS breakdown arrives with the hyper-level engine in M1.</p>
    </div>
  );
}

import { useState } from 'react';
import { Disc, Square, Camera, Info, Trash2, FolderInput, HardDrive, ExternalLink } from 'lucide-react';
import type { RecordSpec, StubMapping } from '@/features/mocks/lib/types';
import { useRecordingStatus, useWm, useWrite, qk } from '@/features/mocks/lib/queries';
import { MethodBadge, StatusCode, Field, Checkbox, Callout, Spinner } from '@/features/mocks/components/ui';
import { TagsInput } from '@/features/mocks/stubs/form/PatternEditors';
import { displayName, stubId, urlOf, withFolder } from '@/features/mocks/lib/stub-utils';
import { openStub } from '@/features/mocks/stubs/open';
import { useStubActions } from '@/features/mocks/stubs/actions';
import { promptDialog } from '@/components/ui/dialogs';

interface FormState {
  targetBaseUrl: string;
  urlPattern: string;
  method: string;
  allowNonProxied: boolean;
  captureHeaders: string[];
  bodyMatcher: 'auto' | 'equalToJson' | 'equalTo' | 'equalToXml';
  ignoreArrayOrder: boolean;
  ignoreExtraElements: boolean;
  textThreshold: string;
  binaryThreshold: string;
  persist: boolean;
  repeatsAsScenarios: boolean;
  transformers: string[];
}

const DEFAULTS: FormState = {
  targetBaseUrl: 'https://',
  urlPattern: '',
  method: 'ANY',
  allowNonProxied: false,
  captureHeaders: [],
  bodyMatcher: 'auto',
  ignoreArrayOrder: true,
  ignoreExtraElements: true,
  textThreshold: '',
  binaryThreshold: '',
  persist: false,
  repeatsAsScenarios: true,
  transformers: [],
};

function load(): FormState {
  try {
    return { ...DEFAULTS, ...JSON.parse(sessionStorage.getItem('wmui.record') ?? '{}') };
  } catch {
    return DEFAULTS;
  }
}

function toSpec(f: FormState, forSnapshot: boolean): RecordSpec {
  const spec: RecordSpec = { outputFormat: 'FULL', persist: f.persist, repeatsAsScenarios: f.repeatsAsScenarios };
  if (!forSnapshot) spec.targetBaseUrl = f.targetBaseUrl.replace(/\/$/, '');
  const filters: RecordSpec['filters'] = {};
  if (f.urlPattern) filters.urlPathPattern = f.urlPattern;
  if (f.method !== 'ANY') filters.method = f.method;
  if (forSnapshot && f.allowNonProxied) filters.allowNonProxied = true;
  if (Object.keys(filters).length) spec.filters = filters;
  if (f.captureHeaders.length) spec.captureHeaders = Object.fromEntries(f.captureHeaders.map((h) => [h, { caseInsensitive: true }]));
  spec.requestBodyPattern =
    f.bodyMatcher === 'equalToJson'
      ? { matcher: 'equalToJson', ignoreArrayOrder: f.ignoreArrayOrder, ignoreExtraElements: f.ignoreExtraElements }
      : f.bodyMatcher === 'auto'
        ? { matcher: 'auto', ignoreArrayOrder: f.ignoreArrayOrder, ignoreExtraElements: f.ignoreExtraElements }
        : { matcher: f.bodyMatcher };
  if (f.textThreshold || f.binaryThreshold)
    spec.extractBodyCriteria = { ...(f.textThreshold ? { textSizeThreshold: f.textThreshold } : {}), ...(f.binaryThreshold ? { binarySizeThreshold: f.binaryThreshold } : {}) };
  if (f.transformers.length) spec.transformers = f.transformers;
  return spec;
}

export default function RecordingPage() {
  const status = useRecordingStatus();
  const { server } = useWm();
  const write = useWrite();
  const actions = useStubActions();
  const [f, setF] = useState<FormState>(load);
  const [result, setResult] = useState<StubMapping[] | null>(null);
  const [keep, setKeep] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const recording = status.data?.status === 'Recording';
  const upd = (patch: Partial<FormState>) => {
    const n = { ...f, ...patch };
    setF(n);
    sessionStorage.setItem('wmui.record', JSON.stringify(n));
  };
  const inv = (s: string) => [qk.recording(s), qk.mappings(s), qk.scenarios(s)];
  const showResult = (mappings: StubMapping[]) => {
    setResult(mappings);
    setKeep(new Set(mappings.map(stubId)));
  };

  const start = async () => {
    setBusy(true);
    try {
      await write((c) => c.startRecording(toSpec(f, false)), { label: 'Start recording', success: `Recording — send traffic to ${server?.url}`, invalidate: inv });
    } finally {
      setBusy(false);
    }
  };
  const stop = async () => {
    setBusy(true);
    try {
      const r = await write((c) => c.stopRecording(), { label: 'Stop recording', success: (x) => `Captured ${(x as { mappings?: unknown[] })?.mappings?.length ?? 0} stubs`, invalidate: inv });
      if (r?.mappings) showResult(r.mappings);
    } finally {
      setBusy(false);
    }
  };
  const snapshot = async () => {
    setBusy(true);
    try {
      const r = await write((c) => c.snapshot(toSpec(f, true)), { label: 'Snapshot', success: (x) => `Snapshot created ${(x as { mappings?: unknown[] })?.mappings?.length ?? 0} stubs`, invalidate: inv });
      if (r?.mappings) showResult(r.mappings);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page">
      <div className="page-header">
        <Disc size={16} className={recording ? 'text-danger' : 'subtle'} />
        <h1>Record & snapshot</h1>
        <span className={`badge ${recording ? 'danger' : 'outline'}`}>{recording && <span className="dot err pulse" />} {status.data?.status ?? '…'}</span>
        <div className="spacer" />
        {recording ? (
          <button className="btn btn-danger" onClick={stop} disabled={busy}>
            {busy ? <Spinner /> : <Square />} Stop & review
          </button>
        ) : (
          <button className="btn btn-primary" onClick={start} disabled={busy || !/^https?:\/\/.+/.test(f.targetBaseUrl)}>
            {busy ? <Spinner /> : <Disc />} Start recording
          </button>
        )}
      </div>
      <div className="page-body">
        <div className="col-lg" style={{ maxWidth: 1100 }}>
          {result && (
            <div className="panel">
              <div className="panel-header">
                <h3 className="grow">Review captured stubs ({result.length})</h3>
                <span className="subtle xs">They are already live on the server. Discard what you don't want.</span>
              </div>
              <div className="panel-body col">
                <table className="table">
                  <tbody>
                    {result.map((s) => (
                      <tr key={stubId(s)}>
                        <td style={{ width: 28 }}>
                          <Checkbox checked={keep.has(stubId(s))} onChange={(v) => {
                            const n = new Set(keep);
                            if (v) n.add(stubId(s));
                            else n.delete(stubId(s));
                            setKeep(n);
                          }} />
                        </td>
                        <td style={{ width: 60 }}>
                          <MethodBadge method={s.request.method} />
                        </td>
                        <td className="mono xs truncate">{urlOf(s.request)}</td>
                        <td style={{ width: 50 }}>
                          <StatusCode status={s.response.status} />
                        </td>
                        <td className="xs subtle" style={{ width: 160 }}>
                          {s.scenarioName && `scenario: ${s.scenarioName}`}
                          {s.response.bodyFileName && `file: ${s.response.bodyFileName}`}
                        </td>
                        <td style={{ width: 40 }}>
                          <button className="icon-btn sm" onClick={() => openStub(s)} title="Open">
                            <ExternalLink />
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <div className="row wrap">
                  <button className="btn btn-sm btn-danger-ghost" disabled={result.length === keep.size} onClick={async () => {
                    await actions.remove(result.filter((s) => !keep.has(stubId(s))));
                    setResult(result.filter((s) => keep.has(stubId(s))));
                  }}>
                    <Trash2 /> Discard {result.length - keep.size} unchecked
                  </button>
                  <button className="btn btn-sm" onClick={async () => {
                    const folder = await promptDialog({ title: 'Put recorded stubs in folder', initial: `recorded/${new Date().toISOString().slice(0, 10)}` });
                    if (folder === null) return;
                    await actions.updateMany(result.filter((s) => keep.has(stubId(s))).map((s) => withFolder(s, folder)), 'Moved');
                  }}>
                    <FolderInput /> Move kept to folder…
                  </button>
                  <button className="btn btn-sm" onClick={() => actions.bulk.persistent(result.filter((s) => keep.has(stubId(s))), true)}>
                    <HardDrive /> Persist kept to disk
                  </button>
                  <span className="spacer" />
                  <button className="btn btn-sm btn-ghost" onClick={() => setResult(null)}>
                    Done
                  </button>
                </div>
              </div>
            </div>
          )}

          <Callout>
            <p className="strong">How recording works</p>
            <ol className="small" style={{ margin: 0, paddingLeft: 18 }}>
              <li>
                WireMock adds a temporary proxy stub that forwards everything to the <b>target URL</b>.
              </li>
              <li>
                Point your app (or tests) at <code className="code-inline">{server?.url}</code> instead of the real API and exercise it.
              </li>
              <li>On stop, each distinct request/response becomes a stub. Repeated requests with different responses become scenario steps.</li>
            </ol>
          </Callout>

          <div className="grid-2">
            <div className="panel">
              <div className="panel-header">
                <h3>Target & filters</h3>
              </div>
              <div className="panel-body col">
                <Field label="Target base URL (recording only)" hint="Real API to forward to, e.g. https://api.github.com">
                  <input className="input mono" value={f.targetBaseUrl} onChange={(e) => upd({ targetBaseUrl: e.target.value })} disabled={recording} />
                </Field>
                <div className="grid-2" style={{ gridTemplateColumns: '1fr 110px' }}>
                  <Field label="Only paths matching (regex)">
                    <input className="input mono" placeholder="/api/.*" value={f.urlPattern} onChange={(e) => upd({ urlPattern: e.target.value })} />
                  </Field>
                  <Field label="Method">
                    <select className="select" value={f.method} onChange={(e) => upd({ method: e.target.value })}>
                      {['ANY', 'GET', 'POST', 'PUT', 'PATCH', 'DELETE'].map((m) => (
                        <option key={m}>{m}</option>
                      ))}
                    </select>
                  </Field>
                </div>
                <Field label="Request headers to match on" hint="Recorded stubs will require these headers (case-insensitive).">
                  <TagsInput value={f.captureHeaders} onChange={(captureHeaders) => upd({ captureHeaders })} placeholder="Accept, Authorization…" />
                </Field>
              </div>
            </div>
            <div className="panel">
              <div className="panel-header">
                <h3>Output</h3>
              </div>
              <div className="panel-body col">
                <Field label="Request body matching">
                  <select className="select" value={f.bodyMatcher} onChange={(e) => upd({ bodyMatcher: e.target.value as FormState['bodyMatcher'] })}>
                    <option value="auto">Auto (JSON / XML / text by content type)</option>
                    <option value="equalToJson">equalToJson</option>
                    <option value="equalToXml">equalToXml</option>
                    <option value="equalTo">equalTo (exact text)</option>
                  </select>
                </Field>
                {(f.bodyMatcher === 'auto' || f.bodyMatcher === 'equalToJson') && (
                  <div className="row">
                    <Checkbox checked={f.ignoreArrayOrder} onChange={(ignoreArrayOrder) => upd({ ignoreArrayOrder })} label="ignore array order" />
                    <Checkbox checked={f.ignoreExtraElements} onChange={(ignoreExtraElements) => upd({ ignoreExtraElements })} label="ignore extra fields" />
                  </div>
                )}
                <div className="grid-2">
                  <Field label="Extract text bodies over" hint='to __files, e.g. "10 kb"'>
                    <input className="input" placeholder="never" value={f.textThreshold} onChange={(e) => upd({ textThreshold: e.target.value })} />
                  </Field>
                  <Field label="Extract binary bodies over" hint='e.g. "1 kb"'>
                    <input className="input" placeholder="never" value={f.binaryThreshold} onChange={(e) => upd({ binaryThreshold: e.target.value })} />
                  </Field>
                </div>
                <Checkbox checked={f.repeatsAsScenarios} onChange={(repeatsAsScenarios) => upd({ repeatsAsScenarios })} label="Record repeated requests as scenario steps" />
                <Checkbox checked={f.persist} onChange={(persist) => upd({ persist })} label="Persist recorded stubs to the server's mappings folder" />
                <Field label="Recording transformers (extensions)">
                  <TagsInput value={f.transformers} onChange={(transformers) => upd({ transformers })} placeholder="transformer name" />
                </Field>
              </div>
            </div>
          </div>

          <div className="panel">
            <div className="panel-header">
              <Camera size={15} className="subtle" />
              <h3 className="grow">Snapshot from the journal</h3>
              <button className="btn btn-sm" onClick={snapshot} disabled={busy}>
                <Camera /> Take snapshot
              </button>
            </div>
            <div className="panel-body col">
              <p className="muted small">
                Turns requests already in the journal into stubs, using the filters and output options above (target URL is ignored). By default only requests that were proxied are included.
              </p>
              <Checkbox checked={f.allowNonProxied} onChange={(allowNonProxied) => upd({ allowNonProxied })} label="Include requests that were not proxied (e.g. answered by existing stubs)" />
              <div className="row-sm subtle xs">
                <Info size={13} /> Tip: no proxy possible? Export a HAR file from your browser's DevTools and import it on the Import page.
              </div>
            </div>
          </div>
          {result === null && status.data?.status === 'Stopped' && <div className="subtle small">Last recording stopped. Recorded stubs are in the Stubs list (sort by match order).</div>}
          {result && result.length > 0 && <div className="subtle xs">Stub names default to the request; rename them later. Kept {keep.size}: {result.filter((s) => keep.has(stubId(s))).slice(0, 3).map(displayName).join(', ')}…</div>}
        </div>
      </div>
    </div>
  );
}

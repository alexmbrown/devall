import { useState } from 'react';
import { useQuery } from '@/features/mocks/lib/query';
import { FilePlus2, Code2, Trash2, ExternalLink, Wrench, CheckCircle2, XCircle } from 'lucide-react';
import type { ServeEvent } from '@/features/mocks/lib/types';
import { useWm, useWrite, useMappings, useScenarios, qk } from '@/features/mocks/lib/queries';
import { displayName, headerValue, stubId } from '@/features/mocks/lib/stub-utils';
import { curlForLogged } from '@/features/mocks/lib/codegen';
import { explainNearMiss, matchRequest } from '@/features/mocks/lib/match';
import { suggestFix } from '@/features/mocks/lib/fix';
import { MethodBadge, StatusCode, copyText, Spinner, formatTime, Callout } from '@/features/mocks/components/ui';
import { MatchReportView } from '@/features/mocks/components/MatchReportView';
import { BodyView, HeadersTable } from './BodyView';
import { CreateStubDialog } from './CreateStubDialog';
import { openNewStub, openStub } from '@/features/mocks/stubs/open';
import { useStubTabs } from '@/features/mocks/store/tabs';
import { toast } from '@/components/ui/toast';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { DecodedLabel, DecodedValues, messageText, useDetections } from '@/components/DecodedValues';

type Tab = 'request' | 'response' | 'match' | 'raw' | 'decoded';

const headerPairs = (h?: Record<string, string | string[]>): [string, string][] => Object.entries(h ?? {}).map(([k, v]) => [k, Array.isArray(v) ? v.join(', ') : v]);

export function RequestDetail({ event, onDeleted }: { event: ServeEvent; onDeleted?(): void }) {
  const [tab, setTab] = useState<Tab>(event.wasMatched ? 'request' : 'match');
  const [creating, setCreating] = useState(false);
  const { server, sid } = useWm();
  const write = useWrite();
  const req = event.request;
  const res = event.response;
  // What the built-in engine's chaos settings did to this request.
  const chaos = event.devallChaos as { latencyMs?: number; errorStatus?: number; fault?: string } | undefined;
  // Unmatched events carry WireMock's placeholder "ANY" stub; ignore it.
  const matched = event.wasMatched ? event.stubMapping : undefined;
  // JWTs and timestamps in the logged exchange (bearer tokens, exp claims…), decoded inline.
  const detections = useDetections([req.absoluteUrl ?? req.url, messageText(headerPairs(req.headers), req.body), res ? messageText(headerPairs(res.headers), res.body) : ''].join('\n'));
  const tabs: Tab[] = ['request', 'response', 'match', 'raw', ...(detections.length ? (['decoded'] as Tab[]) : [])];

  return (
    <div className="page">
      <div className="toolbar">
        <MethodBadge method={req.method} />
        <span className="mono small truncate grow" title={req.absoluteUrl ?? req.url}>
          {req.url}
        </span>
        <StatusCode status={res?.status} />
      </div>
      <div className="toolbar" style={{ gap: 4 }}>
        <button className="btn btn-sm btn-primary" onClick={() => setCreating(true)}>
          <FilePlus2 /> Create stub
        </button>
        <button className="btn btn-sm" onClick={() => copyText(curlForLogged(req, server?.url), 'cURL copied')}>
          <Code2 /> cURL
        </button>
        <span className="spacer" />
        <button
          className="icon-btn sm danger"
          title="Remove from journal"
          onClick={async () => {
            await write((c) => c.deleteRequest(event.id), { label: 'Delete request', invalidate: (s) => [qk.requests(s), qk.unmatchedNearMisses(s)] });
            onDeleted?.();
          }}
        >
          <Trash2 />
        </button>
      </div>
      <div className="subtabs">
        {tabs.map((t) => (
          <button key={t} className={`subtab ${tab === t ? 'on' : ''}`} onClick={() => setTab(t)}>
            {t === 'decoded' ? (
              <DecodedLabel count={detections.length} />
            ) : t === 'match' ? (
              <>
                {event.wasMatched ? <CheckCircle2 size={13} className="text-success" /> : <XCircle size={13} className="text-warning" />}
                {event.wasMatched ? 'Matched stub' : 'Why no match?'}
              </>
            ) : (
              t[0].toUpperCase() + t.slice(1)
            )}
          </button>
        ))}
      </div>
      <div className="scroll p-5 grow">
        {tab === 'request' && (
          <div className="col-lg">
            <dl className="kv">
              <dt>Received</dt>
              <dd>
                {req.loggedDate ? new Date(req.loggedDate).toLocaleString() : ''} <span className="subtle mono xs">{formatTime(req.loggedDate)}</span>
              </dd>
              <dt>URL</dt>
              <dd className="mono xs break">{req.absoluteUrl ?? req.url}</dd>
              <dt>Client</dt>
              <dd className="mono xs">
                {req.clientIp} {req.protocol && <span className="subtle">· {req.protocol}</span>} {req.browserProxyRequest && <span className="badge info">browser proxy</span>}
              </dd>
              {event.timing && (
                <>
                  <dt>Timing</dt>
                  <dd className="xs">
                    total {event.timing.totalTime ?? '?'} ms · added delay {event.timing.addedDelay ?? 0} ms · process {event.timing.processTime ?? '?'} ms
                  </dd>
                </>
              )}
              {chaos && (
                <>
                  <dt>Chaos</dt>
                  <dd className="xs">
                    <span className="badge warning">chaos</span> +{chaos.latencyMs ?? 0} ms
                    {chaos.errorStatus ? ` · forced ${chaos.errorStatus}` : ''}
                    {chaos.fault ? ` · ${chaos.fault}` : ''}
                  </dd>
                </>
              )}
            </dl>
            {req.queryParams && Object.keys(req.queryParams).length > 0 && (
              <div className="section">
                <span className="label">Query</span>
                <HeadersTable headers={Object.fromEntries(Object.entries(req.queryParams).map(([k, v]) => [k, v.values]))} />
              </div>
            )}
            <div className="section">
              <span className="label">Headers</span>
              <HeadersTable headers={req.headers} />
            </div>
            <div className="section">
              <span className="label">Body</span>
              <BodyView body={req.body} base64={req.bodyAsBase64} contentType={headerValue(req.headers, 'content-type')} />
            </div>
          </div>
        )}
        {tab === 'response' && (
          <div className="col-lg">
            {res ? (
              <>
                <dl className="kv">
                  <dt>Status</dt>
                  <dd>
                    <StatusCode status={res.status} />
                  </dd>
                  {res.fault && (
                    <>
                      <dt>Fault</dt>
                      <dd>
                        <span className="badge danger">{res.fault}</span>
                      </dd>
                    </>
                  )}
                </dl>
                <div className="section">
                  <span className="label">Headers</span>
                  <HeadersTable headers={res.headers} />
                </div>
                <div className="section">
                  <span className="label">Body</span>
                  <BodyView body={res.body} base64={res.bodyAsBase64} contentType={headerValue(res.headers, 'content-type')} />
                </div>
              </>
            ) : (
              <div className="subtle">No response recorded (the journal may be configured to omit it).</div>
            )}
          </div>
        )}
        {tab === 'match' && (matched ? <MatchedInfo event={event} /> : <WhyNoMatch event={event} sid={sid} />)}
        {tab === 'decoded' && <DecodedValues items={detections} className="col-lg" empty />}
        {tab === 'raw' && (
          <div className="editor-shell" style={{ height: '100%', minHeight: 400 }}>
            <CodeEditor value={JSON.stringify(event, null, 2)} readOnly language="json" />
          </div>
        )}
      </div>
      {creating && <CreateStubDialog request={req} response={res} onClose={() => setCreating(false)} />}
    </div>
  );
}

function MatchedInfo({ event }: { event: ServeEvent }) {
  const s = event.stubMapping!;
  const mappings = useMappings();
  const live = mappings.data?.find((m) => stubId(m) === stubId(s));
  return (
    <div className="col">
      <Callout kind="success">
        Served by <b>{displayName(s)}</b> <span className="mono xs subtle">{stubId(s).slice(0, 8)}</span>
        {s.response?.proxyBaseUrl && <> — proxied to <span className="mono">{s.response.proxyBaseUrl}</span></>}
      </Callout>
      <div className="row">
        {live ? (
          <button className="btn btn-sm" onClick={() => openStub(live)}>
            <ExternalLink /> Open stub
          </button>
        ) : (
          <span className="subtle small">That stub no longer exists on the server.</span>
        )}
      </div>
      <MatchReportView report={matchRequest(s.request, event.request)} />
    </div>
  );
}

function WhyNoMatch({ event, sid }: { event: ServeEvent; sid: string }) {
  const { client } = useWm();
  const nm = useQuery({ queryKey: ['wm', sid, 'nm-req', event.id], queryFn: () => client!.nearMissesForRequest(event.request), enabled: !!client });
  const mappings = useMappings();
  const scenarios = useScenarios();
  if (nm.isPending) return <Spinner />;
  if (nm.isError) return <div className="text-danger">{(nm.error as Error).message}</div>;
  const list = nm.data?.nearMisses ?? [];
  if (!list.length) return <div className="subtle">No stubs are close to this request. Create one from it with “Create stub”.</div>;
  return (
    <div className="col-lg">
      <div className="subtle small">The closest stubs, best first. Rows in red are what stopped each stub from matching.</div>
      {list.slice(0, 5).map((n, i) => {
        const stub = n.stubMapping ?? (n.requestPattern ? { request: n.requestPattern, response: {} } : undefined);
        if (!stub) return null;
        const live = n.stubMapping ? mappings.data?.find((m) => stubId(m) === stubId(n.stubMapping!)) : undefined;
        const report = explainNearMiss(stub, event.request, n.matchResult, scenarios.data?.find((x) => x.name === stub.scenarioName)?.state);
        const fix = live ? suggestFix(live, event.request, matchRequest(live.request, event.request)) : undefined;
        return (
          <div key={i} className="col-sm">
            <div className="row">
              <span className="badge outline">#{i + 1}</span>
              <MethodBadge method={stub.request.method} />
              <span className="strong truncate grow">{n.stubMapping ? displayName(n.stubMapping) : 'Request pattern'}</span>
              <span className="subtle xs">distance {n.matchResult.distance.toFixed(3)}</span>
              {live && (
                <button className="btn btn-sm" onClick={() => openStub(live)}>
                  Open
                </button>
              )}
              {fix && (
                <button
                  className="btn btn-sm"
                  title={fix.summary}
                  onClick={() => {
                    openStub(live!);
                    const t = useStubTabs.getState().findByStub(sid, stubId(live!));
                    if (t) useStubTabs.getState().update(sid, t.key, { draft: fix.stub });
                    toast.info('Fix applied to editor — review and save', fix.summary);
                  }}
                >
                  <Wrench /> Fix stub
                </button>
              )}
            </div>
            <MatchReportView report={report} />
          </div>
        );
      })}
      <div>
        <button className="btn btn-sm" onClick={() => openNewStub({ request: { method: event.request.method, urlPath: event.request.url.split('?')[0] }, response: { status: 200 } })}>
          <FilePlus2 /> Blank stub for this path
        </button>
      </div>
    </div>
  );
}

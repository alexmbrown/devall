import { useState } from 'react';
import { ListChecks, Play, Trash2, CheckCircle2, XCircle, Code2, Search, Tags } from 'lucide-react';
import type { LoggedRequest, RequestPattern, StubMapping } from '@/features/mocks/lib/types';
import { useWm, useWrite, qk } from '@/features/mocks/lib/queries';
import { URL_KEYS, URL_KEY_LABEL, urlKeyOf, displayName, stubId, urlOf } from '@/features/mocks/lib/stub-utils';
import { Field, Callout, MethodBadge, Spinner, copyText, formatTime } from '@/features/mocks/components/ui';
import { PatternMapEditor, BodyPatternsEditor, MethodSelect } from '@/features/mocks/stubs/form/PatternEditors';
import { javaDsl } from '@/features/mocks/lib/codegen';
import { openStub } from '@/features/mocks/stubs/open';
import { toast } from '@/components/ui/toast';

type Cmp = 'exactly' | 'atLeast' | 'atMost' | 'none';

export default function VerifyPage() {
  const { client } = useWm();
  const write = useWrite();
  const [p, setP] = useState<RequestPattern>({ method: 'ANY', urlPathPattern: '/api/.*' });
  const [cmp, setCmp] = useState<Cmp>('atLeast');
  const [n, setN] = useState(1);
  const [count, setCount] = useState<number | null>(null);
  const [found, setFound] = useState<LoggedRequest[] | null>(null);
  const [busy, setBusy] = useState(false);
  const key = urlKeyOf(p) ?? 'urlPathPattern';

  const pass = count === null ? null : cmp === 'exactly' ? count === n : cmp === 'atLeast' ? count >= n : cmp === 'atMost' ? count <= n : count === 0;

  const run = async () => {
    if (!client) return;
    setBusy(true);
    try {
      const [c, f] = await Promise.all([client.countRequests(p), client.findRequests(p)]);
      setCount(c.count);
      setFound(f.requests);
    } catch (e) {
      toast.error('Verification failed', e);
    } finally {
      setBusy(false);
    }
  };

  const javaVerify = () => {
    const dsl = javaDsl({ request: p, response: {} })
      .replace(/^\/\/.*\n/, '')
      .replace(/^stubFor\(/, '')
      .replace(/\n\s+\.willReturn\([\s\S]*$/, '');
    const fn = cmp === 'exactly' ? `exactly(${n})` : cmp === 'atLeast' ? `moreThanOrExactly(${n})` : cmp === 'atMost' ? `lessThanOrExactly(${n})` : 'exactly(0)';
    return `verify(${fn}, ${dsl.replace(/^(\w+)\(/, (_, m) => `${m === 'any' ? 'anyRequestedFor' : `${m}RequestedFor`}(`)});`;
  };

  return (
    <div className="page">
      <div className="page-header">
        <ListChecks size={16} className="subtle" />
        <h1>Verify</h1>
        <span className="subtle small">Assert how often requests were received — the interactive version of WireMock's <code className="code-inline">verify()</code>.</span>
      </div>
      <div className="page-body">
        <div className="col-lg" style={{ maxWidth: 1080 }}>
          <div className="panel">
            <div className="panel-header">
              <h3 className="grow">Request pattern</h3>
            </div>
            <div className="panel-body col">
              <div className="row items-start">
                <Field label="Method" className="shrink-0">
                  <MethodSelect value={p.method ?? 'ANY'} onChange={(method) => setP({ ...p, method })} />
                </Field>
                <Field label="URL match" className="shrink-0">
                  <select
                    className="select"
                    style={{ width: 150 }}
                    value={key}
                    onChange={(e) => {
                      const v = p[key];
                      const next: RequestPattern = { ...p };
                      for (const k of URL_KEYS) delete next[k];
                      next[e.target.value as (typeof URL_KEYS)[number]] = v as string;
                      setP(next);
                    }}
                  >
                    {URL_KEYS.map((k) => (
                      <option key={k} value={k}>
                        {URL_KEY_LABEL[k]}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="URL" className="grow">
                  <input className="input mono" value={String(p[key] ?? '')} onChange={(e) => setP({ ...p, [key]: e.target.value })} />
                </Field>
              </div>
              <Field label="Query parameters">
                <PatternMapEditor value={p.queryParameters} onChange={(queryParameters) => setP({ ...p, queryParameters })} addLabel="Add parameter" />
              </Field>
              <Field label="Headers">
                <PatternMapEditor value={p.headers} onChange={(headers) => setP({ ...p, headers })} addLabel="Add header" />
              </Field>
              <Field label="Body">
                <BodyPatternsEditor value={p.bodyPatterns} onChange={(bodyPatterns) => setP({ ...p, bodyPatterns })} />
              </Field>
              <div className="row wrap">
                <span className="small">Expect</span>
                <select className="select sm" style={{ width: 130 }} value={cmp} onChange={(e) => setCmp(e.target.value as Cmp)}>
                  <option value="exactly">exactly</option>
                  <option value="atLeast">at least</option>
                  <option value="atMost">at most</option>
                  <option value="none">no requests</option>
                </select>
                {cmp !== 'none' && <input className="input sm" type="number" min={0} style={{ width: 80 }} value={n} onChange={(e) => setN(Number(e.target.value))} />}
                <span className="small">matching requests</span>
                <span className="spacer" />
                <button className="btn btn-sm" onClick={() => copyText(javaVerify(), 'Java verify() copied')}>
                  <Code2 /> Copy as Java
                </button>
                <button
                  className="btn btn-sm btn-danger-ghost"
                  onClick={() =>
                    write((c) => c.removeRequests(p), {
                      label: 'Remove matching requests',
                      success: 'Matching requests removed from the journal',
                      invalidate: (s) => [qk.requests(s)],
                      confirm: { title: 'Remove matching requests from the journal?', message: 'Handy to isolate one test run from another.', confirmText: 'Remove' },
                    }).then(run)
                  }
                >
                  <Trash2 /> Remove matching
                </button>
                <button className="btn btn-sm btn-primary" onClick={run} disabled={busy}>
                  {busy ? <Spinner /> : <Play />} Verify
                </button>
              </div>
            </div>
          </div>

          {count !== null && (
            <Callout kind={pass ? 'success' : 'danger'}>
              <div className="row">
                {pass ? <CheckCircle2 size={16} /> : <XCircle size={16} />}
                <span className="strong">
                  {pass ? 'Passed' : 'Failed'} — {count} matching request{count === 1 ? '' : 's'} in the journal
                </span>
              </div>
            </Callout>
          )}
          {found && found.length > 0 && (
            <table className="table">
              <tbody>
                {found.slice(0, 200).map((r, i) => (
                  <tr key={i}>
                    <td style={{ width: 100 }} className="mono xs subtle">
                      {formatTime(r.loggedDate)}
                    </td>
                    <td style={{ width: 60 }}>
                      <MethodBadge method={r.method} />
                    </td>
                    <td className="mono xs truncate">{r.url}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          <MetadataSearch />
        </div>
      </div>
    </div>
  );
}

function MetadataSearch() {
  const { client } = useWm();
  const write = useWrite();
  const [expr, setExpr] = useState("$.wmui.tags[?(@ == 'checkout')]");
  const [res, setRes] = useState<StubMapping[] | null>(null);
  const pattern = { matchesJsonPath: expr };
  return (
    <div className="panel">
      <div className="panel-header">
        <Tags size={15} className="subtle" />
        <h3 className="grow">Find stubs by metadata</h3>
        <span className="subtle xs">POST /mappings/find-by-metadata</span>
      </div>
      <div className="panel-body col">
        <div className="row">
          <input className="input mono grow" value={expr} onChange={(e) => setExpr(e.target.value)} placeholder="$.owner" />
          <button
            className="btn btn-sm"
            onClick={async () => {
              try {
                setRes((await client!.findByMetadata(pattern)).mappings);
              } catch (e) {
                toast.error('Search failed', e);
              }
            }}
          >
            <Search /> Find
          </button>
          <button
            className="btn btn-sm btn-danger-ghost"
            onClick={() =>
              write((c) => c.removeByMetadata(pattern), {
                label: 'Remove by metadata',
                success: 'Matching stubs removed',
                invalidate: (s) => [qk.mappings(s)],
                confirm: { title: 'Delete every stub whose metadata matches?', message: <code className="code-inline">{expr}</code>, danger: true, confirmText: 'Delete' },
              }).then(() => setRes(null))
            }
          >
            <Trash2 /> Delete matching
          </button>
        </div>
        <div className="subtle xs">
          Examples: <code className="code-inline">$['wiremock-gui'].folder</code> · <code className="code-inline">{"$.wmui.tags[?(@ == 'smoke')]"}</code> · <code className="code-inline">{"$[?(@.owner == 'payments')]"}</code>. Tag a test run's stubs and delete them together afterwards.
        </div>
        {res && (
          <div className="col-sm">
            <span className="small">{res.length} stub{res.length === 1 ? '' : 's'}</span>
            {res.map((s) => (
              <div key={stubId(s)} className="list-item" onClick={() => openStub(s)}>
                <MethodBadge method={s.request.method} />
                <span className="truncate grow">{displayName(s)}</span>
                <span className="mono xs subtle">{urlOf(s.request)}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

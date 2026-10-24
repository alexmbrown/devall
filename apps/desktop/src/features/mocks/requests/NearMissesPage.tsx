import { useMemo, useState } from 'react';
import { Crosshair, RefreshCw, FilePlus2, Wrench, ExternalLink, Eraser } from 'lucide-react';
import type { LoggedRequest, NearMiss, StubMapping } from '@/features/mocks/lib/types';
import { useUnmatchedNearMisses, useMappings, useScenarios, useWm, useWrite, qk } from '@/features/mocks/lib/queries';
import { displayName, stubId } from '@/features/mocks/lib/stub-utils';
import { explainNearMiss, matchRequest } from '@/features/mocks/lib/match';
import { suggestFix } from '@/features/mocks/lib/fix';
import { MethodBadge, RelTime, Spinner, Empty, SearchInput } from '@/features/mocks/components/ui';
import { MatchReportView } from '@/features/mocks/components/MatchReportView';
import { CreateStubDialog } from './CreateStubDialog';
import { openStub } from '@/features/mocks/stubs/open';
import { useStubTabs } from '@/features/mocks/store/tabs';
import { toast } from '@/components/ui/toast';

interface Group {
  key: string;
  request: LoggedRequest;
  misses: NearMiss[];
}

export default function NearMissesPage() {
  const nm = useUnmatchedNearMisses(true);
  const mappings = useMappings();
  const { sid } = useWm();
  const write = useWrite();
  const [q, setQ] = useState('');
  const [open, setOpen] = useState<string | null>(null);
  const [creating, setCreating] = useState<LoggedRequest | null>(null);

  const groups = useMemo(() => {
    const m = new Map<string, Group>();
    for (const n of nm.data?.nearMisses ?? []) {
      const key = `${n.request.loggedDate}|${n.request.method}|${n.request.url}`;
      const g = m.get(key) ?? { key, request: n.request, misses: [] };
      g.misses.push(n);
      m.set(key, g);
    }
    // Also include unmatched requests with no near miss at all? WireMock only returns those with candidates.
    return [...m.values()]
      .map((g) => ({ ...g, misses: g.misses.sort((a, b) => a.matchResult.distance - b.matchResult.distance) }))
      .filter((g) => !q || `${g.request.method} ${g.request.url}`.toLowerCase().includes(q.toLowerCase()))
      .sort((a, b) => (b.request.loggedDate ?? 0) - (a.request.loggedDate ?? 0));
  }, [nm.data, q]);

  const live = (s?: StubMapping) => (s ? mappings.data?.find((m) => stubId(m) === stubId(s)) : undefined);
  const scenarios = useScenarios();
  const explain = (s: StubMapping, req: LoggedRequest, n?: NearMiss) => explainNearMiss(s, req, n?.matchResult, scenarios.data?.find((x) => x.name === s.scenarioName)?.state);

  const applyFix = (stub: StubMapping, req: LoggedRequest) => {
    const fix = suggestFix(stub, req, matchRequest(stub.request, req));
    if (!fix) return toast.info('Nothing to change automatically', 'Open the stub and adjust it by hand.');
    openStub(stub);
    const t = useStubTabs.getState().findByStub(sid, stubId(stub));
    if (t) useStubTabs.getState().update(sid, t.key, { draft: fix.stub });
    toast.info('Fix applied in the editor — review and save', fix.summary);
  };

  return (
    <div className="page">
      <div className="page-header">
        <Crosshair size={16} className="subtle" />
        <h1>Near misses</h1>
        <span className="subtle small">Unmatched requests and the stubs that came closest — with exactly which fields differed.</span>
        <div className="spacer" />
        <button className="icon-btn" onClick={() => nm.refetch()} title="Refresh">
          {nm.isFetching ? <Spinner /> : <RefreshCw />}
        </button>
        <button
          className="btn btn-sm"
          onClick={() =>
            write((c) => c.clearRequests(), {
              label: 'Clear journal',
              success: 'Journal cleared',
              invalidate: (s) => [qk.requests(s), qk.unmatchedNearMisses(s)],
            })
          }
        >
          <Eraser /> Clear journal
        </button>
      </div>
      <div className="toolbar">
        <SearchInput value={q} onChange={setQ} placeholder="Filter by method or URL" className="grow" inputRef={(el) => el?.setAttribute('data-page-search', '')} />
        <span className="subtle xs">{groups.length} unmatched request{groups.length === 1 ? '' : 's'}</span>
      </div>
      <div className="scroll p-5 grow">
        {nm.isPending && <Spinner />}
        {nm.isSuccess && groups.length === 0 && (
          <Empty icon={<Crosshair />} title="No near misses">
            Every request in the journal matched a stub (or no stub came close). Unmatched requests with no similar stub are listed on the Requests page.
          </Empty>
        )}
        <div className="col" style={{ maxWidth: 1180 }}>
          {groups.map((g) => {
            const best = g.misses[0];
            const bestStub = best.stubMapping;
            const isOpen = open === g.key || (open === null && g === groups[0]);
            const report = bestStub ? explain(bestStub, g.request, best) : undefined;
            const failing = report?.fields.filter((f) => f.verdict === 'nomatch').map((f) => f.field) ?? [];
            return (
              <div key={g.key} className="fieldset">
                <div className="fieldset-head" onClick={() => setOpen(isOpen ? '' : g.key)}>
                  <MethodBadge method={g.request.method} />
                  <span className="mono xs truncate grow">{g.request.url}</span>
                  {failing.length > 0 && <span className="text-danger xs truncate" style={{ maxWidth: 320 }}>differs: {failing.join(', ')}</span>}
                  <span className="subtle xs nowrap">
                    <RelTime ts={g.request.loggedDate} />
                  </span>
                </div>
                {isOpen && (
                  <div className="fieldset-body">
                    {g.misses.slice(0, 3).map((n, i) => {
                      const s = n.stubMapping;
                      if (!s) return null;
                      const l = live(s);
                      return (
                        <div key={i} className="col-sm">
                          <div className="row">
                            <span className="badge outline">#{i + 1}</span>
                            <MethodBadge method={s.request.method} />
                            <span className="strong truncate grow">{displayName(s)}</span>
                            <span className="subtle xs">distance {n.matchResult.distance.toFixed(3)}</span>
                            {l && (
                              <>
                                <button className="btn btn-sm" onClick={() => openStub(l)}>
                                  <ExternalLink /> Open
                                </button>
                                <button className="btn btn-sm" onClick={() => applyFix(l, g.request)}>
                                  <Wrench /> Fix stub to match
                                </button>
                              </>
                            )}
                          </div>
                          {i === 0 ? <MatchReportView report={explain(s, g.request, n)} /> : <MatchReportView report={explain(s, g.request, n)} onlyFailures />}
                        </div>
                      );
                    })}
                    <div className="row">
                      <button className="btn btn-sm btn-primary" onClick={() => setCreating(g.request)}>
                        <FilePlus2 /> Create a new stub from this request
                      </button>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
      {creating && <CreateStubDialog request={creating} onClose={() => setCreating(null)} />}
    </div>
  );
}

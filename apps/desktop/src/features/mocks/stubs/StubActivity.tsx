import { useMemo, useState } from 'react';
import { useQuery } from '@/features/mocks/lib/query';
import { RefreshCw, Crosshair, Activity, Wrench } from 'lucide-react';
import type { StubMapping } from '@/features/mocks/lib/types';
import { useRequests, useScenarios, useWm } from '@/features/mocks/lib/queries';
import { stubId } from '@/features/mocks/lib/stub-utils';
import { explainNearMiss } from '@/features/mocks/lib/match';
import { MethodBadge, StatusCode, RelTime, Spinner, formatTime } from '@/features/mocks/components/ui';
import { MatchReportView } from '@/features/mocks/components/MatchReportView';
import { useNav } from '@/features/mocks/store/nav';
import { suggestFix } from '@/features/mocks/lib/fix';

/** Requests this stub served, plus journal requests that *almost* matched it. */
export function StubActivity({ stub, onApplyFix }: { stub: StubMapping; onApplyFix?(s: StubMapping): void }) {
  const { client, sid } = useWm();
  const requests = useRequests({ live: true });
  const scenarios = useScenarios();
  const servedQ = useQuery({
    queryKey: ['wm', sid, 'stub-served', stubId(stub)],
    queryFn: () => client!.requests(200, undefined, { matchingStub: stubId(stub) }),
    enabled: !!client,
    refetchInterval: 5000,
  });
  const go = useNav((s) => s.go);
  const id = stubId(stub);
  // Server-side filter (?matchingStub=) when supported, else filter the recent journal.
  const served = useMemo(
    () => (servedQ.data?.requests ?? requests.data?.requests ?? []).filter((e) => e.wasMatched && e.stubMapping && stubId(e.stubMapping) === id),
    [servedQ.data, requests.data, id],
  );
  const near = useQuery({
    queryKey: ['wm', sid, 'stub-near', id, JSON.stringify(stub.request)],
    queryFn: () => client!.nearMissesForPattern(stub.request),
    enabled: !!client,
  });
  const [open, setOpen] = useState<number | null>(0);
  const misses = (near.data?.nearMisses ?? []).filter((n) => !(n.request.loggedDate && served.some((s) => s.request.loggedDate === n.request.loggedDate)));

  return (
    <div className="scroll p-5 grow">
      <div className="col-lg" style={{ maxWidth: 1080 }}>
        <div className="section">
          <div className="section-head">
            <Activity size={15} className="subtle" />
            <h3>Served by this stub</h3>
            <span className="subtle xs grow">{served.length} request{served.length === 1 ? '' : 's'} in the journal</span>
            <button className="icon-btn sm" onClick={() => servedQ.refetch()} title="Refresh">
              {servedQ.isFetching ? <Spinner /> : <RefreshCw />}
            </button>
          </div>
          {served.length === 0 ? (
            <div className="subtle small">No matching traffic yet. Copy the request as cURL from the ⋯ menu to try it.</div>
          ) : (
            <table className="table">
              <tbody>
                {served.slice(0, 50).map((e) => (
                  <tr key={e.id} className="clickable" onClick={() => go('requests', e.id)}>
                    <td style={{ width: 90 }} className="mono xs subtle">
                      {formatTime(e.request.loggedDate)}
                    </td>
                    <td style={{ width: 60 }}>
                      <MethodBadge method={e.request.method} />
                    </td>
                    <td className="mono xs truncate">{e.request.url}</td>
                    <td style={{ width: 50 }}>
                      <StatusCode status={e.response?.status} />
                    </td>
                    <td style={{ width: 70 }} className="subtle xs">
                      {e.timing?.totalTime !== undefined ? `${e.timing.totalTime} ms` : ''}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        <div className="section">
          <div className="section-head">
            <Crosshair size={15} className="subtle" />
            <h3>Near misses — requests that almost matched</h3>
            <span className="spacer" />
            {near.isFetching && <Spinner />}
          </div>
          {near.isError && <div className="text-danger small">{String((near.error as Error).message)}</div>}
          {misses.length === 0 && !near.isPending && <div className="subtle small">No near misses in the journal for this stub.</div>}
          {misses.slice(0, 20).map((n, i) => {
            const report = explainNearMiss(stub, n.request, n.matchResult, scenarios.data?.find((x) => x.name === stub.scenarioName)?.state);
            const fix = suggestFix(stub, n.request, report);
            return (
              <div key={i} className="fieldset">
                <div className="fieldset-head" onClick={() => setOpen(open === i ? null : i)}>
                  <MethodBadge method={n.request.method} />
                  <span className="mono xs truncate grow">{n.request.url}</span>
                  <span className="subtle xs">distance {n.matchResult.distance.toFixed(2)}</span>
                  <span className="subtle xs">
                    <RelTime ts={n.request.loggedDate} />
                  </span>
                </div>
                {open === i && (
                  <div className="fieldset-body">
                    <MatchReportView report={report} />
                    {fix && onApplyFix && (
                      <div className="row">
                        <span className="subtle xs grow">{fix.summary}</span>
                        <button className="btn btn-sm" onClick={() => onApplyFix(fix.stub)}>
                          <Wrench /> Adjust stub to accept this request
                        </button>
                      </div>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

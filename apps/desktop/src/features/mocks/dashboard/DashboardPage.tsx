import { useMemo, useState } from 'react';
import { Plus, Save, RotateCcw, Eraser, Disc, Crosshair, FileJson2, Activity, ArrowRight, Ghost, Trash2 } from 'lucide-react';
import { useHealth, useMappings, useRequests, useJournalWindow, ACTIVITY_LIMIT, useUnmatchedNearMisses, useScenarios, useRecordingStatus, useSettings, useUnmatchedStubs, useWm, useWrite, qk } from '@/features/mocks/lib/queries';
import { useNav } from '@/features/mocks/store/nav';
import { ENV_LABEL } from '@/features/mocks/store/target';
import { MethodBadge, RelTime, Callout, Segmented } from '@/features/mocks/components/ui';
import { displayName, stubId, traitsOf, urlOf } from '@/features/mocks/lib/stub-utils';
import { openNewStub, openStub, BLANK_STUB } from '@/features/mocks/stubs/open';
import { TrafficPanel, useNow } from './TrafficPanel';
import { RANGES, windowFor, type RangeKey } from './activity';

const RANGE_KEY = 'wmui.dash.range';

export function DashboardPage() {
  const { server, client } = useWm();
  const health = useHealth();
  const mappings = useMappings();
  const requests = useRequests({ live: true });
  const nm = useUnmatchedNearMisses(true);
  const scenarios = useScenarios();
  const rec = useRecordingStatus();
  const settings = useSettings();
  const unused = useUnmatchedStubs();
  const go = useNav((s) => s.go);
  const write = useWrite();

  const stubs = mappings.data ?? [];
  const events = requests.data?.requests ?? [];
  const unmatchedCount = events.filter((e) => !e.wasMatched).length;
  const traits = stubs.map(traitsOf);
  const count = (f: (t: ReturnType<typeof traitsOf>) => boolean) => traits.filter(f).length;

  const [range, setRangeState] = useState<RangeKey>(() => {
    try {
      return (sessionStorage.getItem(RANGE_KEY) as RangeKey) || '1h';
    } catch {
      return '1h';
    }
  });
  const setRange = (r: RangeKey) => {
    setRangeState(r);
    try {
      sessionStorage.setItem(RANGE_KEY, r);
    } catch {
      /* storage unavailable */
    }
  };
  const now = useNow();
  const rangeSpec = RANGES.find((r) => r.key === range)!;
  // The Activity section loads its own time window (?since=), so it is not limited by the
  // Requests page's "last N" setting; everything in the section is scoped to that window.
  const windowQ = useJournalWindow(rangeSpec.ms);
  const activity = windowQ.data?.requests ?? [];
  const scoped = useMemo(() => windowFor(activity, range, now).events, [activity, range, now]);
  const truncated = !!windowQ.data?.capped;

  const topEndpoints = useMemo(() => {
    const m = new Map<string, { method: string; url: string; n: number; unmatched: number }>();
    for (const e of scoped) {
      const path = e.request.url.split('?')[0];
      const k = `${e.request.method} ${path}`;
      const cur = m.get(k) ?? { method: e.request.method, url: path, n: 0, unmatched: 0 };
      cur.n++;
      if (!e.wasMatched) cur.unmatched++;
      m.set(k, cur);
    }
    return [...m.values()].sort((a, b) => b.n - a.n).slice(0, 25);
  }, [scoped]);
  const recentUnmatched = scoped.filter((e) => !e.wasMatched);

  return (
    <div className="page">
      <div className="page-header">
        <h1>Overview</h1>
        {server?.env && <span className={`env-chip env-${server.env}`}>{ENV_LABEL[server.env]}</span>}
        <span className="muted small truncate">
          {server.kind === 'builtin' ? 'Built-in mock server' : server.name} · <span className="mono">{client?.base ?? server.url}</span>
        </span>
        <div className="spacer" />
        <button className="btn btn-sm" onClick={() => write((c) => c.saveMappings(), { label: 'Save mappings', success: 'All stubs persisted to disk' })} title="POST /__admin/mappings/save">
          <Save /> Persist all
        </button>
        <button
          className="btn btn-sm"
          onClick={() =>
            write((c) => c.clearRequests(), { label: 'Clear journal', success: 'Request journal cleared', invalidate: (sid) => [qk.requests(sid), qk.unmatchedNearMisses(sid)] })
          }
        >
          <Eraser /> Clear journal
        </button>
        <button
          className="btn btn-sm"
          onClick={() =>
            write((c) => c.resetAll(), {
              label: 'Reset server',
              success: 'Server reset',
              confirm: { title: 'Reset the server?', message: 'Removes all non-persistent stubs, clears the journal and resets all scenarios.', danger: true, confirmText: 'Reset' },
            })
          }
        >
          <RotateCcw /> Reset
        </button>
        <button className="btn btn-sm btn-primary" onClick={() => openNewStub(structuredClone(BLANK_STUB))}>
          <Plus /> New stub
        </button>
      </div>
      <div className="page-body">
        <div className="col-lg">
          {!server?.auth || server.auth.type === 'none' ? (
            server && !/localhost|127\.0\.0\.1|\[::1\]/.test(server.url) ? (
              <Callout kind="warning">
                This server's admin API is reachable without credentials. WireMock 3 and 4 allow CORS from any origin on <code className="code-inline">/__admin</code>, so any web page
                you visit could change stubs on it. For shared environments start WireMock with <code className="code-inline">--admin-api-basic-auth user:pass</code> (and HTTPS).
              </Callout>
            ) : null
          ) : null}

          <div className="grid-4">
            <StatCard label="Stubs" value={stubs.length} sub={`${count((t) => t.persistent)} persistent · ${count((t) => t.disabled)} disabled`} onClick={() => go('stubs')} icon={<FileJson2 />} />
            <StatCard label="Requests in journal" value={requests.data?.meta.total ?? '—'} sub={`${unmatchedCount} unmatched in last ${events.length}`} onClick={() => go('requests')} icon={<Activity />} />
            <StatCard
              label="Unmatched requests"
              value={Math.max(unmatchedCount, new Set(nm.data?.nearMisses.map((n) => n.request.loggedDate)).size)}
              sub="no stub answered — see near misses"
              onClick={() => go('near-misses')}
              icon={<Crosshair />}
              alert={unmatchedCount > 0 || (nm.data?.nearMisses.length ?? 0) > 0}
            />
            <StatCard
              label="Recording"
              value={rec.data?.status === 'Recording' ? 'ON' : 'off'}
              sub={`${plural(scenarios.data?.length ?? 0, 'scenario')} · ${plural(count((t) => t.proxy), 'proxy stub')}`}
              onClick={() => go('recording')}
              icon={<Disc />}
              alert={rec.data?.status === 'Recording'}
            />
          </div>

          <div className="section-title">
            <h2>Activity</h2>
            <Segmented value={range} onChange={setRange} options={RANGES.map((r) => ({ value: r.key, label: r.label }))} />
            <span className="subtle xs">{range === 'all' ? `latest ${activity.length} requests` : `last ${rangeSpec.label}`} · updates live</span>
          </div>

          <div className="grid-3" style={{ gridTemplateColumns: 'minmax(0, 2fr) minmax(0, 1fr)' }}>
            <TrafficPanel events={activity} range={range} onShowAll={() => setRange('all')} truncated={truncated} limit={ACTIVITY_LIMIT} />
            <div className="panel panel-fill">
              <div className="panel-header">
                <h3 className="grow">Top endpoints</h3>
                <button className="btn btn-ghost btn-sm" onClick={() => go('requests')}>
                  Journal <ArrowRight />
                </button>
              </div>
              <div className="fill-scroll">
                <div className="fill-inner">
                  <table className="table">
                <tbody>
                  {topEndpoints.length === 0 && (
                    <tr>
                      <td className="subtle small">No requests in this range.</td>
                    </tr>
                  )}
                  {topEndpoints.map((e) => (
                    <tr key={e.method + e.url}>
                      <td style={{ width: 60 }}>
                        <MethodBadge method={e.method} />
                      </td>
                      <td className="mono xs truncate" style={{ maxWidth: 200 }} title={e.url}>
                        {e.url}
                      </td>
                      <td className="nowrap xs" style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                        <span className="strong">{e.n}</span>
                        {e.unmatched > 0 && <span className="text-warning"> · {e.unmatched} unmatched</span>}
                      </td>
                    </tr>
                  ))}
                </tbody>
                  </table>
                </div>
              </div>
            </div>
          </div>

          <div className="panel">
            <div className="panel-header">
              <h3 className="grow">Recent unmatched requests</h3>
              <button className="btn btn-ghost btn-sm" onClick={() => go('near-misses')}>
                Near-miss analysis <ArrowRight />
              </button>
            </div>
            <table className="table">
              <tbody>
                {recentUnmatched.length === 0 && (
                  <tr>
                    <td className="subtle small">Nothing unmatched in this range.</td>
                  </tr>
                )}
                {recentUnmatched.slice(0, 6).map((e) => (
                  <tr key={e.id} className="clickable" onClick={() => go('requests', e.id)}>
                    <td style={{ width: 60 }}>
                      <MethodBadge method={e.request.method} />
                    </td>
                    <td className="mono xs truncate">{e.request.url}</td>
                    <td className="subtle xs nowrap" style={{ textAlign: 'right' }}>
                      <RelTime ts={e.request.loggedDate} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="section-title">
            <h2>Server & stubs</h2>
          </div>
          <div className="grid-3" style={{ gridTemplateColumns: 'minmax(0, 1fr) minmax(0, 2fr)' }}>
            <div className="panel">
              <div className="panel-header">
                <h3 className="grow">Server</h3>
              </div>
              <div className="panel-body">
                <dl className="kv">
                  <dt>Version</dt>
                  <dd>{health.data?.version ?? 'unknown (2.x?)'}</dd>
                  <dt>Latency</dt>
                  <dd>{health.data?.latencyMs ?? '—'} ms</dd>
                  <dt>Global delay</dt>
                  <dd>{settings.data?.fixedDelay ? `${settings.data.fixedDelay} ms` : settings.data?.delayDistribution ? settings.data.delayDistribution.type : 'none'}</dd>
                  <dt>Proxy pass-through</dt>
                  <dd>{settings.data?.proxyPassThrough === false ? 'off' : 'on'}</dd>
                  <dt>Faults</dt>
                  <dd>{plural(count((t) => !!t.fault), 'stub')}</dd>
                  <dt>Templated</dt>
                  <dd>{plural(count((t) => t.templated), 'stub')}</dd>
                  <dt>Webhooks</dt>
                  <dd>{plural(count((t) => t.webhook), 'stub')}</dd>
                </dl>
                <button className="btn btn-sm mt-5" onClick={() => go('settings')}>
                  Server settings <ArrowRight />
                </button>
              </div>
            </div>
            <div className="panel panel-fill">
              <div className="panel-header">
                <Ghost size={15} className="subtle" />
                <h3 className="grow">Unused stubs</h3>
                <span className="subtle xs">never matched since last journal reset</span>
                {(unused.data?.length ?? 0) > 0 && (
                  <button
                    className="btn btn-sm btn-danger-ghost"
                    onClick={() =>
                      write((c) => c.removeUnmatchedMappings(), {
                        label: 'Remove unused stubs',
                        success: 'Unused stubs removed',
                        confirm: {
                          title: `Remove ${unused.data?.length} unused stubs?`,
                          message: 'Deletes every stub that has not matched a request in the current journal. Useful after a test run to prune dead mocks.',
                          danger: true,
                          confirmText: 'Remove',
                        },
                      })
                    }
                  >
                    <Trash2 /> Remove all
                  </button>
                )}
              </div>
              <div className="fill-scroll">
                <div className="fill-inner">
                {unused.isError && <div className="p-5 subtle small">Needs WireMock 3.x or later (GET /__admin/mappings/unmatched).</div>}
                {unused.data?.length === 0 && <div className="p-5 subtle small">Every stub has been hit at least once.</div>}
                {unused.data?.slice(0, 50).map((s) => (
                  <div key={stubId(s)} className="list-item" onClick={() => openStub(s)}>
                    <MethodBadge method={s.request.method} />
                    <span className="truncate grow">{s.name || urlOf(s.request)}</span>
                    <span className="meta mono">{displayName(s) !== s.name ? '' : urlOf(s.request)}</span>
                  </div>
                ))}
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function plural(n: number, w: string) {
  return `${n} ${w}${n === 1 ? '' : 's'}`;
}

function StatCard({ label, value, sub, onClick, icon, alert }: { label: string; value: React.ReactNode; sub?: string; onClick?(): void; icon?: React.ReactNode; alert?: boolean }) {
  return (
    <div className="card hoverable" onClick={onClick}>
      <div className="row between">
        <span className="label">{label}</span>
        <span className={alert ? 'text-warning' : 'subtle'}>{icon}</span>
      </div>
      <div className="stat mt-4">
        <span className="v" style={alert ? { color: 'var(--warning)' } : undefined}>
          {value}
        </span>
        {sub && <span className="k">{sub}</span>}
      </div>
    </div>
  );
}

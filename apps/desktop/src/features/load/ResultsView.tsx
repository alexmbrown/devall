import { useMemo, useState } from 'react';
import { AlertTriangle, CheckCircle2, CircleStop, XCircle } from 'lucide-react';
import { Callout, CopyButton, MethodBadge, Segmented, formatBytes } from '@/components/ui/misc';
import type { LoadSummary, Percentiles, ThresholdResult, TimelinePoint } from '@/lib/api/load';
import { LineChart, StatusMix, type ChartSeries } from './charts';
import { describeExecutor, fmtCount, fmtDuration, fmtMs, fmtPct, fmtRate, fmtSecs } from './format';
import type { LiveRun } from './store';

// ---------- timeline ----------

export function TimelineCharts({ points, bucket, xMax }: { points: TimelinePoint[]; bucket: number; xMax?: number }) {
  const [view, setView] = useState<'charts' | 'table'>('charts');
  const x = useMemo(() => points.map((p) => p.t), [points]);
  const rps: ChartSeries[] = useMemo(() => [{ key: 'rps', label: 'Requests/s', color: 'var(--chart-matched)', values: points.map((p) => p.rps) }], [points]);
  const latency: ChartSeries[] = useMemo(
    () => [
      { key: 'p50', label: 'p50', color: 'var(--lat-p50)', values: points.map((p) => (p.requests ? p.p50 : null)) },
      { key: 'p95', label: 'p95', color: 'var(--lat-p95)', values: points.map((p) => (p.requests ? p.p95 : null)) },
      { key: 'p99', label: 'p99', color: 'var(--lat-p99)', values: points.map((p) => (p.requests ? p.p99 : null)) },
    ],
    [points],
  );
  const failures: ChartSeries[] = useMemo(() => {
    const s: ChartSeries[] = [{ key: 'failed', label: 'Failed/s', color: 'var(--chart-5xx)', values: points.map((p) => p.failed / bucket) }];
    if (points.some((p) => p.dropped > 0)) s.push({ key: 'dropped', label: 'Dropped/s', color: 'var(--chart-4xx)', values: points.map((p) => p.dropped / bucket) });
    return s;
  }, [points, bucket]);
  const vus: ChartSeries[] = useMemo(() => [{ key: 'vus', label: 'VUs', color: 'var(--chart-matched)', values: points.map((p) => p.vus) }], [points]);

  return (
    <section className="section">
      <div className="section-head">
        <h3>Over time</h3>
        <span className="subtle xs">{bucket > 1 ? `${bucket}s buckets` : 'per second'}</span>
        <span className="spacer" />
        <Segmented
          value={view}
          onChange={setView}
          options={[
            { value: 'charts', label: 'Charts' },
            { value: 'table', label: 'Table' },
          ]}
        />
      </div>
      {view === 'charts' ? (
        <div className="load-charts">
          <div className="card">
            <LineChart title="Throughput (requests/s)" x={x} series={rps} format={fmtRate} area xMax={xMax} bucket={bucket} />
          </div>
          <div className="card">
            <LineChart title="Latency" x={x} series={latency} format={fmtMs} axisFormat={(v) => (v >= 1000 ? `${fmtCount(v / 1000)}s` : `${fmtCount(v)}ms`)} xMax={xMax} bucket={bucket} />
          </div>
          <div className="card">
            <LineChart title={failures.length > 1 ? 'Failures and dropped iterations (per second)' : 'Failed requests (per second)'} x={x} series={failures} format={(v) => `${v.toFixed(v < 10 ? 1 : 0)}/s`} xMax={xMax} bucket={bucket} />
          </div>
          <div className="card">
            <LineChart title="Busy virtual users" x={x} series={vus} format={(v) => `${v} VUs`} area xMax={xMax} bucket={bucket} />
          </div>
        </div>
      ) : (
        <div className="panel scroll" style={{ maxHeight: 360 }}>
          <table className="table load-table">
            <thead>
              <tr>
                <th>Time</th>
                <th className="num">Requests</th>
                <th className="num">Req/s</th>
                <th className="num">Failed</th>
                <th className="num">Dropped</th>
                <th className="num">p50</th>
                <th className="num">p95</th>
                <th className="num">p99</th>
                <th className="num">Max</th>
                <th className="num">VUs</th>
              </tr>
            </thead>
            <tbody>
              {points.map((p) => (
                <tr key={p.t}>
                  <td className="mono">{fmtSecs(p.t)}</td>
                  <td className="num">{p.requests.toLocaleString()}</td>
                  <td className="num">{p.rps}</td>
                  <td className="num">{p.failed}</td>
                  <td className="num">{p.dropped}</td>
                  <td className="num">{p.requests ? fmtMs(p.p50) : '—'}</td>
                  <td className="num">{p.requests ? fmtMs(p.p95) : '—'}</td>
                  <td className="num">{p.requests ? fmtMs(p.p99) : '—'}</td>
                  <td className="num">{p.requests ? fmtMs(p.max) : '—'}</td>
                  <td className="num">{p.vus}</td>
                </tr>
              ))}
              {!points.length && (
                <tr>
                  <td colSpan={10} className="subtle">
                    No data yet.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function Kpi({ k, v, d }: { k: string; v: React.ReactNode; d?: React.ReactNode }) {
  return (
    <div className="kpi">
      <span className="k">{k}</span>
      <span className="v">{v}</span>
      {d != null && <span className="d">{d}</span>}
    </div>
  );
}

// ---------- live ----------

export function LiveView({ live, onStop }: { live: LiveRun; onStop(): void }) {
  const s = live.snapshot;
  const planned = s?.plannedDurationMs ?? 0;
  const elapsed = s?.elapsedMs ?? 0;
  const pct = planned ? Math.min(100, (elapsed / planned) * 100) : 0;
  const stopping = live.stopping || s?.phase === 'stopping';
  return (
    <div className="load-results">
      <div className="load-progress" aria-live="polite">
        <span className={stopping ? 'dot warn pulse' : 'dot ok pulse'} />
        <span className="small strong">{!s ? 'Starting…' : stopping ? 'Finishing — waiting for in-flight requests' : 'Running'}</span>
        <div className="progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)}>
          <div style={{ width: `${pct}%` }} />
        </div>
        <span className="subtle xs mono">
          {fmtDuration(elapsed)} / {fmtDuration(planned)}
        </span>
        <button className="btn btn-sm btn-danger" onClick={onStop} disabled={live.stopping}>
          <CircleStop /> {live.stopping ? 'Stopping…' : 'Stop'}
        </button>
      </div>
      <div className="load-kpis">
        <Kpi k="Throughput" v={fmtRate(s?.rps)} d={`${fmtCount(s?.requests ?? 0)} requests`} />
        <Kpi k="p95 latency" v={s?.requests ? fmtMs(s.latency.p95) : '—'} d={s?.requests ? `p50 ${fmtMs(s.latency.p50)} · p99 ${fmtMs(s.latency.p99)}` : undefined} />
        <Kpi k="Error rate" v={s?.requests ? fmtPct(s.failed / s.requests) : '—'} d={`${fmtCount(s?.failed ?? 0)} failed`} />
        <Kpi k="Busy VUs" v={s ? `${s.vus}` : '—'} d={s ? `${s.vusAllocated} allocated` : undefined} />
        <Kpi k="Iterations" v={fmtCount(s?.iterations ?? 0)} d={s?.droppedIterations ? `${fmtCount(s.droppedIterations)} dropped` : 'none dropped'} />
      </div>
      <TimelineCharts points={live.points} bucket={s?.bucketSeconds ?? 1} xMax={planned / 1000} />
      {s && s.requests > 0 && (
        <section className="section">
          <div className="section-head">
            <h3>Responses</h3>
          </div>
          <StatusMix codes={s.statusCodes} transportErrors={s.errorKinds.connect + s.errorKinds.timeout + s.errorKinds.other} />
        </section>
      )}
    </div>
  );
}

// ---------- summary ----------

export function ThresholdBadges({ results }: { results: ThresholdResult[] }) {
  if (!results.length) return <p className="subtle small">No thresholds — add some in Design to get a pass/fail verdict (and a CI exit code).</p>;
  return (
    <div className="load-thresholds">
      {results.map((t, i) => (
        <span key={i} className={`th-badge ${t.ok ? 'ok' : 'bad'}`} title={t.message}>
          {t.ok ? <CheckCircle2 aria-label="passed" /> : <XCircle aria-label="failed" />}
          <span className="mono">{t.expr}</span>
          <span className="subtle">{t.actual != null ? `${t.actual}${t.unit}` : 'no data'}</span>
        </span>
      ))}
    </div>
  );
}

function PercentileRow({ label, method, p, requests, failed, rps, sub }: { label: React.ReactNode; method?: string; p: Percentiles; requests?: number; failed?: number; rps?: number; sub?: boolean }) {
  return (
    <tr className={sub ? 'sub' : undefined}>
      <td>
        <span className="row-sm">
          {method && <MethodBadge method={method} short />}
          <span className="truncate">{label}</span>
        </span>
      </td>
      <td className="num">{requests != null ? requests.toLocaleString() : ''}</td>
      <td className="num">{failed != null ? failed.toLocaleString() : ''}</td>
      <td className="num">{rps != null ? fmtRate(rps) : ''}</td>
      <td className="num">{fmtMs(p.p50)}</td>
      <td className="num">{fmtMs(p.p75)}</td>
      <td className="num">{fmtMs(p.p90)}</td>
      <td className="num">{fmtMs(p.p95)}</td>
      <td className="num">{fmtMs(p.p99)}</td>
      <td className="num">{fmtMs(p.p999)}</td>
      <td className="num">{fmtMs(p.max)}</td>
      <td className="num">{fmtMs(p.mean)}</td>
    </tr>
  );
}

export function SummaryView({ summary: s, file }: { summary: LoadSummary; file: string | null }) {
  const cloud = !!s.agents?.length && s.agents.every((a) => a.address.startsWith('cloud:'));
  const t = s.totals;
  const verdict = s.aborted ? 'stop' : s.thresholdsPassed ? 'pass' : 'fail';
  const transport = s.errorKinds.connect + s.errorKinds.timeout + s.errorKinds.other;
  return (
    <div className="load-results">
      <div className="load-banner">
        <span className={`verdict ${verdict}`}>
          {verdict === 'pass' ? <CheckCircle2 /> : verdict === 'fail' ? <XCircle /> : <AlertTriangle />}
          {verdict === 'pass' ? 'Passed' : verdict === 'fail' ? 'Failed' : 'Stopped early'}
        </span>
        {s.abortReason && <span className="muted small">{s.abortReason}</span>}
        {s.mockBaseline && (
          <span className="badge info" title={`Ran against the project's mock server ${s.mockBaseline.url}${s.mockBaseline.replacedHosts?.length ? ` instead of ${s.mockBaseline.replacedHosts.join(', ')}` : ''}`}>
            Mock baseline
          </span>
        )}
        {s.agents && (
          <span
            className="badge info"
            title={
              cloud
                ? "Generated from the relay's cloud regions (best-effort placement); percentiles come from their merged histograms, thresholds were evaluated here"
                : 'Generated by load agents; percentiles and thresholds come from their merged HDR histograms'
            }
          >
            {s.agents.length} {cloud ? 'cloud region' : 'agent'}
            {s.agents.length === 1 ? '' : 's'}
          </span>
        )}
        <span className="spacer" />
        {file && (
          <span className="row-sm subtle xs mono" title="Run summary (commit it to track performance over time)">
            {file}
            <CopyButton text={file} title="Copy path" />
          </span>
        )}
      </div>
      <div className="load-meta">
        <span>
          <b>Started</b> {new Date(s.startedAt).toLocaleString()}
        </span>
        <span>
          <b>Duration</b> {fmtDuration(s.durationMs)}
          {s.plannedDurationMs && Math.abs(s.durationMs - s.plannedDurationMs) > 1500 ? ` (planned ${fmtDuration(s.plannedDurationMs)})` : ''}
        </span>
        <span>
          <b>Profile</b> {describeExecutor(s.executor)}
        </span>
        {s.environment && (
          <span>
            <b>Environment</b> {s.environment}
          </span>
        )}
        {s.git && (
          <span>
            <b>Commit</b> <span className="mono">{s.git.commit}</span>
            {s.git.branch ? ` on ${s.git.branch}` : ''}
          </span>
        )}
        <span>
          <b>{cloud ? 'Started from' : s.agents ? 'Coordinator' : 'Machine'}</b> {s.machine.os}, {s.machine.cpus} CPUs
        </span>
      </div>

      {s.agents && s.agents.length > 0 && (
        <section className="section">
          <div className="section-head">
            <h3>{cloud ? 'Cloud regions' : 'Agents'}</h3>
            <span className="subtle xs">
              {cloud ? 'each region’s share and the data center it ran in (when detected); the totals above merge their histograms' : 'each agent’s share; the totals above merge their histograms exactly'}
            </span>
          </div>
          <div className="panel">
            <table className="table load-table">
              <thead>
                <tr>
                  <th>{cloud ? 'Region' : 'Agent'}</th>
                  <th className="num">Share</th>
                  <th>{cloud ? 'Where' : 'Machine'}</th>
                  <th className="num">Requests</th>
                  <th className="num">Failed</th>
                  <th className="num">p95</th>
                  <th>Status</th>
                </tr>
              </thead>
              <tbody>
                {s.agents.map((a) => (
                  <tr key={a.address}>
                    <td className="mono">{a.address}</td>
                    <td className="num">{Math.round(a.share * 100)}%</td>
                    <td className="subtle">{cloud ? a.machine.os : `${a.machine.os || '?'} · ${a.machine.cpus} CPUs${a.version ? ` · ${a.version}` : ''}`}</td>
                    <td className="num">{a.requests.toLocaleString()}</td>
                    <td className="num">{a.failed.toLocaleString()}</td>
                    <td className="num">{a.requests ? fmtMs(a.p95) : '—'}</td>
                    <td className={a.ok ? 'text-success' : 'text-danger'} title={a.error ?? a.abortedReason}>
                      {a.ok ? (a.abortedReason ? 'stopped' : 'ok') : (a.error ?? 'no results')}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}

      <ThresholdBadges results={s.thresholds} />

      <div className="load-kpis">
        <Kpi k="Throughput" v={fmtRate(t.rps)} d={`${t.requests.toLocaleString()} requests`} />
        <Kpi k="p95 latency" v={t.requests ? fmtMs(s.latency.p95) : '—'} d={`p99 ${fmtMs(s.latency.p99)} · max ${fmtMs(s.latency.max)}`} />
        <Kpi k="Error rate" v={fmtPct(t.errorRate)} d={`${t.failed.toLocaleString()} failed`} />
        <Kpi k="Checks" v={fmtPct(t.checksRate)} d={`${t.checksFailed.toLocaleString()} failed`} />
        <Kpi k="Iterations" v={fmtCount(t.iterations)} d={t.droppedIterations ? `${t.droppedIterations.toLocaleString()} dropped` : `${fmtRate(t.iterationsPerSec)} · none dropped`} />
        <Kpi k="Data" v={formatBytes(t.bytesIn)} d={`${formatBytes(t.bytesOut)} sent · ${t.maxVus} VUs max`} />
      </div>

      {!s.openModel && (
        <Callout>
          Closed model: latency is measured from the actual send, so a stalling server also slows the load and the slowest moments are under-sampled
          (coordinated omission). Use an arrival-rate executor for latency SLOs.
        </Callout>
      )}
      {(s.warnings ?? []).map((w, i) => (
        <Callout key={i} kind="warning">
          {w}
        </Callout>
      ))}

      <TimelineCharts points={s.timeline} bucket={s.bucketSeconds} xMax={Math.max(s.plannedDurationMs, s.durationMs) / 1000} />

      <section className="section">
        <div className="section-head">
          <h3>Latency</h3>
          <span className="subtle xs">{s.openModel ? 'From each iteration’s intended start (coordinated-omission corrected)' : 'From the actual send'}</span>
        </div>
        <div className="panel scroll">
          <table className="table load-table">
            <thead>
              <tr>
                <th>Request</th>
                <th className="num">Requests</th>
                <th className="num">Failed</th>
                <th className="num">Rate</th>
                <th className="num">p50</th>
                <th className="num">p75</th>
                <th className="num">p90</th>
                <th className="num">p95</th>
                <th className="num">p99</th>
                <th className="num">p99.9</th>
                <th className="num">Max</th>
                <th className="num">Mean</th>
              </tr>
            </thead>
            <tbody>
              <PercentileRow label={<span className="strong">All requests</span>} p={s.latency} requests={t.requests} failed={t.failed} rps={t.rps} />
              {s.serviceTime && <PercentileRow label="Service time (from the actual send)" p={s.serviceTime} sub />}
              {s.requests.length > 1 && s.requests.map((r) => <PercentileRow key={r.path} label={r.name} method={r.method} p={r.latency} requests={r.requests} failed={r.failed} rps={r.rps} />)}
            </tbody>
          </table>
        </div>
      </section>

      <div className="load-grid-2">
        <section className="section">
          <div className="section-head">
            <h3>Responses</h3>
          </div>
          <StatusMix codes={s.statusCodes} transportErrors={transport} />
        </section>
        <section className="section">
          <div className="section-head">
            <h3>Errors without a response</h3>
          </div>
          {transport === 0 ? (
            <p className="subtle small">None — every request got a response.</p>
          ) : (
            <>
              <dl className="kv" style={{ margin: 0 }}>
                <dt>Connection</dt>
                <dd>{s.errorKinds.connect.toLocaleString()}</dd>
                <dt>Timeout</dt>
                <dd>{s.errorKinds.timeout.toLocaleString()}</dd>
                <dt>Other</dt>
                <dd>{s.errorKinds.other.toLocaleString()}</dd>
              </dl>
              {(s.errorSamples ?? []).map((e, i) => (
                <pre key={i} className="code">
                  {e}
                </pre>
              ))}
            </>
          )}
        </section>
      </div>
    </div>
  );
}

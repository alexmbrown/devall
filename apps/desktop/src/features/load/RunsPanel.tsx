import { useEffect, useState } from 'react';
import clsx from 'clsx';
import { ArrowDownRight, ArrowUpRight, CheckCircle2, GitCompare, History, Server, Trash2, XCircle, AlertTriangle } from 'lucide-react';
import { Callout, Checkbox, Empty, RelTime, Spinner } from '@/components/ui/misc';
import { Modal } from '@/components/ui/Modal';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { loadApi, type BaselineBreakdown, type Comparison, type MetricDelta, type RunEntry } from '@/lib/api/load';
import { fmtCount, fmtDuration, fmtMs, fmtPct, fmtRate } from './format';
import { useLoadRuns, type FinishedRun } from './store';

export function RunsPanel({ root, path, onOpen }: { root: string; path: string; onOpen(run: FinishedRun): void }) {
  const version = useLoadRuns((s) => s.runsVersion);
  const [runs, setRuns] = useState<RunEntry[] | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [comparison, setComparison] = useState<Comparison | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    loadApi
      .runs(root, path)
      .then((r) => alive && setRuns(r))
      .catch((e) => {
        if (alive) setRuns([]);
        toast.error('Could not list runs', errorMessage(e));
      });
    return () => {
      alive = false;
    };
  }, [root, path, version]);

  useEffect(() => setSelected((sel) => sel.filter((f) => runs?.some((r) => r.file === f))), [runs]);

  const toggle = (file: string, on: boolean) => setSelected((sel) => (on ? [...sel.filter((f) => f !== file), file].slice(-2) : sel.filter((f) => f !== file)));

  const open = async (r: RunEntry) => {
    try {
      onOpen({ summary: await loadApi.readRun(root, r.file), file: r.file });
    } catch (e) {
      toast.error('Could not open run', errorMessage(e));
    }
  };

  const compare = async () => {
    if (selected.length !== 2 || !runs) return;
    // Older run is the base, newer the target.
    const [a, b] = [...selected].sort((x, y) => (runs.find((r) => r.file === x)?.startedAt ?? '').localeCompare(runs.find((r) => r.file === y)?.startedAt ?? ''));
    setBusy(true);
    try {
      setComparison(await loadApi.compare(root, a, b));
    } catch (e) {
      toast.error('Compare failed', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  /** The newest mock-baseline run, to compare a selected real run against. */
  const baseline = runs?.find((r) => r.mockBaseline && !r.aborted);
  const selectedReal = selected.length === 1 ? runs?.find((r) => r.file === selected[0] && !r.mockBaseline) : undefined;
  const compareWithMock = async () => {
    if (!baseline || !selectedReal) return;
    setBusy(true);
    try {
      setComparison(await loadApi.compare(root, baseline.file, selectedReal.file));
    } catch (e) {
      toast.error('Compare failed', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    const ok = await confirmDialog({
      title: selected.length === 1 ? 'Delete this run?' : `Delete ${selected.length} runs?`,
      message: 'The summary files move to your system trash.',
      confirmText: 'Move to trash',
      danger: true,
    });
    if (!ok) return;
    for (const f of selected) await loadApi.deleteRun(root, f).catch((e) => toast.error('Delete failed', errorMessage(e)));
    setSelected([]);
    useLoadRuns.getState().bumpRuns();
  };

  if (!runs)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  if (!runs.length)
    return (
      <Empty icon={<History />} title="No runs yet">
        Every run keeps a summary on this computer (never in the project) to compare performance across branches and releases. For CI, <code className="code-inline">devall load run --out summary.json</code> writes one where you want it.
      </Empty>
    );

  return (
    <div className="col">
      <div className="row">
        <span className="subtle small">{runs.length} saved runs · select two to compare</span>
        <span className="spacer" />
        <button className="btn btn-sm" disabled={selected.length !== 2 || busy} onClick={compare} title="Compare the two selected runs">
          <GitCompare /> Compare
        </button>
        <button
          className="btn btn-sm"
          disabled={!baseline || !selectedReal || busy}
          onClick={compareWithMock}
          title={baseline ? 'Split the selected real run into client/tooling overhead (the mock baseline) and server + network time' : 'No mock baseline yet: use “Run against mock” first'}
        >
          <Server /> Compare with mock baseline
        </button>
        <button className="btn btn-sm btn-danger-ghost" disabled={!selected.length} onClick={remove}>
          <Trash2 /> Delete
        </button>
      </div>
      <div className="panel scroll">
        <table className="table load-table load-runs">
          <thead>
            <tr>
              <th aria-label="Select" />
              <th>Started</th>
              <th>Result</th>
              <th>Env</th>
              <th>Commit</th>
              <th className="num">Duration</th>
              <th className="num">Requests</th>
              <th className="num">Rate</th>
              <th className="num">p95</th>
              <th className="num">p99</th>
              <th className="num">Errors</th>
            </tr>
          </thead>
          <tbody>
            {runs.map((r) => (
              <tr
                key={r.file}
                className={clsx('clickable', selected.includes(r.file) && 'on')}
                tabIndex={0}
                onClick={() => open(r)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') open(r);
                  if (e.key === ' ') {
                    e.preventDefault();
                    toggle(r.file, !selected.includes(r.file));
                  }
                }}
                title={r.file}
              >
                <td onClick={(e) => e.stopPropagation()}>
                  <Checkbox checked={selected.includes(r.file)} onChange={(v) => toggle(r.file, v)} title="Select for comparison" />
                </td>
                <td>
                  <RelTime ts={r.startedAt} />
                </td>
                <td>
                  {r.aborted ? (
                    <span className="badge warning">
                      <AlertTriangle /> Stopped
                    </span>
                  ) : r.thresholdsPassed ? (
                    <span className="badge success">
                      <CheckCircle2 /> Passed
                    </span>
                  ) : (
                    <span className="badge danger">
                      <XCircle /> Failed
                    </span>
                  )}
                </td>
                <td className="small">
                  {r.mockBaseline ? (
                    <span className="badge info" title="Ran against the project's mock server">
                      <Server /> mock
                    </span>
                  ) : (
                    (r.environment ?? '—')
                  )}
                </td>
                <td className="mono" title={r.git?.branch}>
                  {r.git?.commit.slice(0, 8) ?? '—'}
                </td>
                <td className="num">{fmtDuration(r.durationMs)}</td>
                <td className="num">{fmtCount(r.requests)}</td>
                <td className="num">{fmtRate(r.rps)}</td>
                <td className="num">{fmtMs(r.p95)}</td>
                <td className="num">{fmtMs(r.p99)}</td>
                <td className="num">{fmtPct(r.errorRate)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <Modal open={!!comparison} onClose={() => setComparison(null)} title="Compare runs" size="xl">
        {comparison && <CompareView c={comparison} />}
      </Modal>
    </div>
  );
}

function fmtValue(v: number, unit: string): string {
  if (unit === 'ms') return fmtMs(v);
  if (unit === '%') return `${v.toFixed(2)}%`;
  if (unit === '/s') return fmtRate(v);
  return fmtCount(v);
}

function fmtDelta(m: MetricDelta): string {
  const sign = m.delta > 0 ? '+' : m.delta < 0 ? '−' : '±';
  const abs = Math.abs(m.delta);
  const v = m.unit === '%' ? `${abs.toFixed(2)} pp` : fmtValue(abs, m.unit);
  return `${sign}${v}`;
}

function VerdictBadge({ m }: { m: MetricDelta }) {
  if (m.verdict === 'regression')
    return (
      <span className="badge danger verdict-badge">
        {m.better === 'lower' ? <ArrowUpRight /> : <ArrowDownRight />} Regression
      </span>
    );
  if (m.verdict === 'improvement')
    return (
      <span className="badge success verdict-badge">
        {m.better === 'lower' ? <ArrowDownRight /> : <ArrowUpRight />} Improvement
      </span>
    );
  return <span className="subtle xs">—</span>;
}

function DeltaTable({ metrics }: { metrics: MetricDelta[] }) {
  return (
    <table className="table load-table">
      <thead>
        <tr>
          <th>Metric</th>
          <th className="num">Base</th>
          <th className="num">Target</th>
          <th className="num">Change</th>
          <th className="num">%</th>
          <th>Verdict</th>
        </tr>
      </thead>
      <tbody>
        {metrics.map((m) => (
          <tr key={m.key}>
            <td>{m.label}</td>
            <td className="num">{fmtValue(m.base, m.unit)}</td>
            <td className="num">{fmtValue(m.target, m.unit)}</td>
            <td className={`num delta ${m.verdict}`}>{fmtDelta(m)}</td>
            <td className={`num delta ${m.verdict}`}>{m.deltaPct == null ? '—' : `${m.deltaPct > 0 ? '+' : ''}${m.deltaPct.toFixed(1)}%`}</td>
            <td>
              <VerdictBadge m={m} />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function BaselineView({ b }: { b: BaselineBreakdown }) {
  return (
    <section className="section">
      <div className="section-head">
        <h3>Where the time goes</h3>
        <span className="subtle xs">mock baseline = client, load generator and local stack · the rest = server + network</span>
      </div>
      <table className="table load-table">
        <thead>
          <tr>
            <th>Latency</th>
            <th className="num">Real run</th>
            <th className="num">Mock baseline</th>
            <th className="num">Server + network</th>
            <th className="num">Client share</th>
          </tr>
        </thead>
        <tbody>
          {b.rows.map((r) => (
            <tr key={r.key}>
              <td>{r.label}</td>
              <td className="num">{fmtMs(r.real)}</td>
              <td className="num">{fmtMs(r.mock)}</td>
              <td className="num strong">{fmtMs(r.server)}</td>
              <td className="num">{(r.clientShare * 100).toFixed(1)}%</td>
            </tr>
          ))}
        </tbody>
      </table>
      {b.requests.length > 1 && (
        <table className="table load-table">
          <thead>
            <tr>
              <th>Request (p95)</th>
              <th className="num">Real</th>
              <th className="num">Mock</th>
              <th className="num">Server + network</th>
            </tr>
          </thead>
          <tbody>
            {b.requests.map((r) => (
              <tr key={r.path}>
                <td>
                  {r.name} <span className="subtle xs mono">{r.path}</span>
                </td>
                <td className="num">{fmtMs(r.realP95)}</td>
                <td className="num">{fmtMs(r.mockP95)}</td>
                <td className="num strong">{fmtMs(r.serverP95)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

export function CompareView({ c }: { c: Comparison }) {
  const ref = (r: Comparison['base'], label: string) => (
    <div className="card col-sm">
      <span className="label">
        {label}
        {r.mockBaseline && <span className="badge info"> mock baseline</span>}
      </span>
      <span className="strong">{new Date(r.startedAt).toLocaleString()}</span>
      <span className="subtle xs">
        {r.environment ?? 'no environment'}
        {r.commit ? ` · ${r.commit.slice(0, 8)}` : ''}
        {r.aborted ? ' · stopped early' : ''}
      </span>
    </div>
  );
  const changed = c.thresholds.filter((t) => t.baseOk !== t.targetOk);
  return (
    <div className="col-lg">
      <div className="grid-2">
        {ref(c.base, 'Base (older)')}
        {ref(c.target, 'Target (newer)')}
      </div>
      <div className="row">
        {c.baseline ? (
          <span className="badge info">
            <Server /> Real run vs mock baseline
          </span>
        ) : c.regressions > 0 ? (
          <span className="badge danger">
            <ArrowUpRight /> {c.regressions} regression{c.regressions === 1 ? '' : 's'}
          </span>
        ) : (
          <span className="badge success">
            <CheckCircle2 /> No regressions
          </span>
        )}
        {c.improvements > 0 && (
          <span className="badge success">
            <ArrowDownRight /> {c.improvements} improvement{c.improvements === 1 ? '' : 's'}
          </span>
        )}
        <span className="subtle xs">Changes under 10% (or under 1 ms / 0.5 pp) count as unchanged.</span>
      </div>
      {c.warnings.map((w, i) => (
        <Callout key={i} kind={c.baseline && i === 0 ? 'info' : 'warning'}>
          {w}
        </Callout>
      ))}
      {c.baseline && <BaselineView b={c.baseline} />}
      <DeltaTable metrics={c.metrics} />
      {changed.length > 0 && (
        <section className="section">
          <div className="section-head">
            <h3>Thresholds that changed</h3>
          </div>
          <table className="table load-table">
            <thead>
              <tr>
                <th>Threshold</th>
                <th>Base</th>
                <th>Target</th>
              </tr>
            </thead>
            <tbody>
              {changed.map((t) => (
                <tr key={t.expr}>
                  <td className="mono">{t.expr}</td>
                  <td>{t.baseOk == null ? '—' : t.baseOk ? 'passed' : 'failed'}</td>
                  <td className={t.targetOk === false ? 'text-danger' : t.targetOk ? 'text-success' : ''}>{t.targetOk == null ? '—' : t.targetOk ? 'passed' : 'failed'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
      {c.requests.length > 1 &&
        c.requests.map((r) => (
          <details key={r.path} className="fieldset">
            <summary>
              {r.name} <span className="subtle xs mono">{r.path}</span>
              <span className="spacer" />
              {r.metrics.some((m) => m.verdict === 'regression') && <span className="badge danger">regressed</span>}
            </summary>
            <div className="fieldset-body">
              <DeltaTable metrics={r.metrics} />
            </div>
          </details>
        ))}
    </div>
  );
}

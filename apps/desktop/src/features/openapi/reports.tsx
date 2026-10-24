/**
 * Report views for the OpenAPI linter and drift checks (shared by the OpenAPI dialog, the mock
 * server's Drift tab and the inspector).
 */
import { useMemo, useState } from 'react';
import clsx from 'clsx';
import { AlertTriangle, CheckCircle2, Info, XCircle } from 'lucide-react';
import { MethodBadge, SearchInput, Segmented } from '@/components/ui/misc';
import type { DriftFinding, DriftKind, DriftReport, LintReport, Severity } from '@/lib/api/openapi';
import { filterFindings, kindLabel } from './logic';

export function SeverityIcon({ s }: { s: Severity }) {
  if (s === 'error') return <XCircle size={14} className="text-danger" aria-label="error" />;
  if (s === 'warning') return <AlertTriangle size={14} className="text-warning" aria-label="warning" />;
  return <Info size={14} className="subtle" aria-label="info" />;
}

function Counts({ errors, warnings, infos }: { errors: number; warnings: number; infos: number }) {
  if (!errors && !warnings && !infos)
    return (
      <span className="badge success">
        <CheckCircle2 /> No problems
      </span>
    );
  return (
    <span className="row-sm">
      {errors > 0 && <span className="badge danger">{errors} error{errors === 1 ? '' : 's'}</span>}
      {warnings > 0 && <span className="badge warning">{warnings} warning{warnings === 1 ? '' : 's'}</span>}
      {infos > 0 && <span className="badge outline">{infos} info</span>}
    </span>
  );
}

type SevFilter = 'all' | Severity;

export function LintReportView({ report }: { report: LintReport }) {
  const [sev, setSev] = useState<SevFilter>('all');
  const [q, setQ] = useState('');
  const shown = useMemo(() => filterFindings(report.findings, sev, q, (f) => `${f.rule} ${f.message} ${f.location}`), [report, sev, q]);
  return (
    <div className="col">
      <div className="row wrap">
        <span className="strong">{report.title}</span>
        <span className="subtle xs">
          OpenAPI {report.version || '?'} · {report.operations} operation{report.operations === 1 ? '' : 's'}
        </span>
        <Counts errors={report.errors} warnings={report.warnings} infos={report.infos} />
        <span className="spacer" />
        <SearchInput value={q} onChange={setQ} placeholder="Filter findings" className="oa-search" />
        <Segmented<SevFilter>
          value={sev}
          onChange={setSev}
          options={[
            { value: 'all', label: 'All' },
            { value: 'error', label: 'Errors' },
            { value: 'warning', label: 'Warnings' },
            { value: 'info', label: 'Info' },
          ]}
        />
      </div>
      {shown.length > 0 && (
        <div className="panel scroll oa-table-wrap">
          <table className="table oa-table">
            <thead>
              <tr>
                <th aria-label="Severity" />
                <th>Rule</th>
                <th>Problem</th>
                <th>Where</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((f, i) => (
                <tr key={i} className={clsx('oa-row', f.severity)}>
                  <td>
                    <SeverityIcon s={f.severity} />
                  </td>
                  <td className="mono xs nowrap">{f.rule}</td>
                  <td className="small">{f.message}</td>
                  <td className="mono xs subtle break">{f.location}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {!shown.length && report.findings.length > 0 && <div className="subtle small">Nothing matches the filter.</div>}
    </div>
  );
}

function DriftRow({ f, onOpen }: { f: DriftFinding; onOpen?(f: DriftFinding): void }) {
  const [open, setOpen] = useState(false);
  const expandable = f.details.length > 0;
  return (
    <>
      <tr className={clsx('oa-row', f.severity, expandable && 'clickable')} onClick={() => expandable && setOpen(!open)}>
        <td>
          <SeverityIcon s={f.severity} />
        </td>
        <td className="nowrap">
          <span className={clsx('badge', f.sourceKind === 'mock' ? 'accent' : 'info')}>{f.sourceKind === 'mock' ? 'stub' : f.source}</span>
        </td>
        <td>
          <div className="row-sm">
            {f.method && f.method !== 'ANY' && <MethodBadge method={f.method} short />}
            <span className="mono xs truncate" title={f.label}>
              {f.sourceKind === 'mock' ? f.label : f.path}
            </span>
            {f.count > 1 && <span className="badge outline">×{f.count}</span>}
          </div>
          {f.sourceKind === 'mock' && <div className="subtle xs mono truncate">{f.source}</div>}
        </td>
        <td className="small">
          <span className="subtle xs">{kindLabel(f.kind)} · </span>
          {f.message}
          {expandable && <span className="subtle xs"> {open ? '▾' : '▸'}</span>}
        </td>
        <td className="nowrap">
          {onOpen && f.sourceKind === 'mock' && f.refId && (
            <button
              className="btn btn-sm btn-ghost"
              onClick={(e) => {
                e.stopPropagation();
                onOpen(f);
              }}
            >
              Open stub
            </button>
          )}
        </td>
      </tr>
      {open && (
        <tr className="oa-details">
          <td />
          <td colSpan={4}>
            <ul className="mono xs">
              {f.details.map((d, i) => (
                <li key={i}>{d}</li>
              ))}
            </ul>
          </td>
        </tr>
      )}
    </>
  );
}

type KindFilter = 'all' | 'mock' | 'traffic';

export function DriftReportView({ report, onOpenStub }: { report: DriftReport; onOpenStub?(f: DriftFinding): void }) {
  const [sev, setSev] = useState<SevFilter>('all');
  const [src, setSrc] = useState<KindFilter>('all');
  const [q, setQ] = useState('');
  const shown = useMemo(
    () => filterFindings(report.findings, sev, q, (f) => `${f.label} ${f.path} ${f.message} ${f.operation ?? ''} ${f.source}`).filter((f) => src === 'all' || f.sourceKind === src),
    [report, sev, src, q],
  );
  const uncovered = report.coverage.filter((c) => c.mocks === 0 && c.traffic === 0);
  return (
    <div className="col">
      <div className="row wrap">
        <span className="strong">{report.title}</span>
        <span className="subtle xs">
          {report.mocksChecked} stub{report.mocksChecked === 1 ? '' : 's'} and {report.trafficChecked} request{report.trafficChecked === 1 ? '' : 's'} checked against {report.operations} operations
          {report.trafficSkipped > 0 && ` · ${report.trafficSkipped} to other hosts skipped`}
        </span>
        <Counts errors={report.errors} warnings={report.warnings} infos={report.infos} />
        <span className="spacer" />
        <SearchInput value={q} onChange={setQ} placeholder="Filter" className="oa-search" />
        <Segmented<KindFilter>
          value={src}
          onChange={setSrc}
          options={[
            { value: 'all', label: 'All' },
            { value: 'mock', label: 'Stubs' },
            { value: 'traffic', label: 'Traffic' },
          ]}
        />
        <Segmented<SevFilter>
          value={sev}
          onChange={setSev}
          options={[
            { value: 'all', label: 'Any' },
            { value: 'error', label: 'Errors' },
            { value: 'warning', label: 'Warnings' },
            { value: 'info', label: 'Info' },
          ]}
        />
      </div>
      {shown.length > 0 && (
        <div className="panel scroll oa-table-wrap">
          <table className="table oa-table">
            <thead>
              <tr>
                <th aria-label="Severity" />
                <th>Source</th>
                <th>Request / stub</th>
                <th>Problem</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {shown.map((f, i) => (
                <DriftRow key={`${f.refId}-${i}`} f={f} onOpen={onOpenStub} />
              ))}
            </tbody>
          </table>
        </div>
      )}
      {!shown.length && report.findings.length > 0 && <div className="subtle small">Nothing matches the filter.</div>}
      <details className="fieldset" open={uncovered.length > 0 && uncovered.length <= 12}>
        <summary>
          Coverage <span className="subtle xs">— {report.coverage.length - uncovered.length} of {report.coverage.length} operations have a stub or traffic</span>
        </summary>
        <div className="fieldset-body">
          <table className="table oa-table">
            <thead>
              <tr>
                <th>Operation</th>
                <th>operationId</th>
                <th className="num">Stubs</th>
                <th className="num">Requests</th>
              </tr>
            </thead>
            <tbody>
              {report.coverage.map((c) => (
                <tr key={c.operation} className={c.mocks === 0 && c.traffic === 0 ? 'oa-uncovered' : undefined}>
                  <td className="mono xs">{c.operation}</td>
                  <td className="mono xs subtle">{c.operationId ?? '—'}</td>
                  <td className="num">{c.mocks || '—'}</td>
                  <td className="num">{c.traffic || '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </details>
    </div>
  );
}

export type { DriftKind };

import clsx from 'clsx';
import { Check, X, HelpCircle } from 'lucide-react';
import type { MatchReport, Verdict } from '@/features/mocks/lib/match';

export function VerdictIcon({ v, size = 14 }: { v: Verdict; size?: number }) {
  if (v === 'match') return <Check size={size} className="text-success" aria-label="matches" />;
  if (v === 'nomatch') return <X size={size} className="text-danger" aria-label="does not match" />;
  return <HelpCircle size={size} className="subtle" aria-label="evaluated on server" />;
}

export function MatchReportView({ report, onlyFailures }: { report: MatchReport; onlyFailures?: boolean }) {
  const fields = onlyFailures ? report.fields.filter((f) => f.verdict !== 'match') : report.fields;
  return (
    <div style={{ border: '1px solid var(--border)', borderRadius: 'var(--radius)', overflow: 'hidden' }}>
      <div className="nm-row" style={{ background: 'var(--bg-subtle)', fontWeight: 600 }}>
        <span />
        <span>Field</span>
        <span>Stub expects</span>
        <span>Request had</span>
      </div>
      {fields.map((f, i) => (
        <div key={i} className={clsx('nm-row', f.verdict === 'nomatch' && 'bad')}>
          <VerdictIcon v={f.verdict} />
          <span className="strong">{f.field}</span>
          <span className="mono">{f.expected}</span>
          <span className="mono">
            {f.actual}
            {f.note && <div className="subtle" style={{ fontFamily: 'var(--font-sans)' }}>{f.note}</div>}
          </span>
        </div>
      ))}
      {fields.length === 0 && <div className="p-4 subtle small">All checked fields match.</div>}
    </div>
  );
}

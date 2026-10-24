import { CheckCircle2, Cloud, Gift } from 'lucide-react';
import { useNav } from '@/store/nav';
import { ROADMAP } from './modules';

export default function RoadmapPage() {
  const section = useNav((s) => s.section);
  const m = ROADMAP.find((x) => x.section === section);
  if (!m) return null;
  return (
    <div className="page">
      <div className="page-header">
        <h1>{m.name}</h1>
        <span className="badge outline">{m.milestone}</span>
        {m.group === 'free' ? (
          <span className="badge success">
            <Gift /> FREE · LOCAL
          </span>
        ) : (
          <span className="badge info">
            <Cloud /> HOSTED · FREE TIER + SUPPORTER
          </span>
        )}
      </div>
      <div className="page-body narrow col-lg" style={{ maxWidth: 760 }}>
        <p className="muted" style={{ fontSize: 'var(--fs-lg)' }}>
          {m.summary}
        </p>
        <div className="panel panel-body col">
          {m.features.map((f) => (
            <div key={f} className="row-sm items-start">
              <CheckCircle2 size={15} className="text-accent shrink-0" style={{ marginTop: 2 }} />
              <span>{f}</span>
            </div>
          ))}
        </div>
        <p className="small">
          <span className="subtle">Free alternative to: </span>
          {m.replaces}
        </p>
        <p className="subtle xs">
          This module is planned — see <code className="code-inline">docs/IMPLEMENTATION_PLAN.md</code>. Everything that runs on your machine stays free, with no account. Only features that need our servers (public tunnel and webhook URLs) have a paid tier, priced near cost.
        </p>
      </div>
    </div>
  );
}

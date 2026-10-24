import { useState } from 'react';
import { GitBranch, RotateCcw, RefreshCw, Plus } from 'lucide-react';
import type { Scenario, StubMapping } from '@/features/mocks/lib/types';
import { useScenarios, useMappings, useWrite, qk } from '@/features/mocks/lib/queries';
import { MethodBadge, Spinner, Empty, SearchInput } from '@/features/mocks/components/ui';
import { methodLabel, stubId, urlOf } from '@/features/mocks/lib/stub-utils';
import { openNewStub, openStub } from '@/features/mocks/stubs/open';

export default function ScenariosPage() {
  const scenarios = useScenarios();
  const write = useWrite();
  const [q, setQ] = useState('');
  const list = (scenarios.data ?? []).filter((s) => !q || s.name.toLowerCase().includes(q.toLowerCase()));

  return (
    <div className="page">
      <div className="page-header">
        <GitBranch size={16} className="subtle" />
        <h1>Scenarios</h1>
        <span className="subtle small">State machines built from stubs with a scenario name. Set or reset the current state to steer tests.</span>
        <div className="spacer" />
        <button className="icon-btn" onClick={() => scenarios.refetch()} title="Refresh">
          {scenarios.isFetching ? <Spinner /> : <RefreshCw />}
        </button>
        <button
          className="btn btn-sm"
          onClick={() =>
            openNewStub({ name: 'Scenario step', scenarioName: 'My scenario', requiredScenarioState: 'Started', newScenarioState: 'Next', request: { method: 'POST', urlPath: '/api/step' }, response: { status: 200 } }, 'New scenario step')
          }
        >
          <Plus /> New scenario stub
        </button>
        <button className="btn btn-sm" onClick={() => write((c) => c.resetScenarios(), { label: 'Reset scenarios', success: 'All scenarios reset to Started', invalidate: (s) => [qk.scenarios(s)] })}>
          <RotateCcw /> Reset all
        </button>
      </div>
      <div className="toolbar">
        <SearchInput value={q} onChange={setQ} placeholder="Filter scenarios" className="grow" inputRef={(el) => el?.setAttribute('data-page-search', '')} />
      </div>
      <div className="scroll p-5 grow">
        {scenarios.isPending && <Spinner />}
        {scenarios.isSuccess && list.length === 0 && (
          <Empty icon={<GitBranch />} title="No scenarios">
            Give stubs a scenario name with required / new states (or use the “Stateful scenario” template) to model flows like “cart empty → item added → checked out”.
          </Empty>
        )}
        <div className="col-lg" style={{ maxWidth: 1180 }}>
          {list.map((s) => (
            <ScenarioCard key={s.id} s={s} />
          ))}
        </div>
      </div>
    </div>
  );
}

function ScenarioCard({ s }: { s: Scenario }) {
  const write = useWrite();
  const mappings = useMappings();
  const stubs = (s.mappings?.length ? s.mappings : (mappings.data ?? []).filter((m) => m.scenarioName === s.name)) as StubMapping[];
  const [target, setTarget] = useState(s.state);
  const setState = (state?: string) =>
    write((c) => c.setScenarioState(s.name, state), { label: 'Set scenario state', success: state ? `"${s.name}" → ${state}` : `"${s.name}" reset`, invalidate: (sid) => [qk.scenarios(sid)] });
  return (
    <div className="panel">
      <div className="panel-header">
        <h3 className="grow">{s.name}</h3>
        <span className="subtle xs">current</span>
        <span className="badge accent">{s.state}</span>
        <select className="select sm" style={{ width: 160 }} value={target} onChange={(e) => setTarget(e.target.value)}>
          {s.possibleStates.map((p) => (
            <option key={p}>{p}</option>
          ))}
        </select>
        <button className="btn btn-sm" disabled={target === s.state} onClick={() => setState(target)}>
          Set state
        </button>
        <button className="btn btn-sm btn-ghost" onClick={() => setState(undefined)} title="Reset this scenario to Started">
          <RotateCcw /> Reset
        </button>
      </div>
      <div className="panel-body col">
        <StateGraph scenario={s} stubs={stubs} onPick={(st) => setState(st)} />
        <table className="table">
          <thead>
            <tr>
              <th>When state is</th>
              <th>Request</th>
              <th>Moves to</th>
            </tr>
          </thead>
          <tbody>
            {stubs.map((m) => (
              <tr key={stubId(m)} className="clickable" onClick={() => {
                const live = mappings.data?.find((x) => stubId(x) === stubId(m));
                if (live) openStub(live);
              }}>
                <td>
                  <span className={`badge ${m.requiredScenarioState === s.state ? 'accent' : 'outline'}`}>{m.requiredScenarioState ?? 'any'}</span>
                </td>
                <td>
                  <div className="row-sm">
                    <MethodBadge method={m.request.method} />
                    <span className="mono xs truncate">{urlOf(m.request)}</span>
                    <span className="subtle xs truncate">{m.name}</span>
                  </div>
                </td>
                <td>{m.newScenarioState ? <span className="badge outline">{m.newScenarioState}</span> : <span className="subtle xs">(stays)</span>}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/** Layered state diagram: columns by distance from "Started". Click a state to jump to it. */
function StateGraph({ scenario, stubs, onPick }: { scenario: Scenario; stubs: StubMapping[]; onPick(state: string): void }) {
  const states = [...new Set(['Started', ...scenario.possibleStates])];
  const edges = stubs
    .filter((m) => m.requiredScenarioState || m.newScenarioState)
    .map((m) => ({ from: m.requiredScenarioState ?? '*', to: m.newScenarioState ?? m.requiredScenarioState ?? '*', label: `${methodLabel(m.request.method)} ${urlOf(m.request)}` }));
  const depth = new Map<string, number>([['Started', 0]]);
  const queue = ['Started'];
  while (queue.length) {
    const cur = queue.shift()!;
    for (const e of edges)
      if (e.from === cur && !depth.has(e.to)) {
        depth.set(e.to, depth.get(cur)! + 1);
        queue.push(e.to);
      }
  }
  let maxD = Math.max(0, ...depth.values());
  for (const s of states) if (!depth.has(s)) depth.set(s, ++maxD);
  const cols = new Map<number, string[]>();
  for (const s of states) cols.set(depth.get(s)!, [...(cols.get(depth.get(s)!) ?? []), s]);
  const W = 250;
  const H = 70;
  const NW = 128;
  const NH = 30;
  const pos = new Map<string, { x: number; y: number }>();
  for (const [d, ss] of cols) ss.forEach((s, i) => pos.set(s, { x: 20 + d * W, y: 40 + i * H }));
  const width = 40 + (maxD + 1) * W;
  const height = 70 + Math.max(...[...cols.values()].map((c) => c.length)) * H;

  return (
    <div className="scenario-graph">
      <svg width={Math.max(width, 300)} height={height + 10} role="img" aria-label={`State diagram for ${scenario.name}`}>
        <defs>
          <marker id={`arrow-${scenario.id}`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--text-subtle)" />
          </marker>
        </defs>
        {edges.map((e, i) => {
          const a = pos.get(e.from);
          const b = pos.get(e.to);
          if (!a || !b) return null;
          if (e.from === e.to) {
            const x = a.x + NW / 2;
            const y = a.y;
            return (
              <g key={i}>
                <path d={`M ${x - 12} ${y} C ${x - 24} ${y - 26}, ${x + 24} ${y - 26}, ${x + 12} ${y}`} fill="none" stroke="var(--border-strong)" markerEnd={`url(#arrow-${scenario.id})`} />
                <text x={x} y={y - 24} fontSize={9.5} fill="var(--text-subtle)" textAnchor="middle">
                  {e.label.length > 30 ? e.label.slice(0, 29) + '…' : e.label}
                </text>
                <title>{e.label}</title>
              </g>
            );
          }
          const x1 = a.x + NW;
          const y1 = a.y + NH / 2;
          const x2 = b.x;
          const y2 = b.y + NH / 2;
          const back = x2 <= x1;
          const d = back ? `M ${a.x + NW / 2} ${a.y + NH} C ${a.x + NW / 2} ${a.y + NH + 40}, ${b.x + NW / 2} ${b.y + NH + 40}, ${b.x + NW / 2} ${b.y + NH}` : `M ${x1} ${y1} C ${(x1 + x2) / 2} ${y1}, ${(x1 + x2) / 2} ${y2}, ${x2} ${y2}`;
          return (
            <g key={i}>
              <path d={d} fill="none" stroke="var(--border-strong)" strokeWidth={1.4} markerEnd={`url(#arrow-${scenario.id})`} />
              <text x={(x1 + x2) / 2} y={(y1 + y2) / 2 - 5} fontSize={9.5} fill="var(--text-subtle)" textAnchor="middle">
                {e.label.length > 30 ? e.label.slice(0, 29) + '…' : e.label}
              </text>
              <title>{e.label}</title>
            </g>
          );
        })}
        {states.map((s) => {
          const p = pos.get(s)!;
          const cur = s === scenario.state;
          return (
            <g key={s} onClick={() => onPick(s)} style={{ cursor: 'pointer' }}>
              <title>{cur ? `${s} (current)` : `Click to set state to ${s}`}</title>
              <rect x={p.x} y={p.y} width={NW} height={NH} rx={8} fill={cur ? 'var(--accent)' : 'var(--bg-elevated)'} stroke={cur ? 'var(--accent)' : 'var(--border-strong)'} />
              <text x={p.x + NW / 2} y={p.y + NH / 2 + 4} fontSize={11.5} fontWeight={600} textAnchor="middle" fill={cur ? 'var(--accent-text)' : 'var(--text)'}>
                {s.length > 18 ? s.slice(0, 17) + '…' : s}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}

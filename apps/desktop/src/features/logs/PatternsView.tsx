import { Fragment, useEffect, useState } from 'react';
import { Shapes } from 'lucide-react';
import { Callout, Empty, Spinner } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { LEVELS, type Filter, type Pattern, type PatternResult } from '@/lib/api/logs';
import type { DataApi } from './source';
import { STACK_ORDER, filterKey, fmtCount, fmtTs, levelVar } from './format';

function Sparkline({ values }: { values: number[] }) {
  const w = 120;
  const h = 22;
  if (!values.length) return <svg width={w} height={h} />;
  const max = Math.max(1, ...values);
  const step = values.length > 1 ? w / (values.length - 1) : w;
  const pts = values.map((v, i) => `${(i * step).toFixed(1)},${(h - 2 - (v / max) * (h - 4)).toFixed(1)}`).join(' ');
  return (
    <svg width={w} height={h} className="sparkline" aria-hidden>
      <polyline points={`0,${h - 2} ${pts} ${w},${h - 2}`} className="area" />
      <polyline points={pts} className="line" />
    </svg>
  );
}

function LevelBar({ p }: { p: Pattern }) {
  const total = p.levels.reduce((a, b) => a + b, 0) || 1;
  return (
    <span className="lvl-bar" title={STACK_ORDER.filter((l) => p.levels[LEVELS.indexOf(l)]).map((l) => `${l}: ${p.levels[LEVELS.indexOf(l)]}`).join(', ')}>
      {STACK_ORDER.map((l) => {
        const c = p.levels[LEVELS.indexOf(l)];
        return c ? <span key={l} style={{ width: `${(c / total) * 100}%`, background: levelVar(l) }} /> : null;
      })}
    </span>
  );
}

/** Highlights placeholders (`<NUM>`, `<*>`, …) in a template. */
function Template({ text }: { text: string }) {
  const parts = text.split(/(<(?:\*|NUM|UUID|IP|HEX|TS|EMAIL|URL|ID)>)/);
  return (
    <span className="tpl">
      {parts.map((s, i) => (i % 2 ? <mark key={i}>{s}</mark> : <Fragment key={i}>{s}</Fragment>))}
    </span>
  );
}

interface Props {
  api: DataApi;
  filter: Filter;
  version: number;
  /** Click: narrow to the pattern; Shift+click: exclude it. */
  onPick(clause: string): void;
}

/** "Logs Drilldown"-style message patterns: masked templates with counts, levels and trend. */
export function PatternsView({ api, filter, version, onPick }: Props) {
  const [res, setRes] = useState<PatternResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const fkey = filterKey(filter);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError(null);
    const t = setTimeout(() => {
      api
        .patterns(filter)
        .then((r) => alive && setRes(r))
        .catch((e) => alive && setError(errorMessage(e)))
        .finally(() => alive && setLoading(false));
    }, 150);
    return () => {
      alive = false;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, fkey, version]);

  if (error) return <Callout kind="danger" className="m-3">{error}</Callout>;
  if (!res)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  if (!res.patterns.length) return <Empty icon={<Shapes />} title="No patterns">Nothing matches the current filter.</Empty>;

  const max = res.patterns[0]?.count ?? 1;
  return (
    <div className="logs-patterns">
      <div className="logs-patterns-head small subtle">
        {fmtCount(res.patterns.length)} patterns from {fmtCount(res.total)} entries ({fmtCount(res.templates)} distinct templates)
        {res.sampled && ' · clustered the most recent 500k matches'}
        {loading && <Spinner />}
        <span className="grow" />
        Click to filter · <kbd className="kbd">Shift</kbd>+click to exclude
      </div>
      <div className="logs-patterns-list" role="list">
        {res.patterns.map((p) => (
          <button
            key={p.id}
            role="listitem"
            className="logs-pattern"
            onClick={(e) => onPick(e.shiftKey ? `NOT ${p.query}` : p.query)}
            title={`Sample: ${p.sample}\nFirst ${fmtTs(p.firstTs, true)} · last ${fmtTs(p.lastTs, true)}`}
          >
            <span className="count">
              <b>{fmtCount(p.count)}</b>
              <span className="share">{(p.share * 100).toFixed(p.share < 0.01 ? 2 : 1)}%</span>
              <span className="share-bar">
                <span style={{ width: `${(p.count / max) * 100}%` }} />
              </span>
            </span>
            <LevelBar p={p} />
            <Sparkline values={p.sparkline} />
            <span className="tpl-wrap">
              <Template text={p.template} />
              <span className="sample subtle truncate">{p.sample}</span>
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

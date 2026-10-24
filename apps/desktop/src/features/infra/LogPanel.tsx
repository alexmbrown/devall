import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { ArrowDownToLine, Clock, Copy, Eraser, History, ScrollText, WrapText } from 'lucide-react';
import { SearchInput } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { uid } from '@/lib/id';
import { infraApi, type LogLine, type LogMsg } from '@/lib/api/infra';
import { ansiRuns, cssFor, stripAnsi } from './ansi';
import { useOpenInLogViewer } from './logViewer';

export interface LogStartOpts {
  follow: boolean;
  /** Last N lines; null = everything. */
  tail: number | null;
  timestamps: boolean;
  previous: boolean;
}

const MAX_KEEP = 20000;
const MAX_SHOW = 4000;
const TAILS: { value: number | null; label: string }[] = [
  { value: 100, label: 'Last 100' },
  { value: 500, label: 'Last 500' },
  { value: 2000, label: 'Last 2000' },
  { value: null, label: 'All' },
];

function shortTs(ts?: string): string {
  if (!ts) return '';
  const t = ts.indexOf('T');
  return t >= 0 ? ts.slice(t + 1, t + 13) : ts;
}

const Row = memo(function Row({ line, wrap }: { line: LogLine; wrap: boolean }) {
  return (
    <div className={clsx('log-row', line.stream === 'stderr' && 'stderr', wrap && 'wrap')}>
      {line.ts && (
        <span className="log-ts" title={line.ts}>
          {shortTs(line.ts)}
        </span>
      )}
      <span className="log-text">
        {ansiRuns(line.text).map((r, i) => (
          <span key={i} style={cssFor(r.style)}>
            {r.text}
          </span>
        ))}
      </span>
    </div>
  );
});

interface LogPanelProps {
  /** Source name (toolbar + log viewer). */
  title: string;
  /** The stream restarts when this changes (different container, pod, …). */
  sourceKey: string;
  start(streamId: string, opts: LogStartOpts, onMsg: (m: LogMsg) => void): Promise<void>;
  /** Kubernetes: offer "previous container". */
  allowPrevious?: boolean;
  /** Extra toolbar controls (container picker, …). */
  extra?: ReactNode;
}

/** Streaming log view with follow, tail, timestamps, search, and "Open in log viewer". */
export function LogPanel({ title, sourceKey, start, allowPrevious, extra }: LogPanelProps) {
  const [follow, setFollow] = useState(true);
  const [tail, setTail] = useState<number | null>(500);
  const [timestamps, setTimestamps] = useState(false);
  const [previous, setPrevious] = useState(false);
  const [wrap, setWrap] = useState(true);
  const [query, setQuery] = useState('');
  const [ended, setEnded] = useState<{ error: string | null } | null>(null);
  const [version, setVersion] = useState(0);
  const lines = useRef<LogLine[]>([]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const openInViewer = useOpenInLogViewer();
  const startRef = useRef(start);
  startRef.current = start;

  useEffect(() => {
    const id = `logs-${uid()}`;
    lines.current = [];
    setEnded(null);
    setVersion((v) => v + 1);
    atBottom.current = true;
    let live = true;
    let scheduled = false;
    const onMsg = (m: LogMsg) => {
      if (!live) return;
      if (m.type === 'lines') {
        if (m.lines.length === 0) return;
        const all = lines.current;
        all.push(...m.lines);
        if (all.length > MAX_KEEP) all.splice(0, all.length - MAX_KEEP);
      } else setEnded({ error: m.error });
      if (!scheduled) {
        scheduled = true;
        requestAnimationFrame(() => {
          scheduled = false;
          if (live) setVersion((v) => v + 1);
        });
      }
    };
    startRef.current(id, { follow, tail, timestamps, previous }, onMsg).catch((e) => live && setEnded({ error: errorMessage(e) }));
    return () => {
      live = false;
      void infraApi.streamStop(id);
    };
  }, [sourceKey, follow, tail, timestamps, previous]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    const all = lines.current;
    const filtered = q ? all.filter((l) => stripAnsi(l.text).toLowerCase().includes(q)) : all;
    return { total: filtered.length, rows: filtered.length > MAX_SHOW ? filtered.slice(filtered.length - MAX_SHOW) : filtered };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, version]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && atBottom.current) el.scrollTop = el.scrollHeight;
  }, [shown]);

  const plain = () => lines.current.map((l) => `${l.ts ? `${l.ts} ` : ''}${stripAnsi(l.text)}`).join('\n');

  return (
    <div className="log-panel">
      <div className="toolbar">
        {extra}
        <select className="select sm" value={tail === null ? 'all' : String(tail)} onChange={(e) => setTail(e.target.value === 'all' ? null : Number(e.target.value))} aria-label="How many lines">
          {TAILS.map((t) => (
            <option key={t.label} value={t.value === null ? 'all' : String(t.value)}>
              {t.label}
            </option>
          ))}
        </select>
        <button className={clsx('btn btn-sm', follow && 'btn-subtle')} onClick={() => setFollow(!follow)} title="Keep streaming new lines" aria-pressed={follow}>
          <ArrowDownToLine /> Follow
        </button>
        <button className={clsx('icon-btn sm', timestamps && 'active')} onClick={() => setTimestamps(!timestamps)} title="Timestamps" aria-pressed={timestamps}>
          <Clock />
        </button>
        {allowPrevious && (
          <button className={clsx('icon-btn sm', previous && 'active')} onClick={() => setPrevious(!previous)} title="Previous container instance (after a crash)" aria-pressed={previous}>
            <History />
          </button>
        )}
        <button className={clsx('icon-btn sm', wrap && 'active')} onClick={() => setWrap(!wrap)} title="Wrap lines" aria-pressed={wrap}>
          <WrapText />
        </button>
        <SearchInput value={query} onChange={setQuery} placeholder="Filter lines…" className="log-search" />
        <span className="grow" />
        <span className="xs subtle nowrap">
          {query ? `${shown.total} / ${lines.current.length}` : lines.current.length} lines
          {ended && (ended.error ? ' · failed' : follow ? ' · stream ended' : '')}
        </span>
        <button className="icon-btn sm" title="Clear" aria-label="Clear" onClick={() => ((lines.current = []), setVersion((v) => v + 1))}>
          <Eraser />
        </button>
        <button
          className="icon-btn sm"
          title="Copy"
          aria-label="Copy"
          onClick={() => navigator.clipboard.writeText(plain()).then(() => toast.success('Logs copied'), () => toast.error('Clipboard unavailable'))}
        >
          <Copy />
        </button>
        <button className="btn btn-sm" title="Open these lines in the log viewer (search, SQL, patterns)" onClick={() => openInViewer({ name: title, text: plain() })} disabled={lines.current.length === 0}>
          <ScrollText /> Open in log viewer
        </button>
      </div>
      {ended?.error && <div className="log-error small">{ended.error}</div>}
      <div
        ref={scrollRef}
        className="log-body"
        onScroll={(e) => {
          const el = e.currentTarget;
          atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
      >
        {shown.total > shown.rows.length && <div className="xs subtle log-more">Showing the last {MAX_SHOW} of {shown.total} lines — open in the log viewer for everything.</div>}
        {shown.rows.map((l, i) => (
          <Row key={shown.total - shown.rows.length + i} line={l} wrap={wrap} />
        ))}
        {lines.current.length === 0 && !ended && <div className="subtle small log-empty">Waiting for log lines…</div>}
        {lines.current.length === 0 && ended && !ended.error && <div className="subtle small log-empty">No log lines.</div>}
      </div>
    </div>
  );
}

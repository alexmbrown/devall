import { forwardRef, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent } from 'react';
import clsx from 'clsx';
import { ChevronDown, ChevronRight, ExternalLink, Link2 } from 'lucide-react';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { fieldClause, type Filter, type LogEntry } from '@/lib/api/logs';
import type { DataApi } from './source';
import { openRequest } from './DetailDrawer';
import { LEVEL_LABEL, chipFields, filterKey, firstLine, fmtFull, fmtTs, pretty, valueText } from './format';

const PAGE = 200;
const ROW_H = 22;
const EXP_H = 220;
/** Browsers cap element heights (~33M px in Chromium); scale the scrollbar beyond this. */
const MAX_PX = 8_000_000;

export interface LogTableHandle {
  focus(): void;
  scrollToRow(row: number): void;
  scrollToEnd(): void;
}

interface Props {
  api: DataApi;
  filter: Filter;
  /** Bumped when the underlying data changed (indexing, tail). */
  version: number;
  follow: boolean;
  /** Show the source column (merged files, events). */
  sourceColumn: boolean;
  /** Show dates in the time column (data spans several days). */
  withDate: boolean;
  selected: number | null;
  onSelect(row: number, entry: LogEntry | undefined): void;
  onOpen(entry: LogEntry): void;
  onAddClause(clause: string): void;
  onTotals(total: number, viewTotal: number, indexing: boolean, error?: string): void;
  /** The user scrolled away from the end while following. */
  onLeaveEnd(): void;
}

interface Store {
  gen: number;
  pages: Map<number, LogEntry[]>;
  stale: Map<number, LogEntry[]>;
  inflight: Set<number>;
}

export const LogTable = forwardRef<LogTableHandle, Props>(function LogTable(
  { api, filter, version, follow, sourceColumn, withDate, selected, onSelect, onOpen, onAddClause, onTotals, onLeaveEnd },
  ref,
) {
  const scroller = useRef<HTMLDivElement>(null);
  const store = useRef<Store>({ gen: 0, pages: new Map(), stale: new Map(), inflight: new Set() });
  const [total, setTotal] = useState(0);
  const [, setTick] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(400);
  const [expanded, setExpanded] = useState<number[]>([]);
  const programmatic = useRef(false);
  const fkey = filterKey(filter);
  const followRef = useRef(follow);
  followRef.current = follow;

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    setViewH(el.clientHeight);
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // ---------- geometry ----------
  const exp = useMemo(() => expanded.filter((r) => r < total).sort((a, b) => a - b), [expanded, total]);
  const contentH = total * ROW_H + exp.length * EXP_H;
  const virtH = Math.min(contentH, MAX_PX);
  const scale = contentH > virtH && virtH > viewH ? (contentH - viewH) / (virtH - viewH) : 1;
  const expBefore = useCallback(
    (row: number) => {
      let lo = 0;
      let hi = exp.length;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (exp[mid] < row) lo = mid + 1;
        else hi = mid;
      }
      return lo;
    },
    [exp],
  );
  const rowTop = useCallback((row: number) => row * ROW_H + expBefore(row) * EXP_H, [expBefore]);
  const rowAt = useCallback(
    (y: number) => {
      let lo = 0;
      let hi = Math.max(0, total - 1);
      while (lo < hi) {
        const mid = (lo + hi + 1) >> 1;
        if (rowTop(mid) <= y) lo = mid;
        else hi = mid - 1;
      }
      return lo;
    },
    [rowTop, total],
  );
  const virtTop = scrollTop * scale;
  const first = total ? rowAt(virtTop) : 0;
  const visible: number[] = [];
  for (let r = Math.max(0, first - 5); r < total && rowTop(r) < virtTop + viewH + ROW_H * 5; r++) visible.push(r);

  // ---------- data ----------
  const fetchPage = useCallback(
    (page: number) => {
      const s = store.current;
      if (s.pages.has(page) || s.inflight.has(page)) return;
      s.inflight.add(page);
      const gen = s.gen;
      api
        .query(filter, page * PAGE, PAGE)
        .then((p) => {
          if (store.current.gen !== gen) return;
          store.current.pages.set(page, p.entries);
          store.current.inflight.delete(page);
          setTotal(p.total);
          onTotals(p.total, p.viewTotal, p.indexing);
          setTick((t) => t + 1);
        })
        .catch((e) => {
          if (store.current.gen !== gen) return;
          store.current.inflight.delete(page);
          onTotals(0, 0, false, errorMessage(e));
        });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [api, fkey],
  );

  // New data generation: keep showing the old pages until the new ones arrive.
  useEffect(() => {
    const s = store.current;
    s.stale = s.pages.size ? s.pages : s.stale;
    s.pages = new Map();
    s.inflight = new Set();
    s.gen += 1;
    const gen = s.gen;
    api
      .query(filter, 0, PAGE)
      .then((p) => {
        if (store.current.gen !== gen) return;
        store.current.pages.set(0, p.entries);
        setTotal(p.total);
        onTotals(p.total, p.viewTotal, p.indexing);
        if (followRef.current && p.total > PAGE) fetchPage(Math.floor((p.total - 1) / PAGE));
        setTick((t) => t + 1);
      })
      .catch((e) => {
        if (store.current.gen !== gen) return;
        setTotal(0);
        onTotals(0, 0, false, errorMessage(e));
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, fkey, version]);

  // Filter change: back to the top, collapse rows.
  useEffect(() => {
    store.current.stale = new Map();
    setExpanded([]);
    if (scroller.current && scroller.current.scrollTop !== 0) {
      programmatic.current = true;
      scroller.current.scrollTop = 0;
    }
    setScrollTop(0);
  }, [api.key, fkey]);

  // Load what's visible.
  useEffect(() => {
    if (!total) return;
    const a = Math.floor((visible[0] ?? 0) / PAGE);
    const b = Math.floor((visible[visible.length - 1] ?? 0) / PAGE);
    for (let p = a; p <= b; p++) fetchPage(p);
  });

  const entryAt = (row: number): LogEntry | undefined => {
    const p = Math.floor(row / PAGE);
    const s = store.current;
    return (s.pages.get(p) ?? s.stale.get(p))?.[row % PAGE];
  };

  // ---------- scrolling ----------
  const scrollToRow = useCallback(
    (row: number, align: 'start' | 'nearest' | 'third' = 'nearest') => {
      const el = scroller.current;
      if (!el) return;
      const top = rowTop(row) / scale;
      const h = ROW_H / scale;
      let next = el.scrollTop;
      if (align === 'start') next = top;
      else if (align === 'third') next = Math.max(0, top - el.clientHeight / 3);
      else if (top < el.scrollTop) next = top;
      else if (top + h > el.scrollTop + el.clientHeight) next = top + h - el.clientHeight;
      if (next !== el.scrollTop) {
        programmatic.current = true;
        el.scrollTop = next;
      }
    },
    [rowTop, scale],
  );

  const scrollToEnd = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    const target = el.scrollHeight - el.clientHeight;
    if (Math.abs(el.scrollTop - target) < 1) return;
    programmatic.current = true;
    el.scrollTop = target;
  }, []);

  useImperativeHandle(ref, () => ({ focus: () => scroller.current?.focus(), scrollToRow: (r) => scrollToRow(r, 'third'), scrollToEnd }), [scrollToRow, scrollToEnd]);

  // Following: stick to the end as rows arrive.
  useEffect(() => {
    if (follow && total) requestAnimationFrame(scrollToEnd);
  }, [follow, total, version, scrollToEnd]);

  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    if (programmatic.current) {
      programmatic.current = false;
      return;
    }
    if (followRef.current && el.scrollHeight - el.scrollTop - el.clientHeight > ROW_H * 2) onLeaveEnd();
  };

  // ---------- keyboard ----------
  const move = (row: number) => {
    if (!total) return;
    const r = Math.min(total - 1, Math.max(0, row));
    onSelect(r, entryAt(r));
    scrollToRow(r);
  };
  const toggle = (row: number) => setExpanded((x) => (x.includes(row) ? x.filter((r) => r !== row) : [...x, row]));

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const cur = selected ?? -1;
    const pageRows = Math.max(1, Math.floor(viewH / ROW_H) - 1);
    switch (e.key) {
      case 'ArrowDown':
      case 'j':
        move(cur + 1);
        break;
      case 'ArrowUp':
      case 'k':
        move(cur < 0 ? first : cur - 1);
        break;
      case 'PageDown':
        move(cur + pageRows);
        break;
      case 'PageUp':
        move(cur - pageRows);
        break;
      case 'Home':
        move(0);
        break;
      case 'End':
        move(total - 1);
        break;
      case 'Enter': {
        const en = cur >= 0 ? entryAt(cur) : undefined;
        if (en) onOpen(en);
        break;
      }
      case ' ':
        if (cur >= 0) toggle(cur);
        break;
      case 'c':
        if (cur >= 0 && !e.ctrlKey && !e.metaKey) {
          const en = entryAt(cur);
          if (en) navigator.clipboard.writeText(en.raw).then(() => toast.success('Copied raw line'), () => toast.error('Clipboard unavailable'));
        }
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const chip = (e: MouseEvent, clause: string) => {
    e.stopPropagation();
    onAddClause(e.shiftKey || e.altKey ? `-${clause}` : clause);
  };

  return (
    <div className="logs-table" role="grid" aria-rowcount={total}>
      <div className={clsx('logs-row logs-head', sourceColumn && 'with-source')} role="row">
        <span />
        <span role="columnheader">Time</span>
        <span role="columnheader">Level</span>
        {sourceColumn && <span role="columnheader">{api.isEvents ? 'Kind' : 'Source'}</span>}
        <span role="columnheader">Message</span>
      </div>
      <div ref={scroller} className="logs-scroll" tabIndex={0} onScroll={onScroll} onKeyDown={onKey} aria-label="Log entries">
        <div style={{ height: virtH, position: 'relative' }}>
          {visible.map((row) => {
            const e = entryAt(row);
            const top = scrollTop + (rowTop(row) - virtTop);
            const isExp = exp.includes(row);
            const [line, more] = e ? firstLine(e.message) : ['', 0];
            return (
              <div key={row} style={{ position: 'absolute', top, left: 0, right: 0 }}>
                <div
                  className={clsx('logs-row', sourceColumn && 'with-source', `lvl-${e?.level ?? 'unknown'}`, selected === row && 'on')}
                  role="row"
                  aria-selected={selected === row}
                  onClick={() => onSelect(row, e)}
                  onDoubleClick={() => e && onOpen(e)}
                >
                  <button className="exp" tabIndex={-1} aria-label={isExp ? 'Collapse' : 'Expand'} onClick={(ev) => (ev.stopPropagation(), toggle(row))}>
                    {isExp ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                  </button>
                  <span className="ts" title={e ? fmtFull(e.ts) : undefined}>
                    {e ? fmtTs(e.ts, withDate) : ''}
                  </span>
                  <span className="lvl">{e ? (e.level === 'unknown' ? '' : LEVEL_LABEL[e.level].toUpperCase()) : ''}</span>
                  {sourceColumn && (
                    <span className="src truncate" title={e?.source}>
                      {api.isEvents ? String(e?.fields.kind ?? '') : (e?.source ?? '')}
                    </span>
                  )}
                  <span className="msg">
                    {e ? (
                      <>
                        <span className="text">{line}</span>
                        {more > 0 && <span className="more">+{more} lines</span>}
                        {api.isEvents && typeof e.fields.requestPath === 'string' && (
                          <button
                            className="chip link"
                            tabIndex={-1}
                            title={`Open request ${e.fields.requestPath}`}
                            onClick={(ev) => {
                              ev.stopPropagation();
                              openRequest(e.fields.requestPath as string);
                            }}
                          >
                            <ExternalLink size={10} />
                            {String(e.fields.requestPath).replace(/\.req\.yaml$/, '')}
                          </button>
                        )}
                        {e.correlationId && (
                          <button className="chip corr" tabIndex={-1} title="Filter by correlation id (Shift: exclude)" onClick={(ev) => chip(ev, fieldClause('trace', e.correlationId))}>
                            <Link2 size={10} />
                            {e.correlationId.length > 14 ? `${e.correlationId.slice(0, 12)}…` : e.correlationId}
                          </button>
                        )}
                        {chipFields(e).map(([k, v]) => (
                          <button key={k} className="chip" tabIndex={-1} title={`Filter ${k}=${valueText(v)} (Shift: exclude)`} onClick={(ev) => chip(ev, fieldClause(k, v))}>
                            <span className="k">{k}</span>
                            <span className="v">{valueText(v)}</span>
                          </button>
                        ))}
                      </>
                    ) : (
                      <span className="loading-bar" />
                    )}
                  </span>
                </div>
                {isExp && (
                  <div className="logs-expanded" style={{ height: EXP_H }} onClick={(ev) => ev.stopPropagation()}>
                    <pre>{e ? pretty(e) : '…'}</pre>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
});

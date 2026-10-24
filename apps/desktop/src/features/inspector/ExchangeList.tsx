import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { AlertTriangle, ArrowRightLeft, FileCode2, FileWarning, Lock, Pause, Pencil, Radio, RotateCw, ShieldAlert, Timer, Wand2 } from 'lucide-react';
import { MethodBadge, StatusCode, formatBytes } from '@/components/ui/misc';
import type { ExchangeId, ExchangeSummary } from '@/lib/api/proxy';
import { KIND_LABEL, kindOf } from './filter';
import { fmtClock, fmtMs } from './format';
import { useInspector } from './store';

export const ROW_H = 26;
const OVERSCAN = 10;

function StatusCell({ x }: { x: ExchangeSummary }) {
  if (x.paused) return <span className="insp-state warn">PAUSE</span>;
  if (x.state === 'tunnel') return <span className="insp-state">TUN</span>;
  if (x.state === 'failed' && x.status == null) return <span className="insp-state err">ERR</span>;
  if (x.state === 'aborted') return <span className="insp-state err">ABRT</span>;
  if (x.state === 'blocked' && x.status == null) return <span className="insp-state err">BLK</span>;
  if (x.status == null) return <span className="insp-state">…</span>;
  return <StatusCode status={x.status} />;
}

function Markers({ x }: { x: ExchangeSummary }) {
  const t = (tag: string) => x.tags.includes(tag);
  return (
    <span className="insp-markers">
      {x.paused && <Pause size={12} className="text-warning" aria-label="Paused at a breakpoint" />}
      {x.state === 'failed' && (t('tlsRejected') ? <ShieldAlert size={12} className="text-danger" aria-label="TLS rejected" /> : <AlertTriangle size={12} className="text-danger" aria-label="Failed" />)}
      {t('edited') && <Pencil size={12} className="text-accent" aria-label="Edited at a breakpoint" />}
      {(t('mocked') || t('mapLocal')) && <FileCode2 size={12} className="text-accent" aria-label="Answered by a rule" />}
      {t('mapRemote') && <ArrowRightLeft size={12} className="text-accent" aria-label="Map Remote" />}
      {t('throttled') && <Timer size={12} className="subtle" aria-label="Throttled" />}
      {t('replay') && <RotateCw size={12} className="subtle" aria-label="Replay" />}
      {x.violations > 0 && (
        <span className="insp-spec-badge" title={`${x.violations} OpenAPI violation${x.violations === 1 ? '' : 's'}`}>
          <FileWarning size={12} /> {x.violations}
        </span>
      )}
      {x.state === 'tunnel' && <Lock size={12} className="subtle" aria-label="Tunnelled (not decrypted)" />}
      {(x.state === 'webSocket' || x.wsFrames > 0) && <Radio size={12} className="subtle" aria-label="WebSocket" />}
      {x.rules.length > 0 && !t('edited') && !t('mocked') && !t('mapLocal') && !t('mapRemote') && <Wand2 size={12} className="subtle" aria-label={`Rules: ${x.rules.join(', ')}`} />}
    </span>
  );
}

const Row = memo(function Row({ x, top, selected, focused, onPick }: { x: ExchangeSummary; top: number; selected: boolean; focused: boolean; onPick(id: ExchangeId, e: React.MouseEvent): void }) {
  const kind = kindOf(x);
  const size = x.responseSize ?? (x.state === 'tunnel' ? x.requestSize : null);
  return (
    <div
      className={clsx('insp-row', selected && 'on', focused && 'focus', x.state === 'failed' && 'failed', x.paused && 'paused')}
      style={{ top, height: ROW_H }}
      role="row"
      aria-selected={selected}
      onMouseDown={(e) => onPick(x.id, e)}
      title={x.error ?? x.url}
    >
      <Markers x={x} />
      <MethodBadge method={x.method} short />
      <span className="insp-status">
        <StatusCell x={x} />
      </span>
      <span className="truncate insp-host">{x.host}</span>
      <span className="truncate mono insp-path">{x.path || x.url}</span>
      <span className="subtle insp-type">{x.state === 'tunnel' ? '' : KIND_LABEL[kind]}</span>
      <span className="subtle num">{size != null ? formatBytes(size) : ''}</span>
      <span className="subtle num" title={fmtClock(x.startedAt)}>
        {fmtMs(x.durationMs)}
      </span>
    </div>
  );
});

/** Windowed exchange list: only rows in view (plus overscan) are rendered. Follows the tail
 * while scrolled to the bottom. */
export function ExchangeList({ ids, onKeyDown, listRef }: { ids: ExchangeId[]; onKeyDown(e: React.KeyboardEvent): void; listRef: React.RefObject<HTMLDivElement | null> }) {
  const items = useInspector((s) => s.items);
  useInspector((s) => s.version);
  const selected = useInspector((s) => s.selected);
  const focus = useInspector((s) => s.focus);
  const select = useInspector((s) => s.select);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(400);
  const follow = useRef(true);

  useLayoutEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    setHeight(el.clientHeight);
    return () => ro.disconnect();
  }, [listRef]);

  useLayoutEffect(() => {
    const el = listRef.current;
    if (el && follow.current) el.scrollTop = el.scrollHeight;
  }, [ids.length, listRef]);

  // Keep the focused row in view when it moves by keyboard.
  useEffect(() => {
    const el = listRef.current;
    if (!el || focus == null) return;
    const i = ids.indexOf(focus);
    if (i < 0) return;
    const top = i * ROW_H;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW_H > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW_H - el.clientHeight;
  }, [focus, ids, listRef]);

  const onPick = useCallback(
    (id: ExchangeId, e: React.MouseEvent) => {
      if (e.button !== 0) return;
      select(id, e.shiftKey ? 'range' : e.ctrlKey || e.metaKey ? 'toggle' : 'replace');
      listRef.current?.focus({ preventScroll: true });
    },
    [select, listRef],
  );

  const sel = new Set(selected);
  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(ids.length, Math.ceil((scrollTop + height) / ROW_H) + OVERSCAN);
  const rows = [];
  for (let i = start; i < end; i++) {
    const x = items.get(ids[i]);
    if (x) rows.push(<Row key={x.id} x={x} top={i * ROW_H} selected={sel.has(x.id)} focused={focus === x.id} onPick={onPick} />);
  }

  return (
    <div className="insp-list">
      <div className="insp-row insp-head" role="row">
        <span />
        <span>Method</span>
        <span>Status</span>
        <span>Host</span>
        <span>Path</span>
        <span>Type</span>
        <span className="num">Size</span>
        <span className="num">Time</span>
      </div>
      <div
        ref={listRef}
        className="insp-rows"
        tabIndex={0}
        role="grid"
        aria-label="Captured exchanges"
        aria-rowcount={ids.length}
        onKeyDown={onKeyDown}
        onScroll={(e) => {
          const el = e.currentTarget;
          setScrollTop(el.scrollTop);
          follow.current = el.scrollTop + el.clientHeight >= el.scrollHeight - ROW_H * 1.5;
        }}
      >
        <div style={{ height: ids.length * ROW_H, position: 'relative' }}>{rows}</div>
      </div>
    </div>
  );
}

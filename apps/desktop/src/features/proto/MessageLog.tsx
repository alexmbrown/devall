import { useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import clsx from 'clsx';
import { AlertTriangle, ArrowDownLeft, ArrowUpRight, Info, Link2, Link2Off, ListTree, Plug, Radio, Trash2 } from 'lucide-react';
import { Checkbox, CopyButton, Empty, SearchInput, Segmented, formatBytes, formatTime } from '@/components/ui/misc';
import type { ProtoEvent } from '@/lib/api/proto';
import { hexDump, prettyBody } from './format';
import type { LogEntry } from './store';

/** Rendering more rows than this makes the log sluggish; older ones stay in memory (and in Copy). */
const MAX_RENDER = 1500;

type DirFilter = 'all' | 'in' | 'out';

function isMessage(e: ProtoEvent): e is Extract<ProtoEvent, { type: 'message' }> {
  return e.type === 'message';
}

function systemText(e: ProtoEvent): string {
  switch (e.type) {
    case 'connecting':
      return `Connecting to ${e.target}`;
    case 'opened':
      return `Connected${e.detail ? ` · ${e.detail}` : ''}`;
    case 'metadata':
      return `${e.trailers ? 'Trailers' : 'Response headers'} (${e.entries.length})`;
    case 'info':
    case 'error':
      return e.message;
    case 'closed':
      return `Closed${e.code !== undefined ? ` · ${e.code}` : ''}${e.reason ? ` · ${e.reason}` : ''}`;
    default:
      return '';
  }
}

function SystemIcon({ e }: { e: ProtoEvent }) {
  switch (e.type) {
    case 'connecting':
      return <Plug className="plog-dir" />;
    case 'opened':
      return <Link2 className="plog-dir" />;
    case 'closed':
      return <Link2Off className="plog-dir" />;
    case 'metadata':
      return <ListTree className="plog-dir" />;
    case 'error':
      return <AlertTriangle className="plog-dir" />;
    default:
      return <Info className="plog-dir" />;
  }
}

function entryText(e: ProtoEvent): string {
  if (isMessage(e)) return `${new Date(e.at).toISOString()} ${e.direction === 'in' ? '<-' : '->'}${e.label ? ` [${e.label}]` : ''} ${e.binary ? `(base64) ${e.body}` : e.body}`;
  if (e.type === 'metadata') return `${new Date(e.at).toISOString()} ${systemText(e)}\n${e.entries.map(([k, v]) => `  ${k}: ${v}`).join('\n')}`;
  return `${new Date(e.at).toISOString()} -- ${systemText(e)}`;
}

interface Props {
  entries: LogEntry[];
  onClear(): void;
  /** Shown when the log is empty. */
  empty?: ReactNode;
  /** Above the toolbar (e.g. gRPC status). */
  header?: ReactNode;
}

/**
 * Live message log: direction arrows, timestamps, sizes, JSON pretty-print on expand, filter,
 * clear and copy. Arrow keys move between rows; Enter/Space expands.
 */
export function MessageLog({ entries, onClear, empty, header }: Props) {
  const [filter, setFilter] = useState('');
  const [dir, setDir] = useState<DirFilter>('all');
  const [system, setSystem] = useState(true);
  const [open, setOpen] = useState<Set<number>>(() => new Set());
  const [stick, setStick] = useState(true);
  const listRef = useRef<HTMLDivElement>(null);

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return entries.filter(({ event: e }) => {
      if (isMessage(e)) {
        if (dir !== 'all' && e.direction !== dir) return false;
        return !q || e.body.toLowerCase().includes(q) || (e.label ?? '').toLowerCase().includes(q);
      }
      if (!system || dir !== 'all') return e.type === 'error' && dir === 'all';
      return !q || systemText(e).toLowerCase().includes(q);
    });
  }, [entries, filter, dir, system]);
  const shown = filtered.length > MAX_RENDER ? filtered.slice(-MAX_RENDER) : filtered;

  const stats = useMemo(() => {
    let inN = 0;
    let outN = 0;
    let bytes = 0;
    for (const { event: e } of entries) {
      if (!isMessage(e)) continue;
      if (e.direction === 'in') inN++;
      else outN++;
      bytes += e.size;
    }
    return { inN, outN, bytes };
  }, [entries]);

  useLayoutEffect(() => {
    const el = listRef.current;
    if (stick && el) el.scrollTop = el.scrollHeight;
  }, [shown.length, stick]);

  const toggle = (seq: number) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(seq)) n.delete(seq);
      else n.add(seq);
      return n;
    });

  const onKeyDown = (ev: KeyboardEvent<HTMLDivElement>) => {
    if (ev.key !== 'ArrowDown' && ev.key !== 'ArrowUp' && ev.key !== 'Home' && ev.key !== 'End') return;
    const rows = Array.from(listRef.current?.querySelectorAll<HTMLButtonElement>('.plog-main') ?? []);
    if (!rows.length) return;
    ev.preventDefault();
    const i = rows.indexOf(document.activeElement as HTMLButtonElement);
    const next = ev.key === 'Home' ? 0 : ev.key === 'End' ? rows.length - 1 : Math.max(0, Math.min(rows.length - 1, i + (ev.key === 'ArrowDown' ? 1 : -1)));
    rows[next]?.focus();
    rows[next]?.scrollIntoView({ block: 'nearest' });
  };

  return (
    <div className="plog">
      {header}
      <div className="toolbar">
        <SearchInput value={filter} onChange={setFilter} placeholder="Filter messages" className="grow" />
        <Segmented
          value={dir}
          onChange={setDir}
          options={[
            { value: 'all', label: 'All' },
            { value: 'in', label: <ArrowDownLeft size={13} />, title: 'Received' },
            { value: 'out', label: <ArrowUpRight size={13} />, title: 'Sent' },
          ]}
        />
        <Checkbox checked={system} onChange={setSystem} label="Events" title="Show connection events, info and metadata" />
        <span className="subtle xs nowrap" title={`${formatBytes(stats.bytes)} in messages`}>
          ↓{stats.inN} ↑{stats.outN}
        </span>
        <CopyButton text={() => filtered.map((x) => entryText(x.event)).join('\n')} title="Copy the (filtered) log" />
        <button className="icon-btn sm" onClick={onClear} title="Clear the log" aria-label="Clear the log" disabled={!entries.length}>
          <Trash2 />
        </button>
      </div>
      {entries.length === 0 ? (
        <div className="grow col" style={{ justifyContent: 'center' }}>
          {empty ?? <Empty icon={<Radio />} title="No messages yet" />}
        </div>
      ) : (
        <div
          ref={listRef}
          className="plog-list"
          onKeyDown={onKeyDown}
          onScroll={(e) => {
            const el = e.currentTarget;
            setStick(el.scrollTop + el.clientHeight >= el.scrollHeight - 24);
          }}
          role="log"
          aria-live="polite"
        >
          {filtered.length > shown.length && (
            <div className="plog-more">
              Showing the latest {shown.length} of {filtered.length} entries — narrow the filter or copy the log to see older ones.
            </div>
          )}
          {shown.length === 0 && <div className="plog-more">Nothing matches the filter.</div>}
          {shown.map(({ seq, event: e }) => (
            <Row key={seq} e={e} open={open.has(seq)} onToggle={() => toggle(seq)} />
          ))}
        </div>
      )}
    </div>
  );
}

function Row({ e, open, onToggle }: { e: ProtoEvent; open: boolean; onToggle(): void }) {
  if (isMessage(e)) {
    const preview = e.binary ? `binary · ${e.body.slice(0, 80)}` : e.body.replace(/\s+/g, ' ').slice(0, 300);
    return (
      <div className={clsx('plog-row', e.direction, open && 'open')}>
        <button className="plog-main" onClick={onToggle} aria-expanded={open} title={new Date(e.at).toLocaleString()}>
          {e.direction === 'in' ? <ArrowDownLeft className="plog-dir" aria-label="received" /> : <ArrowUpRight className="plog-dir" aria-label="sent" />}
          <span className="plog-time">{formatTime(e.at)}</span>
          {e.label && <span className="badge outline plog-label mono">{e.label}</span>}
          <span className="plog-preview">{preview || <span className="subtle">(empty)</span>}</span>
          <span className="plog-size">{formatBytes(e.size)}</span>
        </button>
        {open && <MessageDetail e={e} />}
      </div>
    );
  }
  const expandable = e.type === 'metadata' || (e.type === 'opened' && !!e.headers?.length);
  return (
    <div className={clsx('plog-row system', e.type === 'error' && 'error', open && 'open')}>
      <button className="plog-main" onClick={expandable ? onToggle : undefined} aria-expanded={expandable ? open : undefined}>
        <SystemIcon e={e} />
        <span className="plog-time">{formatTime(e.at)}</span>
        <span className="plog-preview" style={{ fontFamily: 'var(--font-sans)' }} title={systemText(e)}>
          {systemText(e)}
        </span>
      </button>
      {open && expandable && (
        <div className="plog-detail">
          <table className="table">
            <tbody>
              {(e.type === 'metadata' ? e.entries : e.type === 'opened' ? (e.headers ?? []) : []).map(([k, v], i) => (
                <tr key={i}>
                  <td className="mono xs strong nowrap" style={{ width: '30%' }}>
                    {k}
                  </td>
                  <td className="mono xs break">{v}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function MessageDetail({ e }: { e: Extract<ProtoEvent, { type: 'message' }> }) {
  const [view, setView] = useState<'pretty' | 'raw' | 'hex'>(e.binary ? 'hex' : 'pretty');
  const pretty = useMemo(() => (e.binary ? null : prettyBody(e.body)), [e]);
  const text = view === 'hex' ? hexDump(e.body) : view === 'pretty' && pretty ? pretty.text : e.body;
  const meta = Object.entries(e.meta ?? {}).filter(([, v]) => v !== false && v !== null && v !== undefined);
  return (
    <div className="plog-detail">
      <div className="row-sm">
        <Segmented
          value={view}
          onChange={setView}
          options={
            e.binary
              ? [
                  { value: 'hex', label: 'Hex' },
                  { value: 'raw', label: 'Base64' },
                ]
              : [
                  { value: 'pretty', label: pretty?.json ? 'Pretty' : 'Text' },
                  { value: 'raw', label: 'Raw' },
                ]
          }
        />
        {meta.map(([k, v]) => (
          <span key={k} className="badge outline mono" title={k}>
            {k}: {typeof v === 'object' ? JSON.stringify(v) : String(v)}
          </span>
        ))}
        <span className="spacer" />
        <span className="subtle xs">{new Date(e.at).toLocaleString()}</span>
        <CopyButton text={e.body} label="Copy" />
      </div>
      <pre className="code">{text}</pre>
    </div>
  );
}

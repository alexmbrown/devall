import { useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@/features/mocks/lib/virtual';
import clsx from 'clsx';
import { Pause, Play, Eraser, Download, RefreshCw, ScrollText, Clock, X, Activity } from 'lucide-react';
import { useRequests, useJournalSince, useWrite, useWm, qk } from '@/features/mocks/lib/queries';
import { openInLogViewer } from '@/features/logs';
import { usePrefs } from '@/features/mocks/store/prefs';
import { useNav } from '@/features/mocks/store/nav';
import { displayName } from '@/features/mocks/lib/stub-utils';
import { MethodBadge, StatusCode, SearchInput, Segmented, Spinner, Empty, Resizer, formatTime } from '@/features/mocks/components/ui';
import { Menu } from '@/components/ui/Menu';
import { downloadJson } from '@/features/mocks/lib/download';
import { journalToHar } from '@/features/mocks/lib/har';
import { RequestDetail } from './RequestDetail';
import type { ServeEvent } from '@/features/mocks/lib/types';

type Filter = 'all' | 'matched' | 'unmatched';

function matches(e: ServeEvent, q: string): boolean {
  if (!q) return true;
  return q
    .toLowerCase()
    .split(/\s+/)
    .every((t) => {
      const [k, v] = t.includes(':') ? t.split(/:(.*)/) : ['', t];
      if (k === 'method' || k === 'm') return e.request.method.toLowerCase() === v;
      if (k === 'status' || k === 's') return String(e.response?.status ?? '').startsWith(v);
      if (k === 'stub') return (e.wasMatched && e.stubMapping ? displayName(e.stubMapping) : '').toLowerCase().includes(v);
      const hay = `${e.request.method} ${e.request.url} ${e.response?.status ?? ''} ${e.wasMatched && e.stubMapping ? displayName(e.stubMapping) : ''} ${e.request.clientIp ?? ''}`.toLowerCase();
      return hay.includes(t);
    });
}

export default function RequestsPage() {
  const [live, setLive] = useState(true);
  const latest = useRequests({ live });
  const param = useNav((s) => s.param);
  const query = useNav((s) => s.query);
  const go = useNav((s) => s.go);
  // Optional time window from the URL (?from=&to= epoch ms), e.g. set by clicking the overview traffic chart.
  const from = query.from ? Number(query.from) : undefined;
  const to = query.to ? Number(query.to) : undefined;
  const hasRange = from !== undefined && !Number.isNaN(from);
  const open = (id?: string) => go('requests', id, query);
  // With a time filter, load that window directly (?since=) instead of only the latest N requests.
  const windowOpen = !to || Number.isNaN(to) || to > Date.now() - 60_000;
  const ranged = useJournalSince(hasRange ? from : undefined, live && windowOpen);
  const requests = hasRange ? ranged : latest;
  const clearRange = () => go('requests', param);
  const [q, setQ] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const pollMs = usePrefs((s) => s.pollMs);
  const limit = usePrefs((s) => s.journalLimit);
  const setPref = usePrefs((s) => s.set);
  const [width, setWidth] = useState(() => Math.round(Math.min(640, Math.max(380, window.innerWidth * 0.42))));
  const write = useWrite();
  const { server } = useWm();

  const allEvents = requests.data?.requests ?? [];
  const events = useMemo(
    () =>
      hasRange
        ? allEvents.filter((e) => {
            const t = e.request.loggedDate ?? 0;
            return t >= from! && (to === undefined || Number.isNaN(to) || t < to);
          })
        : allEvents,
    [allEvents, hasRange, from, to],
  );
  // Capped when the server had more requests since `from` than one fetch returns.
  const rangeTruncated = hasRange && !!ranged.data?.capped && (ranged.data.requests.at(-1)?.request.loggedDate ?? 0) > from!;
  const rows = useMemo(
    () => events.filter((e) => (filter === 'all' ? true : filter === 'matched' ? e.wasMatched : !e.wasMatched)).filter((e) => matches(e, q.trim())),
    [events, filter, q],
  );
  const selected = events.find((e) => e.id === param) ?? (param ? undefined : undefined);
  const ref = useRef<HTMLDivElement>(null);
  const virt = useVirtualizer({ count: rows.length, getScrollElement: () => ref.current, estimateSize: () => 30, overscan: 20 });
  const unmatched = events.filter((e) => !e.wasMatched).length;

  return (
    <div className="page">
      <div className="page-header">
        <h1>Request journal</h1>
        <span className="subtle small">
          {requests.data?.meta.total ?? 0} total{requests.data?.requestJournalDisabled && ' · journal disabled on server'}
        </span>
        <div className="spacer" />
        <button className={clsx('btn btn-sm', live && 'active')} onClick={() => setLive(!live)} title={live ? 'Pause live updates' : 'Resume live updates'}>
          {live && pollMs > 0 ? <Pause /> : <Play />} {live && pollMs > 0 ? `Live · ${pollMs / 1000}s` : 'Paused'}
        </button>
        <select className="select sm" style={{ width: 'auto' }} value={pollMs} onChange={(e) => setPref('pollMs', Number(e.target.value))} title="Poll interval">
          {[1000, 2000, 3000, 5000, 10000].map((v) => (
            <option key={v} value={v}>
              every {v / 1000}s
            </option>
          ))}
        </select>
        <select className="select sm" style={{ width: 'auto' }} value={limit} onChange={(e) => setPref('journalLimit', Number(e.target.value))} title="How many recent requests to load">
          {[50, 100, 200, 500, 1000, 5000].map((v) => (
            <option key={v} value={v}>
              last {v}
            </option>
          ))}
        </select>
        {server.kind === 'builtin' && (
          <button className="btn btn-sm btn-ghost" onClick={() => openInLogViewer({ type: 'events', query: 'mock.request' })} title="Mock traffic is also recorded in the project event timeline (searchable, kept across restarts)">
            <Activity /> Event timeline
          </button>
        )}
        <button className="icon-btn" onClick={() => requests.refetch()} title="Refresh">
          {requests.isFetching ? <Spinner /> : <RefreshCw />}
        </button>
        <Menu
          align="right"
          items={[
            { label: 'Export as HAR', icon: <Download />, onSelect: () => downloadJson('wiremock-journal.har', journalToHar(rows)) },
            { label: 'Export as JSON', icon: <Download />, onSelect: () => downloadJson('wiremock-journal.json', { requests: rows }) },
          ]}
          trigger={(p) => (
            <button className="btn btn-sm" {...p}>
              <Download /> Export
            </button>
          )}
        />
        <button
          className="btn btn-sm"
          onClick={() =>
            write((c) => c.clearRequests(), {
              label: 'Clear journal',
              success: 'Request journal cleared',
              invalidate: (sid) => [qk.requests(sid), qk.unmatchedNearMisses(sid), qk.unmatchedStubs(sid)],
            }).then(() => go('requests'))
          }
        >
          <Eraser /> Clear
        </button>
      </div>
      <div className="toolbar">
        {hasRange && (
          <span className="tag" style={{ height: 'var(--control-h-sm)', background: 'var(--accent-subtle)', borderColor: 'var(--accent-border)', color: 'var(--text)' }} title="Showing only requests received in this time window">
            <Clock /> {formatRange(from!, to)}
            <button onClick={clearRange} aria-label="Clear time filter" title="Clear time filter">
              <X />
            </button>
          </span>
        )}
        <SearchInput value={q} onChange={setQ} placeholder="Filter  (method:post status:4 stub:name or any text)" className="grow" inputRef={(el) => el?.setAttribute('data-page-search', '')} />
        <Segmented<Filter>
          value={filter}
          onChange={setFilter}
          options={[
            { value: 'all', label: `All ${events.length}` },
            { value: 'matched', label: 'Matched' },
            { value: 'unmatched', label: `Unmatched ${unmatched || ''}` },
          ]}
        />
      </div>
      {rangeTruncated && (
        <div className="toolbar subtle xs" style={{ minHeight: 28 }}>
          This window has more requests than one fetch returns ({allEvents.length}); the oldest part of it is not shown.
        </div>
      )}
      <div className="split">
        <div className="pane fill">
          <div className="table" style={{ display: 'grid', gridTemplateColumns: '96px 64px 1fr 48px 200px 64px', borderBottom: '1px solid var(--border)', fontSize: 'var(--fs-xs)', color: 'var(--text-subtle)', fontWeight: 600 }}>
            {['Time', 'Method', 'URL', 'Status', 'Matched stub', 'ms'].map((h) => (
              <div key={h} style={{ padding: '6px 8px' }}>
                {h}
              </div>
            ))}
          </div>
          <div ref={ref} className="list">
            {requests.isPending && (
              <div className="empty">
                <Spinner />
              </div>
            )}
            {requests.isSuccess && rows.length === 0 && (
              <Empty icon={<ScrollText />} title={events.length ? 'No requests match the filter' : 'No requests yet'}>
                Send traffic to your WireMock server and it appears here live.
              </Empty>
            )}
            <div style={{ height: virt.getTotalSize(), position: 'relative' }}>
              {virt.getVirtualItems().map((vi) => {
                const e = rows[vi.index];
                return (
                  <div
                    key={e.id}
                    className={clsx('list-item', e.id === param && 'on')}
                    style={{ position: 'absolute', top: 0, left: 0, right: 0, height: 30, transform: `translateY(${vi.start}px)`, display: 'grid', gridTemplateColumns: '96px 64px 1fr 48px 200px 64px', gap: 0, padding: 0 }}
                    onClick={() => open(e.id)}
                  >
                    <span className="mono xs subtle" style={{ padding: '0 8px' }}>
                      {formatTime(e.request.loggedDate)}
                    </span>
                    <span style={{ padding: '0 8px' }}>
                      <MethodBadge method={e.request.method} />
                    </span>
                    <span className="mono xs truncate" style={{ padding: '0 8px' }} title={e.request.url}>
                      {e.request.url}
                    </span>
                    <span style={{ padding: '0 8px' }}>
                      <StatusCode status={e.response?.status} />
                    </span>
                    <span className="xs truncate" style={{ padding: '0 8px' }}>
                      {e.wasMatched ? (
                        <span className="muted">{e.stubMapping ? displayName(e.stubMapping) : '✓'}</span>
                      ) : (
                        <span className="badge warning">UNMATCHED</span>
                      )}
                    </span>
                    <span className="xs subtle" style={{ padding: '0 8px', textAlign: 'right' }}>
                      {e.timing?.totalTime ?? ''}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
        {selected && (
          <>
            <Resizer width={width} onChange={setWidth} min={360} max={1100} />
            <div className="pane" style={{ width, borderLeft: '1px solid var(--border)', order: 2 }}>
              <RequestDetail key={selected.id} event={selected} onDeleted={() => open()} />
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function formatRange(from: number, to?: number): string {
  const a = new Date(from);
  const sameDay = a.toDateString() === new Date().toDateString();
  const opts: Intl.DateTimeFormatOptions = { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false };
  const day = sameDay ? '' : `${a.toLocaleDateString([], { month: 'short', day: 'numeric' })} `;
  const start = a.toLocaleTimeString([], opts);
  if (to === undefined || Number.isNaN(to)) return `since ${day}${start}`;
  return `${day}${start} – ${new Date(to).toLocaleTimeString([], opts)}`;
}

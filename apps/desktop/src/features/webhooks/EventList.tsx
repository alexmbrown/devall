import { useCallback, useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { Ban, CheckCircle2, CircleDashed, Clock, Copy, CornerDownRight, Globe, Pencil, Play, Radio, Trash2, XCircle } from 'lucide-react';
import { confirmDialog } from '@/components/ui/dialogs';
import { CopyButton, Empty, MethodBadge, RelTime, SearchInput, Spinner, StatusCode, copyText, formatBytes } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { webhooksApi, type EventSummary, type InboxItem, type SignatureBadge, type StoreNotice } from '@/lib/api/webhooks';
import { ALL } from './store';

const PAGE = 100;

interface Props {
  root: string;
  /** `*` = all, `''` = unrouted, else an inbox id. */
  filter: string;
  inbox?: InboxItem;
  selected: string | null;
  onSelect(id: string | null): void;
  /** Counts changed (events added/removed). */
  onChanged(): void;
  receiverRunning: boolean;
  onStartReceiver(): Promise<void>;
  onEditInbox(id: string): void;
}

function matches(filter: string, e: EventSummary) {
  if (filter === ALL) return true;
  return (e.inbox ?? '') === filter;
}

/** Live list of captured events for the selected inbox (newest first). */
export function EventList({ root, filter, inbox, selected, onSelect, onChanged, receiverRunning, onStartReceiver, onEditInbox }: Props) {
  const [events, setEvents] = useState<EventSummary[] | null>(null);
  const [total, setTotal] = useState(0);
  const [search, setSearch] = useState('');
  const [loadingMore, setLoadingMore] = useState(false);
  const query = useRef({ filter, search });
  query.current = { filter, search };
  const onChangedRef = useRef(onChanged);
  onChangedRef.current = onChanged;
  const listRef = useRef<HTMLDivElement>(null);
  const eventsRef = useRef(events);
  eventsRef.current = events;

  const load = useCallback(async () => {
    try {
      const page = await webhooksApi.events(root, { inbox: filter === ALL ? undefined : filter, search: search.trim() || undefined, limit: PAGE });
      setEvents(page.events);
      setTotal(page.total);
    } catch (e) {
      setEvents([]);
      toast.error('Could not load events', errorMessage(e));
    }
  }, [root, filter, search]);

  useEffect(() => {
    const t = setTimeout(load, search ? 200 : 0);
    return () => clearTimeout(t);
  }, [load, search]);

  // Live updates for this project.
  useEffect(() => {
    let sub: string | null = null;
    let cancelled = false;
    let reload: ReturnType<typeof setTimeout> | undefined;
    const onNotice = (n: StoreNotice) => {
      const { filter: f, search: s } = query.current;
      switch (n.type) {
        case 'added':
          onChangedRef.current();
          if (!matches(f, n.event)) return;
          if (s.trim()) {
            clearTimeout(reload);
            reload = setTimeout(load, 150);
            return;
          }
          setEvents((list) => (list && !list.some((e) => e.id === n.event.id) ? [n.event, ...list] : list));
          setTotal((t) => t + 1);
          return;
        case 'updated':
          setEvents((list) => list?.map((e) => (e.id === n.event.id ? n.event : e)) ?? list);
          return;
        case 'deleted':
          if (eventsRef.current?.some((e) => e.id === n.id)) {
            setEvents((list) => list?.filter((e) => e.id !== n.id) ?? list);
            setTotal((t) => Math.max(0, t - 1));
          }
          onChangedRef.current();
          return;
        case 'cleared':
          clearTimeout(reload);
          reload = setTimeout(load, 50);
          onChangedRef.current();
          return;
        case 'error':
          toast.error('Webhook receiver', n.message);
      }
    };
    webhooksApi
      .subscribe(root, onNotice)
      .then((id) => {
        if (cancelled) void webhooksApi.unsubscribe(id);
        else sub = id;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      clearTimeout(reload);
      if (sub) void webhooksApi.unsubscribe(sub);
    };
  }, [root, load]);

  const more = async () => {
    if (!events) return;
    setLoadingMore(true);
    try {
      const page = await webhooksApi.events(root, { inbox: filter === ALL ? undefined : filter, search: search.trim() || undefined, limit: PAGE, offset: events.length });
      setEvents([...events, ...page.events.filter((e) => !events.some((x) => x.id === e.id))]);
      setTotal(page.total);
    } finally {
      setLoadingMore(false);
    }
  };

  const clear = async () => {
    const what = filter === ALL ? 'all events' : filter === '' ? 'unrouted events' : `the events of ${inbox?.file?.name ?? filter}`;
    if (!(await confirmDialog({ title: `Clear ${what}?`, message: `${total} event(s) are removed from local history.`, confirmText: 'Clear', danger: true }))) return;
    try {
      await webhooksApi.clear(root, filter === ALL ? undefined : filter);
      onSelect(null);
    } catch (e) {
      toast.error('Could not clear events', errorMessage(e));
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (!events?.length) return;
    const i = events.findIndex((x) => x.id === selected);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const next = events[Math.min(events.length - 1, Math.max(0, i + (e.key === 'ArrowDown' ? 1 : -1)))];
      if (next) {
        onSelect(next.id);
        listRef.current?.querySelector<HTMLElement>(`[data-id="${next.id}"]`)?.scrollIntoView({ block: 'nearest' });
      }
    } else if (e.key === 'Delete' && selected) {
      void webhooksApi.deleteEvent(root, selected).then(() => onSelect(events[i + 1]?.id ?? events[i - 1]?.id ?? null));
    }
  };

  const title = filter === ALL ? 'All events' : filter === '' ? 'Unrouted' : (inbox?.file?.name ?? filter);

  return (
    <div className="wh-events">
      <div className="wh-events-head">
        <div className="row-sm">
          <span className="strong small truncate grow">{title}</span>
          <span className="xs subtle">{total ? `${total} event${total === 1 ? '' : 's'}` : ''}</span>
          {inbox?.file && (
            <button className="icon-btn sm" onClick={() => onEditInbox(inbox.id)} title="Edit inbox" aria-label="Edit inbox">
              <Pencil />
            </button>
          )}
          <button className="icon-btn sm danger" onClick={clear} disabled={!total} title="Clear events" aria-label="Clear events">
            <Trash2 />
          </button>
        </div>
        {inbox?.file && (
          <div className="col-sm wh-inbox-urls">
            <UrlRow icon={<Radio size={12} />} label="Local" url={inbox.localUrl} empty="start the receiver" />
            {inbox.lanUrl && <UrlRow icon={<Radio size={12} />} label="LAN" url={inbox.lanUrl} />}
            <UrlRow icon={<Globe size={12} />} label="Public" url={inbox.publicUrl} empty="expose with a tunnel" />
            {inbox.file.forward?.url && (
              <div className="row-sm xs subtle" title="Forward target">
                <CornerDownRight size={12} />
                <span className="truncate mono">{inbox.file.forward.url}</span>
                {inbox.file.forward.enabled === false && <span className="badge">off</span>}
                {inbox.file.forward.passThrough && <span className="badge info">pass-through</span>}
              </div>
            )}
          </div>
        )}
        <SearchInput value={search} onChange={setSearch} placeholder="Filter by path, type or body…" />
      </div>
      {!events ? (
        <div className="empty">
          <Spinner />
        </div>
      ) : events.length === 0 ? (
        search ? (
          <Empty title="No matching events" />
        ) : (
          <Empty
            icon={<CircleDashed />}
            title="Waiting for webhooks"
            action={
              !receiverRunning ? (
                <button className="btn btn-sm btn-primary" onClick={onStartReceiver}>
                  <Play /> Start receiver
                </button>
              ) : inbox?.localUrl ? (
                <button className="btn btn-sm" onClick={() => copyText(`curl -X POST ${inbox.localUrl} -H 'Content-Type: application/json' -d '{"hello":"world"}'`, 'cURL copied')}>
                  <Copy /> Copy a test cURL
                </button>
              ) : undefined
            }
          >
            Point the provider at the inbox URL (or a tunnel URL). Every request is kept here, with no limit, on this machine.
          </Empty>
        )
      ) : (
        <div className="list wh-event-list" role="listbox" aria-label="Events" tabIndex={0} onKeyDown={onKey} ref={listRef}>
          {events.map((e) => (
            <EventRow key={e.id} e={e} on={e.id === selected} onClick={() => onSelect(e.id)} showInbox={filter === ALL} />
          ))}
          {events.length < total && (
            <button className="btn btn-sm btn-ghost wh-more" onClick={more} disabled={loadingMore}>
              {loadingMore ? <Spinner /> : null} Load more ({total - events.length})
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function UrlRow({ icon, label, url, empty }: { icon: React.ReactNode; label: string; url: string | null; empty?: string }) {
  return (
    <div className="row-sm xs wh-url">
      <span className="subtle row-sm wh-url-label">
        {icon}
        {label}
      </span>
      {url ? (
        <>
          <code className="code-inline truncate" title={url}>
            {url}
          </code>
          <CopyButton text={url} title={`Copy ${label.toLowerCase()} URL`} />
        </>
      ) : (
        <span className="subtle">{empty ?? '—'}</span>
      )}
    </div>
  );
}

export function SignatureIcon({ sig, size = 14 }: { sig: SignatureBadge | null; size?: number }) {
  if (!sig) return null;
  if (sig.valid) return <CheckCircle2 size={size} className="text-success" aria-label="Signature valid" />;
  if (sig.status === 'expired') return <Clock size={size} className="text-warning" aria-label="Signature expired" />;
  if (sig.status === 'missingSecret') return <Ban size={size} className="subtle" aria-label="Secret not set" />;
  return <XCircle size={size} className="text-danger" aria-label="Signature invalid" />;
}

function EventRow({ e, on, onClick, showInbox }: { e: EventSummary; on: boolean; onClick(): void; showInbox: boolean }) {
  const fwd = e.forwardStatus !== null ? <StatusCode status={e.forwardStatus} /> : e.forwardError ? <span className="badge danger" title={e.forwardError}>fwd ✗</span> : null;
  return (
    <div role="option" aria-selected={on} data-id={e.id} className={clsx('list-item wh-event', on && 'on')} onClick={onClick}>
      <MethodBadge method={e.method} short />
      <div className="col-sm grow wh-event-text">
        <div className="row-sm">
          <span className={clsx('truncate small', e.label && 'strong')}>{e.label ?? e.path}</span>
          {e.source === 'replay' && <span className="badge accent">replay</span>}
          {e.source === 'relay' && <span className="badge info">relay</span>}
        </div>
        <span className="xs subtle truncate mono">
          {showInbox && e.inbox ? `${e.inbox} · ` : ''}
          {e.label ? e.path : ''}
          {e.query ? `?${e.query}` : ''} {formatBytes(e.bodySize)}
        </span>
      </div>
      <span className="row-sm wh-event-badges" title={[e.signature ? `Signature: ${e.signature.reason}` : '', e.forwardError ? `Forward: ${e.forwardError}` : e.forwardStatus ? `Forwarded: ${e.forwardStatus}${e.forwardMs ? ` in ${Math.round(e.forwardMs)} ms` : ''}` : ''].filter(Boolean).join('\n')}>
        <SignatureIcon sig={e.signature} />
        {fwd}
      </span>
      <span className="meta wh-time">
        <RelTime ts={e.receivedAt} />
      </span>
    </div>
  );
}

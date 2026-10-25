import { useCallback, useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { AlertTriangle, Cloud, Globe, History, Inbox, MoreHorizontal, Pencil, Play, Plus, Square, Trash2, Webhook } from 'lucide-react';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { copyLinkItems } from '@/components/links/LinkChip';
import { formatLink } from '@/lib/links/syntax';
import { confirmDialog } from '@/components/ui/dialogs';
import { Callout, CopyButton, Empty, Resizer, Spinner, Switch, copyText } from '@/components/ui/misc';
import { SidePane } from '@/components/layout/SidePane';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { webhooksApi, type InboxItem, type ProviderKind, type Retention } from '@/lib/api/webhooks';
import { useNav, useViewLabel, useViewState } from '@/store/nav';
import { usePrefs } from '@/store/prefs';
import { useWorkspace } from '@/store/workspace';
import { EventDetail } from './EventDetail';
import { EventList } from './EventList';
import { InboxDialog } from './InboxDialog';
import { RetentionDialog, retentionLabel } from './RetentionDialog';
import { TunnelPanel } from './TunnelPanel';
import { ALL, useWebhooks } from './store';
import { useByoTunnels } from '@/features/tunnels/byoStore';
import './webhooks.css';

const LIST_KEY = 'devall.webhooks.listWidth';

function readWidth(key: string, fallback: number) {
  try {
    return Number(localStorage.getItem(key)) || fallback;
  } catch {
    return fallback;
  }
}
function writeWidth(key: string, w: number) {
  try {
    localStorage.setItem(key, String(w));
  } catch {
    /* storage unavailable */
  }
}

export default function WebhooksPage() {
  const project = useWorkspace((s) => s.project);
  const root = project?.root;
  if (!project || !root)
    return (
      <Empty icon={<Webhook />} title="Open a project to receive webhooks">
        Inboxes are files under <code className="code-inline">webhooks/</code>; captured events stay on this machine, outside the project.
      </Empty>
    );
  return <WebhooksView key={root} root={root} />;
}

function WebhooksView({ root }: { root: string }) {
  const envId = usePrefs((s) => s.activeEnv[root]);
  const status = useWebhooks((s) => s.receivers[root]);
  const tunnels = useByoTunnels((s) => s.tunnels);
  const loadProviders = useWebhooks((s) => s.loadProviders);
  const refreshReceiver = useWebhooks((s) => s.refreshReceiver);
  const setReceiver = useWebhooks((s) => s.setReceiver);
  const [inboxes, setInboxes] = useState<InboxItem[] | null>(null);
  const [filter, setFilter] = useViewState<string>('filter', ALL);
  const [selected, setSelected] = useState<string | null>(null);
  const [dialog, setDialog] = useState<{ mode: 'new' } | { mode: 'edit'; id: string } | null>(null);
  const [showTunnels, setShowTunnels] = useState(false);
  const [port, setPort] = useState<string>('');
  const [bindAll, setBindAll] = useState(false);
  const [busy, setBusy] = useState(false);
  const [listW, setListW] = useState(() => readWidth(LIST_KEY, 420));
  const [retention, setRetention] = useState<Retention | null>(null);
  const [retentionOpen, setRetentionOpen] = useState(false);
  const ctx = useContextMenu();
  // Deep link (e.g. from a note): the inbox id = the file stem in webhooks/.
  const param = useNav((s) => s.param);
  useEffect(() => {
    if (param && inboxes?.some((i) => i.id === param)) setFilter(param);
  }, [param, inboxes, setFilter]);
  const filtered = inboxes?.find((i) => i.id === filter);
  useViewLabel(filter === ALL ? null : filter === '' ? 'Unrouted' : inboxes ? (filtered ? (filtered.file?.name ?? filtered.id) : null) : undefined);
  const running = !!status?.running;
  const tunnelCount = Object.values(tunnels).filter((t) => t.state === 'online' || t.state === 'starting').length;

  const reloadInboxes = useCallback(async () => {
    try {
      setInboxes(await webhooksApi.list(root));
    } catch (e) {
      setInboxes([]);
      toast.error('Could not list inboxes', errorMessage(e));
    }
  }, [root]);

  // Every captured event changes counts; coalesce bursts.
  const reloadTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const reloadSoon = useCallback(() => {
    clearTimeout(reloadTimer.current);
    reloadTimer.current = setTimeout(() => void reloadInboxes(), 400);
  }, [reloadInboxes]);
  useEffect(() => () => clearTimeout(reloadTimer.current), []);

  useEffect(() => {
    webhooksApi.retention(root).then(setRetention).catch(() => {});
  }, [root]);

  useEffect(() => {
    void loadProviders();
    void reloadInboxes();
    void refreshReceiver(root).then((s) => {
      if (s?.info) {
        setPort(String(s.info.port));
        setBindAll(s.info.bindAll);
      } else if (s) setPort(String(s.defaultPort));
    });
    void useByoTunnels.getState().refresh();
  }, [root, loadProviders, reloadInboxes, refreshReceiver]);

  // Keep URLs and counts fresh when the receiver/tunnel state changes.
  const statusKey = `${status?.running}|${status?.info?.port}|${status?.publicBase}`;
  useEffect(() => {
    void reloadInboxes();
  }, [statusKey, reloadInboxes]);

  // Background verification (receiver and hosted relay) uses the active environment's secrets.
  useEffect(() => {
    webhooksApi.setEnv(root, envId).catch(() => {});
  }, [envId, root]);

  const start = async () => {
    setBusy(true);
    try {
      const p = Number(port);
      const s = await webhooksApi.receiverStart(root, { port: Number.isFinite(p) && p > 0 ? p : undefined, bindAll, env: envId });
      setReceiver(root, s);
      if (s.info) setPort(String(s.info.port));
    } catch (e) {
      toast.error('Could not start the receiver', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      setReceiver(root, await webhooksApi.receiverStop(root));
    } catch (e) {
      toast.error('Could not stop the receiver', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const inboxMenu = (it: InboxItem): MenuItemDef[] => [
    { label: 'Edit…', icon: <Pencil />, onSelect: () => setDialog({ mode: 'edit', id: it.id }), disabled: !it.file },
    { separator: true },
    { label: 'Copy local URL', disabled: !it.localUrl, onSelect: () => it.localUrl && copyText(it.localUrl, 'Local URL copied') },
    { label: 'Copy public URL', disabled: !it.publicUrl, onSelect: () => it.publicUrl && copyText(it.publicUrl, 'Public URL copied') },
    ...copyLinkItems(formatLink('webhook', it.id), it.file?.name ?? it.id),
    { separator: true },
    {
      label: 'Clear events',
      disabled: it.events === 0,
      onSelect: async () => {
        if (!(await confirmDialog({ title: `Clear ${it.events} event(s) of ${it.file?.name ?? it.id}?`, message: 'They are removed from local history.', confirmText: 'Clear', danger: true }))) return;
        await webhooksApi.clear(root, it.id).catch((e) => toast.error('Could not clear', errorMessage(e)));
        void reloadInboxes();
      },
    },
    {
      label: 'Delete inbox…',
      icon: <Trash2 />,
      danger: true,
      onSelect: async () => {
        if (!(await confirmDialog({ title: `Delete inbox ${it.file?.name ?? it.id}?`, message: 'The file moves to the OS trash. Its captured events stay in history.', confirmText: 'Delete', danger: true }))) return;
        try {
          await webhooksApi.remove(root, it.id);
          if (filter === it.id) setFilter(ALL);
          void reloadInboxes();
        } catch (e) {
          toast.error('Could not delete inbox', errorMessage(e));
        }
      },
    },
  ];

  const current = inboxes?.find((i) => i.id === filter);

  return (
    <div className="page webhooks-page">
      <div className="toolbar wh-receiver" role="region" aria-label="Webhook receiver">
        <span className={clsx('dot', running && 'ok')} aria-hidden />
        <span className="strong small">Receiver</span>
        {running && status?.info ? (
          <>
            <code className="code-inline">{status.info.localUrl}</code>
            <CopyButton text={status.info.localUrl} title="Copy receiver URL" />
            {status.info.lanUrl && (
              <span className="badge warning" title="Reachable from your local network">
                LAN {status.info.lanUrl}
              </span>
            )}
          </>
        ) : (
          <span className="subtle small">stopped — senders can't reach your inboxes</span>
        )}
        {status?.publicBase && (
          <span className="wh-public">
            <Globe size={13} />
            <code className="code-inline">{status.publicBase}</code>
            <CopyButton text={status.publicBase} title="Copy public URL" />
          </span>
        )}
        <span className="spacer grow" />
        <label className="row-sm small subtle">
          Port
          <input
            className="input sm mono wh-port"
            inputMode="numeric"
            value={port}
            onChange={(e) => setPort(e.target.value.replace(/\D/g, '').slice(0, 5))}
            onKeyDown={(e) => e.key === 'Enter' && start()}
            aria-label="Receiver port"
          />
        </label>
        <Switch checked={bindAll} onChange={setBindAll} label="LAN" title="Listen on all network interfaces so phones and other machines on your network can send to it" />
        {running ? (
          <>
            {(String(status?.info?.port) !== port || status?.info?.bindAll !== bindAll) && (
              <button className="btn btn-sm" onClick={start} disabled={busy}>
                Apply
              </button>
            )}
            <button className="btn btn-sm" onClick={stop} disabled={busy}>
              {busy ? <Spinner /> : <Square />} Stop
            </button>
          </>
        ) : (
          <button className="btn btn-sm btn-primary" onClick={start} disabled={busy}>
            {busy ? <Spinner /> : <Play />} Start
          </button>
        )}
        <button className={clsx('btn btn-sm', showTunnels && 'active')} onClick={() => setShowTunnels((v) => !v)} aria-expanded={showTunnels} title="Expose the receiver through your own ngrok, cloudflared or Tailscale">
          <Cloud /> Tunnels{tunnelCount ? ` (${tunnelCount})` : ''}
        </button>
      </div>
      {running && status?.info?.bindAll && (
        <Callout kind="warning" className="wh-banner">
          Listening on all interfaces: anyone on your network can send requests to this receiver.
        </Callout>
      )}
      {showTunnels && <TunnelPanel root={root} receiverPort={status?.info?.port} onNeedReceiver={start} />}

      <div className="split wh-main">
        <SidePane
          title="Inboxes"
          actions={
            <button className="icon-btn" onClick={() => setDialog({ mode: 'new' })} title="New inbox" aria-label="New inbox">
              <Plus />
            </button>
          }
          footer={
            <button className="btn btn-sm btn-ghost wh-history-btn" onClick={() => setRetentionOpen(true)} title="Local history retention">
              <History /> History: {retentionLabel(retention)}
            </button>
          }
        >
          {!inboxes ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : (
            <div className="list wh-inboxes" role="listbox" aria-label="Inboxes">
              <InboxRow label="All events" on={filter === ALL} onClick={() => setFilter(ALL)} icon={<Inbox size={14} className="subtle" />} />
              {inboxes.map((it) => (
                <div
                  key={it.id}
                  role="option"
                  aria-selected={filter === it.id}
                  tabIndex={0}
                  className={clsx('list-item wh-inbox', filter === it.id && 'on', (it.error || it.issues.length > 0) && 'invalid')}
                  onClick={() => setFilter(it.id)}
                  onDoubleClick={() => it.file && setDialog({ mode: 'edit', id: it.id })}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') setFilter(it.id);
                    if (e.key === 'F2' && it.file) setDialog({ mode: 'edit', id: it.id });
                  }}
                  onContextMenu={(e) => ctx.open(e, inboxMenu(it))}
                  title={it.error ?? (it.issues.map((i) => i.message).join('\n') || it.file?.path)}
                >
                  <div className="col-sm grow wh-inbox-text">
                    <div className="row-sm">
                      {it.error || it.issues.length ? <AlertTriangle size={13} className="text-warning shrink-0" /> : null}
                      <span className="truncate strong small">{it.file?.name ?? it.id}</span>
                      <ProviderBadge kind={it.file?.provider} />
                    </div>
                    <span className="mono xs subtle truncate">{it.file?.path ?? 'invalid file'}</span>
                  </div>
                  <span className="meta">{it.events || ''}</span>
                  <Menu
                    align="right"
                    items={() => inboxMenu(it)}
                    trigger={(p) => (
                      <button className="icon-btn sm wh-row-menu" {...p} aria-label={`Actions for ${it.file?.name ?? it.id}`}>
                        <MoreHorizontal />
                      </button>
                    )}
                  />
                </div>
              ))}
              <InboxRow label="Unrouted" hint="Requests to paths no inbox matches" on={filter === ''} onClick={() => setFilter('')} icon={<Webhook size={14} className="subtle" />} />
              {inboxes.length === 0 && (
                <div className="wh-side-empty">
                  <p className="small muted">Create an inbox per provider (Stripe, GitHub, Slack…) to verify signatures and forward events to your app.</p>
                  <button className="btn btn-sm btn-primary" onClick={() => setDialog({ mode: 'new' })}>
                    <Plus /> New inbox
                  </button>
                </div>
              )}
            </div>
          )}
        </SidePane>
        <div className="pane fill">
          <div className="split">
            <div className="pane wh-events-pane" style={{ width: listW }}>
              <EventList
                root={root}
                filter={filter}
                inbox={current}
                selected={selected}
                onSelect={setSelected}
                onChanged={reloadSoon}
                receiverRunning={running}
                onStartReceiver={start}
                onEditInbox={(id) => setDialog({ mode: 'edit', id })}
              />
            </div>
            <Resizer
              width={listW}
              min={300}
              max={760}
              onChange={(w) => {
                setListW(w);
                writeWidth(LIST_KEY, w);
              }}
            />
            <div className="pane fill">
              {selected ? (
                <EventDetail key={selected} root={root} id={selected} envId={envId} inboxes={inboxes ?? []} onSelect={setSelected} onDeleted={() => setSelected(null)} />
              ) : (
                <Empty icon={<Webhook />} title="Select an event">
                  Inspect headers and body, see why a signature failed, forward it again or replay it with a fresh signature.
                </Empty>
              )}
            </div>
          </div>
        </div>
      </div>
      {dialog && (
        <InboxDialog
          root={root}
          envId={envId}
          id={dialog.mode === 'edit' ? dialog.id : undefined}
          onClose={() => setDialog(null)}
          onSaved={(id) => {
            void reloadInboxes();
            if (dialog.mode === 'new') setFilter(id);
          }}
        />
      )}
      {retentionOpen && <RetentionDialog root={root} onClose={() => setRetentionOpen(false)} onSaved={setRetention} />}
      {ctx.node}
    </div>
  );
}

function InboxRow({ label, hint, on, onClick, icon }: { label: string; hint?: string; on: boolean; onClick(): void; icon: React.ReactNode }) {
  return (
    <div role="option" aria-selected={on} tabIndex={0} className={clsx('list-item wh-inbox', on && 'on')} onClick={onClick} onKeyDown={(e) => e.key === 'Enter' && onClick()} title={hint}>
      {icon}
      <span className="grow truncate small">{label}</span>
    </div>
  );
}

export function ProviderBadge({ kind }: { kind?: ProviderKind | null }) {
  const label = useWebhooks((s) => s.provider(kind ?? 'none')?.label);
  if (!kind || kind === 'none') return null;
  return <span className="badge outline wh-provider">{label ?? kind}</span>;
}

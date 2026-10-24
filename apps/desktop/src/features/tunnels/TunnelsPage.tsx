import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Activity, ArrowRight, Cable, Cloud, ExternalLink, Globe, Heart, Inbox, KeyRound, LogIn, LogOut, Play, Plug, RefreshCw, Server, Share2, Square, Unplug } from 'lucide-react';
import { ResponsiveTabs, type TabDef } from '@/components/ui/ResponsiveTabs';
import { Callout, CopyButton, Empty, Field, MethodBadge, RelTime, Segmented, Spinner, StatusCode, Switch, formatBytes, formatTime } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { mockApi } from '@/lib/api/mock';
import { relayApi, type ConnState, type Limits, type RelayMode } from '@/lib/api/relay';
import { webhooksApi } from '@/lib/api/webhooks';
import { useNav, useViewLabel } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { DomainsPanel, HostedMocksPanel, MonitorsPanel, SharesPanel, TcpPanel, type CloudCtx } from './CloudPanels';
import { ByoTab } from './ByoTab';
import { useTunnels } from './store';
import type { InboxDelivery } from '@/lib/api/relay';
import './tunnels.css';

type Tab = 'byo' | 'tunnel' | 'tcp' | 'domains' | 'mocks' | 'shares' | 'monitors';
const TABS: TabDef<Tab>[] = [
  { key: 'byo', label: 'Your own tunnel (free)', icon: <Plug size={14} /> },
  { key: 'tunnel', label: 'HTTP tunnel & inbox', icon: <Cloud size={14} /> },
  { key: 'tcp', label: 'TCP tunnels', icon: <Cable size={14} /> },
  { key: 'domains', label: 'Custom domains', icon: <Globe size={14} /> },
  { key: 'mocks', label: 'Hosted mocks', icon: <Server size={14} /> },
  { key: 'shares', label: 'Shared links', icon: <Share2 size={14} /> },
  { key: 'monitors', label: 'Monitors', icon: <Activity size={14} /> },
];
/** Workbench tab titles where the sub-tab label doesn't read well on its own. */
const TAB_TITLES: Partial<Record<Tab, string>> = { byo: 'Your own tunnel', monitors: 'API monitors' };

/** A local server the user may want to expose (e.g. the running mock server or webhook receiver). */
export interface LocalPortOption {
  port: number;
  label: string;
}

export interface TunnelsPageProps {
  /** Suggested local ports (running mock servers, webhook receivers…), shown as quick picks. */
  localPorts?: LocalPortOption[];
}

const PLAN_LABEL: Record<string, string> = { free: 'Free', supporter: 'Supporter', selfhost: 'Self-hosted' };
const TAB_KEY = 'devall.tunnels.tab';

function lastTab(): Tab {
  try {
    const t = localStorage.getItem(TAB_KEY);
    if (t && TABS.some((x) => x.key === t)) return t as Tab;
  } catch {
    /* storage unavailable */
  }
  return 'byo';
}

function stateDot(state: ConnState | undefined) {
  return clsx('dot', state === 'connected' && 'ok', (state === 'connecting' || state === 'reconnecting') && 'warn pulse', state === 'failed' && 'err');
}

const STATE_LABEL: Record<ConnState, string> = {
  connecting: 'Connecting…',
  connected: 'Online',
  reconnecting: 'Reconnecting…',
  stopped: 'Stopped',
  failed: 'Failed',
};

function Meter({ label, used, limit, format = (n) => String(n), hint, warn = true }: { label: string; used: number; limit: number | null; format?: (n: number) => string; hint?: ReactNode; warn?: boolean }) {
  const pct = limit ? Math.min(100, (used / limit) * 100) : 0;
  return (
    <div className="tn-meter">
      <div className="tn-meter-head">
        <span>{label}</span>
        <span className="mono xs subtle">
          {format(used)} / {limit === null ? '∞' : format(limit)}
        </span>
      </div>
      <div className={clsx('progress', warn && pct >= 90 && 'tn-warn')}>
        <div style={{ width: `${limit === null ? 0 : pct}%` }} />
      </div>
      {hint && <div className="xs subtle">{hint}</div>}
    </div>
  );
}

function hours(s: number | null) {
  if (s === null) return 'unlimited';
  if (s % 86400 === 0) return `${s / 86400} day${s === 86400 ? '' : 's'}`;
  return `${Math.round(s / 3600)} h`;
}

/**
 * Local servers worth exposing: the caller's list plus whatever is running in this app (mock
 * servers, the project's webhook receiver). Modules that aren't registered are simply skipped.
 */
function useLocalPorts(extra: LocalPortOption[], root: string | undefined): LocalPortOption[] {
  const [found, setFound] = useState<LocalPortOption[]>([]);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const out: LocalPortOption[] = [];
      try {
        for (const m of await mockApi.list()) if (m.running && m.port) out.push({ port: m.port, label: 'Mock server' });
      } catch {
        // mock module not available
      }
      if (root) {
        try {
          const r = await webhooksApi.receiverStatus(root);
          if (r.running && r.info) out.push({ port: r.info.port, label: 'Webhook receiver' });
        } catch {
          // webhooks module not available
        }
      }
      if (!cancelled) setFound(out);
    })();
    return () => {
      cancelled = true;
    };
  }, [root]);
  return useMemo(() => {
    const seen = new Set<number>();
    return [...extra, ...found].filter((p) => (seen.has(p.port) ? false : (seen.add(p.port), true)));
  }, [extra, found]);
}

export default function TunnelsPage({ localPorts: extraPorts = [] }: TunnelsPageProps) {
  const project = useWorkspace((s) => s.project);
  const localPorts = useLocalPorts(extraPorts, project?.root);
  const navParam = useNav((s) => s.param);
  const st = useTunnels();
  const [busy, setBusy] = useState<string | null>(null);
  const go = useNav((s) => s.go);
  // The sub-tab is the view's route (survives tab switches, names the tab); new views open the last one used.
  const [fallbackTab] = useState(lastTab);
  const tab: Tab = TABS.some((t) => t.key === navParam) ? (navParam as Tab) : fallbackTab;
  const setTab = (t: Tab) => {
    go('tunnels', t);
    try {
      localStorage.setItem(TAB_KEY, t);
    } catch {
      /* storage unavailable */
    }
  };
  useViewLabel(TAB_TITLES[tab] ?? TABS.find((t) => t.key === tab)?.label);

  const refresh = useCallback(async () => {
    try {
      const [status, settings] = await Promise.all([relayApi.status(), relayApi.settingsGet()]);
      useTunnels.getState().set({ status, settings, tunnel: status.tunnel ?? useTunnels.getState().tunnel });
      for (const i of status.inboxes) useTunnels.getState().setInbox(i.root, i.state);
      if (status.signedIn) {
        relayApi
          .account()
          .then((account) => useTunnels.getState().set({ account, accountError: null }))
          .catch((e) => useTunnels.getState().set({ accountError: errorMessage(e) }));
      } else {
        useTunnels.getState().set({ account: null, accountError: null });
      }
    } catch (e) {
      toast.error('Could not read relay status', errorMessage(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const run = async (key: string, fn: () => Promise<unknown>, ok?: string) => {
    setBusy(key);
    try {
      await fn();
      if (ok) toast.success(ok);
    } catch (e) {
      toast.error('Relay', errorMessage(e));
    } finally {
      setBusy(null);
      refresh();
    }
  };

  const status = st.status;
  const settings = st.settings;
  const tabs = <ResponsiveTabs tabs={TABS} value={tab} onChange={setTab} className="tn-tabs" label="Tunnels" prefer="labels" />;
  // Your own tunnels work without the hosted relay (or its status) entirely.
  if (tab === 'byo' || !status || !settings)
    return (
      <div className="page tunnels-page">
        <div className="page-header">
          <h1>Tunnels</h1>
          {status && <span className="badge outline">{status.mode === 'hosted' ? 'Hosted relay' : 'Self-hosted relay'}</span>}
          <div className="grow" />
          <button className="icon-btn" onClick={refresh} title="Refresh" aria-label="Refresh">
            <RefreshCw />
          </button>
        </div>
        <div className="page-body">
          <div className="tn-layout">
            {tabs}
            {tab === 'byo' ? (
              <ByoTab localPorts={localPorts} />
            ) : (
              <div className="empty">
                <Spinner />
              </div>
            )}
          </div>
        </div>
      </div>
    );

  const signedIn = status.signedIn;
  const limits: Limits | null = st.account?.limits ?? st.tunnel?.limits ?? null;
  const plan = st.account?.plan ?? st.tunnel?.plan ?? null;
  const showSelfHost = () => {
    setTab('tunnel');
    requestAnimationFrame(() => document.getElementById('tn-selfhost')?.scrollIntoView({ behavior: 'smooth' }));
  };
  const cloud: CloudCtx = { signedIn, mode: status.mode, plan, limits, onUpgrade: () => run('upgrade', () => relayApi.upgrade()), onSelfHost: showSelfHost };

  return (
    <div className="page tunnels-page">
      <div className="page-header">
        <h1>Tunnels</h1>
        <span className="badge outline">{status.mode === 'hosted' ? 'Hosted relay' : 'Self-hosted relay'}</span>
        <span className="subtle xs truncate mono">{status.endpoint}</span>
        <div className="grow" />
        <button className="icon-btn" onClick={refresh} title="Refresh" aria-label="Refresh">
          <RefreshCw />
        </button>
      </div>
      <div className="page-body">
        <div className="tn-layout">
          <AccountCard
            mode={status.mode}
            signedIn={signedIn}
            hostedConfigured={status.hostedConfigured}
            keychainAvailable={status.keychainAvailable}
            busy={busy}
            plan={plan}
            limits={limits}
            onSignIn={() => run('signin', () => relayApi.signIn(), 'Signed in to the relay')}
            onSignOut={async () => {
              if (await confirmDialog({ title: 'Sign out of the relay?', message: 'Running tunnels and inboxes stop. Buffered webhooks stay on the relay until you sign in again.', confirmText: 'Sign out' })) {
                run('signout', () => relayApi.signOut());
              }
            }}
            onUpgrade={() => run('upgrade', () => relayApi.upgrade())}
            onSelfHost={showSelfHost}
          />
          {tabs}
          {tab === 'tunnel' && (
            <>
              <TunnelPanel signedIn={signedIn} plan={plan} localPorts={localPorts} busy={busy} run={run} />
              <div className="tn-grid">
                <InboxPanel signedIn={signedIn} root={project?.root ?? null} busy={busy} run={run} />
                <SelfHostPanel mode={status.mode} hasToken={settings.hasToken} endpoint={settings.selfHostedEndpoint} hostedConfigured={settings.hostedConfigured} busy={busy} run={run} />
              </div>
              <Callout>
                <div className="row-sm tn-byo">
                  <span className="grow">
                    Prefer <b>ngrok</b>, <b>cloudflared</b> or <b>Tailscale Funnel</b>? Bring your own tunnel — free, no /dev/all account, any local port.
                  </span>
                  <button className="btn btn-sm" onClick={() => setTab('byo')}>
                    <Globe /> Your own tunnel <ArrowRight />
                  </button>
                </div>
              </Callout>
            </>
          )}
          {tab === 'tcp' && <TcpPanel ctx={cloud} />}
          {tab === 'domains' && <DomainsPanel ctx={cloud} />}
          {tab === 'mocks' && <HostedMocksPanel ctx={cloud} />}
          {tab === 'shares' && <SharesPanel ctx={cloud} />}
          {tab === 'monitors' && <MonitorsPanel ctx={cloud} />}
        </div>
      </div>
    </div>
  );
}

// ---------- account ----------

function AccountCard(p: {
  mode: RelayMode;
  signedIn: boolean;
  hostedConfigured: boolean;
  keychainAvailable: boolean;
  busy: string | null;
  plan: string | null;
  limits: Limits | null;
  onSignIn(): void;
  onSignOut(): void;
  onUpgrade(): void;
  onSelfHost(): void;
}) {
  const account = useTunnels((s) => s.account);
  const accountError = useTunnels((s) => s.accountError);
  const tunnel = useTunnels((s) => s.tunnel);
  const inboxes = useTunnels((s) => s.inboxes);

  if (p.mode === 'hosted' && !p.hostedConfigured)
    return (
      <section className="card tn-account">
        <div className="tn-account-head">
          <Cloud />
          <div className="grow">
            <h2>Hosted relay not available in this build</h2>
            <p className="subtle sm">Self-host the open-source relay on your own Cloudflare account — every feature is free.</p>
          </div>
          <button className="btn btn-primary" onClick={p.onSelfHost}>
            <Server /> Self-host
          </button>
        </div>
      </section>
    );

  if (!p.signedIn)
    return (
      <section className="card tn-account">
        <div className="tn-account-head">
          <Cloud />
          <div className="grow">
            <h2>{p.mode === 'hosted' ? 'Sign in to use the hosted relay (free tier)' : 'Add your relay token'}</h2>
            <p className="subtle sm">
              {p.mode === 'hosted' ? (
                <>
                  Public HTTPS URLs for localhost and a webhook inbox that buffers while your laptop sleeps. An account is only needed for these hosted features —{' '}
                  <button className="tn-link" onClick={p.onSelfHost}>
                    or self-host for free
                  </button>
                  .
                </>
              ) : (
                'Paste the token you set as RELAY_TOKENS on your relay under Self-hosting below.'
              )}
            </p>
          </div>
          {p.mode === 'hosted' && (
            <button className="btn btn-primary" onClick={p.onSignIn} disabled={p.busy === 'signin'}>
              {p.busy === 'signin' ? <Spinner /> : <LogIn />} {p.busy === 'signin' ? 'Waiting for the browser…' : 'Sign in'}
            </button>
          )}
        </div>
        {p.mode === 'hosted' && (
          <div className="tn-plans">
            <div>
              <b>Free</b>
              <span className="subtle xs">
                Random URL per 8 h session · 1 GB/month · visitor warning page for browsers · 1 inbox, 24 h buffer, 500 events/day · 1 small hosted mock · 7-day shared links
              </span>
            </div>
            <div>
              <b>Supporter · ~$4/month</b>
              <span className="subtle xs">
                Reserved subdomains · no warning page · 50 GB/month · 5 tunnels · 30-day buffer · an inbox per project · custom domains · raw TCP tunnels · 10 hosted mocks · monitors every 5 min · cloud load regions (verified hosts)
              </span>
            </div>
          </div>
        )}
        {!p.keychainAvailable && <Callout kind="warning">The OS keychain isn't available, so the sign-in only lasts until the app closes.</Callout>}
      </section>
    );

  const limits = p.limits;
  const inboxList = Object.values(inboxes);
  const today = inboxList.reduce((n, i) => n + i.todayCount, 0);
  return (
    <section className="card tn-account">
      <div className="tn-account-head">
        <Cloud />
        <div className="grow">
          <h2 className="row-sm">
            {account?.email ?? (p.mode === 'selfHosted' ? 'Self-hosted relay' : 'Signed in')}
            {p.plan && <span className={clsx('badge', p.plan === 'free' ? 'outline' : 'success')}>{PLAN_LABEL[p.plan] ?? p.plan}</span>}
          </h2>
          {accountError ? <p className="tn-error xs">{accountError}</p> : <p className="subtle xs">{account ? `Domain ${account.domain}` : 'Loading account…'}</p>}
        </div>
        {p.plan === 'free' && (
          <button className="btn btn-primary" onClick={p.onUpgrade} disabled={p.busy === 'upgrade'}>
            <Heart /> Become a Supporter
          </button>
        )}
        {p.plan === 'supporter' && (
          <button className="btn" onClick={p.onUpgrade}>
            Manage billing
          </button>
        )}
        {p.mode === 'hosted' && (
          <button className="btn btn-ghost" onClick={p.onSignOut} disabled={p.busy === 'signout'}>
            <LogOut /> Sign out
          </button>
        )}
      </div>
      {limits && (
        <div className="tn-meters">
          <Meter label="Bandwidth this month" used={tunnel?.usage?.bandwidthBytes ?? account?.usage.bandwidthBytes ?? 0} limit={limits.bandwidthBytesPerMonth} format={formatBytes} />
          <Meter label="Tunnels running" used={account?.tunnels.length ?? (tunnel?.state === 'connected' ? 1 : 0)} limit={limits.maxTunnels} warn={false} />
          <Meter label="Webhooks today" used={today} limit={limits.inboxEventsPerDay} hint={`Buffered for ${hours(limits.inboxRetentionSeconds)} while you're offline`} />
          <div className="tn-meter">
            <div className="tn-meter-head">
              <span>Reserved subdomains</span>
              <span className="mono xs subtle">
                {account?.reservedSubdomains.length ?? 0} / {limits.reservedSubdomains === null ? '∞' : limits.reservedSubdomains}
              </span>
            </div>
            <div className="tn-subs">
              {account?.reservedSubdomains.length ? (
                account.reservedSubdomains.map((s) => (
                  <span key={s} className="badge outline mono">
                    {s}
                  </span>
                ))
              ) : (
                <span className="xs subtle">{p.plan === 'free' ? 'Supporters can reserve stable names.' : p.plan === 'selfhost' ? 'Any name works on your relay.' : 'None yet — reserve one from the billing page.'}</span>
              )}
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

// ---------- tunnel ----------

function TunnelPanel({ signedIn, plan, localPorts, busy, run }: { signedIn: boolean; plan: string | null; localPorts: LocalPortOption[]; busy: string | null; run(key: string, fn: () => Promise<unknown>, ok?: string): void }) {
  const tunnel = useTunnels((s) => s.tunnel);
  const log = useTunnels((s) => s.log);
  const account = useTunnels((s) => s.account);
  const [port, setPort] = useState<string>(() => (localPorts[0] ? String(localPorts[0].port) : '3000'));
  const [sub, setSub] = useState('');
  const [keepHost, setKeepHost] = useState(true);
  const active = tunnel && tunnel.state !== 'stopped' && tunnel.state !== 'failed';
  const reserved = account?.reservedSubdomains ?? [];
  const portNum = Number(port);
  const validPort = Number.isInteger(portNum) && portNum > 0 && portNum < 65536;

  const start = () =>
    run(
      'tunnel',
      async () => {
        useTunnels.getState().clearLog();
        const s = await relayApi.tunnelStart(portNum, sub || undefined, !keepHost, (e) => useTunnels.getState().onTunnelEvent(e));
        useTunnels.getState().set({ tunnel: s });
        if (s.state !== 'connected') throw new Error(s.lastError ?? 'The tunnel did not come online.');
      },
      'Tunnel online',
    );

  return (
    <section className="panel">
      <div className="panel-header">
        <Globe className="tn-icon" />
        <h3 className="grow">Tunnel</h3>
        {tunnel && (
          <span className="row-sm xs">
            <span className={stateDot(tunnel.state)} /> {STATE_LABEL[tunnel.state]}
          </span>
        )}
      </div>
      <div className="panel-body col">
        {!active ? (
          <>
            <div className="tn-form">
              <Field label="Local port" hint={`Forwards to http://127.0.0.1:${validPort ? portNum : '…'}`}>
                <input className={clsx('input mono', !validPort && 'invalid')} inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, '').slice(0, 5))} disabled={!signedIn} />
              </Field>
              <Field label="Subdomain" hint={plan === 'free' ? 'Free tunnels get a random name for the session' : plan === 'selfhost' ? 'Any name, or empty for random' : 'One of your reserved names, or random'}>
                {plan === 'supporter' ? (
                  <select className="select" value={sub} onChange={(e) => setSub(e.target.value)} disabled={!signedIn}>
                    <option value="">Random</option>
                    {reserved.map((s) => (
                      <option key={s} value={s}>
                        {s}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input className="input mono" value={plan === 'free' ? '' : sub} placeholder={plan === 'free' ? 'random' : 'my-app'} onChange={(e) => setSub(e.target.value.toLowerCase())} disabled={!signedIn || plan === 'free'} />
                )}
              </Field>
              <div className="tn-start">
                <button className="btn btn-primary" onClick={start} disabled={!signedIn || !validPort || busy === 'tunnel'}>
                  {busy === 'tunnel' ? <Spinner /> : <Play />} Start tunnel
                </button>
              </div>
            </div>
            {localPorts.length > 0 && (
              <div className="row-sm tn-ports">
                <span className="xs subtle">Running locally:</span>
                {localPorts.map((p) => (
                  <button key={p.port} className={clsx('btn btn-sm', String(p.port) === port && 'active')} onClick={() => setPort(String(p.port))}>
                    {p.label} <span className="mono subtle">:{p.port}</span>
                  </button>
                ))}
              </div>
            )}
            <Switch checked={keepHost} onChange={setKeepHost} label="Pass the public Host header to the app (needed for absolute URLs and Twilio signatures)" />
            {tunnel?.state === 'failed' && tunnel.lastError && <Callout kind="danger">{tunnel.lastError}</Callout>}
            {!signedIn && <p className="xs subtle">Sign in (or configure a self-hosted relay) to start a tunnel.</p>}
          </>
        ) : (
          <>
            <div className="tn-url">
              <span className={stateDot(tunnel.state)} />
              <span className="mono tn-public truncate">{tunnel.publicUrl ?? '…'}</span>
              {tunnel.publicUrl && <CopyButton text={tunnel.publicUrl} label="Copy" />}
              {tunnel.publicUrl && (
                <button className="btn btn-sm" onClick={() => openUrl(tunnel.publicUrl!).catch(() => {})}>
                  <ExternalLink /> Open
                </button>
              )}
              <ArrowRight className="subtle tn-arrow" />
              <span className="mono xs subtle truncate">{tunnel.local}</span>
              <div className="grow" />
              <button className="btn btn-sm btn-danger-ghost" onClick={() => run('tunnel-stop', () => relayApi.tunnelStop(), 'Tunnel stopped')} disabled={busy === 'tunnel-stop'}>
                <Square /> Stop
              </button>
            </div>
            <div className="tn-stats">
              <div className="stat">
                <span className="v">{tunnel.requests}</span>
                <span className="k">requests</span>
              </div>
              <div className="stat">
                <span className="v">{formatBytes(tunnel.bytesIn)}</span>
                <span className="k">in</span>
              </div>
              <div className="stat">
                <span className="v">{formatBytes(tunnel.bytesOut)}</span>
                <span className="k">out</span>
              </div>
              <div className="stat">
                <span className="v">{tunnel.activeStreams}</span>
                <span className="k">in flight</span>
              </div>
              <div className="stat">
                <span className="v">{tunnel.websockets}</span>
                <span className="k">WebSockets</span>
              </div>
              {tunnel.sessionExpiresAt && (
                <div className="stat">
                  <span className="v tn-small">{new Date(tunnel.sessionExpiresAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
                  <span className="k">session renews</span>
                </div>
              )}
            </div>
            {tunnel.quota && <Callout kind="warning">{tunnel.quota.message}</Callout>}
            {tunnel.state !== 'connected' && tunnel.lastError && <Callout kind="warning">{tunnel.lastError} — reconnecting automatically.</Callout>}
            {tunnel.limits?.interstitial && (
              <p className="xs subtle">
                Browsers see a one-time warning page on free tunnels; webhooks, APIs and anything sending <code className="code-inline">devall-skip-browser-warning: 1</code> don't.
              </p>
            )}
          </>
        )}
        <RequestLogTable />
        {log.length === 0 && active && <p className="xs subtle">Requests to the public URL show up here.</p>}
      </div>
    </section>
  );
}

function RequestLogTable() {
  const log = useTunnels((s) => s.log);
  if (log.length === 0) return null;
  return (
    <div className="tn-log">
      <div className="row-sm">
        <Activity className="tn-icon" />
        <span className="xs subtle grow">Live requests (latest {log.length})</span>
        <button className="btn btn-sm btn-ghost" onClick={() => useTunnels.getState().clearLog()}>
          Clear
        </button>
      </div>
      <table className="table">
        <thead>
          <tr>
            <th>Time</th>
            <th>Method</th>
            <th>Path</th>
            <th>Status</th>
            <th className="tn-num">Duration</th>
            <th className="tn-num">Size</th>
          </tr>
        </thead>
        <tbody>
          {log.map((r) => (
            <tr key={`${r.stream}-${r.atMs}`} title={r.error ?? r.clientIp ?? undefined}>
              <td className="mono xs subtle">{formatTime(r.atMs)}</td>
              <td>{r.websocket ? <span className="badge info">WS</span> : <MethodBadge method={r.method} short />}</td>
              <td className="mono xs tn-path truncate">{r.path}</td>
              <td>{r.error && !r.status ? <span className="badge danger">error</span> : <StatusCode status={r.status ?? undefined} />}</td>
              <td className="mono xs tn-num">{r.durationMs} ms</td>
              <td className="mono xs tn-num">{formatBytes(r.bytesOut)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ---------- inbox ----------

/** Stable empty list: a fresh `[]` from a store selector re-renders forever. */
const NO_DELIVERIES: InboxDelivery[] = [];

function InboxPanel({ signedIn, root, busy, run }: { signedIn: boolean; root: string | null; busy: string | null; run(key: string, fn: () => Promise<unknown>, ok?: string): void }) {
  const inbox = useTunnels((s) => (root ? s.inboxes[root] : undefined));
  const deliveries = useTunnels((s) => (root ? s.deliveries[root] : undefined)) ?? NO_DELIVERIES;
  const connected = inbox && inbox.state !== 'stopped' && inbox.state !== 'failed';

  const body = useMemo(() => {
    if (!root)
      return (
        <Empty icon={<Inbox />} title="Open a project">
          Each project gets its own hosted webhook inbox.
        </Empty>
      );
    if (!connected)
      return (
        <div className="col">
          <p className="sm subtle">A public webhook URL that keeps receiving while your laptop is asleep. Events are delivered here in order when you reconnect, with their raw bodies intact for signature checks.</p>
          {inbox?.state === 'failed' && inbox.lastError && <Callout kind="danger">{inbox.lastError}</Callout>}
          <div>
            <button
              className="btn btn-primary"
              disabled={!signedIn || busy === 'inbox'}
              onClick={() =>
                run('inbox', async () => {
                  const s = await relayApi.inboxConnect(root, (e) => useTunnels.getState().onInboxUpdate(root, e));
                  useTunnels.getState().setInbox(root, s);
                  if (s.state !== 'connected') throw new Error(s.lastError ?? 'The inbox did not connect.');
                })
              }
            >
              {busy === 'inbox' ? <Spinner /> : <Plug />} Connect inbox
            </button>
          </div>
        </div>
      );
    return (
      <div className="col">
        <div className="tn-url">
          <span className={stateDot(inbox.state)} />
          <span className="mono tn-public truncate">{inbox.url ?? '…'}</span>
          {inbox.url && <CopyButton text={inbox.url} label="Copy" />}
        </div>
        <dl className="kv">
          <dt>Waiting on relay</dt>
          <dd>{inbox.buffered}</dd>
          <dt>Delivered</dt>
          <dd>
            {inbox.delivered}
            {inbox.duplicates > 0 && <span className="subtle"> · {inbox.duplicates} redelivered (skipped)</span>}
          </dd>
          <dt>Today</dt>
          <dd>
            {inbox.todayCount}
            {inbox.dailyLimit !== null && <span className="subtle"> / {inbox.dailyLimit}</span>}
          </dd>
          <dt>Buffer</dt>
          <dd>{hours(inbox.retentionSeconds)}</dd>
          <dt>Answers</dt>
          <dd className="mono xs">
            {inbox.response ? `${inbox.response.status} ${inbox.response.body.slice(0, 40)}` : '200 {"ok":true}'}
          </dd>
        </dl>
        {inbox.lastError && <Callout kind="warning">{inbox.lastError}</Callout>}
        {deliveries.length > 0 && (
          <ul className="tn-deliveries">
            {deliveries.slice(0, 8).map((d) => (
              <li key={`${d.id}-${d.atMs}`}>
                <MethodBadge method={d.method} short />
                <span className="mono xs truncate grow">{d.path}</span>
                {d.duplicate && <span className="badge outline">dup</span>}
                <span className="xs subtle">{formatBytes(d.bodySize)}</span>
                <span className="xs subtle">
                  <RelTime ts={d.receivedAt} />
                </span>
              </li>
            ))}
          </ul>
        )}
        <div>
          <button className="btn btn-sm" onClick={() => run('inbox-off', async () => {
            await relayApi.inboxDisconnect(root);
            useTunnels.getState().setInbox(root, null);
          })} disabled={busy === 'inbox-off'}>
            <Unplug /> Disconnect
          </button>
        </div>
      </div>
    );
  }, [root, connected, inbox, deliveries, signedIn, busy, run]);

  return (
    <section className="panel">
      <div className="panel-header">
        <Inbox className="tn-icon" />
        <h3 className="grow">Hosted webhook inbox</h3>
        {inbox && (
          <span className="row-sm xs">
            <span className={stateDot(inbox.state)} /> {STATE_LABEL[inbox.state]}
          </span>
        )}
      </div>
      <div className="panel-body">{body}</div>
    </section>
  );
}

// ---------- self-hosting ----------

function SelfHostPanel(p: { mode: RelayMode; hasToken: boolean; endpoint: string; hostedConfigured: boolean; busy: string | null; run(key: string, fn: () => Promise<unknown>, ok?: string): void }) {
  const [mode, setMode] = useState<RelayMode>(p.mode);
  const [endpoint, setEndpoint] = useState(p.endpoint);
  const [token, setToken] = useState('');
  useEffect(() => setMode(p.mode), [p.mode]);
  useEffect(() => setEndpoint(p.endpoint), [p.endpoint]);
  const dirty = mode !== p.mode || endpoint !== p.endpoint || token !== '';

  return (
    <section className="panel" id="tn-selfhost">
      <div className="panel-header">
        <Server className="tn-icon" />
        <h3 className="grow">Self-hosting</h3>
      </div>
      <div className="panel-body col">
        <p className="sm subtle">
          The relay is open source (AGPL-3.0). Run it on your own Cloudflare account — no Firebase, no account with us — and every feature is free: any subdomain, no warning page, no quotas.
        </p>
        <Segmented<RelayMode>
          value={mode}
          onChange={setMode}
          options={[
            { value: 'hosted', label: 'Hosted relay', title: p.hostedConfigured ? undefined : 'Not available in this build' },
            { value: 'selfHosted', label: 'My relay' },
          ]}
        />
        {mode === 'selfHosted' && (
          <>
            <Field label="Relay address" hint="Your Worker's relay host, e.g. wss://relay.example.com">
              <input className="input mono" value={endpoint} placeholder="wss://relay.example.com" onChange={(e) => setEndpoint(e.target.value)} />
            </Field>
            <Field label="Token" hint={p.hasToken ? 'A token is stored in the OS keychain; leave empty to keep it.' : 'One of the RELAY_TOKENS you set on the Worker. Stored in the OS keychain.'}>
              <div className="input-group">
                <KeyRound />
                <input className="input mono" type="password" value={token} placeholder={p.hasToken ? '•••••••• (stored)' : 'paste token'} onChange={(e) => setToken(e.target.value)} autoComplete="off" />
              </div>
            </Field>
          </>
        )}
        <div className="row-sm">
          <button
            className="btn btn-primary btn-sm"
            disabled={!dirty || p.busy === 'settings'}
            onClick={() =>
              p.run(
                'settings',
                async () => {
                  await relayApi.settingsSet(mode, mode === 'selfHosted' ? endpoint : undefined, token || undefined);
                  setToken('');
                  useTunnels.getState().set({ tunnel: null, account: null });
                },
                'Relay settings saved',
              )
            }
          >
            Save
          </button>
          <span className="xs subtle">
            Deploy guide: <code className="code-inline">relay/README.md</code> (Cloudflare only, a few commands).
          </span>
        </div>
      </div>
    </section>
  );
}

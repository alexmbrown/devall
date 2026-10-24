import { useCallback, useEffect, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Activity, Cable, ExternalLink, Globe2, Heart, KeyRound, Link2, Pause, Play, Plus, RefreshCw, RotateCcw, Server, Share2, Square, Trash2, Unplug, Waypoints } from 'lucide-react';
import { Callout, CopyButton, Empty, Field, MethodBadge, RelTime, Spinner, StatusCode, Switch, formatBytes, formatTime } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import type { Limits, RelayMode } from '@/lib/api/relay';
import { cloudApi, intervalLabel, type CustomDomain, type HostedMock, type MockJournalEntry, type MonitorLimits, type MonitorView, type ShareInfo, type TcpConnLog, type TcpView } from '@/lib/api/cloud';
import { useNav } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { useTunnels } from './store';
import { MonitorDialog } from './MonitorDialog';

export interface CloudCtx {
  signedIn: boolean;
  mode: RelayMode;
  plan: string | null;
  limits: Limits | null;
  onUpgrade(): void;
  onSelfHost(): void;
}

/** "Not on your plan" notice with the two ways forward: Supporter, or self-hosting (free). */
export function PlanNotice({ ctx, what, children }: { ctx: CloudCtx; what: string; children?: ReactNode }) {
  return (
    <Callout kind="info">
      <div className="col-sm">
        <span>
          <b>{what}</b> {children ?? 'is part of the Supporter plan (~$4/month, priced near what it costs to run).'}
        </span>
        <div className="row-sm">
          {ctx.mode === 'hosted' && ctx.plan === 'free' && (
            <button className="btn btn-sm btn-primary" onClick={ctx.onUpgrade}>
              <Heart /> Become a Supporter
            </button>
          )}
          <button className="tn-link" onClick={ctx.onSelfHost}>
            or self-host the relay for free
          </button>
        </div>
      </div>
    </Callout>
  );
}

function NeedsSignIn() {
  return <p className="xs subtle">Sign in (or set a self-hosted relay token) on the Tunnel tab first.</p>;
}

function useRun() {
  const [busy, setBusy] = useState<string | null>(null);
  const run = useCallback(async (key: string, fn: () => Promise<unknown>, ok?: string) => {
    setBusy(key);
    try {
      await fn();
      if (ok) toast.success(ok);
      return true;
    } catch (e) {
      toast.error('Relay', errorMessage(e));
      return false;
    } finally {
      setBusy(null);
    }
  }, []);
  return { busy, run };
}

const STATE_DOT: Record<string, string> = { connected: 'dot ok', connecting: 'dot warn pulse', reconnecting: 'dot warn pulse', failed: 'dot err', stopped: 'dot' };

// ---------- raw TCP tunnels ----------

export function TcpPanel({ ctx }: { ctx: CloudCtx }) {
  const [view, setView] = useState<TcpView | null>(null);
  const [log, setLog] = useState<TcpConnLog[]>([]);
  const [port, setPort] = useState('5432');
  const [host, setHost] = useState('127.0.0.1');
  const [name, setName] = useState('');
  const [useSecret, setUseSecret] = useState(true);
  const [cName, setCName] = useState('');
  const [cPort, setCPort] = useState('15432');
  const [cSecret, setCSecret] = useState('');
  const account = useTunnels((s) => s.account);
  const { busy, run } = useRun();
  const refresh = useCallback(() => cloudApi.tcpStatus().then(setView).catch(() => {}), []);
  useEffect(() => {
    void refresh();
  }, [refresh]);

  const allowed = ctx.limits?.tcpTunnels === true || ctx.plan === 'selfhost';
  const pub = view?.publisher;
  const active = pub && pub.state !== 'stopped' && pub.state !== 'failed';
  const portNum = Number(port);
  const cPortNum = Number(cPort);
  const connectCmd = pub?.name ? `devall tcp connect ${pub.name} --local-port ${port}${view?.secret ? ` --secret ${view.secret}` : ''}` : '';

  return (
    <div className="tn-grid">
      <section className="panel">
        <div className="panel-header">
          <Cable className="tn-icon" />
          <h3 className="grow">Publish a TCP port</h3>
          {pub && (
            <span className="row-sm xs">
              <span className={STATE_DOT[pub.state] ?? 'dot'} /> {pub.state}
            </span>
          )}
        </div>
        <div className="panel-body col">
          <p className="sm subtle">
            Databases, Redis, SSH — anything TCP — through the relay over WebSockets on port 443 (like <code className="code-inline">cloudflared access tcp</code>). Nobody can connect without your
            account or the tunnel's access secret.
          </p>
          {!ctx.signedIn ? (
            <NeedsSignIn />
          ) : !allowed ? (
            <PlanNotice ctx={ctx} what="Raw TCP tunnels" />
          ) : !active ? (
            <>
              <div className="tn-form">
                <Field label="Local port">
                  <input className={clsx('input mono', !(portNum > 0 && portNum < 65536) && 'invalid')} value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, '').slice(0, 5))} />
                </Field>
                <Field label="Host" hint="Usually this machine">
                  <input className="input mono" value={host} onChange={(e) => setHost(e.target.value)} />
                </Field>
                <Field label="Name" hint={ctx.plan === 'selfhost' ? 'Any name, or empty for random' : 'Reserved name, or random'}>
                  {ctx.plan === 'supporter' ? (
                    <select className="select" value={name} onChange={(e) => setName(e.target.value)}>
                      <option value="">Random</option>
                      {(account?.reservedSubdomains ?? []).map((s) => (
                        <option key={s} value={s}>
                          {s}
                        </option>
                      ))}
                    </select>
                  ) : (
                    <input className="input mono" value={name} placeholder="random" onChange={(e) => setName(e.target.value.toLowerCase())} />
                  )}
                </Field>
              </div>
              <Switch checked={useSecret} onChange={setUseSecret} label="Let teammates connect with an access secret (otherwise only your own account can)" />
              <div>
                <button
                  className="btn btn-primary"
                  disabled={busy === 'tcp' || !(portNum > 0 && portNum < 65536)}
                  onClick={() =>
                    run(
                      'tcp',
                      async () => {
                        setLog([]);
                        const v = await cloudApi.tcpStart(portNum, host, name || undefined, useSecret, (e) => {
                          if (e.type === 'connection') setLog((l) => [e, ...l].slice(0, 100));
                          else void refresh();
                        });
                        setView(v);
                        if (v.publisher?.state !== 'connected') throw new Error(v.publisher?.lastError ?? 'The TCP tunnel did not come online.');
                      },
                      'TCP tunnel online',
                    )
                  }
                >
                  {busy === 'tcp' ? <Spinner /> : <Play />} Start TCP tunnel
                </button>
              </div>
              {pub?.state === 'failed' && pub.lastError && <Callout kind="danger">{pub.lastError}</Callout>}
            </>
          ) : (
            <>
              <div className="tn-url">
                <span className={STATE_DOT[pub.state] ?? 'dot'} />
                <span className="mono tn-public">{pub.name}</span>
                <span className="mono xs subtle">→ {pub.local}</span>
                <div className="grow" />
                <button className="btn btn-sm btn-danger-ghost" onClick={() => run('tcp-stop', async () => { await cloudApi.tcpStop(); await refresh(); }, 'TCP tunnel stopped')}>
                  <Square /> Stop
                </button>
              </div>
              <Field label="Connect from another machine">
                <div className="row-sm">
                  <code className="code-inline tn-cmd grow">{connectCmd}</code>
                  <CopyButton text={connectCmd} label="Copy" />
                </div>
              </Field>
              {view?.secret && (
                <div className="row-sm">
                  <KeyRound className="tn-icon" />
                  <span className="xs subtle grow">Anyone with the secret can reach this port while the tunnel runs. Rotate it to lock previous holders out.</span>
                  <button
                    className="btn btn-sm"
                    onClick={async () => {
                      if (!(await confirmDialog({ title: 'Rotate the access secret?', message: 'Teammates need the new secret; restart the tunnel to apply it.', confirmText: 'Rotate' }))) return;
                      await run('rotate', async () => { await cloudApi.tcpSecret(true); await refresh(); }, 'New secret — restart the tunnel to use it');
                    }}
                  >
                    <RotateCcw /> Rotate
                  </button>
                </div>
              )}
              <div className="tn-stats">
                <div className="stat"><span className="v">{pub.connections}</span><span className="k">open</span></div>
                <div className="stat"><span className="v">{pub.totalConnections}</span><span className="k">total</span></div>
                <div className="stat"><span className="v">{formatBytes(pub.bytesIn)}</span><span className="k">in</span></div>
                <div className="stat"><span className="v">{formatBytes(pub.bytesOut)}</span><span className="k">out</span></div>
              </div>
              {pub.state !== 'connected' && pub.lastError && <Callout kind="warning">{pub.lastError} — reconnecting automatically.</Callout>}
              {log.length > 0 && (
                <ul className="tn-deliveries">
                  {log.slice(0, 10).map((l) => (
                    <li key={`${l.stream}-${l.atMs}`}>
                      <span className="xs mono subtle">{formatTime(l.atMs)}</span>
                      <span className="badge outline">{l.via}</span>
                      <span className="xs mono grow truncate">{l.clientIp ?? '—'}</span>
                      <span className="xs subtle">{l.durationMs} ms · {formatBytes(l.bytesIn + l.bytesOut)}</span>
                      {l.error && <span className="badge danger" title={l.error}>error</span>}
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
      </section>

      <section className="panel">
        <div className="panel-header">
          <Waypoints className="tn-icon" />
          <h3 className="grow">Connect to a TCP tunnel</h3>
        </div>
        <div className="panel-body col">
          <p className="sm subtle">
            Opens a port on this machine (<code className="code-inline">127.0.0.1</code>) that leads to someone's TCP tunnel — point <code className="code-inline">psql</code>, a DB client or{' '}
            <code className="code-inline">ssh</code> at it. Free for everyone: only the publisher needs the plan, and with the tunnel's access secret you don't even need an account.
          </p>
          {
            <>
              <div className="tn-form">
                <Field label="Tunnel name">
                  <input className="input mono" value={cName} placeholder="brave-otter-1234" onChange={(e) => setCName(e.target.value.toLowerCase())} />
                </Field>
                <Field label="Local port">
                  <input className="input mono" value={cPort} onChange={(e) => setCPort(e.target.value.replace(/\D/g, '').slice(0, 5))} />
                </Field>
                <Field label="Access secret" hint={ctx.signedIn ? 'Empty = your own account' : 'Required without an account'}>
                  <input className="input mono" type="password" value={cSecret} placeholder="dtcp_…" onChange={(e) => setCSecret(e.target.value)} autoComplete="off" />
                </Field>
              </div>
              <div>
                <button
                  className="btn"
                  disabled={!cName || busy === 'connect' || !(cPortNum > 0 && cPortNum < 65536) || (!ctx.signedIn && !cSecret.startsWith('dtcp_'))}
                  onClick={() =>
                    run(
                      'connect',
                      async () => {
                        await cloudApi.tcpConnect(cName, cPortNum, cSecret || undefined, () => void refresh());
                        setCSecret('');
                        await refresh();
                      },
                      `Listening on 127.0.0.1:${cPortNum}`,
                    )
                  }
                >
                  {busy === 'connect' ? <Spinner /> : <Cable />} Connect
                </button>
              </div>
            </>
          }
          {(view?.consumers ?? []).map((c) => (
            <div key={c.name} className="tn-url">
              <span className={c.running ? 'dot ok' : 'dot'} />
              <span className="mono">{c.listen}</span>
              <span className="xs subtle">→ {c.name}</span>
              <span className="xs subtle grow">
                {c.connections} open · {formatBytes(c.bytesIn + c.bytesOut)}
              </span>
              {c.lastError && <span className="badge danger" title={c.lastError}>error</span>}
              <CopyButton text={c.listen} />
              <button className="btn btn-sm btn-ghost" onClick={() => run('disc', async () => { await cloudApi.tcpDisconnect(c.name); await refresh(); })}>
                <Unplug /> Close
              </button>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}

// ---------- custom domains ----------

const DOMAIN_BADGE: Record<string, string> = { active: 'success', pending: 'warning', error: 'danger', static: 'outline' };

export function DomainsPanel({ ctx }: { ctx: CloudCtx }) {
  const account = useTunnels((s) => s.account);
  const [domains, setDomains] = useState<CustomDomain[]>([]);
  const [target, setTarget] = useState<string>('');
  const [hostname, setHostname] = useState('');
  const [sub, setSub] = useState('');
  const { busy, run } = useRun();
  const refresh = useCallback(() => {
    cloudApi
      .domains()
      .then((d) => {
        setDomains(d.domains);
        setTarget(d.target);
      })
      .catch(() => {});
  }, []);
  useEffect(() => {
    if (ctx.signedIn) refresh();
  }, [ctx.signedIn, refresh]);
  const reserved = account?.reservedSubdomains ?? [];
  const max = ctx.limits?.customDomains;
  const hostedAllowed = ctx.mode === 'hosted' && (max ?? 0) > 0;
  const selfHost = ctx.mode === 'selfHosted';

  return (
    <section className="panel">
      <div className="panel-header">
        <Globe2 className="tn-icon" />
        <h3 className="grow">Custom domains</h3>
        {ctx.signedIn && (
          <button className="icon-btn" title="Refresh" onClick={refresh}>
            <RefreshCw />
          </button>
        )}
      </div>
      <div className="panel-body col">
        <p className="sm subtle">
          Serve a tunnel at your own hostname, e.g. <code className="code-inline">api.example.com</code>: add a CNAME to <code className="code-inline">{target || 'the relay'}</code> and the
          certificate is issued and renewed automatically.
        </p>
        {!ctx.signedIn ? (
          <NeedsSignIn />
        ) : selfHost ? (
          <Callout kind="info">
            On your relay, map hostnames to tunnels with <code className="code-inline">CUSTOM_HOSTS = {'\'{"api.example.com":"my-box"}\''}</code> in <code className="code-inline">wrangler.toml</code> and
            add a Worker route (or Custom Domain) for each hostname in the same Cloudflare account — see <code className="code-inline">relay/README.md</code>.
          </Callout>
        ) : !hostedAllowed ? (
          <PlanNotice ctx={ctx} what="Custom domains" />
        ) : (
          <div className="tn-form">
            <Field label="Hostname">
              <input className="input mono" value={hostname} placeholder="api.example.com" onChange={(e) => setHostname(e.target.value.trim().toLowerCase())} />
            </Field>
            <Field label="Reserved subdomain" hint={reserved.length ? undefined : 'Reserve one first (billing page)'}>
              <select className="select" value={sub} onChange={(e) => setSub(e.target.value)}>
                <option value="">Pick…</option>
                {reserved.map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </select>
            </Field>
            <div className="tn-start">
              <button
                className="btn btn-primary"
                disabled={!hostname || !sub || busy === 'add' || (max !== null && max !== undefined && domains.filter((d) => d.source === 'hosted').length >= max)}
                onClick={() =>
                  run('add', async () => {
                    await cloudApi.domainAdd(hostname, sub);
                    setHostname('');
                    refresh();
                  }, 'Domain added — add the DNS records below')
                }
              >
                {busy === 'add' ? <Spinner /> : <Plus />} Add
              </button>
            </div>
          </div>
        )}
        {domains.length === 0 && ctx.signedIn && <p className="xs subtle">No custom domains yet.</p>}
        {domains.map((d) => (
          <div key={d.hostname} className="tn-domain card">
            <div className="row-sm">
              <span className="mono strong">{d.hostname}</span>
              <span className="xs subtle">→ {d.subdomain}</span>
              <span className={clsx('badge', DOMAIN_BADGE[d.status] ?? 'outline')}>{d.status}</span>
              {d.sslStatus && d.status !== 'active' && <span className="xs subtle">certificate: {d.sslStatus.replace(/_/g, ' ')}</span>}
              <div className="grow" />
              {d.status === 'active' && (
                <button className="btn btn-sm btn-ghost" onClick={() => openUrl(`https://${d.hostname}`).catch(() => {})}>
                  <ExternalLink /> Open
                </button>
              )}
              {d.source === 'hosted' && (
                <>
                  <button className="btn btn-sm" disabled={busy === `r:${d.hostname}`} onClick={() => run(`r:${d.hostname}`, async () => { await cloudApi.domainRefresh(d.hostname); refresh(); })}>
                    <RefreshCw /> Check now
                  </button>
                  <button
                    className="btn btn-sm btn-danger-ghost"
                    onClick={async () => {
                      if (!(await confirmDialog({ title: `Remove ${d.hostname}?`, message: 'The hostname stops reaching your tunnel and its certificate is released.', confirmText: 'Remove', danger: true }))) return;
                      await run(`x:${d.hostname}`, async () => { await cloudApi.domainRemove(d.hostname); refresh(); }, 'Domain removed');
                    }}
                  >
                    <Trash2 />
                  </button>
                </>
              )}
            </div>
            {d.records.length > 0 && d.status !== 'active' && (
              <table className="table tn-records">
                <thead>
                  <tr>
                    <th>Type</th>
                    <th>Name</th>
                    <th>Value</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {d.records.map((r) => (
                    <tr key={`${r.type}-${r.name}`}>
                      <td className="mono xs">{r.type}</td>
                      <td className="mono xs">
                        {r.name} <CopyButton text={r.name} />
                      </td>
                      <td className="mono xs tn-path">
                        {r.value} <CopyButton text={r.value} />
                      </td>
                      <td className="xs subtle">{r.purpose === 'routing' ? 'required' : 'optional (pre-validation)'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {d.errors.length > 0 && d.status !== 'active' && <p className="xs subtle">{d.errors.join(' · ')}</p>}
          </div>
        ))}
      </div>
    </section>
  );
}

// ---------- hosted mocks ----------

export function HostedMocksPanel({ ctx }: { ctx: CloudCtx }) {
  const [mocks, setMocks] = useState<HostedMock[] | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [journal, setJournal] = useState<MockJournalEntry[]>([]);
  const go = useNav((s) => s.go);
  const { busy, run } = useRun();
  const refresh = useCallback(() => {
    cloudApi.mocks().then((m) => setMocks(m.mocks)).catch(() => setMocks([]));
  }, []);
  useEffect(() => {
    if (ctx.signedIn) refresh();
  }, [ctx.signedIn, refresh]);
  const loadJournal = useCallback((name: string) => cloudApi.mockJournal(name, 50).then((j) => setJournal(j.requests)).catch((e) => toast.error('Journal', errorMessage(e))), []);
  useEffect(() => {
    if (open) void loadJournal(open);
  }, [open, loadJournal]);
  const max = ctx.limits?.hostedMocks;

  return (
    <section className="panel">
      <div className="panel-header">
        <Server className="tn-icon" />
        <h3 className="grow">Hosted mocks</h3>
        {max !== undefined && (
          <span className="xs subtle">
            {mocks?.length ?? 0} / {max === null ? '∞' : max}
          </span>
        )}
        <button className="btn btn-sm" onClick={() => go('mocks')}>
          <Globe2 /> Publish from the Mock server
        </button>
      </div>
      <div className="panel-body col">
        <p className="sm subtle">
          Your project's WireMock stubs served from the edge, so they keep answering when your laptop is off. Requests land in a journal you can read here. Or run them in your own CI with{' '}
          <code className="code-inline">devall mock serve</code> / the Docker image.
        </p>
        {!ctx.signedIn ? (
          <NeedsSignIn />
        ) : max === 0 ? (
          <PlanNotice ctx={ctx} what="Hosted mocks" />
        ) : mocks === null ? (
          <Spinner />
        ) : mocks.length === 0 ? (
          <Empty icon={<Server />} title="Nothing published">
            Mock server → <b>Publish to the web</b>, or <code className="code-inline">devall mock publish</code>.
          </Empty>
        ) : (
          mocks.map((m) => (
            <div key={m.name} className="tn-domain card">
              <div className="row-sm">
                <span className="mono strong">{m.name}</span>
                <span className="mono xs truncate grow">{m.url}</span>
                <CopyButton text={m.url} label="Copy" />
                <span className="xs subtle">
                  {m.stubs} stubs · {formatBytes(m.bytes)} · {m.requestsToday} today
                </span>
                <button className={clsx('btn btn-sm', open === m.name && 'active')} onClick={() => setOpen(open === m.name ? null : m.name)}>
                  <Activity /> Requests
                </button>
                <button
                  className="btn btn-sm btn-danger-ghost"
                  title="Unpublish"
                  disabled={busy === m.name}
                  onClick={async () => {
                    if (!(await confirmDialog({ title: `Unpublish ${m.name}?`, message: 'The URL stops answering and its request journal is deleted. Your project files are untouched.', confirmText: 'Unpublish', danger: true }))) return;
                    await run(m.name, async () => { await cloudApi.mockUnpublish(m.name); if (open === m.name) setOpen(null); refresh(); }, 'Unpublished');
                  }}
                >
                  <Trash2 />
                </button>
              </div>
              {m.pathUrl && m.pathUrl !== m.url && <span className="xs subtle mono">also {m.pathUrl}</span>}
              {open === m.name && (
                <div className="tn-log">
                  <div className="row-sm">
                    <span className="xs subtle grow">Latest {journal.length} requests</span>
                    <button className="btn btn-sm btn-ghost" onClick={() => loadJournal(m.name)}>
                      <RefreshCw /> Refresh
                    </button>
                    <button className="btn btn-sm btn-ghost" onClick={() => run('reset', () => cloudApi.mockResetScenarios(m.name), 'Scenarios reset')}>
                      <RotateCcw /> Reset scenarios
                    </button>
                    <button className="btn btn-sm btn-ghost" onClick={() => run('clear', async () => { await cloudApi.mockJournalClear(m.name); setJournal([]); })}>
                      Clear
                    </button>
                  </div>
                  {journal.length === 0 ? (
                    <p className="xs subtle">No requests yet.</p>
                  ) : (
                    <table className="table">
                      <tbody>
                        {journal.map((r) => (
                          <tr key={r.id} title={r.body || undefined}>
                            <td className="xs subtle mono">
                              <RelTime ts={r.at} />
                            </td>
                            <td>
                              <MethodBadge method={r.method} short />
                            </td>
                            <td className="mono xs tn-path truncate">{r.url}</td>
                            <td>
                              <StatusCode status={r.status} />
                            </td>
                            <td className="xs">{r.matched ? (r.stubName ?? 'matched') : <span className="text-danger">no stub matched</span>}</td>
                            <td className="mono xs tn-num">{r.durationMs} ms</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  )}
                </div>
              )}
            </div>
          ))
        )}
      </div>
    </section>
  );
}

// ---------- shared snapshots ----------

export function SharesPanel({ ctx }: { ctx: CloudCtx }) {
  const [shares, setShares] = useState<ShareInfo[] | null>(null);
  const [limit, setLimit] = useState<number | null>(null);
  const { run } = useRun();
  const go = useNav((s) => s.go);
  const refresh = useCallback(() => {
    cloudApi
      .shares()
      .then((s) => {
        setShares(s.shares);
        setLimit(s.limits.shares);
      })
      .catch(() => setShares([]));
  }, []);
  useEffect(() => {
    if (ctx.signedIn) refresh();
  }, [ctx.signedIn, refresh]);

  return (
    <section className="panel">
      <div className="panel-header">
        <Share2 className="tn-icon" />
        <h3 className="grow">Shared links</h3>
        {limit !== undefined && shares && (
          <span className="xs subtle">
            {shares.length} / {limit === null ? '∞' : limit} active
          </span>
        )}
      </div>
      <div className="panel-body col">
        <p className="sm subtle">
          Read-only, expiring links to a request and its response — secrets and auth headers are masked before anything leaves this machine. Share from a request's <b>History</b> tab.
        </p>
        {!ctx.signedIn ? (
          <NeedsSignIn />
        ) : shares === null ? (
          <Spinner />
        ) : shares.length === 0 ? (
          <Empty icon={<Link2 />} title="No shared links" action={<button className="btn btn-sm" onClick={() => go('http')}>Open requests</button>}>
            HTTP → a request → History → <b>Share…</b>
          </Empty>
        ) : (
          <table className="table">
            <tbody>
              {shares.map((s) => (
                <tr key={s.id}>
                  <td className="strong">{s.title}</td>
                  <td className="mono xs tn-path truncate">{s.url}</td>
                  <td className="xs subtle">
                    expires <RelTime ts={s.expiresAt} />
                  </td>
                  <td className="xs subtle">{formatBytes(s.bytes)}</td>
                  <td className="tn-num">
                    <CopyButton text={s.url} />
                    <button className="icon-btn sm" title="Open" onClick={() => openUrl(s.url).catch(() => {})}>
                      <ExternalLink />
                    </button>
                    <button
                      className="icon-btn sm danger"
                      title="Revoke"
                      onClick={async () => {
                        if (!(await confirmDialog({ title: 'Revoke this link?', message: 'Anyone opening it gets "link expired".', confirmText: 'Revoke', danger: true }))) return;
                        await run('revoke', async () => { await cloudApi.shareRevoke(s.id); refresh(); }, 'Link revoked');
                      }}
                    >
                      <Trash2 />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </section>
  );
}

// ---------- monitors ----------

/** Response times of the latest runs as bars (failures in the danger colour). */
export function Sparkline({ points }: { points: { ok: boolean; durationMs: number; at: string }[] }) {
  if (!points.length) return <span className="xs subtle">no runs yet</span>;
  const max = Math.max(...points.map((p) => p.durationMs), 1);
  const w = 3;
  const gap = 1;
  const h = 22;
  return (
    <svg className="tn-spark" width={points.length * (w + gap)} height={h} role="img" aria-label="Recent runs">
      {points.map((p, i) => {
        const bh = Math.max(2, Math.round((p.durationMs / max) * (h - 2)));
        return (
          <rect key={i} x={i * (w + gap)} y={h - bh} width={w} height={bh} rx={1} className={p.ok ? 'ok' : 'fail'}>
            <title>{`${new Date(p.at).toLocaleString()} · ${p.ok ? 'ok' : 'failed'} · ${p.durationMs} ms`}</title>
          </rect>
        );
      })}
    </svg>
  );
}

export function MonitorsPanel({ ctx }: { ctx: CloudCtx }) {
  const project = useWorkspace((s) => s.project);
  const [monitors, setMonitors] = useState<MonitorView[] | null>(null);
  const [limits, setLimits] = useState<MonitorLimits | null>(null);
  const [editing, setEditing] = useState<MonitorView | 'new' | null>(null);
  const [details, setDetails] = useState<string | null>(null);
  const { busy, run } = useRun();
  const refresh = useCallback(() => {
    cloudApi
      .monitors()
      .then((m) => {
        setMonitors(m.monitors);
        setLimits(m.limits);
      })
      .catch(() => setMonitors([]));
  }, []);
  useEffect(() => {
    if (ctx.signedIn) refresh();
  }, [ctx.signedIn, refresh]);
  const max = ctx.limits?.monitors ?? limits?.monitors;
  const full = max !== null && max !== undefined && (monitors?.length ?? 0) >= max;

  return (
    <section className="panel">
      <div className="panel-header">
        <Activity className="tn-icon" />
        <h3 className="grow">Scheduled monitors</h3>
        {max !== undefined && monitors && (
          <span className="xs subtle">
            {monitors.length} / {max === null ? '∞' : max}
          </span>
        )}
        {ctx.signedIn && max !== 0 && (
          <button className="btn btn-sm btn-primary" disabled={!project || full} title={!project ? 'Open a project' : full ? 'Your plan’s monitors are in use' : undefined} onClick={() => setEditing('new')}>
            <Plus /> New monitor
          </button>
        )}
      </div>
      <div className="panel-body col">
        <p className="sm subtle">
          Run saved requests + their assertions from the edge every {limits ? intervalLabel(limits.monitorMinIntervalMinutes).replace(/^every /, '') : 'few minutes'}; get a webhook or Slack
          message when something breaks or recovers. Prefer your own infrastructure? Run <code className="code-inline">devall run</code> on a schedule in CI — free.
        </p>
        <p className="xs subtle">
          Load from the edge instead? Load page → <b>Cloud regions…</b> (Supporter / self-host): short, bounded runs against hosts you verified with a token file or DNS record.
        </p>
        {!ctx.signedIn ? (
          <NeedsSignIn />
        ) : max === 0 ? (
          <PlanNotice ctx={ctx} what="Scheduled monitors">
            are part of the Supporter plan (~$4/month, priced near what it costs to run). Free alternative: <code className="code-inline">devall run</code> on a schedule in your CI.
          </PlanNotice>
        ) : monitors === null ? (
          <Spinner />
        ) : monitors.length === 0 ? (
          <Empty icon={<Activity />} title="No monitors">
            Pick a few saved requests; their assertions decide up or down.
          </Empty>
        ) : (
          monitors.map((m) => (
            <div key={m.id} className="tn-domain card">
              <div className="row-sm">
                <span className={m.paused ? 'dot' : m.lastOk === null ? 'dot warn' : m.lastOk ? 'dot ok' : 'dot err'} />
                <span className="strong">{m.name}</span>
                <span className="xs subtle">
                  {intervalLabel(m.effectiveIntervalMinutes)} · {m.requests.length} request{m.requests.length === 1 ? '' : 's'}
                </span>
                {m.paused && <span className="badge outline">paused</span>}
                <div className="grow" />
                <Sparkline points={m.recent} />
                <span className="xs mono" title={`${m.runs} runs kept`}>
                  {m.uptime === null ? '—' : `${(m.uptime * 100).toFixed(m.uptime === 1 ? 0 : 1)}%`}
                </span>
                <button className="icon-btn sm" title="Run now" disabled={busy === `run:${m.id}`} onClick={() => run(`run:${m.id}`, async () => { const r = await cloudApi.monitorRun(m.id); refresh(); toast[r.ok ? 'success' : 'error'](r.ok ? 'All checks passed' : 'Checks failed', `${m.name} · ${r.durationMs} ms`); })}>
                  {busy === `run:${m.id}` ? <Spinner /> : <Play />}
                </button>
                <button
                  className="icon-btn sm"
                  title={m.paused ? 'Resume' : 'Pause'}
                  onClick={() => run(`p:${m.id}`, async () => { await cloudApi.monitorUpdate(m.id, { name: m.name, intervalMinutes: m.intervalMinutes, requests: m.requests, alerts: m.alerts, paused: !m.paused }); refresh(); })}
                >
                  {m.paused ? <Play /> : <Pause />}
                </button>
                <button className="btn btn-sm btn-ghost" disabled={!project} onClick={() => setEditing(m)}>
                  Edit
                </button>
                <button className={clsx('btn btn-sm btn-ghost', details === m.id && 'active')} onClick={() => setDetails(details === m.id ? null : m.id)}>
                  Results
                </button>
                <button
                  className="icon-btn sm danger"
                  title="Delete"
                  onClick={async () => {
                    if (!(await confirmDialog({ title: `Delete ${m.name}?`, message: 'Its results and uploaded secrets are deleted from the relay.', confirmText: 'Delete', danger: true }))) return;
                    await run(`d:${m.id}`, async () => { await cloudApi.monitorDelete(m.id); refresh(); }, 'Monitor deleted');
                  }}
                >
                  <Trash2 />
                </button>
              </div>
              {m.pausedReason && <Callout kind="warning">{m.pausedReason}</Callout>}
              {m.lastAlertError && <p className="xs text-danger">Alert delivery failed: {m.lastAlertError}</p>}
              {details === m.id && m.last && (
                <div className="tn-log">
                  <span className="xs subtle">
                    Last run <RelTime ts={m.last.at} /> ({m.last.trigger}) · {m.last.durationMs} ms
                  </span>
                  <table className="table">
                    <tbody>
                      {m.last.checks.map((c, i) => (
                        <tr key={i}>
                          <td>{c.ok ? <span className="badge success">ok</span> : <span className="badge danger">fail</span>}</td>
                          <td>
                            <MethodBadge method={c.method} short />
                          </td>
                          <td className="xs strong">{c.name}</td>
                          <td>{c.status ? <StatusCode status={c.status} /> : '—'}</td>
                          <td className="mono xs tn-num">{c.durationMs} ms</td>
                          <td className="xs">{c.error ?? c.failures.join('; ')}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          ))
        )}
      </div>
      {editing && project && limits && (
        <MonitorDialog
          root={project.root}
          existing={editing === 'new' ? null : editing}
          limits={limits}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}
    </section>
  );
}

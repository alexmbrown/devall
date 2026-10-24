import { useEffect, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ArrowRight, CheckCircle2, CircleDashed, ExternalLink, KeyRound, Play, Settings2, Square, Wifi, XCircle } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, CopyButton, Field, Segmented, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { useNav } from '@/store/nav';
import { webhooksApi, type CloudflaredMode, type ToolInfo, type TunnelKind, type TunnelSecretName, type TunnelSettings, type TunnelStatus, type TunnelTest } from '@/lib/api/webhooks';
import { installGuide, platformOf } from './byo';
import { isLive, useByoTunnels } from './byoStore';
import './tunnels.css';

const PLATFORM = platformOf(typeof navigator !== 'undefined' ? navigator.userAgent : '', typeof navigator !== 'undefined' ? navigator.platform : '');
const KIND_NAME: Record<TunnelKind, string> = { ngrok: 'ngrok', cloudflared: 'cloudflared', tailscale: 'Tailscale Funnel' };

/** Status of one running (or failed) tunnel: state, public URL, hint and the tool's output. */
export function TunnelStateView({ s, label, onStop }: { s: TunnelStatus; label?: string; onStop?(): void }) {
  return (
    <div className="col-sm byo-state">
      <div className="row-sm byo-state-head">
        <span className={clsx('dot', s.state === 'online' && 'ok', s.state === 'starting' && 'warn pulse', s.state === 'error' && 'err')} aria-hidden />
        <span className="xs strong">{s.state === 'online' ? 'Online' : s.state === 'starting' ? 'Starting…' : s.state === 'error' ? 'Failed' : 'Stopped'}</span>
        <span className="xs subtle truncate grow" title={s.message ?? undefined}>
          {label ? `${label} · 127.0.0.1:${s.port}` : `127.0.0.1:${s.port}`}
          {s.message && s.state !== 'online' ? ` · ${s.message}` : ''}
        </span>
        {onStop && (
          <button className="btn btn-sm" onClick={onStop} title={isLive(s) ? 'Stop the tunnel' : 'Dismiss'}>
            <Square /> {isLive(s) ? 'Stop' : 'Dismiss'}
          </button>
        )}
      </div>
      {s.publicUrl && s.state === 'online' && (
        <div className="row-sm byo-url">
          <code className="code-inline truncate" title={s.publicUrl}>
            {s.publicUrl}
          </code>
          <CopyButton text={s.publicUrl} title="Copy public URL" />
          <button className="icon-btn sm" onClick={() => openUrl(s.publicUrl!).catch(() => {})} title="Open in browser" aria-label="Open in browser">
            <ExternalLink />
          </button>
        </div>
      )}
      {s.hint && s.state !== 'online' && (
        <Callout kind={s.state === 'error' ? 'danger' : 'warning'} className="xs">
          {s.hint}
        </Callout>
      )}
      {s.log.length > 0 && (
        <details className="byo-log">
          <summary className="xs subtle">Output ({s.log.length} lines)</summary>
          <pre className="code xs">{s.log.join('\n')}</pre>
        </details>
      )}
    </div>
  );
}

/** Exact install commands for this OS (others one click away) and the account steps. */
export function InstallSteps({ kind }: { kind: TunnelKind }) {
  const [platform, setPlatform] = useState(PLATFORM);
  const guide = installGuide(kind, platform);
  return (
    <div className="col-sm byo-install">
      <div className="row-sm">
        <span className="xs strong grow">Install</span>
        <Segmented
          value={platform}
          onChange={setPlatform}
          options={[
            { value: 'windows', label: 'Windows' },
            { value: 'macos', label: 'macOS' },
            { value: 'linux', label: 'Linux' },
          ]}
        />
      </div>
      {guide.options.map((o) => (
        <div key={o.via} className="byo-cmd">
          <span className="xs subtle byo-cmd-via">{o.via}</span>
          {o.command ? (
            <>
              <code className="code-inline truncate grow" title={o.command}>
                {o.command}
              </code>
              <CopyButton text={o.command} title="Copy command" />
            </>
          ) : (
            <button className="link xs grow" onClick={() => openUrl(o.url!).catch(() => {})}>
              {o.url} <ExternalLink size={10} />
            </button>
          )}
        </div>
      ))}
      <ol className="byo-then xs">
        {guide.then.map((t) => (
          <li key={t}>{t}</li>
        ))}
      </ol>
    </div>
  );
}

/**
 * One tunnel tool: readiness, install / setup help, its running tunnels and an expose action.
 * Used on the Tunnels page and the Webhooks page.
 */
export function ToolCard({
  tool,
  running,
  exposeLabel,
  exposeTitle,
  onExpose,
  disabled,
  labels,
}: {
  tool: ToolInfo;
  running: TunnelStatus[];
  exposeLabel: ReactNode;
  exposeTitle?: string;
  onExpose(): void;
  disabled?: boolean;
  labels?: Record<string, string>;
}) {
  const stop = useByoTunnels((s) => s.stop);
  const settings = useByoTunnels((s) => s.settings);
  const [editing, setEditing] = useState(false);
  const live = running.filter(isLive);
  const last = live.length ? undefined : running[running.length - 1];
  const needsToken = tool.kind === 'ngrok' && tool.installed && tool.ready === false;
  const customized = settingsSummary(tool.kind, settings?.settings);

  return (
    <div className={clsx('card byo-tool', !tool.installed && 'missing')}>
      <div className="row-sm byo-tool-head">
        {tool.installed ? tool.ready === false ? <CircleDashed size={14} className="text-warning" /> : <CheckCircle2 size={14} className="text-success" /> : <XCircle size={14} className="subtle" />}
        <span className="strong small grow truncate">{tool.label}</span>
        {tool.version && <span className="mono xs subtle">{tool.version}</span>}
        <button className="icon-btn sm" onClick={() => setEditing(true)} title={`${KIND_NAME[tool.kind]} settings`} aria-label={`${KIND_NAME[tool.kind]} settings`}>
          <Settings2 />
        </button>
      </div>
      <p className="xs subtle">{tool.notes}</p>
      {customized && <p className="xs byo-custom truncate" title={customized}>{customized}</p>}
      {!tool.installed ? (
        <>
          <InstallSteps kind={tool.kind} />
          <button className="link xs" onClick={() => openUrl(tool.docsUrl).catch(() => {})}>
            Docs <ExternalLink size={10} />
          </button>
        </>
      ) : (
        <>
          {needsToken && (
            <Callout kind="warning" className="xs">
              <span>
                ngrok needs your free authtoken.{' '}
                <button className="link xs" onClick={() => setEditing(true)}>
                  Paste it here
                </button>{' '}
                — it is kept in your OS keychain.
              </span>
            </Callout>
          )}
          {tool.hint && tool.ready === false && !needsToken && !live.length && (
            <Callout kind="warning" className="xs">
              {tool.hint}
            </Callout>
          )}
          {tool.predictedUrl && !live.length && <div className="xs subtle mono truncate">Will serve {tool.predictedUrl}</div>}
          {live.map((s) => (
            <TunnelStateView key={s.id} s={s} label={labels?.[s.id]} onStop={() => stop(s.id)} />
          ))}
          {last && <TunnelStateView s={last} label={labels?.[last.id]} onStop={() => stop(last.id)} />}
          <div className="row-sm byo-tool-actions">
            <button className="btn btn-sm btn-primary" onClick={onExpose} disabled={disabled || needsToken} title={exposeTitle}>
              <Play /> {exposeLabel}
            </button>
            {tool.path && (
              <span className="mono xs subtle truncate" title={tool.path}>
                {tool.path}
              </span>
            )}
          </div>
        </>
      )}
      {editing && <TunnelSettingsModal kind={tool.kind} onClose={() => setEditing(false)} />}
    </div>
  );
}

function settingsSummary(kind: TunnelKind, s: TunnelSettings | undefined): string | null {
  if (!s) return null;
  const parts: string[] = [];
  if (kind === 'ngrok') {
    if (s.ngrok.domain) parts.push(`domain ${s.ngrok.domain}`);
    if (s.ngrok.region) parts.push(`region ${s.ngrok.region}`);
    if (s.ngrok.basicAuthUser) parts.push('basic auth');
    if (s.ngrok.oauthProvider) parts.push(`OAuth ${s.ngrok.oauthProvider}`);
  } else if (kind === 'cloudflared') {
    if (s.cloudflared.mode !== 'quick') parts.push(`${s.cloudflared.mode} tunnel${s.cloudflared.hostname ? ` · ${s.cloudflared.hostname}` : ''}`);
  } else {
    if (s.tailscale.httpsPort && s.tailscale.httpsPort !== 443) parts.push(`:${s.tailscale.httpsPort}`);
    if (s.tailscale.path) parts.push(s.tailscale.path);
  }
  return parts.length ? `Configured: ${parts.join(' · ')}` : null;
}

/** A keychain-backed credential field: shows only whether it is stored. */
function SecretField({ name, label, hint, stored, onChanged }: { name: TunnelSecretName; label: string; hint?: ReactNode; stored: boolean; onChanged(): void }) {
  const [value, setValue] = useState('');
  const [busy, setBusy] = useState(false);
  const save = async (v: string | null) => {
    setBusy(true);
    try {
      await webhooksApi.tunnelSecretSet(name, v);
      setValue('');
      toast.success(v ? `${label} stored in your keychain` : `${label} removed`);
      onChanged();
    } catch (e) {
      toast.error('Keychain error', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Field label={label} hint={hint}>
      <div className="row-sm">
        <input
          className="input mono grow"
          type="password"
          autoComplete="off"
          value={value}
          placeholder={stored ? '•••••••• stored in keychain — paste to replace' : 'paste here'}
          onChange={(e) => setValue(e.target.value)}
          aria-label={label}
        />
        <button className="btn btn-sm" disabled={!value.trim() || busy} onClick={() => save(value)}>
          <KeyRound /> Save
        </button>
        {stored && (
          <button className="btn btn-sm btn-danger-ghost" disabled={busy} onClick={() => save(null)}>
            Remove
          </button>
        )}
      </div>
    </Field>
  );
}

const lines = (s: string) =>
  s
    .split(/[\n,]/)
    .map((x) => x.trim())
    .filter(Boolean);
const opt = (s: string) => (s.trim() ? s.trim() : null);

/** Per-tool settings (user prefs file + keychain credentials) with a live command preview and Test. */
export function TunnelSettingsModal({ kind, onClose }: { kind: TunnelKind; onClose(): void }) {
  const view = useByoTunnels((s) => s.settings);
  const loadSettings = useByoTunnels((s) => s.loadSettings);
  const saveSettings = useByoTunnels((s) => s.saveSettings);
  const [draft, setDraft] = useState<TunnelSettings | null>(view?.settings ?? null);
  const [command, setCommand] = useState<string>('');
  const [testing, setTesting] = useState(false);
  const [test, setTest] = useState<TunnelTest | null>(null);

  useEffect(() => {
    void loadSettings().then((v) => v && setDraft((d) => d ?? v.settings));
  }, [loadSettings]);
  const dirty = !!draft && JSON.stringify(draft) !== JSON.stringify(view?.settings);

  // Preview the command for the saved settings (it reads the file + keychain).
  const savedKey = JSON.stringify(view?.settings);
  const secretsKey = JSON.stringify(view?.secrets);
  useEffect(() => {
    webhooksApi
      .tunnelCommand(kind, 8080)
      .then(setCommand)
      .catch((e) => setCommand(`⚠ ${errorMessage(e)}`));
  }, [kind, savedKey, secretsKey]);

  if (!draft || !view)
    return (
      <Modal open onClose={onClose} title={`${KIND_NAME[kind]} settings`}>
        <Spinner />
      </Modal>
    );

  const save = async () => {
    if (await saveSettings(draft)) {
      toast.success('Tunnel settings saved');
      void useByoTunnels.getState().detect();
    }
  };
  const runTest = async () => {
    if (dirty && !(await saveSettings(draft))) return;
    setTesting(true);
    setTest(null);
    try {
      setTest(await webhooksApi.tunnelTest(kind));
    } catch (e) {
      setTest({ ok: false, publicUrl: null, message: errorMessage(e), hint: null });
    } finally {
      setTesting(false);
    }
  };
  const secretChanged = () => {
    void loadSettings();
    void useByoTunnels.getState().detect();
  };

  const n = draft.ngrok;
  const c = draft.cloudflared;
  const t = draft.tailscale;
  const setN = (p: Partial<TunnelSettings['ngrok']>) => setDraft({ ...draft, ngrok: { ...n, ...p } });
  const setC = (p: Partial<TunnelSettings['cloudflared']>) => setDraft({ ...draft, cloudflared: { ...c, ...p } });
  const setT = (p: Partial<TunnelSettings['tailscale']>) => setDraft({ ...draft, tailscale: { ...t, ...p } });
  const program = kind === 'ngrok' ? n.program : kind === 'cloudflared' ? c.program : t.program;
  const setProgram = (v: string) => (kind === 'ngrok' ? setN({ program: opt(v) }) : kind === 'cloudflared' ? setC({ program: opt(v) }) : setT({ program: opt(v) }));

  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      title={`${KIND_NAME[kind]} settings`}
      footer={
        <>
          <span className="xs subtle grow">Saved for your user on this machine (app data folder), never in a project. Credentials stay in the OS keychain.</span>
          <button className="btn" onClick={runTest} disabled={testing} title="Starts the tunnel briefly with these settings">
            {testing ? <Spinner /> : <Wifi />} Test
          </button>
          <button className="btn btn-primary" onClick={save} disabled={!dirty}>
            Save
          </button>
        </>
      }
    >
      <div className="col-lg byo-settings">
        {!view.keychainAvailable && <Callout kind="warning">The OS keychain is unavailable: credentials are kept in memory until the app quits.</Callout>}
        {kind === 'ngrok' && (
          <>
            <SecretField
              name="ngrok-authtoken"
              label="Authtoken"
              stored={!!view.secrets['ngrok-authtoken']}
              onChanged={secretChanged}
              hint={
                <>
                  Free at{' '}
                  <button className="link xs" onClick={() => openUrl('https://dashboard.ngrok.com/get-started/your-authtoken').catch(() => {})}>
                    dashboard.ngrok.com
                  </button>
                  . Passed to ngrok as NGROK_AUTHTOKEN; a token in your ngrok config file works too.
                </>
              }
            />
            <div className="byo-grid">
              <Field label="Reserved / static domain" hint="Your free static domain, e.g. my-app.ngrok-free.app — a stable URL for webhooks.">
                <input className="input mono" value={n.domain ?? ''} onChange={(e) => setN({ domain: opt(e.target.value) })} placeholder="my-app.ngrok-free.app" />
              </Field>
              <Field label="Region" hint="Empty: closest (recommended).">
                <select className="select" value={n.region ?? ''} onChange={(e) => setN({ region: opt(e.target.value) })}>
                  <option value="">automatic</option>
                  {['us', 'eu', 'ap', 'au', 'sa', 'jp', 'in'].map((r) => (
                    <option key={r}>{r}</option>
                  ))}
                </select>
              </Field>
              <Field label="Basic auth user" hint="Visitors must log in. Password below (≥ 8 characters).">
                <input className="input" value={n.basicAuthUser ?? ''} onChange={(e) => setN({ basicAuthUser: opt(e.target.value) })} placeholder="optional" autoComplete="off" />
              </Field>
              <Field label="OAuth provider" hint="ngrok's OAuth in front of the URL.">
                <select className="select" value={n.oauthProvider ?? ''} onChange={(e) => setN({ oauthProvider: opt(e.target.value) })}>
                  <option value="">none</option>
                  {['google', 'github', 'microsoft', 'gitlab', 'linkedin', 'twitch'].map((p) => (
                    <option key={p}>{p}</option>
                  ))}
                </select>
              </Field>
            </div>
            {n.basicAuthUser && (
              <SecretField name="ngrok-basic-auth-password" label="Basic auth password" stored={!!view.secrets['ngrok-basic-auth-password']} onChanged={secretChanged} hint="Stored in the keychain, but ngrok only accepts it as a --basic-auth argument, so other programs on this machine can see it in the process list while the tunnel runs. Prefer OAuth for anything sensitive." />
            )}
            {n.oauthProvider && (
              <div className="byo-grid">
                <Field label="Allowed emails" hint="Comma or newline separated.">
                  <textarea className="textarea" rows={2} value={n.oauthAllowEmails.join('\n')} onChange={(e) => setN({ oauthAllowEmails: lines(e.target.value) })} />
                </Field>
                <Field label="Allowed domains">
                  <textarea className="textarea" rows={2} value={n.oauthAllowDomains.join('\n')} onChange={(e) => setN({ oauthAllowDomains: lines(e.target.value) })} placeholder="example.com" />
                </Field>
              </div>
            )}
            <Field label="ngrok config file" hint="Optional --config (e.g. with your own endpoints and policies).">
              <input className="input mono" value={n.configFile ?? ''} onChange={(e) => setN({ configFile: opt(e.target.value) })} placeholder="optional path to ngrok.yml" />
            </Field>
          </>
        )}
        {kind === 'cloudflared' && (
          <>
            <Field label="Mode">
              <Segmented
                value={c.mode}
                onChange={(mode: CloudflaredMode) => setC({ mode })}
                options={[
                  { value: 'quick', label: 'Quick tunnel', title: 'Random trycloudflare.com URL, no account' },
                  { value: 'named', label: 'Named tunnel', title: 'cloudflared tunnel login + create, routed to the port' },
                  { value: 'token', label: 'Token', title: 'A tunnel from the Cloudflare dashboard, run with its token' },
                ]}
              />
            </Field>
            {c.mode === 'quick' && <p className="xs subtle">No account needed; the URL changes on every start.</p>}
            {c.mode === 'named' && (
              <div className="byo-grid">
                <Field label="Tunnel name or UUID" hint="From `cloudflared tunnel create <name>`.">
                  <input className="input mono" value={c.tunnelName ?? ''} onChange={(e) => setC({ tunnelName: opt(e.target.value) })} placeholder="devbox" />
                </Field>
                <Field label="Public hostname" hint="The DNS route of the tunnel (`cloudflared tunnel route dns`).">
                  <input className="input mono" value={c.hostname ?? ''} onChange={(e) => setC({ hostname: opt(e.target.value) })} placeholder="hooks.example.com" />
                </Field>
                <Field label="Config file" hint="Optional --config.">
                  <input className="input mono" value={c.configFile ?? ''} onChange={(e) => setC({ configFile: opt(e.target.value) })} placeholder="~/.cloudflared/config.yml" />
                </Field>
              </div>
            )}
            {c.mode === 'token' && (
              <>
                <SecretField
                  name="cloudflared-token"
                  label="Tunnel token"
                  stored={!!view.secrets['cloudflared-token']}
                  onChanged={secretChanged}
                  hint="Cloudflare dashboard → Networks → Tunnels → your tunnel → install command. Passed as TUNNEL_TOKEN."
                />
                <Field label="Public hostname" hint="Routes live in the dashboard: point the hostname's service at http://127.0.0.1:<port>.">
                  <input className="input mono" value={c.hostname ?? ''} onChange={(e) => setC({ hostname: opt(e.target.value) })} placeholder="hooks.example.com" />
                </Field>
              </>
            )}
          </>
        )}
        {kind === 'tailscale' && (
          <div className="byo-grid">
            <Field label="Public HTTPS port" hint="Funnel serves on 443, 8443 or 10000.">
              <select className="select" value={String(t.httpsPort ?? 443)} onChange={(e) => setT({ httpsPort: Number(e.target.value) === 443 ? null : Number(e.target.value) })}>
                {[443, 8443, 10000].map((p) => (
                  <option key={p}>{p}</option>
                ))}
              </select>
            </Field>
            <Field label="Path" hint="Serve under a path (--set-path), e.g. /hooks.">
              <input className="input mono" value={t.path ?? ''} onChange={(e) => setT({ path: opt(e.target.value) })} placeholder="/" />
            </Field>
          </div>
        )}
        <div className="byo-grid">
          <Field label={`${KIND_NAME[kind]} binary`} hint="Empty: found on PATH or in the usual install folders.">
            <input className="input mono" value={program ?? ''} onChange={(e) => setProgram(e.target.value)} placeholder="detected automatically" />
          </Field>
          <Field label="Extra arguments" hint='Shell-style, e.g. --request-header-add "x-env: dev".'>
            <input
              className="input mono"
              value={kind === 'ngrok' ? n.extraArgs : kind === 'cloudflared' ? c.extraArgs : t.extraArgs}
              onChange={(e) => (kind === 'ngrok' ? setN({ extraArgs: e.target.value }) : kind === 'cloudflared' ? setC({ extraArgs: e.target.value }) : setT({ extraArgs: e.target.value }))}
              placeholder="optional"
            />
          </Field>
        </div>
        <Field label="Command (for port 8080, saved settings)">
          <div className="row-sm">
            <code className="code-inline grow byo-command">{command || '…'}</code>
            {command && !command.startsWith('⚠') && <CopyButton text={command} title="Copy command" />}
          </div>
        </Field>
        {test && (
          <Callout kind={test.ok ? 'success' : 'danger'}>
            <div className="col-sm">
              <span>{test.message}</span>
              {test.hint && <span className="xs">{test.hint}</span>}
            </div>
          </Callout>
        )}
      </div>
    </Modal>
  );
}

/** Settings → Tunnels: the same per-tool settings, reachable without a running tunnel. */
export function TunnelSettingsSection() {
  const [editing, setEditing] = useState<TunnelKind | null>(null);
  const go = useNav((s) => s.go);
  return (
    <section className="col-sm">
      <h3 className="small strong">Tunnels — bring your own (free)</h3>
      <p className="xs subtle">
        ngrok authtoken, static domain and region; cloudflared quick, named or token tunnels; the Tailscale Funnel port. Saved for your user in the app data folder — never in a
        project — with credentials in the OS keychain.
      </p>
      <div className="row-sm wrap">
        {(['ngrok', 'cloudflared', 'tailscale'] as const).map((k) => (
          <button key={k} className="btn btn-sm" onClick={() => setEditing(k)}>
            <Settings2 /> {KIND_NAME[k]}…
          </button>
        ))}
        <button className="btn btn-sm btn-ghost" onClick={() => go('tunnels', 'byo')}>
          Open Tunnels <ArrowRight />
        </button>
      </div>
      {editing && <TunnelSettingsModal kind={editing} onClose={() => setEditing(null)} />}
    </section>
  );
}

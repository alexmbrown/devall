import { useState } from 'react';
import { KeyRound, RefreshCw, Trash2 } from 'lucide-react';
import { Callout, Field, Segmented, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { VarInput } from '@/components/VarInput';
import { errorMessage } from '@/lib/ipc';
import { usePluginCatalog } from '@/lib/api/plugins';
import type { Auth, OAuth2Grant, TokenInfo, VarInfo } from '@/lib/types';

const TYPES: { value: Auth['type']; label: string }[] = [
  { value: 'inherit', label: 'Inherit' },
  { value: 'none', label: 'None' },
  { value: 'bearer', label: 'Bearer' },
  { value: 'basic', label: 'Basic' },
  { value: 'apiKey', label: 'API key' },
  { value: 'oauth2', label: 'OAuth 2.0' },
  { value: 'awsSigV4', label: 'AWS SigV4' },
  { value: 'digest', label: 'Digest' },
  { value: 'ntlm', label: 'NTLM' },
  { value: 'plugin', label: 'Plugin…' },
];

function blank(type: Auth['type']): Auth {
  switch (type) {
    case 'basic':
      return { type, username: '', password: '' };
    case 'bearer':
      return { type, token: '' };
    case 'apiKey':
      return { type, key: 'X-API-Key', value: '', location: 'header' };
    case 'oauth2':
      return { type, grant: 'clientCredentials', tokenUrl: '', clientId: '', clientSecret: '{{secret.CLIENT_SECRET}}', scope: '' };
    case 'awsSigV4':
      return { type, accessKeyId: '{{secret.AWS_ACCESS_KEY_ID}}', secretAccessKey: '{{secret.AWS_SECRET_ACCESS_KEY}}', region: 'us-east-1', service: 'execute-api' };
    case 'digest':
      return { type, username: '', password: '' };
    case 'ntlm':
      return { type, username: '', password: '{{secret.PASSWORD}}' };
    case 'plugin': {
      const first = usePluginCatalog.getState().catalog.auth[0];
      return { type, plugin: first?.plugin ?? '', provider: first?.id ?? '', config: Object.fromEntries((first?.fields ?? []).map((f) => [f.name, f.default])) };
    }
    default:
      return { type } as Auth;
  }
}

export interface TokenActions {
  get(force: boolean): Promise<TokenInfo>;
  clear(): Promise<void>;
}

export function AuthEditor({
  auth,
  onChange,
  vars,
  isFolder,
  tokens,
}: {
  auth: Auth;
  onChange(a: Auth): void;
  vars: VarInfo[];
  isFolder?: boolean;
  /** OAuth 2.0 token actions (requests only). */
  tokens?: TokenActions;
}) {
  const v = (value: string | undefined, set: (s: string) => void, placeholder?: string) => <VarInput value={value ?? ''} onChange={set} vars={vars} placeholder={placeholder} />;
  return (
    <div className="col-lg" style={{ maxWidth: 680 }}>
      <select className="select" style={{ maxWidth: 240 }} value={auth.type} onChange={(e) => onChange(blank(e.target.value as Auth['type']))} aria-label="Auth type">
        {TYPES.map((t) => (
          <option key={t.value} value={t.value}>
            {t.label}
          </option>
        ))}
      </select>
      {auth.type === 'inherit' && (
        <p className="muted small">
          {isFolder ? 'Requests in this folder use the parent folder’s auth.' : 'Uses the auth of the nearest folder that sets one (folder settings), or none.'}
        </p>
      )}
      {auth.type === 'none' && <p className="muted small">No Authorization header is added.</p>}
      {auth.type === 'bearer' && (
        <>
          <Field label="Token" hint="Tip: store tokens as secrets — {{secret.API_TOKEN}} — so they never land in git.">
            {v(auth.token, (token) => onChange({ ...auth, token }), '{{secret.API_TOKEN}}')}
          </Field>
          <Field label="Prefix" hint="Sent as “<prefix> <token>”. Leave empty for the default “Bearer”.">
            <input className="input" value={auth.prefix ?? ''} placeholder="Bearer" onChange={(e) => onChange({ ...auth, prefix: e.target.value || undefined })} />
          </Field>
        </>
      )}
      {(auth.type === 'basic' || auth.type === 'digest') && (
        <>
          <div className="grid-2">
            <Field label="Username">{v(auth.username, (username) => onChange({ ...auth, username }))}</Field>
            <Field label="Password">{v(auth.password, (password) => onChange({ ...auth, password }), '{{secret.PASSWORD}}')}</Field>
          </div>
          {auth.type === 'digest' && <p className="subtle xs">The first response’s 401 challenge is answered automatically (MD5, SHA-256, -sess variants, qop=auth).</p>}
        </>
      )}
      {auth.type === 'ntlm' && (
        <>
          <div className="grid-2">
            <Field label="Username" hint={'Also accepts DOMAIN\\user or user@domain.'}>
              {v(auth.username, (username) => onChange({ ...auth, username }))}
            </Field>
            <Field label="Password">{v(auth.password, (password) => onChange({ ...auth, password }), '{{secret.PASSWORD}}')}</Field>
            <Field label="Domain (optional)">{v(auth.domain, (d) => onChange({ ...auth, domain: d || undefined }), 'CORP')}</Field>
            <Field label="Workstation (optional)">{v(auth.workstation, (w) => onChange({ ...auth, workstation: w || undefined }))}</Field>
          </div>
          <p className="subtle xs">
            NTLMv2 handshake (negotiate → challenge → authenticate) on one kept-alive HTTP/1.1 connection. Servers that close the connection between the two legs can’t complete it.
          </p>
        </>
      )}
      {auth.type === 'apiKey' && (
        <>
          <div className="grid-2">
            <Field label="Key">
              <input className="input mono" value={auth.key} onChange={(e) => onChange({ ...auth, key: e.target.value })} />
            </Field>
            <Field label="Value">{v(auth.value, (value) => onChange({ ...auth, value }), '{{secret.API_KEY}}')}</Field>
          </div>
          <Field label="Add to">
            <Segmented
              value={auth.location}
              onChange={(location) => onChange({ ...auth, location })}
              options={[
                { value: 'header', label: 'Header' },
                { value: 'query', label: 'Query parameter' },
              ]}
            />
          </Field>
        </>
      )}
      {auth.type === 'awsSigV4' && (
        <>
          <div className="grid-2">
            <Field label="Access key ID">{v(auth.accessKeyId, (accessKeyId) => onChange({ ...auth, accessKeyId }))}</Field>
            <Field label="Secret access key">{v(auth.secretAccessKey, (secretAccessKey) => onChange({ ...auth, secretAccessKey }))}</Field>
            <Field label="Region">{v(auth.region, (region) => onChange({ ...auth, region }), 'us-east-1')}</Field>
            <Field label="Service">{v(auth.service, (service) => onChange({ ...auth, service }), 'execute-api, s3, lambda…')}</Field>
          </div>
          <Field label="Session token (optional)">{v(auth.sessionToken, (s) => onChange({ ...auth, sessionToken: s || undefined }), '{{secret.AWS_SESSION_TOKEN}}')}</Field>
        </>
      )}
      {auth.type === 'oauth2' && <OAuth2Form auth={auth} onChange={onChange} v={v} tokens={tokens} />}
      {auth.type === 'plugin' && <PluginAuthForm auth={auth} onChange={onChange} v={v} />}
    </div>
  );
}

function OAuth2Form({
  auth,
  onChange,
  v,
  tokens,
}: {
  auth: Extract<Auth, { type: 'oauth2' }>;
  onChange(a: Auth): void;
  v(value: string | undefined, set: (s: string) => void, placeholder?: string): React.ReactNode;
  tokens?: TokenActions;
}) {
  const [info, setInfo] = useState<TokenInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const grant = auth.grant ?? 'clientCredentials';
  const set = (patch: Partial<typeof auth>) => onChange({ ...auth, ...patch });
  const get = async (force: boolean) => {
    if (!tokens) return;
    setBusy(true);
    try {
      setInfo(await tokens.get(force));
      toast.success('Token ready', 'Cached in your OS keychain; refreshed automatically.');
    } catch (e) {
      toast.error('Could not get a token', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Field label="Grant type">
        <Segmented<OAuth2Grant>
          value={grant}
          onChange={(g) => set({ grant: g })}
          options={[
            { value: 'clientCredentials', label: 'Client credentials' },
            { value: 'authorizationCode', label: 'Authorization code' },
            { value: 'password', label: 'Password' },
          ]}
        />
      </Field>
      {grant === 'authorizationCode' && <Field label="Authorization URL">{v(auth.authUrl, (authUrl) => set({ authUrl }), 'https://login.example.com/oauth2/authorize')}</Field>}
      <Field label="Token URL">{v(auth.tokenUrl, (tokenUrl) => set({ tokenUrl }), 'https://login.example.com/oauth2/token')}</Field>
      <div className="grid-2">
        <Field label="Client ID">{v(auth.clientId, (clientId) => set({ clientId }))}</Field>
        <Field label="Client secret">{v(auth.clientSecret, (clientSecret) => set({ clientSecret }), '{{secret.CLIENT_SECRET}}')}</Field>
        <Field label="Scope">{v(auth.scope, (scope) => set({ scope }), 'read write')}</Field>
        <Field label="Audience (optional)">{v(auth.audience, (a) => set({ audience: a || undefined }))}</Field>
      </div>
      {grant === 'password' && (
        <div className="grid-2">
          <Field label="Username">{v(auth.username, (username) => set({ username }))}</Field>
          <Field label="Password">{v(auth.password, (password) => set({ password }), '{{secret.PASSWORD}}')}</Field>
        </div>
      )}
      {grant === 'authorizationCode' && (
        <div className="row wrap">
          <Switch checked={auth.usePkce !== false} onChange={(on) => set({ usePkce: on ? undefined : false })} label="PKCE (S256)" />
          <Field label="Callback port (optional)" hint={`Redirect URI: http://127.0.0.1:${auth.redirectPort ?? '<random>'}/callback`}>
            <input
              className="input sm"
              type="number"
              min={1}
              max={65535}
              style={{ width: 120 }}
              value={auth.redirectPort ?? ''}
              onChange={(e) => set({ redirectPort: e.target.value ? Number(e.target.value) : undefined })}
            />
          </Field>
        </div>
      )}
      <div className="row wrap">
        <Field label="Client authentication">
          <Segmented
            value={auth.clientAuth ?? 'basic'}
            onChange={(c) => set({ clientAuth: c === 'basic' ? undefined : c })}
            options={[
              { value: 'basic', label: 'Basic header' },
              { value: 'body', label: 'In body' },
            ]}
          />
        </Field>
        <Field label="Header prefix">
          <input className="input sm" style={{ width: 120 }} value={auth.tokenPrefix ?? ''} placeholder="Bearer" onChange={(e) => set({ tokenPrefix: e.target.value || undefined })} />
        </Field>
      </div>
      {tokens && (
        <div className="panel panel-body row wrap">
          <KeyRound size={15} className="text-accent" />
          {info ? (
            <span className="small">
              Token <span className="mono">{info.preview}</span>
              {info.expiresAt && <> · expires {new Date(info.expiresAt * 1000).toLocaleTimeString()}</>}
              {info.hasRefreshToken && ' · refreshable'}
            </span>
          ) : (
            <span className="muted small">Tokens are fetched automatically on send and cached in your keychain.</span>
          )}
          <span className="spacer" />
          <button className="btn btn-sm" disabled={busy} onClick={() => get(false)}>
            <KeyRound /> Get token
          </button>
          <button className="btn btn-sm btn-ghost" disabled={busy} onClick={() => get(true)} title="Ignore the cached token">
            <RefreshCw /> New token
          </button>
          <button
            className="btn btn-sm btn-ghost"
            onClick={async () => {
              await tokens.clear();
              setInfo(null);
              toast.info('Cached token removed');
            }}
          >
            <Trash2 /> Clear
          </button>
        </div>
      )}
    </>
  );
}

/** `auth: { type: plugin }`: pick a plugin auth provider and fill its fields. */
function PluginAuthForm({
  auth,
  onChange,
  v,
}: {
  auth: Extract<Auth, { type: 'plugin' }>;
  onChange(a: Auth): void;
  v(value: string | undefined, set: (s: string) => void, placeholder?: string): React.ReactNode;
}) {
  const providers = usePluginCatalog((s) => s.catalog.auth);
  const current = providers.find((p) => p.plugin === auth.plugin && p.id === auth.provider);
  const key = `${auth.plugin}/${auth.provider}`;
  const config = auth.config ?? {};
  return (
    <>
      <Field label="Auth provider" hint="Provided by an installed (or trusted project) plugin — see the Plugins page.">
        <select
          className="select"
          value={key}
          onChange={(e) => {
            const p = providers.find((x) => `${x.plugin}/${x.id}` === e.target.value);
            if (p) onChange({ type: 'plugin', plugin: p.plugin, provider: p.id, config: Object.fromEntries(p.fields.map((f) => [f.name, config[f.name] ?? f.default])) });
          }}
        >
          {!current && <option value={key}>{auth.plugin ? `${key} (not available)` : 'Choose…'}</option>}
          {providers.map((p) => (
            <option key={`${p.plugin}/${p.id}`} value={`${p.plugin}/${p.id}`}>
              {p.name} — {p.pluginName}
            </option>
          ))}
        </select>
      </Field>
      {!current && (
        <Callout kind="warning">
          {providers.length === 0
            ? 'No plugin with an auth provider is installed or trusted for this project.'
            : `The plugin “${auth.plugin}” is not installed, trusted or enabled here; requests using it will fail.`}
        </Callout>
      )}
      {current?.description && <p className="muted small">{current.description}</p>}
      {(current?.fields ?? Object.keys(config).map((name) => ({ name, label: name, kind: 'text', default: '' }))).map((f) => (
        <Field key={f.name} label={f.label || f.name} hint={f.kind === 'secret' ? 'Use {{secret.NAME}} so the value stays in the keychain.' : undefined}>
          {f.kind.startsWith('select:') ? (
            <select className="select" value={config[f.name] ?? f.default} onChange={(e) => onChange({ ...auth, config: { ...config, [f.name]: e.target.value } })}>
              {f.kind
                .slice(7)
                .split('|')
                .map((o) => (
                  <option key={o}>{o}</option>
                ))}
            </select>
          ) : (
            v(config[f.name], (val) => onChange({ ...auth, config: { ...config, [f.name]: val } }), f.kind === 'secret' ? '{{secret.NAME}}' : f.default)
          )}
        </Field>
      ))}
    </>
  );
}

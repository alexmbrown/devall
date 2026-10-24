import { useEffect, useMemo, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ExternalLink, KeyRound, Save } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Field, Segmented, Spinner, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import { webhooksApi, type ForwardSpec, type HmacAlgorithm, type InboxFile, type Issue, type ProviderKind, type SignatureEncoding } from '@/lib/api/webhooks';
import { useWebhooks } from './store';

const SECRET_RE = /^\s*\{\{\s*secret\.([A-Za-z0-9_.-]+)\s*\}\}\s*$/;

function slug(s: string) {
  return (
    s
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 64) || 'inbox'
  );
}

function headersToText(h?: Record<string, string>) {
  return Object.entries(h ?? {})
    .map(([k, v]) => `${k}: ${v}`)
    .join('\n');
}

function textToHeaders(t: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of t.split('\n')) {
    const i = line.indexOf(':');
    if (i <= 0) continue;
    out[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return out;
}

/** New / edit inbox. Secrets are stored in the OS keychain and referenced as `{{secret.NAME}}`. */
export function InboxDialog({ root, envId, id, onClose, onSaved }: { root: string; envId?: string; id?: string; onClose(): void; onSaved(id: string): void }) {
  const providers = useWebhooks((s) => s.providers);
  const [draft, setDraft] = useState<InboxFile | null>(id ? null : { name: '', path: '', provider: 'stripe', secret: '{{secret.STRIPE_WEBHOOK_SECRET}}' });
  const [pathTouched, setPathTouched] = useState(!!id);
  const [secretValue, setSecretValue] = useState('');
  const [secretScope, setSecretScope] = useState<'all' | 'env'>('all');
  const [respHeaders, setRespHeaders] = useState('');
  const [issues, setIssues] = useState<Issue[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!id) return;
    webhooksApi
      .read(root, id)
      .then((f) => {
        setDraft(f);
        setRespHeaders(headersToText(f.response?.headers));
      })
      .catch((e) => {
        toast.error('Could not open inbox', errorMessage(e));
        onClose();
      });
  }, [root, id, onClose]);

  const info = providers.find((p) => p.kind === (draft?.provider ?? 'none'));
  const secretName = useMemo(() => draft?.secret?.match(SECRET_RE)?.[1], [draft?.secret]);
  const plainSecret = !!draft?.secret && !draft.secret.includes('{{');

  if (!draft)
    return (
      <Modal open onClose={onClose} title="Inbox">
        <div className="empty">
          <Spinner />
        </div>
      </Modal>
    );

  const set = (patch: Partial<InboxFile>) => setDraft((d) => (d ? { ...d, ...patch } : d));
  const setForward = (patch: Partial<ForwardSpec> | null) =>
    setDraft((d) => (d ? { ...d, forward: patch === null ? undefined : { url: '', ...d.forward, ...patch } } : d));

  const pickProvider = (kind: ProviderKind) => {
    const p = providers.find((x) => x.kind === kind);
    const wasDefault = !draft.secret || SECRET_RE.test(draft.secret);
    set({
      provider: kind,
      secret: kind === 'none' ? '' : wasDefault && p?.secretName ? `{{secret.${p.secretName}}}` : draft.secret,
      genericHmac: kind === 'genericHmac' ? (draft.genericHmac ?? { header: 'X-Signature', algorithm: 'sha256', encoding: 'hex', prefix: 'sha256=' }) : undefined,
    });
  };

  const save = async () => {
    setBusy(true);
    try {
      const file: InboxFile = { ...draft, name: draft.name.trim(), path: draft.path.trim() || `/${slug(draft.name)}` };
      if (!file.path.startsWith('/')) file.path = `/${file.path}`;
      const headers = textToHeaders(respHeaders);
      file.response = { ...file.response, headers: Object.keys(headers).length ? headers : undefined };
      if (file.response && !file.response.headers && !file.response.body && !file.response.delayMs && (file.response.status ?? 200) === 200) delete file.response;
      if (file.forward && !file.forward.url.trim()) delete file.forward;
      let targetId = id;
      if (!targetId) targetId = await webhooksApi.create(root, file.name || 'Inbox', file.provider ?? 'none');
      const found = await webhooksApi.save(root, targetId, file);
      if (secretValue && secretName) {
        await api.setSecret(root, secretScope === 'env' && envId ? envId : '', secretName, secretValue);
        setSecretValue('');
      }
      setIssues(found);
      onSaved(targetId);
      if (found.length === 0) {
        toast.success(id ? 'Inbox saved' : 'Inbox created', `webhooks/${targetId}.webhook.yaml`);
        onClose();
      }
    } catch (e) {
      toast.error('Could not save inbox', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const g = draft.genericHmac;
  const fw = draft.forward;

  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      title={id ? `Edit inbox — ${draft.name || id}` : 'New inbox'}
      footer={
        <>
          <span className="subtle xs grow">{id ? `webhooks/${id}.webhook.yaml` : 'Saved as webhooks/<name>.webhook.yaml — commit it with your project.'}</span>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={save} disabled={busy || !draft.name.trim()}>
            {busy ? <Spinner /> : <Save />} {id ? 'Save' : 'Create'}
          </button>
        </>
      }
    >
      <div className="wh-dialog">
        {issues.length > 0 && (
          <Callout kind="warning">
            Saved with problems:
            <ul>
              {issues.map((i, n) => (
                <li key={n}>
                  <code className="code-inline">{i.field}</code> {i.message}
                </li>
              ))}
            </ul>
          </Callout>
        )}
        <div className="grid-2">
          <Field label="Name">
            <input
              className="input"
              value={draft.name}
              autoFocus
              placeholder="Stripe"
              onChange={(e) => set({ name: e.target.value, ...(pathTouched ? {} : { path: `/${slug(e.target.value)}` }) })}
            />
          </Field>
          <Field label="Path" hint="Receiver path; sub-paths route here too.">
            <input
              className="input mono"
              value={draft.path}
              placeholder="/stripe"
              onChange={(e) => {
                setPathTouched(true);
                set({ path: e.target.value });
              }}
            />
          </Field>
        </div>

        <Field label="Provider">
          <div className="wh-providers">
            {providers.map((p) => (
              <button key={p.kind} type="button" className={draft.provider === p.kind || (!draft.provider && p.kind === 'none') ? 'on' : ''} onClick={() => pickProvider(p.kind)}>
                {p.label}
              </button>
            ))}
          </div>
        </Field>
        {info && info.kind !== 'none' && (
          <div className="wh-provider-help">
            <div className="xs">
              <span className="subtle">Signature: </span>
              <span className="mono">{info.algorithm}</span>
            </div>
            <div className="xs">
              <KeyRound size={12} className="subtle" /> {info.secretHint}
              {info.docsUrl && (
                <>
                  {' '}
                  <button className="link xs" onClick={() => openUrl(info.docsUrl!).catch(() => {})}>
                    Docs <ExternalLink size={10} />
                  </button>
                </>
              )}
            </div>
          </div>
        )}

        {draft.provider === 'genericHmac' && (
          <div className="wh-fields">
            <Field label="Signature header">
              <input className="input sm mono" value={g?.header ?? ''} placeholder="X-Signature" onChange={(e) => set({ genericHmac: { ...g, header: e.target.value } })} />
            </Field>
            <Field label="Algorithm">
              <Segmented<HmacAlgorithm>
                value={g?.algorithm ?? 'sha256'}
                onChange={(v) => set({ genericHmac: { header: g?.header ?? '', ...g, algorithm: v } })}
                options={[
                  { value: 'sha256', label: 'SHA-256' },
                  { value: 'sha1', label: 'SHA-1' },
                  { value: 'sha512', label: 'SHA-512' },
                ]}
              />
            </Field>
            <Field label="Encoding">
              <Segmented<SignatureEncoding>
                value={g?.encoding ?? 'hex'}
                onChange={(v) => set({ genericHmac: { header: g?.header ?? '', ...g, encoding: v } })}
                options={[
                  { value: 'hex', label: 'Hex' },
                  { value: 'base64', label: 'Base64' },
                ]}
              />
            </Field>
            <Field label="Prefix">
              <input className="input sm mono" value={g?.prefix ?? ''} placeholder="sha256=" onChange={(e) => set({ genericHmac: { header: g?.header ?? '', ...g, prefix: e.target.value } })} />
            </Field>
          </div>
        )}

        {draft.provider && draft.provider !== 'none' && (
          <>
            <div className="grid-2">
              <Field label="Secret reference" hint="Project files only hold the reference; the value stays in your OS keychain." error={plainSecret ? 'This is a literal secret in a project file — use {{secret.NAME}}.' : undefined}>
                <input className="input mono" value={draft.secret ?? ''} placeholder="{{secret.WEBHOOK_SECRET}}" onChange={(e) => set({ secret: e.target.value })} />
              </Field>
              <Field label={secretName ? `Set ${secretName}` : 'Secret value'} hint={secretName ? 'Optional — stored in the keychain on save. Leave empty to keep the current value.' : 'Use {{secret.NAME}} to store a value here.'}>
                <div className="row-sm">
                  <input
                    className="input mono"
                    type="password"
                    autoComplete="off"
                    value={secretValue}
                    disabled={!secretName}
                    placeholder={info?.kind === 'stripe' ? 'whsec_…' : info?.kind === 'standardWebhooks' ? 'whsec_…' : '••••••'}
                    onChange={(e) => setSecretValue(e.target.value)}
                  />
                  {envId && (
                    <Segmented<'all' | 'env'>
                      value={secretScope}
                      onChange={setSecretScope}
                      options={[
                        { value: 'all', label: 'All envs', title: 'Store for every environment of this project' },
                        { value: 'env', label: 'This env', title: 'Store only for the active environment' },
                      ]}
                    />
                  )}
                </div>
              </Field>
            </div>
            <div className="grid-2">
              {info?.timestamped && (
                <Field label="Timestamp tolerance (s)" hint="Older signed timestamps fail verification (providers use 300).">
                  <input
                    className="input sm mono"
                    inputMode="numeric"
                    value={draft.toleranceSeconds ?? 300}
                    onChange={(e) => set({ toleranceSeconds: Number(e.target.value.replace(/\D/g, '')) || 0 })}
                  />
                </Field>
              )}
              <Field
                label="Public URL"
                hint={draft.provider === 'twilio' ? 'Twilio signs the exact URL it calls — enter the URL configured in the Twilio console (an exposed tunnel fills it for the session).' : 'Optional: the exact URL the provider calls (stable tunnel or relay URL).'}
              >
                <input className="input sm mono" value={draft.publicUrl ?? ''} placeholder="https://…/twilio" onChange={(e) => set({ publicUrl: e.target.value || undefined })} />
              </Field>
            </div>
          </>
        )}

        <fieldset className="wh-group">
          <legend>
            <Switch checked={!!fw} onChange={(v) => setForward(v ? { url: fw?.url ?? 'http://localhost:3000/webhooks' } : null)} label="Forward to my app" />
          </legend>
          {fw && (
            <div className="col">
              <Field label="Target URL" hint="The exact body and headers are sent; the received query string is appended. {{variables}} resolve with the active environment.">
                <input className="input mono" value={fw.url} placeholder="http://localhost:3000/webhooks/stripe" onChange={(e) => setForward({ url: e.target.value })} />
              </Field>
              <div className="row wrap">
                <Switch checked={fw.enabled !== false} onChange={(v) => setForward({ enabled: v })} label="Enabled" />
                <Switch checked={!!fw.passThrough} onChange={(v) => setForward({ passThrough: v })} label="Answer with my app's response" title="Wait for your app and relay its status, headers and body to the sender (Slack slash commands, Twilio TwiML)" />
                <Switch checked={!!fw.preserveHost} onChange={(v) => setForward({ preserveHost: v })} label="Preserve Host header" />
                <label className="row-sm small subtle">
                  Timeout (ms)
                  <input className="input sm mono wh-num" inputMode="numeric" value={fw.timeoutMs ?? 10000} onChange={(e) => setForward({ timeoutMs: Number(e.target.value.replace(/\D/g, '')) || 0 })} />
                </label>
              </div>
            </div>
          )}
        </fieldset>

        <fieldset className="wh-group">
          <legend className="small strong">Response to the sender{fw?.passThrough ? ' (used when your app is unreachable: 502)' : ''}</legend>
          <div className="wh-fields">
            <Field label="Status">
              <input
                className="input sm mono"
                inputMode="numeric"
                value={draft.response?.status ?? 200}
                onChange={(e) => set({ response: { ...draft.response, status: Number(e.target.value.replace(/\D/g, '').slice(0, 3)) || 0 } })}
              />
            </Field>
            <Field label="Delay (ms)" hint="Test the sender's timeout (Shopify gives up after 5 s).">
              <input
                className="input sm mono"
                inputMode="numeric"
                value={draft.response?.delayMs ?? 0}
                onChange={(e) => set({ response: { ...draft.response, delayMs: Number(e.target.value.replace(/\D/g, '')) || undefined } })}
              />
            </Field>
          </div>
          <div className="grid-2">
            <Field label="Headers" hint="One per line: Name: value">
              <textarea className="textarea mono" rows={3} value={respHeaders} placeholder="Content-Type: application/json" onChange={(e) => setRespHeaders(e.target.value)} />
            </Field>
            <Field label="Body">
              <textarea className="textarea mono" rows={3} value={draft.response?.body ?? ''} placeholder='{"received": true}' onChange={(e) => set({ response: { ...draft.response, body: e.target.value || undefined } })} />
            </Field>
          </div>
        </fieldset>
      </div>
    </Modal>
  );
}

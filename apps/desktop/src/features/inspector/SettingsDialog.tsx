import { useEffect, useState } from 'react';
import { Settings2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Field, Spinner, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { proxyApi, type ProxyConfig } from '@/lib/api/proxy';

const lines = (v: string) =>
  v
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean);

/** Proxy settings: bypass list, upstream TLS, capture limits, timeouts. */
export function SettingsDialog({ open, onClose }: { open: boolean; onClose(): void }) {
  const [cfg, setCfg] = useState<ProxyConfig | null>(null);
  const [bypass, setBypass] = useState('');
  const [ignore, setIgnore] = useState('');
  const [roots, setRoots] = useState('');

  useEffect(() => {
    if (!open) return;
    proxyApi
      .settings()
      .then((c) => {
        setCfg(c);
        setBypass(c.bypass.join('\n'));
        setIgnore(c.ignoreTlsErrors.join('\n'));
        setRoots(c.extraRootsPem.join('\n'));
      })
      .catch((e) => toast.error('Could not load settings', errorMessage(e)));
  }, [open]);

  const save = async () => {
    if (!cfg) return;
    const pems = roots.match(/-----BEGIN CERTIFICATE-----[\s\S]*?-----END CERTIFICATE-----/g) ?? [];
    try {
      await proxyApi.saveSettings({ ...cfg, bypass: lines(bypass), ignoreTlsErrors: lines(ignore), extraRootsPem: pems });
      toast.success('Proxy settings saved');
      onClose();
    } catch (e) {
      toast.error('Could not save settings', errorMessage(e));
    }
  };

  const num = (k: keyof ProxyConfig, scale = 1) => ({
    value: cfg ? String(Math.round((cfg[k] as number) / scale)) : '',
    onChange: (e: React.ChangeEvent<HTMLInputElement>) => cfg && setCfg({ ...cfg, [k]: Math.max(0, Number(e.target.value) || 0) * scale }),
  });

  return (
    <Modal
      open={open}
      onClose={onClose}
      size="lg"
      title={
        <span className="row-sm">
          <Settings2 size={16} /> Proxy settings
        </span>
      }
      footer={
        <>
          <span className="spacer" />
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={save} disabled={!cfg}>
            Save
          </button>
        </>
      }
    >
      {!cfg ? (
        <div className="empty">
          <Spinner />
        </div>
      ) : (
        <div className="col">
          <div className="grid-2">
            <Field label="Bypass — tunnel without decrypting" hint="One host per line; example.com also covers subdomains, *.apple.com is a glob. Use for certificate-pinned apps.">
              <textarea className="textarea mono" rows={4} value={bypass} onChange={(e) => setBypass(e.target.value)} placeholder={'*.apple.com\nlogin.microsoftonline.com'} />
            </Field>
            <Field label="Ignore upstream TLS errors for" hint="Self-signed dev servers. Same pattern syntax.">
              <textarea className="textarea mono" rows={4} value={ignore} onChange={(e) => setIgnore(e.target.value)} placeholder={'localhost\n*.internal'} />
            </Field>
          </div>
          <div className="row wrap">
            <Switch checked={cfg.systemRoots} onChange={(v) => setCfg({ ...cfg, systemRoots: v })} label="Verify upstream certificates against the OS trust store" />
            <Switch checked={cfg.upstreamHttp2} onChange={(v) => setCfg({ ...cfg, upstreamHttp2: v })} label="HTTP/2 to servers" />
          </div>
          <Field label="Extra trusted root certificates (PEM)" hint="Corporate or local CAs your servers use.">
            <textarea className="textarea mono" rows={3} value={roots} onChange={(e) => setRoots(e.target.value)} placeholder="-----BEGIN CERTIFICATE-----" />
          </Field>
          <div className="grid-3">
            <Field label="Port" hint="Applies on next start">
              <input className="input sm" {...num('port')} />
            </Field>
            <Field label="Exchanges kept">
              <input className="input sm" {...num('maxExchanges')} />
            </Field>
            <Field label="Body kept per message (MB)">
              <input className="input sm" {...num('maxBodyBytes', 1024 * 1024)} />
            </Field>
            <Field label="Capture memory budget (MB)">
              <input className="input sm" {...num('maxTotalBodyBytes', 1024 * 1024)} />
            </Field>
            <Field label="Breakpoint timeout (s)">
              <input className="input sm" {...num('breakpointTimeoutSecs')} />
            </Field>
            <Field label="Upstream response timeout (s)">
              <input className="input sm" {...num('upstreamTimeoutSecs')} />
            </Field>
          </div>
        </div>
      )}
    </Modal>
  );
}

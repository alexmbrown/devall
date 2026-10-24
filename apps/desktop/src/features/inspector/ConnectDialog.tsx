import { useCallback, useEffect, useState } from 'react';
import { Globe, RefreshCw, ShieldCheck, ShieldOff, ShieldQuestion, Terminal } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, CopyButton, Segmented, Spinner } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { proxyApi, type BrowserKind, type CaView, type EnvSnippet } from '@/lib/api/proxy';
import { useInspector } from './store';

export function TrustBadge({ installed }: { installed: boolean | null | undefined }) {
  if (installed === true)
    return (
      <span className="badge success">
        <ShieldCheck /> CA trusted
      </span>
    );
  if (installed === false)
    return (
      <span className="badge warning">
        <ShieldOff /> CA not trusted
      </span>
    );
  return (
    <span className="badge">
      <ShieldQuestion /> CA status unknown
    </span>
  );
}

export async function launchBrowser(kind: BrowserKind, name: string) {
  try {
    const l = await proxyApi.launchBrowser(kind);
    toast.success(`${name} started through the proxy`, `Isolated profile: ${l.profileDir}`);
    void useInspector.getState().refreshStatus();
  } catch (e) {
    toast.error(`Could not start ${name}`, errorMessage(e));
  }
}

/** "Connect apps & trust the CA": what the CA is, exact OS commands (run only after confirming),
 * no-install browser launch, terminal snippets and per-tool notes. */
export function ConnectDialog({ open, onClose, onTrustChanged }: { open: boolean; onClose(): void; onTrustChanged(installed: boolean | null): void }) {
  const [view, setView] = useState<CaView | null>(null);
  const [checking, setChecking] = useState(false);
  const [shell, setShell] = useState<EnvSnippet['shell']>(() => (navigator.platform.startsWith('Win') ? 'powershell' : 'bash'));
  const [busy, setBusy] = useState(false);
  const [output, setOutput] = useState<string | null>(null);

  const load = useCallback(
    async (check: boolean) => {
      setChecking(check);
      try {
        const v = await proxyApi.caInfo(check);
        setView(v);
        if (check) onTrustChanged(v.trust.installed);
      } catch (e) {
        toast.error('Could not read the CA', errorMessage(e));
      } finally {
        setChecking(false);
      }
    },
    [onTrustChanged],
  );

  useEffect(() => {
    if (open) {
      setOutput(null);
      void load(true);
    }
  }, [open, load]);

  const run = async (which: 'install' | 'uninstall') => {
    const cmd = which === 'install' ? view?.trust.install : view?.trust.uninstall;
    if (!cmd) return;
    const ok = await confirmDialog({
      title: which === 'install' ? 'Trust the /dev/all CA?' : 'Remove the /dev/all CA?',
      message: (
        <div className="col-sm">
          <span>
            {which === 'install'
              ? 'Apps that use the system trust store will accept certificates issued by this CA — which only exists on this computer. You can remove it again at any time.'
              : 'HTTPS interception stops working for apps that relied on the system trust store.'}
          </span>
          <span className="xs subtle">This runs:</span>
          <code className="code-inline insp-cmd">{cmd.display}</code>
          {cmd.prompt && <span className="xs">{cmd.prompt}</span>}
        </div>
      ),
      confirmText: which === 'install' ? 'Run it' : 'Remove',
      danger: which === 'uninstall',
    });
    if (!ok) return;
    setBusy(true);
    try {
      const out = which === 'install' ? await proxyApi.caInstall() : await proxyApi.caUninstall();
      setOutput([out.stdout, out.stderr].filter((s) => s.trim()).join('\n') || (out.success ? 'Done.' : `Exit code ${out.code}`));
      if (out.success) toast.success(which === 'install' ? 'CA trusted' : 'CA removed');
      else toast.error('The command failed', out.stderr || out.stdout);
      await load(true);
    } catch (e) {
      toast.error('Could not run the command', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const regenerate = async () => {
    const ok = await confirmDialog({
      title: 'Regenerate the CA?',
      message: 'A new CA replaces the current one. The proxy stops and the capture is cleared; anything that trusted the old CA must trust the new one (remove the old one first).',
      confirmText: 'Regenerate',
      danger: true,
      typeToConfirm: 'regenerate',
    });
    if (!ok) return;
    try {
      await proxyApi.caRegenerate();
      await useInspector.getState().restart();
      toast.success('New CA generated', 'Trust it again where needed.');
      await load(true);
    } catch (e) {
      toast.error('Could not regenerate the CA', errorMessage(e));
    }
  };

  const snippet = view?.env.find((e) => e.shell === shell);

  return (
    <Modal open={open} onClose={onClose} size="lg" title="Connect apps & trust the CA">
      {!view ? (
        <div className="empty">
          <Spinner />
        </div>
      ) : (
        <div className="col-lg insp-connect">
          <section className="section">
            <div className="section-head">
              <h3>1 · The local CA</h3>
              <span className="spacer" />
              <TrustBadge installed={view.trust.installed} />
              <button className="icon-btn sm" onClick={() => load(true)} disabled={checking} title="Check again" aria-label="Check again">
                <RefreshCw />
              </button>
            </div>
            <p className="small muted">
              To show HTTPS traffic, the proxy presents certificates issued by <strong>{view.ca.commonName}</strong>. It was generated on this computer; its private key stays in{' '}
              <code className="code-inline">{view.ca.dir}</code> and never goes into a project.
            </p>
            <div className="kv insp-kv">
              <span>Certificate</span>
              <span className="row-sm">
                <code className="code-inline truncate">{view.ca.certPath}</code>
                <CopyButton text={view.ca.certPath} title="Copy path" />
              </span>
              <span>SHA-256</span>
              <span className="mono xs break">{view.ca.sha256}</span>
              <span>Valid until</span>
              <span>{new Date(view.ca.notAfter).toLocaleDateString()}</span>
            </div>
          </section>

          <section className="section">
            <div className="section-head">
              <h3>2 · Trust it on this computer</h3>
            </div>
            <p className="small muted">Most apps (and Chrome, Edge, Safari) use the system trust store. This adds the CA for your user only and asks for confirmation first.</p>
            {view.trust.install && (
              <div className="insp-cmd-row">
                <code className="code-inline insp-cmd grow">{view.trust.install.display}</code>
                <CopyButton text={view.trust.install.display} title="Copy command" />
                {view.trust.install.runnable && (
                  <button className="btn btn-sm btn-primary" onClick={() => run('install')} disabled={busy || view.trust.installed === true}>
                    <ShieldCheck /> Trust CA…
                  </button>
                )}
              </div>
            )}
            {view.trust.uninstall && (
              <div className="insp-cmd-row">
                <code className="code-inline insp-cmd grow">{view.trust.uninstall.display}</code>
                <CopyButton text={view.trust.uninstall.display} title="Copy command" />
                {view.trust.uninstall.runnable && (
                  <button className="btn btn-sm btn-danger-ghost" onClick={() => run('uninstall')} disabled={busy || view.trust.installed === false}>
                    <ShieldOff /> Remove…
                  </button>
                )}
              </div>
            )}
            {output && <pre className="insp-output">{output}</pre>}
          </section>

          <section className="section">
            <div className="section-head">
              <h3>3 · Or launch a browser — nothing installed</h3>
            </div>
            <p className="small muted">Starts the browser with a separate temporary profile that routes through the proxy and accepts this CA by its key pin.</p>
            {view.browsers.length === 0 ? (
              <Callout kind="warning">No Chrome, Edge, Brave or Chromium found in the standard locations.</Callout>
            ) : (
              <div className="row wrap">
                {view.browsers.map((b) => (
                  <button key={b.kind} className="btn btn-sm" onClick={() => launchBrowser(b.kind, b.name)} title={b.path}>
                    <Globe /> {b.name}
                  </button>
                ))}
              </div>
            )}
          </section>

          <section className="section">
            <div className="section-head">
              <h3>
                <Terminal size={14} /> 4 · Terminals, Node.js and CLIs
              </h3>
              <span className="spacer" />
              <Segmented value={shell} onChange={setShell} options={view.env.map((e) => ({ value: e.shell, label: e.label }))} />
            </div>
            {snippet && (
              <div className="insp-snippet">
                <pre>{snippet.text}</pre>
                <CopyButton text={snippet.text} title="Copy" />
              </div>
            )}
          </section>

          <section className="section">
            <div className="section-head">
              <h3>Other tools</h3>
            </div>
            <div className="col">
              {view.trust.manual.map((n) => (
                <div key={n.title} className="col-sm">
                  <strong className="small">{n.title}</strong>
                  <span className="xs muted">{n.body}</span>
                  {n.command && (
                    <div className="insp-snippet">
                      <pre>{n.command}</pre>
                      <CopyButton text={n.command} title="Copy" />
                    </div>
                  )}
                </div>
              ))}
            </div>
          </section>

          <section className="section">
            <div className="section-head">
              <h3>Danger zone</h3>
            </div>
            <div className="row">
              <span className="small muted grow">Generate a brand-new CA (for example if the key may have leaked).</span>
              <button className="btn btn-sm btn-danger-ghost" onClick={regenerate}>
                Regenerate CA…
              </button>
            </div>
          </section>
        </div>
      )}
    </Modal>
  );
}

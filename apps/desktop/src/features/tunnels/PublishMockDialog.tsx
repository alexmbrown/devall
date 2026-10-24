import { useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ExternalLink, Globe2, Heart } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, CopyButton, Field, Spinner, Switch, formatBytes } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { relayApi } from '@/lib/api/relay';
import { cloudApi, type MockPlanView, type MockPublished } from '@/lib/api/cloud';
import { useNav } from '@/store/nav';

/**
 * "Publish to the web": uploads the project's stubs to the relay so they keep answering when this
 * machine is off. Checks them against the hosted-mock subset first and lists what won't work.
 */
export function PublishMockDialog({ root, onClose }: { root: string; onClose(): void }) {
  const [cors, setCors] = useState(true);
  const [globalTemplating, setGlobalTemplating] = useState(false);
  const [plan, setPlan] = useState<MockPlanView | null>(null);
  const [name, setName] = useState('');
  const [signedIn, setSignedIn] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<MockPublished | null>(null);
  const go = useNav((s) => s.go);

  useEffect(() => {
    relayApi
      .status()
      .then((s) => setSignedIn(s.signedIn))
      .catch(() => setSignedIn(false));
  }, []);
  useEffect(() => {
    cloudApi
      .mockPlan(root, cors, globalTemplating)
      .then((p) => {
        setPlan(p);
        setName((n) => n || p.suggestedName);
      })
      .catch((e) => toast.error('Hosted mock', errorMessage(e)));
  }, [root, cors, globalTemplating]);

  const validName = /^[a-z0-9](?:[a-z0-9-]{1,38})[a-z0-9]$/.test(name) && !name.includes('--');
  const blocked = (plan?.plan.blocking.length ?? 0) > 0;

  const publish = async () => {
    setBusy(true);
    try {
      const r = await cloudApi.mockPublish(root, name, cors, globalTemplating);
      setResult(r);
      toast.success('Published', r.url);
    } catch (e) {
      toast.error('Could not publish', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const toTunnels = () => {
    onClose();
    go('tunnels', 'mocks');
  };

  return (
    <Modal
      open
      size="lg"
      onClose={onClose}
      title={
        <span className="row-sm">
          <Globe2 size={16} /> Publish to the web
        </span>
      }
      footer={
        result ? (
          <>
            <button className="btn" onClick={toTunnels}>
              Requests & unpublish
            </button>
            <button className="btn btn-primary" onClick={onClose}>
              Done
            </button>
          </>
        ) : (
          <>
            <button className="btn" onClick={onClose}>
              Cancel
            </button>
            <button className="btn btn-primary" disabled={!signedIn || !plan || !validName || blocked || busy} onClick={publish}>
              {busy ? <Spinner /> : <Globe2 />} {busy ? 'Publishing…' : 'Publish'}
            </button>
          </>
        )
      }
    >
      <div className="col-lg">
        {result ? (
          <>
            <Callout kind="success">
              <b>{result.name}</b> is live with {result.stubs} stubs — it keeps answering while this machine is off. Publish again to update it.
            </Callout>
            <div className="tn-url">
              <span className="mono tn-public truncate">{result.url}</span>
              <CopyButton text={result.url} label="Copy" />
              <button className="btn btn-sm" onClick={() => openUrl(result.url).catch(() => {})}>
                <ExternalLink /> Open
              </button>
            </div>
            {result.pathUrl && result.pathUrl !== result.url && <span className="xs subtle mono">also {result.pathUrl}</span>}
          </>
        ) : (
          <>
            <p className="sm subtle">
              Uploads <code className="code-inline">mocks/mappings</code> (and the small <code className="code-inline">__files</code> they use) to the relay. Your files stay the source of
              truth; re-publish after changes. Browsers get no HTML/JS from hosted mocks, and every response is <code className="code-inline">noindex</code>.
            </p>
            {signedIn === false && (
              <Callout kind="info">
                <div className="row-sm">
                  <span className="grow">Publishing needs a relay account (free tier: one small mock) or your own relay — free.</span>
                  <button className="btn btn-sm btn-primary" onClick={toTunnels}>
                    <Heart /> Sign in / self-host
                  </button>
                </div>
              </Callout>
            )}
            <Field label="Name" hint={`Unique on the relay; served at https://mock.<relay domain>/${name || '<name>'}/ (or ${name || '<name>'}.mock.<relay domain>, depending on the relay).`} error={name && !validName ? '3–40 characters: a-z, 0-9 and inner hyphens' : undefined}>
              <input className="input mono" value={name} onChange={(e) => setName(e.target.value.toLowerCase())} />
            </Field>
            <Switch checked={cors} onChange={setCors} label="Allow browser apps to call it (CORS)" />
            <Switch checked={globalTemplating} onChange={setGlobalTemplating} label="Template every response (like --global-response-templating)" />
            {!plan ? (
              <Spinner />
            ) : (
              <>
                <p className="xs subtle">
                  {plan.plan.stubs} stubs · {plan.plan.files.length} files · {formatBytes(plan.plan.bytes)}
                </p>
                {plan.loadErrors.length > 0 && (
                  <Callout kind="warning">
                    {plan.loadErrors.length} mapping file{plan.loadErrors.length === 1 ? '' : 's'} can't be loaded and won't be published: {plan.loadErrors.slice(0, 3).join('; ')}
                  </Callout>
                )}
                {plan.plan.blocking.map((b) => (
                  <Callout key={b} kind="danger">
                    {b} Supporters get more, and a self-hosted relay has no limits.
                  </Callout>
                ))}
                {plan.plan.warnings.length > 0 ? (
                  <Callout kind="warning">
                    <b>Hosted mocks support a subset of WireMock.</b> These stubs use something they don't (unsupported matchers never match; the rest is ignored):
                    <ul className="tn-warnings">
                      {plan.plan.warnings.slice(0, 30).map((w, i) => (
                        <li key={i}>
                          <b>{w.stub}</b> · <code className="code-inline">{w.feature}</code> — {w.message}
                        </li>
                      ))}
                      {plan.plan.warnings.length > 30 && <li>… and {plan.plan.warnings.length - 30} more</li>}
                    </ul>
                  </Callout>
                ) : (
                  <Callout kind="success">Every stub uses features hosted mocks support.</Callout>
                )}
              </>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}

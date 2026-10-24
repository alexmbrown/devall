import { useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ExternalLink, Share2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, CopyButton, Field, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { relayApi, type AccountInfo } from '@/lib/api/relay';
import { cloudApi, type ShareCreated } from '@/lib/api/cloud';
import { useNav } from '@/store/nav';

const DAY = 86400;
const EXPIRY: { label: string; seconds: number }[] = [
  { label: '1 hour', seconds: 3600 },
  { label: '1 day', seconds: DAY },
  { label: '7 days', seconds: 7 * DAY },
  { label: '30 days', seconds: 30 * DAY },
  { label: '90 days', seconds: 90 * DAY },
  { label: '1 year', seconds: 365 * DAY },
];

/**
 * "Share…" a history entry as a read-only, expiring link. Redaction happens in the app before
 * upload: the environment's secret values always, and auth / cookie / API-key headers and token-like
 * query parameters unless unticked. Links need an account (or a self-host token) — never anonymous.
 */
export function ShareDialog({ root, historyId, defaultTitle, onClose }: { root: string; historyId: string; defaultTitle: string; onClose(): void }) {
  const [title, setTitle] = useState(defaultTitle);
  const [maskHeaders, setMaskHeaders] = useState(true);
  const [includeBody, setIncludeBody] = useState(true);
  const [account, setAccount] = useState<AccountInfo | null>(null);
  const [signedIn, setSignedIn] = useState<boolean | null>(null);
  const [ttl, setTtl] = useState(DAY);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ShareCreated | null>(null);
  const go = useNav((s) => s.go);

  useEffect(() => {
    relayApi
      .status()
      .then(async (s) => {
        setSignedIn(s.signedIn);
        if (s.signedIn) setAccount(await relayApi.account());
      })
      .catch(() => setSignedIn(false));
  }, []);
  const maxTtl = account?.limits.shareTtlSeconds ?? 7 * DAY;
  const options = EXPIRY.filter((e) => e.seconds <= maxTtl);

  const share = async () => {
    setBusy(true);
    try {
      const r = await cloudApi.shareCreate(root, historyId, { title, maskHeaders, includeResponseBody: includeBody, ttlSeconds: Math.min(ttl, maxTtl) });
      setResult(r);
    } catch (e) {
      toast.error('Could not share', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      title={
        <span className="row-sm">
          <Share2 size={16} /> Share request & response
        </span>
      }
      footer={
        result ? (
          <button className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        ) : (
          <>
            <button className="btn" onClick={onClose}>
              Cancel
            </button>
            <button className="btn btn-primary" disabled={!signedIn || busy || !title.trim()} onClick={share}>
              {busy ? <Spinner /> : <Share2 />} Create link
            </button>
          </>
        )
      }
    >
      <div className="col-lg">
        {result ? (
          <>
            <Callout kind="success">Anyone with the link can view this snapshot until {new Date(result.expiresAt).toLocaleString()}. Revoke it any time on Tunnels → Shared links.</Callout>
            <div className="tn-url">
              <span className="mono tn-public truncate">{result.url}</span>
              <CopyButton text={result.url} label="Copy" />
              <button className="btn btn-sm" onClick={() => openUrl(result.url).catch(() => {})}>
                <ExternalLink /> Open
              </button>
            </div>
          </>
        ) : (
          <>
            {signedIn === false && (
              <Callout kind="info">
                <div className="row-sm">
                  <span className="grow">Shared links need a relay account (free) or your own relay — they're never anonymous. Or export the request file instead.</span>
                  <button
                    className="btn btn-sm btn-primary"
                    onClick={() => {
                      onClose();
                      go('tunnels');
                    }}
                  >
                    Sign in / self-host
                  </button>
                </div>
              </Callout>
            )}
            <Field label="Title">
              <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} maxLength={200} />
            </Field>
            <Field label="Expires after">
              <select className="select" value={Math.min(ttl, maxTtl)} onChange={(e) => setTtl(Number(e.target.value))}>
                {options.map((o) => (
                  <option key={o.seconds} value={o.seconds}>
                    {o.label}
                  </option>
                ))}
              </select>
            </Field>
            <div className="col-sm">
              <span className="xs subtle">The environment's secret values are always masked.</span>
              <Checkbox checked={maskHeaders} onChange={setMaskHeaders} label="Mask Authorization, Cookie and API-key headers and token-like query parameters" />
              <Checkbox checked={includeBody} onChange={setIncludeBody} label="Include the response body" />
            </div>
            {!maskHeaders && <Callout kind="warning">Headers will be shared as sent — make sure they don't carry credentials.</Callout>}
            <p className="xs subtle">
              The page shows the request, response, headers and timing, with "Copy as cURL" and a <code className="code-inline">.req.yaml</code> download to import into /dev/all.
            </p>
          </>
        )}
      </div>
    </Modal>
  );
}

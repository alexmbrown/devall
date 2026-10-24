import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import {
  ArrowLeft,
  Ban,
  CheckCircle2,
  ChevronDown,
  Clock,
  Copy,
  FileJson,
  FilePlus2,
  Info,
  RefreshCw,
  Repeat,
  ShieldAlert,
  Trash2,
  XCircle,
} from 'lucide-react';
import { CodeEditor, languageFor } from '@/components/editor/CodeEditor';
import { Menu } from '@/components/ui/Menu';
import { confirmDialog } from '@/components/ui/dialogs';
import { DecodedLabel, DecodedValues, messageText, useDetections } from '@/components/DecodedValues';
import { Callout, CopyButton, Empty, MethodBadge, Segmented, Spinner, StatusCode, copyText, formatBytes, formatTime } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { headerValue, webhooksApi, type EventView, type ForwardResult, type HeaderPair, type InboxItem, type VerifyResult } from '@/lib/api/webhooks';
import { useWorkspace } from '@/store/workspace';
import { ReplayDialog } from './ReplayDialog';
import { useWebhooks } from './store';

type Tab = 'overview' | 'headers' | 'body' | 'forward' | 'decoded';

interface Props {
  root: string;
  id: string;
  envId?: string;
  inboxes: InboxItem[];
  onSelect(id: string): void;
  onDeleted(): void;
}

export function EventDetail({ root, id, envId, inboxes, onSelect, onDeleted }: Props) {
  const [ev, setEv] = useState<EventView | null | undefined>(undefined);
  const [tab, setTab] = useState<Tab>('overview');
  const [replay, setReplay] = useState(false);
  const [verifying, setVerifying] = useState(false);
  const onDeletedRef = useRef(onDeleted);
  onDeletedRef.current = onDeleted;

  const load = useCallback(async () => {
    try {
      setEv(await webhooksApi.event(root, id));
    } catch (e) {
      toast.error('Could not load the event', errorMessage(e));
      setEv(null);
    }
  }, [root, id]);

  useEffect(() => {
    void load();
  }, [load]);

  // Verification and forward results arrive just after the event itself.
  useEffect(() => {
    let sub: string | null = null;
    let cancelled = false;
    webhooksApi
      .subscribe(root, (n) => {
        if (n.type === 'updated' && n.event.id === id) void load();
        if (n.type === 'deleted' && n.id === id) onDeletedRef.current();
      })
      .then((s) => (cancelled ? void webhooksApi.unsubscribe(s) : (sub = s)))
      .catch(() => {});
    return () => {
      cancelled = true;
      if (sub) void webhooksApi.unsubscribe(sub);
    };
  }, [root, id, load]);

  const inbox = inboxes.find((i) => i.id === ev?.inbox);
  // JWTs and timestamps in the payload or headers (e.g. a token inside a webhook), decoded inline.
  const detections = useDetections(ev ? [ev.url, messageText(ev.headers, ev.bodyText)].join('\n') : null);

  if (ev === undefined)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  if (ev === null) return <Empty title="Event not found">It may have been deleted or pruned by retention.</Empty>;

  const verify = async () => {
    setVerifying(true);
    try {
      const r = await webhooksApi.verify(root, id, envId);
      setEv((e) => (e ? { ...e, verification: r } : e));
      if (r.valid) toast.success('Signature valid', r.reason);
      else toast.info('Signature check', r.reason);
    } catch (e) {
      toast.error('Could not verify', errorMessage(e));
    } finally {
      setVerifying(false);
    }
  };

  const saveRequest = async () => {
    try {
      const path = await webhooksApi.saveAsRequest(root, id, 'webhooks');
      const ws = useWorkspace.getState();
      await ws.refresh();
      await ws.openRequest(path, { preview: false });
      toast.success('Saved as request', `http/${path}`);
    } catch (e) {
      toast.error('Could not save as request', errorMessage(e));
    }
  };

  const saveFixture = async () => {
    try {
      const path = await webhooksApi.saveFixture(root, id);
      toast.success('Fixture saved', path);
    } catch (e) {
      toast.error('Could not save the fixture', errorMessage(e));
    }
  };

  const curl = async (resign: boolean) => {
    try {
      await copyText(await webhooksApi.copyCurl(root, id, { resign, env: envId }), resign ? 'cURL copied (re-signed, valid for a few minutes)' : 'cURL copied');
    } catch (e) {
      toast.error('Could not build cURL', errorMessage(e));
    }
  };

  const remove = async () => {
    if (!(await confirmDialog({ title: 'Delete this event?', message: 'It is removed from local history.', confirmText: 'Delete', danger: true }))) return;
    try {
      await webhooksApi.deleteEvent(root, id);
      onDeleted();
    } catch (e) {
      toast.error('Could not delete', errorMessage(e));
    }
  };

  const tabs: [Tab, React.ReactNode][] = [
    ['overview', 'Overview'],
    ['headers', `Headers (${ev.headers.length})`],
    ['body', `Body (${formatBytes(ev.bodySize)})`],
  ];
  if (ev.forward) tabs.push(['forward', ev.source === 'replay' ? 'Target response' : 'Forward result']);
  if (detections.length) tabs.push(['decoded', <DecodedLabel count={detections.length} />]);
  const canSign = !!inbox?.file?.provider && inbox.file.provider !== 'none';

  return (
    <div className="wh-detail">
      <div className="wh-detail-head">
        <div className="row-sm">
          <MethodBadge method={ev.method} />
          <span className="mono small truncate grow" title={ev.url}>
            {ev.path}
            {ev.query ? `?${ev.query}` : ''}
          </span>
          {ev.source !== 'local' && <span className={clsx('badge', ev.source === 'replay' ? 'accent' : 'info')}>{ev.source}</span>}
        </div>
        <div className="row-sm xs subtle">
          {ev.label && <span className="strong wh-label">{ev.label}</span>}
          <span>{new Date(ev.receivedAt).toLocaleString()}</span>
          <span>·</span>
          <span>{inbox?.file?.name ?? (ev.inbox ? `${ev.inbox} (deleted)` : 'unrouted')}</span>
          {ev.replayOf && (
            <button className="link xs" onClick={() => onSelect(ev.replayOf!)}>
              <ArrowLeft size={11} /> original
            </button>
          )}
        </div>
        <div className="row-sm wrap wh-actions">
          <button className="btn btn-sm btn-primary" onClick={() => setReplay(true)} title="Send again — optionally edited and re-signed">
            <Repeat /> Replay…
          </button>
          <button className="btn btn-sm" onClick={verify} disabled={verifying || !canSign} title={canSign ? 'Verify with the current secret' : 'The inbox has no signature scheme'}>
            {verifying ? <Spinner /> : <RefreshCw />} Verify
          </button>
          <button className="btn btn-sm" onClick={saveRequest} disabled={!ev.bodyText && ev.bodySize > 0} title="Save as an HTTP request (re-signs itself on every send)">
            <FilePlus2 /> Save as request
          </button>
          <button className="btn btn-sm" onClick={saveFixture} title="Save the payload to webhooks/fixtures/ for tests">
            <FileJson /> Save fixture
          </button>
          <Menu
            items={[
              { label: 'Copy cURL (as captured)', icon: <Copy />, onSelect: () => curl(false) },
              { label: 'Copy cURL (re-signed now)', icon: <Copy />, disabled: !canSign, onSelect: () => curl(true) },
            ]}
            trigger={(p) => (
              <button className="btn btn-sm" {...p}>
                <Copy /> cURL <ChevronDown />
              </button>
            )}
          />
          <span className="grow" />
          <button className="icon-btn danger" onClick={remove} title="Delete event" aria-label="Delete event">
            <Trash2 />
          </button>
        </div>
      </div>
      <div className="subtabs" role="tablist">
        {tabs.map(([k, label]) => (
          <button key={k} role="tab" aria-selected={tab === k} className={clsx('subtab', tab === k && 'on')} onClick={() => setTab(k)}>
            {label}
          </button>
        ))}
      </div>
      <div className="wh-detail-body">
        {tab === 'overview' && <Overview ev={ev} inbox={inbox} onVerify={verify} verifying={verifying} onForwardTab={() => setTab('forward')} />}
        {tab === 'headers' && <HeadersTable headers={ev.headers} />}
        {tab === 'body' && <BodyView ev={ev} />}
        {tab === 'forward' && ev.forward && <ForwardView f={ev.forward} />}
        {tab === 'decoded' && <DecodedValues items={detections} empty />}
      </div>
      {replay && (
        <ReplayDialog
          root={root}
          ev={ev}
          inbox={inbox}
          envId={envId}
          onClose={() => setReplay(false)}
          onReplayed={(newId) => {
            setReplay(false);
            onSelect(newId);
          }}
        />
      )}
    </div>
  );
}

const STATUS_TEXT: Record<VerifyResult['status'], string> = {
  valid: 'Signature valid',
  mismatch: 'Signature mismatch',
  expired: 'Signature expired',
  missingHeader: 'Not signed',
  malformed: 'Malformed signature',
  missingSecret: 'Secret not available',
};

function VerifyIcon({ r }: { r: VerifyResult }) {
  if (r.valid) return <CheckCircle2 className="text-success" />;
  if (r.status === 'expired') return <Clock className="text-warning" />;
  if (r.status === 'missingSecret') return <Ban className="subtle" />;
  if (r.status === 'missingHeader' || r.status === 'malformed') return <ShieldAlert className="text-danger" />;
  return <XCircle className="text-danger" />;
}

function Overview({ ev, inbox, onVerify, verifying, onForwardTab }: { ev: EventView; inbox?: InboxItem; onVerify(): void; verifying: boolean; onForwardTab(): void }) {
  const providerLabel = useWebhooks((s) => s.provider(inbox?.file?.provider ?? ev.provider ?? 'none')?.label);
  const r = ev.verification;
  const xff = headerValue(ev.headers, 'x-forwarded-for');
  const noScheme = !inbox?.file?.provider || inbox.file.provider === 'none';
  return (
    <div className="col wh-overview">
      <section className={clsx('wh-verify', r ? (r.valid ? 'ok' : r.status === 'expired' || r.status === 'missingSecret' ? 'warn' : 'bad') : 'none')} aria-label="Signature verification">
        {r ? (
          <>
            <div className="row-sm wh-verify-head">
              <VerifyIcon r={r} />
              <span className="strong">{STATUS_TEXT[r.status]}</span>
              <span className="badge outline">{providerLabel ?? r.provider}</span>
              <span className="grow" />
              <span className="xs subtle">checked {formatTime(r.checkedAt)}</span>
              <button className="btn btn-sm btn-ghost" onClick={onVerify} disabled={verifying}>
                {verifying ? <Spinner /> : <RefreshCw />} Re-check
              </button>
            </div>
            <p className="wh-reason">{r.reason}</p>
            <dl className="kv mono">
              {r.received && (
                <>
                  <dt>Received</dt>
                  <dd className="break">{r.received}</dd>
                </>
              )}
              {r.expected && (
                <>
                  <dt>Expected</dt>
                  <dd>{r.expected}</dd>
                </>
              )}
              {r.computedOver && (
                <>
                  <dt>Signed content</dt>
                  <dd>{r.computedOver}</dd>
                </>
              )}
              {r.signedUrl && (
                <>
                  <dt>Signed URL</dt>
                  <dd className="break">{r.signedUrl}</dd>
                </>
              )}
              {r.timestampAgeSeconds !== null && (
                <>
                  <dt>Timestamp</dt>
                  <dd>
                    {r.timestamp} — {r.timestampAgeSeconds} s old on arrival (tolerance {r.toleranceSeconds} s)
                  </dd>
                </>
              )}
            </dl>
            {r.hints.length > 0 && (
              <ul className="wh-hints">
                {r.hints.map((h, i) => (
                  <li key={i}>{h}</li>
                ))}
              </ul>
            )}
          </>
        ) : noScheme ? (
          <div className="row-sm small muted">
            <Info size={14} /> No signature scheme for {ev.inbox ? 'this inbox' : 'unrouted requests'} — pick a provider in the inbox settings to verify signatures.
          </div>
        ) : (
          <div className="row-sm small muted">
            <Spinner /> Verifying…
          </div>
        )}
      </section>

      {ev.forward && (
        <section className="wh-forward-summary">
          <div className="row-sm">
            <span className="strong small">{ev.source === 'replay' ? 'Sent to' : 'Forwarded to'}</span>
            <code className="code-inline truncate grow" title={ev.forward.url}>
              {ev.forward.url}
            </code>
            {ev.forward.status !== null && <StatusCode status={ev.forward.status} />}
            <span className="xs subtle">{Math.round(ev.forward.durationMs)} ms</span>
            <button className="btn btn-sm btn-ghost" onClick={onForwardTab}>
              Response
            </button>
          </div>
          {ev.forward.error && (
            <Callout kind="danger" className="small">
              {ev.forward.error}
            </Callout>
          )}
        </section>
      )}

      {ev.bodyTruncated && <Callout kind="warning">The body was larger than 10 MB: it was answered with 413 and only the first 10 MB were kept.</Callout>}

      <dl className="kv">
        <dt>URL</dt>
        <dd className="mono xs break">
          {ev.url} <CopyButton text={ev.url} />
        </dd>
        {ev.publicUrl && (
          <>
            <dt>Public URL</dt>
            <dd className="mono xs break">{ev.publicUrl}</dd>
          </>
        )}
        <dt>Received</dt>
        <dd>
          {new Date(ev.receivedAt).toLocaleString()} <span className="subtle xs">({formatTime(ev.receivedAt)})</span>
        </dd>
        {ev.remoteAddr && (
          <>
            <dt>From</dt>
            <dd className="mono xs">
              {ev.remoteAddr}
              {xff ? ` (X-Forwarded-For: ${xff})` : ''}
            </dd>
          </>
        )}
        {ev.responseStatus !== null && (
          <>
            <dt>Answered</dt>
            <dd>
              <StatusCode status={ev.responseStatus} />
            </dd>
          </>
        )}
        <dt>Body</dt>
        <dd>
          {formatBytes(ev.bodySize)}
          {ev.contentType ? <span className="subtle"> · {ev.contentType}</span> : null}
        </dd>
      </dl>
    </div>
  );
}

function HeadersTable({ headers }: { headers: HeaderPair[] }) {
  if (!headers.length) return <Empty title="No headers" />;
  return (
    <table className="table wh-headers">
      <tbody>
        {headers.map(([k, v], i) => (
          <tr key={i}>
            <td className="mono xs strong nowrap">{k}</td>
            <td className="mono xs break">{v}</td>
            <td className="wh-copy-cell">
              <CopyButton text={v} />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

type BodyMode = 'pretty' | 'raw' | 'form';

function BodyView({ ev }: { ev: EventView }) {
  const isForm = /x-www-form-urlencoded/i.test(ev.contentType ?? '');
  const pretty = useMemo(() => {
    if (!ev.bodyText) return null;
    try {
      return JSON.stringify(JSON.parse(ev.bodyText), null, 2);
    } catch {
      return null;
    }
  }, [ev.bodyText]);
  const form = useMemo(() => (isForm && ev.bodyText ? [...new URLSearchParams(ev.bodyText).entries()] : null), [isForm, ev.bodyText]);
  const [mode, setMode] = useState<BodyMode>(pretty ? 'pretty' : form ? 'form' : 'raw');

  if (ev.bodySize === 0) return <Empty title="No body" />;
  if (ev.bodyBase64 !== null)
    return (
      <div className="col">
        <Callout>Binary body ({formatBytes(ev.bodySize)}) — shown as base64.</Callout>
        <div className="editor-shell fill">
          <CodeEditor value={ev.bodyBase64} language="plaintext" readOnly />
        </div>
      </div>
    );
  const options: { value: BodyMode; label: string }[] = [];
  if (pretty) options.push({ value: 'pretty', label: 'Pretty' });
  if (form) options.push({ value: 'form', label: 'Fields' });
  options.push({ value: 'raw', label: 'Raw' });
  return (
    <div className="col wh-body">
      <div className="row-sm">
        {options.length > 1 && <Segmented<BodyMode> value={mode} onChange={setMode} options={options} />}
        <span className="xs subtle grow">{mode === 'raw' ? 'Exact bytes received (signatures cover these).' : ''}</span>
        <CopyButton text={ev.bodyText ?? ''} label="Copy" />
      </div>
      {mode === 'form' && form ? (
        <HeadersTable headers={form as HeaderPair[]} />
      ) : (
        <div className="editor-shell fill">
          <CodeEditor value={mode === 'pretty' && pretty ? pretty : (ev.bodyText ?? '')} language={mode === 'pretty' ? 'json' : languageFor(ev.contentType ?? undefined, ev.bodyText ?? '')} readOnly path={`webhook-${ev.id}.${mode}`} />
        </div>
      )}
    </div>
  );
}

function ForwardView({ f }: { f: ForwardResult }) {
  const ct = headerValue(f.headers, 'content-type');
  return (
    <div className="col wh-body">
      <div className="row-sm">
        {f.status !== null ? <StatusCode status={f.status} /> : <span className="badge danger">no response</span>}
        <span className="small">{f.statusText}</span>
        <code className="code-inline truncate grow" title={f.url}>
          {f.url}
        </code>
        <span className="xs subtle">
          {Math.round(f.durationMs)} ms · {formatBytes(f.bodySize)} · {new Date(f.at).toLocaleTimeString()}
        </span>
      </div>
      {f.error && <Callout kind="danger">{f.error}</Callout>}
      {f.headers.length > 0 && (
        <details className="wh-forward-headers" open={!f.bodyExcerpt}>
          <summary className="small strong">Response headers ({f.headers.length})</summary>
          <HeadersTable headers={f.headers} />
        </details>
      )}
      {f.bodyExcerpt && (
        <div className="editor-shell fill">
          <CodeEditor value={f.bodyExcerpt} language={languageFor(ct, f.bodyExcerpt)} readOnly />
        </div>
      )}
      {f.bodySize > 65536 && <span className="xs subtle">Showing the first 64 KB.</span>}
    </div>
  );
}

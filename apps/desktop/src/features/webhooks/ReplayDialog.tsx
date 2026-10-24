import { useMemo, useState } from 'react';
import { Copy, Repeat, Undo2 } from 'lucide-react';
import { CodeEditor, languageFor } from '@/components/editor/CodeEditor';
import { Modal } from '@/components/ui/Modal';
import { Callout, Field, Spinner, StatusCode, Switch, copyText } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { webhooksApi, type EventView, type HeaderPair, type InboxItem, type ReplayEdits } from '@/lib/api/webhooks';
import { useWebhooks } from './store';

const HOP = new Set(['host', 'content-length', 'connection', 'keep-alive', 'transfer-encoding', 'te', 'trailer', 'upgrade', 'proxy-connection', 'proxy-authorization', 'proxy-authenticate']);

function headersToText(h: HeaderPair[]) {
  return h
    .filter(([k]) => !HOP.has(k.toLowerCase()))
    .map(([k, v]) => `${k}: ${v}`)
    .join('\n');
}

function textToHeaders(t: string): HeaderPair[] {
  const out: HeaderPair[] = [];
  for (const line of t.split('\n')) {
    const i = line.indexOf(':');
    if (i <= 0) continue;
    out.push([line.slice(0, i).trim(), line.slice(i + 1).trim()]);
  }
  return out;
}

/** Replay an event to a target (default: the inbox's forward URL), optionally edited and re-signed. */
export function ReplayDialog({ root, ev, inbox, envId, onClose, onReplayed }: { root: string; ev: EventView; inbox?: InboxItem; envId?: string; onClose(): void; onReplayed(id: string): void }) {
  const provider = inbox?.file?.provider ?? 'none';
  const info = useWebhooks((s) => s.provider(provider));
  const canSign = provider !== 'none';
  const defaultTarget = inbox?.file?.forward?.url ?? '';
  const [target, setTarget] = useState(defaultTarget);
  const [resign, setResign] = useState(canSign);
  const [edit, setEdit] = useState(false);
  const [method, setMethod] = useState(ev.method);
  const originalHeaders = useMemo(() => headersToText(ev.headers), [ev.headers]);
  const [headers, setHeaders] = useState(originalHeaders);
  const [body, setBody] = useState(ev.bodyText ?? '');
  const [busy, setBusy] = useState(false);
  const binary = ev.bodyBase64 !== null;
  const bodyChanged = !binary && body !== (ev.bodyText ?? '');

  const request = () => {
    const edits: ReplayEdits | undefined = edit
      ? {
          method: method !== ev.method ? method : undefined,
          headers: headers !== originalHeaders ? textToHeaders(headers) : undefined,
          body: bodyChanged ? body : undefined,
        }
      : undefined;
    return { target: target.trim() || undefined, edits, resign: canSign && resign, env: envId };
  };

  const send = async () => {
    setBusy(true);
    try {
      const out = await webhooksApi.replay(root, ev.id, request());
      const f = out.forward;
      if (f?.error) toast.error('Replay failed', f.error);
      else toast.success(`Replayed → ${f?.status ?? '?'} ${f?.statusText ?? ''}`, out.verification ? `Signature at target: ${out.verification.reason}` : f?.url);
      onReplayed(out.id);
    } catch (e) {
      toast.error('Could not replay', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const curl = async () => {
    try {
      await copyText(await webhooksApi.copyCurl(root, ev.id, request()), 'cURL copied');
    } catch (e) {
      toast.error('Could not build cURL', errorMessage(e));
    }
  };

  const stale = info?.timestamped && ev.verification?.timestamp ? Date.now() / 1000 - ev.verification.timestamp > (ev.verification.toleranceSeconds ?? 300) : false;

  return (
    <Modal
      open
      onClose={onClose}
      size="xl"
      title="Replay event"
      footer={
        <>
          <span className="subtle xs grow">The replay is recorded as a new event linked to this one, with the target's response.</span>
          <button className="btn" onClick={curl} title="Copy the request as cURL instead of sending it">
            <Copy /> Copy cURL
          </button>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={send} disabled={busy}>
            {busy ? <Spinner /> : <Repeat />} Replay
          </button>
        </>
      }
    >
      <div className="wh-dialog">
        <Field label="Target URL" hint={defaultTarget ? 'Defaults to the inbox forward URL; {{variables}} resolve with the active environment.' : 'Where to send it, e.g. http://localhost:3000/webhooks — defaults to the original URL.'}>
          <input className="input mono" value={target} placeholder={ev.url} onChange={(e) => setTarget(e.target.value)} autoFocus />
        </Field>
        <div className="row wrap">
          <Switch
            checked={canSign && resign}
            disabled={!canSign}
            onChange={setResign}
            label={canSign ? `Re-sign as ${info?.label ?? provider} (fresh timestamp)` : 'Re-sign (inbox has no signature scheme)'}
            title="Replace the signature headers with new ones computed now from the inbox's secret"
          />
          <Switch checked={edit} onChange={setEdit} label="Edit before sending" />
        </div>
        {canSign && !resign && (stale || (edit && (bodyChanged || headers !== originalHeaders))) && (
          <Callout kind="warning">
            {stale
              ? `This signature is older than the ${info?.label ?? ''} tolerance — receivers that check timestamps will reject it unless you re-sign.`
              : 'The request was edited: the captured signature no longer matches. Turn on re-signing.'}
          </Callout>
        )}
        {provider === 'twilio' && resign && <Callout>Twilio signatures cover the URL: the replay is signed for the target URL above.</Callout>}
        {edit && (
          <div className="wh-replay-edit">
            <div className="row-sm">
              <Field label="Method">
                <select className="select sm" value={method} onChange={(e) => setMethod(e.target.value)}>
                  {['POST', 'PUT', 'PATCH', 'DELETE', 'GET'].concat(['POST', 'PUT', 'PATCH', 'DELETE', 'GET'].includes(ev.method) ? [] : [ev.method]).map((m) => (
                    <option key={m}>{m}</option>
                  ))}
                </select>
              </Field>
              <span className="grow" />
              <button
                className="btn btn-sm btn-ghost"
                onClick={() => {
                  setMethod(ev.method);
                  setHeaders(originalHeaders);
                  setBody(ev.bodyText ?? '');
                }}
              >
                <Undo2 /> Reset edits
              </button>
            </div>
            <Field label="Headers" hint="One per line (Name: value). Host and Content-Length are set automatically.">
              <textarea className="textarea mono" rows={6} value={headers} onChange={(e) => setHeaders(e.target.value)} spellCheck={false} />
            </Field>
            <Field label="Body">
              {binary ? (
                <Callout>Binary bodies are replayed unchanged.</Callout>
              ) : (
                <div className="editor-shell wh-replay-body">
                  <CodeEditor value={body} onChange={setBody} language={languageFor(ev.contentType ?? undefined, body)} path={`replay-${ev.id}`} />
                </div>
              )}
            </Field>
          </div>
        )}
        {ev.forward?.status !== undefined && ev.forward?.status !== null && !edit && (
          <div className="row-sm xs subtle">
            Last {ev.source === 'replay' ? 'replay' : 'forward'}: <StatusCode status={ev.forward.status} /> from <span className="mono truncate">{ev.forward.url}</span>
          </div>
        )}
      </div>
    </Modal>
  );
}

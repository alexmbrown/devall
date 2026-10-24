import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { ArrowDownLeft, ArrowRight, ArrowUpRight, CheckCircle2, Copy, RotateCw, Terminal } from 'lucide-react';
import { CodeEditor, languageFor } from '@/components/editor/CodeEditor';
import { DecodedLabel, DecodedValues, messageText, useDetections } from '@/components/DecodedValues';
import { Callout, CopyButton, Empty, MethodBadge, Segmented, Spinner, StatusCode, copyText, formatBytes } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { proxyApi, type ExchangeDetail, type ExchangeId, type MessageView, type SpecValidation, type WsFrame } from '@/lib/api/proxy';
import { fmtClock, fmtMs, hexDump, prettyBody, rawMessage } from './format';
import { useInspector } from './store';

type Tab = 'request' | 'response' | 'timing' | 'frames' | 'decoded';
type View = 'headers' | 'body' | 'raw';

function HeadersTable({ headers }: { headers: [string, string][] }) {
  if (!headers.length) return <p className="muted small p-4">No headers.</p>;
  return (
    <table className="table insp-headers">
      <tbody>
        {headers.map(([k, v], i) => (
          <tr key={`${k}-${i}`}>
            <td className="mono insp-hname">{k}</td>
            <td className="mono insp-hval">{v}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function BodyPane({ m, id, kind }: { m: MessageView; id: ExchangeId; kind: 'request' | 'response' }) {
  const b = m.body;
  const ct = b.contentType ?? '';
  const [hex, setHex] = useState(false);
  const notes: string[] = [];
  if (b.decoded && b.encoding) notes.push(`Decoded from ${b.encoding} (${formatBytes(b.size)} on the wire).`);
  if (b.truncated) notes.push(`Only the first ${formatBytes(b.captured)} of ${formatBytes(b.size)} were captured.`);
  if (b.decodeError) notes.push(`Could not decode ${b.encoding}: ${b.decodeError}. Showing raw bytes.`);
  if (b.evicted) return <Callout className="m-3">The body was dropped to stay within the capture's memory budget.</Callout>;
  if (b.size === 0 && !b.text && !b.base64) return <p className="muted small p-4">No body.</p>;
  const image = b.base64 != null && ct.startsWith('image/') && !b.truncated;
  return (
    <div className="insp-body">
      {notes.length > 0 && (
        <div className="insp-notes">
          {notes.map((n) => (
            <span key={n}>{n}</span>
          ))}
        </div>
      )}
      {image ? (
        <div className="insp-image">
          <img src={`data:${ct.split(';')[0]};base64,${b.base64}`} alt="Response body" />
        </div>
      ) : b.base64 != null ? (
        <>
          <div className="toolbar">
            <span className="subtle xs grow">Binary body ({formatBytes(b.captured)})</span>
            <Segmented value={hex ? 'hex' : 'b64'} onChange={(v) => setHex(v === 'hex')} options={[{ value: 'hex', label: 'Hex' }, { value: 'b64', label: 'Base64' }]} />
          </div>
          <div className="grow min-h-0">
            <CodeEditor key={`${id}-${kind}-bin-${hex}`} value={hex ? hexDump(b.base64) : b.base64} language="plaintext" readOnly lineNumbers={false} />
          </div>
        </>
      ) : (
        <div className="grow min-h-0">
          <CodeEditor key={`${id}-${kind}`} value={prettyBody(b.text ?? '', ct)} language={languageFor(ct, b.text ?? '')} readOnly path={`inspector-${kind}`} />
        </div>
      )}
    </div>
  );
}

function MessagePane({ m, id, kind, url }: { m: MessageView; id: ExchangeId; kind: 'request' | 'response'; url?: string }) {
  const [view, setView] = useState<View>(kind === 'request' ? 'headers' : 'body');
  return (
    <div className="insp-message">
      <div className="toolbar">
        <Segmented
          value={view}
          onChange={setView}
          options={[
            { value: 'headers', label: `Headers (${m.headers.length})` },
            { value: 'body', label: m.body.size ? `Body · ${formatBytes(m.body.size)}` : 'Body' },
            { value: 'raw', label: 'Raw' },
          ]}
        />
        <span className="spacer" />
        {kind === 'request' && url && <CopyButton text={url} title="Copy URL" />}
        <CopyButton text={() => (view === 'headers' ? m.headers.map(([k, v]) => `${k}: ${v}`).join('\n') : view === 'raw' ? rawMessage(m, kind) : (m.body.text ?? m.body.base64 ?? ''))} title="Copy" />
      </div>
      <div className="insp-message-body">
        {view === 'headers' && <HeadersTable headers={m.headers} />}
        {view === 'body' && <BodyPane m={m} id={id} kind={kind} />}
        {view === 'raw' && <CodeEditor key={`${id}-${kind}-raw`} value={rawMessage(m, kind)} language="plaintext" readOnly lineNumbers={false} />}
      </div>
    </div>
  );
}

const PHASES: { key: 'blockedMs' | 'dnsMs' | 'connectMs' | 'tlsMs' | 'waitMs' | 'receiveMs'; label: string; color: string }[] = [
  { key: 'blockedMs', label: 'Blocked (breakpoints, throttling)', color: 'var(--text-disabled)' },
  { key: 'dnsMs', label: 'DNS lookup', color: 'var(--chart-3xx)' },
  { key: 'connectMs', label: 'TCP connect', color: 'var(--warning)' },
  { key: 'tlsMs', label: 'TLS handshake', color: 'var(--chart-other)' },
  { key: 'waitMs', label: 'Waiting (TTFB)', color: 'var(--chart-2xx)' },
  { key: 'receiveMs', label: 'Content download', color: 'var(--accent)' },
];

function TimingPane({ d }: { d: ExchangeDetail }) {
  const t = d.timing;
  const total = Math.max(t.totalMs ?? 0, PHASES.reduce((a, p) => a + (t[p.key] ?? 0), 0), 0.001);
  let offset = 0;
  return (
    <div className="p-5 col">
      <div className="insp-waterfall">
        {PHASES.map((p) => {
          const v = t[p.key];
          const left = offset;
          if (v != null) offset += v;
          return (
            <div key={p.key} className={clsx('insp-wf-row', v == null && 'muted')}>
              <span className="insp-wf-label">{p.label}</span>
              <span className="insp-wf-track">{v != null && <span className="insp-wf-bar" style={{ left: `${(left / total) * 100}%`, width: `max(2px, ${(v / total) * 100}%)`, background: p.color }} />}</span>
              <span className="insp-wf-val mono">{v != null ? fmtMs(v) : '—'}</span>
            </div>
          );
        })}
        <div className="insp-wf-row strong">
          <span className="insp-wf-label">Total</span>
          <span className="insp-wf-track" />
          <span className="insp-wf-val mono">{fmtMs(t.totalMs)}</span>
        </div>
      </div>
      {t.connectionReused && <p className="subtle xs">Reused an open connection to the server, so there was no DNS / connect / TLS time.</p>}
      <div className="kv insp-kv">
        <span>Started</span>
        <span className="mono">{new Date(d.startedAt).toLocaleString()} ({fmtClock(d.startedAt)})</span>
        <span>Client</span>
        <span className="mono">{d.clientAddr ?? '—'}</span>
        <span>Server</span>
        <span className="mono">{d.serverAddr ?? '—'}</span>
        <span>Protocol</span>
        <span className="mono">
          {d.request.httpVersion} client{d.response ? ` · ${d.response.httpVersion} server` : ''}
        </span>
      </div>
    </div>
  );
}

function FramesPane({ d }: { d: ExchangeDetail }) {
  const frames = d.frames;
  const [pick, setPick] = useState<number | null>(null);
  const f: WsFrame | undefined = pick != null ? frames[pick] : undefined;
  if (!frames.length) return <Empty title="No frames yet">Frames appear as the WebSocket carries messages.</Empty>;
  return (
    <div className="insp-frames">
      <div className="insp-frame-list" role="listbox" aria-label="WebSocket frames">
        {frames.map((fr, i) => (
          <div key={i} role="option" aria-selected={pick === i} tabIndex={0} className={clsx('insp-frame', pick === i && 'on')} onClick={() => setPick(i)} onKeyDown={(e) => e.key === 'Enter' && setPick(i)}>
            {fr.direction === 'send' ? <ArrowUpRight size={13} className="text-accent" aria-label="Sent" /> : <ArrowDownLeft size={13} className="text-success" aria-label="Received" />}
            <span className="subtle mono xs">{fmtMs(fr.atMs)}</span>
            <span className="badge outline">{fr.opcode}</span>
            <span className="truncate mono xs grow">{fr.text ?? (fr.base64 ? `base64 ${fr.base64.slice(0, 60)}` : '')}</span>
            <span className="subtle xs">{formatBytes(fr.size)}</span>
          </div>
        ))}
        {d.wsFrames > frames.length && <p className="subtle xs p-2">{d.wsFrames - frames.length} more frame(s) were not kept (per-exchange limit).</p>}
      </div>
      {f && (
        <div className="insp-frame-detail">
          <CodeEditor key={pick} value={f.text != null ? prettyBody(f.text) : f.base64 ? hexDump(f.base64) : ''} language={f.text ? languageFor(undefined, f.text) : 'plaintext'} readOnly lineNumbers={false} />
        </div>
      )}
    </div>
  );
}

/** Findings of a "Validate against OpenAPI" rule for this exchange. */
function SpecFindings({ v }: { v: SpecValidation }) {
  const all = [...v.request.map((x) => ['request', x] as const), ...v.response.map((x) => ['response', x] as const)];
  if (all.length === 0)
    return (
      <div className="insp-spec ok xs">
        <CheckCircle2 size={12} className="text-success" /> Matches {v.spec}
        {v.responseChecked ? '' : ' (response not checked yet)'}
        {v.responseBodySkipped ? ' — response body too large or not captured, only its status was checked' : ''}
      </div>
    );
  return (
    <Callout kind={v.blocked ? 'danger' : 'warning'} className="m-3">
      <div className="strong small">
        {all.length} OpenAPI violation{all.length === 1 ? '' : 's'} ({v.spec}, rule “{v.rule}”){v.blocked ? ' — blocked by the proxy' : ''}
      </div>
      <ul className="small" style={{ margin: '4px 0 0', paddingLeft: 18 }}>
        {all.map(([phase, x], i) => (
          <li key={i}>
            <span className="subtle xs">{phase}</span> {x.message}
            {x.details.length > 0 && (
              <ul className="xs mono subtle" style={{ margin: 0 }}>
                {x.details.map((d, j) => (
                  <li key={j}>{d}</li>
                ))}
              </ul>
            )}
          </li>
        ))}
      </ul>
    </Callout>
  );
}

export function ExchangeDetailView({ id, onReplayed }: { id: ExchangeId; onReplayed(id: ExchangeId): void }) {
  const summary = useInspector((s) => s.items.get(id));
  const [detail, setDetail] = useState<ExchangeDetail | null>(null);
  const [missing, setMissing] = useState(false);
  const [tab, setTab] = useState<Tab>('response');
  const stamp = summary ? `${summary.state}|${summary.status}|${summary.responseSize}|${summary.requestSize}|${summary.wsFrames}|${summary.tags.length}|${summary.paused}|${summary.violations}` : '';

  useEffect(() => {
    let alive = true;
    const t = setTimeout(
      () => {
        proxyApi
          .get(id)
          .then((d) => {
            if (!alive) return;
            setDetail(d);
            setMissing(!d);
          })
          .catch(() => alive && setMissing(true));
      },
      detail?.id === id ? 150 : 0,
    );
    return () => {
      alive = false;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, stamp]);

  useEffect(() => {
    if (summary && (summary.state === 'webSocket' || summary.wsFrames > 0) && tab === 'response') setTab('frames');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  // JWTs and timestamps anywhere in the exchange (URL, headers, bodies), decoded inline.
  const detections = useDetections(
    detail && detail.id === id
      ? [detail.request.url ?? '', messageText(detail.request.headers, detail.request.body.text), detail.response ? messageText(detail.response.headers, detail.response.body.text) : ''].join('\n')
      : null,
  );
  const tabs = useMemo(() => {
    const t: [Tab, React.ReactNode][] = [
      ['request', 'Request'],
      ['response', 'Response'],
      ['timing', 'Timing'],
    ];
    if (summary && (summary.state === 'webSocket' || summary.wsFrames > 0)) t.push(['frames', `Frames (${summary.wsFrames})`]);
    if (detections.length) t.push(['decoded', <DecodedLabel count={detections.length} />]);
    return t;
  }, [summary, detections.length]);

  if (missing) return <Empty title="No longer captured">The exchange was cleared or dropped from the capture.</Empty>;
  if (!detail || detail.id !== id)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );

  const replay = async () => {
    try {
      const nid = await proxyApi.replay(id);
      onReplayed(nid);
    } catch (e) {
      toast.error('Replay failed', errorMessage(e));
    }
  };
  const s = summary ?? detail;

  return (
    <div className="insp-detail">
      <div className="insp-detail-head">
        <MethodBadge method={s.method} />
        {s.status != null ? <StatusCode status={s.status} /> : <span className="badge">{s.state}</span>}
        <span className="mono truncate grow insp-url" title={s.url}>
          {s.url}
        </span>
        <button className="icon-btn sm" onClick={() => copyText(s.url, 'URL copied')} title="Copy URL" aria-label="Copy URL">
          <Copy />
        </button>
        <button
          className="icon-btn sm"
          title="Copy as cURL"
          aria-label="Copy as cURL"
          onClick={() =>
            proxyApi
              .copyCurl(id)
              .then((c) => copyText(c, 'cURL copied (credentials redacted)'))
              .catch((e) => toast.error('Copy failed', errorMessage(e)))
          }
        >
          <Terminal />
        </button>
        <button className="icon-btn sm" title="Replay" aria-label="Replay" onClick={replay} disabled={s.method === 'CONNECT'}>
          <RotateCw />
        </button>
      </div>
      <div className="insp-chips">
        <span className="subtle xs">{fmtClock(s.startedAt)}</span>
        <span className="subtle xs">{fmtMs(s.durationMs)}</span>
        <span className="subtle xs">{s.httpVersion}</span>
        {s.mappedUrl && (
          <span className="tag" title="Sent to">
            <ArrowRight /> {s.mappedUrl}
          </span>
        )}
        {s.rules.map((r) => (
          <span key={r} className="badge accent" title="Matched rule">
            {r}
          </span>
        ))}
        {s.tags
          .filter((t) => t !== 'mapRemote')
          .map((t) => (
            <span key={t} className="badge outline">
              {t}
            </span>
          ))}
      </div>
      {s.error && (
        <Callout kind="danger" className="m-3">
          {s.error}
        </Callout>
      )}
      {detail.validation && <SpecFindings v={detail.validation} />}
      <div className="subtabs">
        {tabs.map(([k, label]) => (
          <button key={k} className={clsx('subtab', tab === k && 'on')} onClick={() => setTab(k)}>
            {label}
          </button>
        ))}
      </div>
      <div className="insp-tab-body">
        {tab === 'request' && <MessagePane key={`req-${id}`} m={detail.request} id={id} kind="request" url={s.url} />}
        {tab === 'response' &&
          (detail.response ? (
            <MessagePane key={`resp-${id}`} m={detail.response} id={id} kind="response" />
          ) : (
            <Empty title={s.state === 'pending' || s.state === 'paused' ? 'Waiting for the response…' : 'No response'}>{s.state === 'tunnel' ? 'This connection was tunnelled without decryption.' : undefined}</Empty>
          ))}
        {tab === 'timing' && <TimingPane d={detail} />}
        {tab === 'frames' && <FramesPane d={detail} />}
        {tab === 'decoded' && <DecodedValues items={detections} empty />}
      </div>
    </div>
  );
}

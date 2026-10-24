import { useEffect, useState } from 'react';
import { Zap, Wand2 } from 'lucide-react';
import type { DelayDistribution, Fault, ResponseDefinition } from '@/features/mocks/lib/types';
import { CodeEditor, languageFor } from '@/components/editor/CodeEditor';
import { Field, Segmented, Switch, Callout } from '@/features/mocks/components/ui';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { StringMapEditor, TagsInput } from './PatternEditors';
import { headerValue, prettyJson, tryParseJson } from '@/features/mocks/lib/stub-utils';
import { TEMPLATE_SNIPPETS } from '@/features/mocks/lib/templates';
import { useCapabilities, useFiles } from '@/features/mocks/lib/queries';
import { AiMockDataButton, FakeDataButton, RandomizeJsonButton } from './FakeData';
import { DataSourceButton } from './DataSource';

export const FAULTS: { value: Fault; label: string; help: string }[] = [
  { value: 'CONNECTION_RESET_BY_PEER', label: 'Connection reset by peer', help: 'Closes the connection with a TCP RST. Clients see "connection reset".' },
  { value: 'EMPTY_RESPONSE', label: 'Empty response', help: 'Closes the connection without sending anything.' },
  { value: 'MALFORMED_RESPONSE_CHUNK', label: 'Malformed chunk', help: 'Sends an OK status and headers, then garbage chunked data, then closes.' },
  { value: 'RANDOM_DATA_THEN_CLOSE', label: 'Random data then close', help: 'Sends random bytes (not HTTP), then closes.' },
];

const COMMON_STATUS = [200, 201, 202, 204, 301, 302, 304, 400, 401, 403, 404, 405, 409, 410, 415, 422, 429, 500, 501, 502, 503, 504];
const RESPONSE_HEADERS = ['Content-Type', 'Cache-Control', 'Location', 'Set-Cookie', 'ETag', 'Retry-After', 'WWW-Authenticate', 'Access-Control-Allow-Origin', 'X-Request-Id', 'Content-Disposition', 'Content-Encoding'];

type Mode = 'respond' | 'proxy' | 'fault' | 'channel';
type BodyKind = 'none' | 'json' | 'text' | 'file' | 'base64';

function bodyKindOf(r: ResponseDefinition): BodyKind {
  if (r.jsonBody !== undefined) return 'json';
  if (r.bodyFileName !== undefined) return 'file';
  if (r.base64Body !== undefined) return 'base64';
  if (r.body !== undefined) return 'text';
  return 'none';
}

/** Fields WireMock 4 rejects next to openWebsocketChannel / openSseChannel. */
const CHANNEL_CONFLICTS = ['body', 'jsonBody', 'base64Body', 'bodyFileName', 'fault', 'proxyBaseUrl', 'proxyUrlPrefixToRemove', 'additionalProxyRequestHeaders', 'removeProxyRequestHeaders'] as const;

export function ResponseSection({ value, onChange }: { value: ResponseDefinition; onChange(r: ResponseDefinition): void }) {
  const r = value ?? {};
  const caps = useCapabilities();
  const mode: Mode = r.openWebsocketChannel || r.openSseChannel ? 'channel' : r.fault ? 'fault' : r.proxyBaseUrl !== undefined ? 'proxy' : 'respond';
  const set = (patch: Partial<ResponseDefinition>) => {
    const next = { ...r, ...patch };
    for (const k of Object.keys(next)) if (next[k] === undefined) delete next[k];
    onChange(next);
  };

  const switchMode = (m: Mode) => {
    const next: ResponseDefinition = { ...r };
    delete next.fault;
    delete next.openWebsocketChannel;
    delete next.openSseChannel;
    if (m === 'channel') {
      for (const k of CHANNEL_CONFLICTS) delete next[k];
      delete next.status;
      next.openWebsocketChannel = true;
      return onChange(next);
    }
    if (m !== 'proxy') {
      delete next.proxyBaseUrl;
      delete next.proxyUrlPrefixToRemove;
      delete next.additionalProxyRequestHeaders;
      delete next.removeProxyRequestHeaders;
    }
    if (m === 'fault') next.fault = 'CONNECTION_RESET_BY_PEER';
    if (m === 'proxy') next.proxyBaseUrl = next.proxyBaseUrl ?? 'https://';
    if (m === 'respond' && next.status === undefined) next.status = 200;
    onChange(next);
  };

  return (
    <div className="col">
      <Segmented<Mode>
        value={mode}
        onChange={switchMode}
        options={[
          { value: 'respond', label: 'Respond' },
          { value: 'proxy', label: 'Proxy to upstream' },
          { value: 'fault', label: 'Simulate fault' },
          ...(caps.channelStubs || mode === 'channel' ? [{ value: 'channel' as const, label: 'Open channel', title: 'WireMock 4: accept a WebSocket or start a server-sent events stream' }] : []),
        ]}
      />
      {mode === 'respond' && <RespondFields r={r} set={set} />}
      {mode === 'channel' && <ChannelFields r={r} set={set} />}
      {mode === 'proxy' && <ProxyFields r={r} set={set} />}
      {mode === 'fault' && (
        <div className="col-sm">
          {FAULTS.map((f) => (
            <label key={f.value} className="check" style={{ alignItems: 'flex-start' }}>
              <input type="radio" name="fault" checked={r.fault === f.value} onChange={() => set({ fault: f.value })} style={{ marginTop: 3 }} />
              <span>
                <span className="strong">{f.label}</span> <code className="code-inline xs">{f.value}</code>
                <div className="subtle xs">{f.help}</div>
              </span>
            </label>
          ))}
          <div className="subtle xs">Browsers report every fault as a generic network error; use cURL or your app to see the real behaviour.</div>
        </div>
      )}
      {mode !== 'channel' && <DelayEditor r={r} set={set} />}
    </div>
  );
}

function ChannelFields({ r, set }: { r: ResponseDefinition; set(p: Partial<ResponseDefinition>): void }) {
  const caps = useCapabilities();
  const ws = !!r.openWebsocketChannel;
  return (
    <div className="col-sm">
      <Segmented<'websocket' | 'sse'>
        value={ws ? 'websocket' : 'sse'}
        onChange={(k) => set(k === 'websocket' ? { openWebsocketChannel: true, openSseChannel: undefined, status: undefined } : { openSseChannel: true, openWebsocketChannel: undefined })}
        options={[
          { value: 'websocket', label: 'WebSocket' },
          { value: 'sse', label: 'Server-sent events' },
        ]}
      />
      <div className="subtle xs">
        {ws ? (
          <>
            Matching requests are upgraded to a WebSocket (<code className="code-inline">openWebsocketChannel</code>). Since WireMock 4.0.0-beta.39 an upgrade needs a stub like
            this one; without it the client gets a 404.
          </>
        ) : (
          <>
            Matching requests get an open <code className="code-inline">text/event-stream</code> (<code className="code-inline">openSseChannel</code>). Events are pushed by
            message stubs or the admin API; a body, proxy or fault is not allowed.
          </>
        )}{' '}
        What gets sent over the channel is defined by{' '}
message stubs (WireMock 4; edit them through that server’s admin API — the built-in engine has no channels).
      </div>
      {!ws && (
        <Field label="Headers">
          <StringMapEditor value={r.headers} onChange={(headers) => set({ headers })} keySuggestions={RESPONSE_HEADERS} />
        </Field>
      )}
      {caps.loaded && !caps.channelStubs && <Callout kind="warning">This server does not support channel stubs (WireMock 4.0.0-beta.39 or later needed).</Callout>}
    </div>
  );
}

function RespondFields({ r, set }: { r: ResponseDefinition; set(p: Partial<ResponseDefinition>): void }) {
  const kind = bodyKindOf(r);
  const templated = !!r.transformers?.includes('response-template');
  const ct = headerValue(r.headers as Record<string, string>, 'content-type');
  const files = useFiles();

  const switchKind = (k: BodyKind) => {
    const cur = r.jsonBody !== undefined ? prettyJson(r.jsonBody) : (r.body ?? '');
    const patch: Partial<ResponseDefinition> = { body: undefined, jsonBody: undefined, bodyFileName: undefined, base64Body: undefined };
    if (k === 'json') {
      const p = tryParseJson(cur);
      patch.jsonBody = p.ok ? p.value : {};
    } else if (k === 'text') patch.body = cur;
    else if (k === 'file') patch.bodyFileName = '';
    else if (k === 'base64') patch.base64Body = '';
    set(patch);
  };

  return (
    <>
      <div className="row items-start">
        <Field label="Status" className="shrink-0" hint={statusText(r.status)}>
          <input className="input" style={{ width: 90 }} type="number" list="wmui-status" value={r.status ?? ''} placeholder="200" onChange={(e) => set({ status: e.target.value ? Number(e.target.value) : undefined })} />
        </Field>
        <StatusMessageField value={r.statusMessage} onChange={(statusMessage) => set({ statusMessage })} />
        <datalist id="wmui-status">
          {COMMON_STATUS.map((s) => (
            <option key={s} value={s}>
              {statusText(s)}
            </option>
          ))}
        </datalist>
      </div>
      <Field label="Headers">
        <StringMapEditor value={r.headers} onChange={(headers) => set({ headers })} keySuggestions={RESPONSE_HEADERS} />
      </Field>
      <div className="col-sm">
        <div className="row-sm">
          <span className="field-label grow">Body</span>
          <Segmented<BodyKind>
            value={kind}
            onChange={switchKind}
            options={[
              { value: 'none', label: 'None' },
              { value: 'json', label: 'JSON' },
              { value: 'text', label: 'Text / template' },
              { value: 'file', label: 'File' },
              { value: 'base64', label: 'Binary' },
            ]}
          />
        </div>
        {kind === 'json' && (
          <JsonBodyEditor
            value={r.jsonBody}
            onChange={(jsonBody) => set({ jsonBody })}
            onTemplate={(body) => set({ jsonBody: undefined, body, transformers: withTemplating(r.transformers), headers: ct ? r.headers : { ...(r.headers ?? {}), 'Content-Type': 'application/json' } })}
          />
        )}
        {kind === 'text' && (
          <TextBodyEditor
            value={r.body ?? ''}
            onChange={(body) => set({ body })}
            templated={templated}
            contentType={ct}
            onInsertHelper={(snippet) => set({ body: (r.body ?? '') + snippet, transformers: withTemplating(r.transformers) })}
            extraTools={
              <DataSourceButton
                onInsert={(snippet) => set({ body: (r.body ?? '') + snippet, transformers: withTemplating(r.transformers) })}
                onLookup={(dataSource) => set({ transformerParameters: { ...(r.transformerParameters ?? {}), dataSource }, transformers: withTemplating(r.transformers) })}
              />
            }
          />
        )}
        {kind === 'file' && (
          <Field hint="Path relative to the server's __files folder. Manage files on the Files page.">
            <input className="input mono" list="wmui-files" value={r.bodyFileName ?? ''} placeholder="responses/user.json" onChange={(e) => set({ bodyFileName: e.target.value })} />
            <datalist id="wmui-files">
              {(files.data ?? []).map((f) => (
                <option key={f} value={f} />
              ))}
            </datalist>
          </Field>
        )}
        {kind === 'base64' && (
          <Field hint="Base64-encoded bytes (images, PDFs, protobuf…).">
            <textarea className="textarea mono" rows={4} value={r.base64Body ?? ''} onChange={(e) => set({ base64Body: e.target.value.replace(/\s+/g, '') })} />
          </Field>
        )}
      </div>
      <div className="row wrap">
        <Switch
          checked={templated}
          onChange={(v) => {
            const t = new Set(r.transformers ?? []);
            if (v) t.add('response-template');
            else t.delete('response-template');
            set({ transformers: t.size ? [...t] : undefined });
          }}
          label={
            <span className="row-sm">
              <Zap size={13} /> Response templating
            </span>
          }
          title="Adds the response-template transformer (Handlebars). Not needed if the server runs with --global-response-templating."
        />
        <span className="subtle xs">Other transformers:</span>
        <div style={{ minWidth: 220 }} className="grow">
          <TagsInput value={(r.transformers ?? []).filter((t) => t !== 'response-template')} onChange={(v) => set({ transformers: [...(templated ? ['response-template'] : []), ...v].length ? [...(templated ? ['response-template'] : []), ...v] : undefined })} placeholder="extension name" />
        </div>
      </div>
      {(templated || r.transformerParameters) && <TransformerParams value={r.transformerParameters} onChange={(transformerParameters) => set({ transformerParameters })} />}
    </>
  );
}

function StatusMessageField({ value, onChange }: { value?: string; onChange(v?: string): void }) {
  const caps = useCapabilities();
  // Jetty 12 (WireMock 4) always sends the standard reason phrase; hide the field unless a stub already sets it.
  if (!caps.statusMessage && !value) return null;
  return (
    <Field
      label="Status message (optional)"
      className="grow"
      error={!caps.statusMessage ? 'Ignored by WireMock 4: Jetty 12 always sends the standard reason phrase.' : undefined}
    >
      <input className="input" value={value ?? ''} placeholder="Default reason phrase" onChange={(e) => onChange(e.target.value || undefined)} />
    </Field>
  );
}

function TransformerParams({ value, onChange }: { value?: Record<string, unknown>; onChange(v?: Record<string, unknown>): void }) {
  const [text, setText] = useState(value ? prettyJson(value) : '');
  const [err, setErr] = useState<string>();
  // Follow changes made elsewhere (e.g. the data source picker) unless the text already says the same.
  useEffect(() => {
    const p = text.trim() ? tryParseJson(text) : { ok: true as const, value: undefined };
    if (!p.ok || JSON.stringify(p.value ?? null) !== JSON.stringify(value ?? null)) setText(value ? prettyJson(value) : '');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);
  return (
    <Field label="Transformer parameters (JSON, available as {{parameters.x}})" error={err}>
      <div className="editor-shell">
        <CodeEditor
          value={text}
          minimal
          autoHeight={{ min: 40, max: 200 }}
          onChange={(t) => {
            setText(t);
            if (!t.trim()) return (setErr(undefined), onChange(undefined));
            const r = tryParseJson<Record<string, unknown>>(t);
            setErr(r.ok ? undefined : r.error);
            if (r.ok) onChange(r.value);
          }}
        />
      </div>
    </Field>
  );
}

function withTemplating(t?: string[]): string[] {
  return t?.includes('response-template') ? t : [...(t ?? []), 'response-template'];
}

function JsonBodyEditor({ value, onChange, onTemplate }: { value: unknown; onChange(v: unknown): void; onTemplate(body: string): void }) {
  const [text, setText] = useState(() => prettyJson(value));
  const [err, setErr] = useState<string>();
  useEffect(() => {
    const p = tryParseJson(text);
    if (!p.ok || JSON.stringify(p.value) !== JSON.stringify(value)) setText(prettyJson(value));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);
  return (
    <div className="col-sm">
      <div className="editor-shell">
        <CodeEditor
          value={text}
          language="json"
          autoHeight={{ min: 80, max: 480 }}
          onChange={(t) => {
            setText(t);
            const p = tryParseJson(t);
            setErr(p.ok ? undefined : p.error);
            if (p.ok) onChange(p.value);
          }}
        />
      </div>
      <div className="row-sm wrap">
        {err ? <div className="text-danger xs grow">{err}</div> : <div className="subtle xs grow">Stored as <code className="code-inline">jsonBody</code>. Need Handlebars inside JSON? Use “Text / template”.</div>}
        <RandomizeJsonButton json={value} disabled={!!err} onApply={onTemplate} />
        <AiMockDataButton sample={value} disabled={!!err} onResult={onChange} />
      </div>
    </div>
  );
}

function TextBodyEditor({
  value,
  onChange,
  templated,
  contentType,
  onInsertHelper,
  extraTools,
}: {
  value: string;
  onChange(v: string): void;
  templated: boolean;
  contentType?: string;
  onInsertHelper(snippet: string): void;
  extraTools?: React.ReactNode;
}) {
  const lang = templated ? 'handlebars' : languageFor(contentType, value);
  const items = (): MenuItemDef[] => {
    const out: MenuItemDef[] = [];
    let g = '';
    for (const s of TEMPLATE_SNIPPETS) {
      if (s.group !== g) out.push({ heading: (g = s.group) });
      out.push({ label: s.label, hint: s.snippet.length > 28 ? s.snippet.slice(0, 26) + '…' : s.snippet, onSelect: () => onChange(value + s.snippet) });
    }
    return out;
  };
  return (
    <div className="col-sm">
      <div className="editor-shell">
        <CodeEditor value={value} onChange={onChange} language={lang} autoHeight={{ min: 80, max: 480 }} />
      </div>
      <div className="row-sm">
        <Menu
          items={items}
          trigger={(p) => (
            <button className="btn btn-sm btn-ghost" {...p}>
              <Wand2 /> Insert template helper
            </button>
          )}
        />
        <FakeDataButton onInsert={onInsertHelper} />
        {extraTools}
        {!templated && value.includes('{{') && <span className="text-warning xs">Contains {'{{…}}'} but templating is off.</span>}
        <span className="spacer" />
        <button
          className="btn btn-sm btn-ghost"
          onClick={() => {
            const p = tryParseJson(value);
            if (p.ok) onChange(prettyJson(p.value));
          }}
        >
          Format JSON
        </button>
      </div>
    </div>
  );
}

function ProxyFields({ r, set }: { r: ResponseDefinition; set(p: Partial<ResponseDefinition>): void }) {
  return (
    <>
      <Callout>
        Matching requests are forwarded to the upstream and its response is returned. Combine with a low priority (e.g. 10) and a broad URL regex to proxy “everything not stubbed”.
      </Callout>
      <Field label="Upstream base URL" hint="The request path is appended, e.g. https://api.example.com + /v1/users">
        <input className="input mono" value={r.proxyBaseUrl ?? ''} placeholder="https://api.example.com" onChange={(e) => set({ proxyBaseUrl: e.target.value })} />
      </Field>
      <Field label="Remove path prefix" hint="Strip a prefix before forwarding, e.g. /payments → upstream receives /v1/charge for /payments/v1/charge">
        <input className="input mono" value={r.proxyUrlPrefixToRemove ?? ''} placeholder="/payments" onChange={(e) => set({ proxyUrlPrefixToRemove: e.target.value || undefined })} />
      </Field>
      <Field label="Add request headers when proxying">
        <StringMapEditor value={r.additionalProxyRequestHeaders} onChange={(v) => set({ additionalProxyRequestHeaders: v as Record<string, string> | undefined })} keySuggestions={['Authorization', 'X-Api-Key', 'Host']} />
      </Field>
      <Field label="Remove request headers when proxying">
        <TagsInput value={r.removeProxyRequestHeaders ?? []} onChange={(v) => set({ removeProxyRequestHeaders: v.length ? v : undefined })} placeholder="Header name, Enter to add" />
      </Field>
      <Field label="Extra response headers (added on top of upstream's)">
        <StringMapEditor value={r.headers} onChange={(headers) => set({ headers })} />
      </Field>
    </>
  );
}

type DelayKind = 'none' | 'fixed' | 'uniform' | 'lognormal' | 'dribble';

function DelayEditor({ r, set }: { r: ResponseDefinition; set(p: Partial<ResponseDefinition>): void }) {
  const kind: DelayKind = r.chunkedDribbleDelay
    ? 'dribble'
    : r.delayDistribution?.type === 'uniform'
      ? 'uniform'
      : r.delayDistribution?.type === 'lognormal'
        ? 'lognormal'
        : r.fixedDelayMilliseconds || r.delayDistribution?.type === 'fixed'
          ? 'fixed'
          : 'none';
  const dd = r.delayDistribution as DelayDistribution | undefined;
  const num = (v: string) => (v === '' ? undefined : Number(v));
  const change = (k: DelayKind) => {
    const patch: Partial<ResponseDefinition> = { fixedDelayMilliseconds: undefined, delayDistribution: undefined, chunkedDribbleDelay: undefined };
    if (k === 'fixed') patch.fixedDelayMilliseconds = 1000;
    if (k === 'uniform') patch.delayDistribution = { type: 'uniform', lower: 200, upper: 1200 };
    if (k === 'lognormal') patch.delayDistribution = { type: 'lognormal', median: 500, sigma: 0.4 };
    if (k === 'dribble') patch.chunkedDribbleDelay = { numberOfChunks: 5, totalDuration: 2000 };
    set(patch);
  };
  return (
    <div className="col-sm">
      <div className="row-sm">
        <span className="field-label grow">Delay</span>
        <Segmented<DelayKind>
          value={kind}
          onChange={change}
          options={[
            { value: 'none', label: 'None' },
            { value: 'fixed', label: 'Fixed' },
            { value: 'uniform', label: 'Random (uniform)' },
            { value: 'lognormal', label: 'Random (log-normal)' },
            { value: 'dribble', label: 'Chunked dribble' },
          ]}
        />
      </div>
      {kind === 'fixed' && (
        <div className="row-sm">
          <input className="input sm" style={{ width: 110 }} type="number" min={0} value={r.fixedDelayMilliseconds ?? (dd?.type === 'fixed' ? dd.milliseconds : '')} onChange={(e) => set({ fixedDelayMilliseconds: num(e.target.value), delayDistribution: undefined })} />
          <span className="subtle xs">ms before the response is sent</span>
        </div>
      )}
      {kind === 'uniform' && dd?.type === 'uniform' && (
        <div className="row-sm">
          <span className="subtle xs">between</span>
          <input className="input sm" style={{ width: 90 }} type="number" value={dd.lower} onChange={(e) => set({ delayDistribution: { ...dd, lower: Number(e.target.value) } })} />
          <span className="subtle xs">and</span>
          <input className="input sm" style={{ width: 90 }} type="number" value={dd.upper} onChange={(e) => set({ delayDistribution: { ...dd, upper: Number(e.target.value) } })} />
          <span className="subtle xs">ms</span>
        </div>
      )}
      {kind === 'lognormal' && dd?.type === 'lognormal' && (
        <div className="row-sm wrap">
          <span className="subtle xs">median</span>
          <input className="input sm" style={{ width: 90 }} type="number" value={dd.median} onChange={(e) => set({ delayDistribution: { ...dd, median: Number(e.target.value) } })} />
          <span className="subtle xs">ms, sigma</span>
          <input className="input sm" style={{ width: 70 }} type="number" step="0.05" value={dd.sigma} onChange={(e) => set({ delayDistribution: { ...dd, sigma: Number(e.target.value) } })} />
          <span className="subtle xs">max</span>
          <input className="input sm" style={{ width: 90 }} type="number" value={dd.maxValue ?? ''} placeholder="∞" onChange={(e) => set({ delayDistribution: { ...dd, maxValue: num(e.target.value) } })} />
          <span className="subtle xs">ms · realistic latency with a long tail</span>
        </div>
      )}
      {kind === 'dribble' && r.chunkedDribbleDelay && (
        <div className="row-sm">
          <input className="input sm" style={{ width: 80 }} type="number" value={r.chunkedDribbleDelay.numberOfChunks} onChange={(e) => set({ chunkedDribbleDelay: { ...r.chunkedDribbleDelay!, numberOfChunks: Number(e.target.value) } })} />
          <span className="subtle xs">chunks over</span>
          <input className="input sm" style={{ width: 90 }} type="number" value={r.chunkedDribbleDelay.totalDuration} onChange={(e) => set({ chunkedDribbleDelay: { ...r.chunkedDribbleDelay!, totalDuration: Number(e.target.value) } })} />
          <span className="subtle xs">ms</span>
        </div>
      )}
      {kind === 'dribble' && (r.fixedDelayMilliseconds || r.delayDistribution) && <span className="subtle xs">Also has an initial delay — edit it in JSON view.</span>}
    </div>
  );
}

export function statusText(s?: number): string {
  const m: Record<number, string> = {
    200: 'OK', 201: 'Created', 202: 'Accepted', 204: 'No Content', 301: 'Moved Permanently', 302: 'Found', 304: 'Not Modified', 307: 'Temporary Redirect', 308: 'Permanent Redirect',
    400: 'Bad Request', 401: 'Unauthorized', 403: 'Forbidden', 404: 'Not Found', 405: 'Method Not Allowed', 406: 'Not Acceptable', 408: 'Request Timeout', 409: 'Conflict', 410: 'Gone', 413: 'Payload Too Large',
    415: 'Unsupported Media Type', 418: "I'm a teapot", 422: 'Unprocessable Entity', 429: 'Too Many Requests', 500: 'Internal Server Error', 501: 'Not Implemented', 502: 'Bad Gateway', 503: 'Service Unavailable', 504: 'Gateway Timeout',
  };
  return s ? (m[s] ?? '') : '';
}

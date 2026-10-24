import { useMemo, useState, type ReactNode } from 'react';
import { ChevronRight, Plus, X } from 'lucide-react';
import type { ContentPattern, RequestPattern, ServeEventListenerDef, StubMapping } from '@/features/mocks/lib/types';
import { URL_KEYS, URL_KEY_LABEL, urlKeyOf, folderOf, withFolder, tagsOf, withWmui, wmui, prettyJson, tryParseJson, headerValue, GUI_NS, NS, DISABLED_HEADER } from '@/features/mocks/lib/stub-utils';
import { Field, Checkbox, Callout } from '@/features/mocks/components/ui';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { PatternMapEditor, BodyPatternsEditor, TagsInput, InlinePattern, StringMapEditor, MethodSelect } from './form/PatternEditors';
import { ResponseSection } from './form/ResponseSection';
import { useMappings, useScenarios, useWm } from '@/features/mocks/lib/queries';

const REQUEST_HEADERS = ['Content-Type', 'Accept', 'Authorization', 'User-Agent', 'X-Request-Id', 'X-Api-Key', 'Origin', 'Cookie', 'If-None-Match', 'X-Forwarded-For', 'Accept-Language'];

export function Section({ title, count, children, defaultOpen = true, extra }: { title: string; count?: number | string; children: ReactNode; defaultOpen?: boolean; extra?: ReactNode }) {
  return (
    <details className="fieldset" open={defaultOpen}>
      <summary>
        <ChevronRight className="chev" />
        <span>{title}</span>
        {count !== undefined && count !== 0 && <span className="count">{count}</span>}
        <span className="spacer" />
        {extra}
      </summary>
      <div className="fieldset-body">{children}</div>
    </details>
  );
}

function Sub({ label, children, onRemove }: { label: string; children: ReactNode; onRemove?(): void }) {
  return (
    <div className="col-sm">
      <div className="row-sm">
        <span className="field-label grow">{label}</span>
        {onRemove && (
          <button className="icon-btn sm" onClick={onRemove} title={`Remove ${label.toLowerCase()}`}>
            <X />
          </button>
        )}
      </div>
      {children}
    </div>
  );
}

export function StubForm({ stub, onChange }: { stub: StubMapping; onChange(s: StubMapping): void; onSave?(): void }) {
  const mappings = useMappings();
  const scenarios = useScenarios();
  const builtin = useWm().target.kind === 'builtin';
  const up = (fn: (d: StubMapping) => void) => {
    const n = structuredClone(stub);
    fn(n);
    onChange(n);
  };
  const setReq = (patch: Partial<RequestPattern>) =>
    up((d) => {
      d.request = { ...d.request, ...patch };
      for (const k of Object.keys(d.request)) if (d.request[k] === undefined) delete d.request[k];
    });

  const allFolders = useMemo(() => [...new Set((mappings.data ?? []).map(folderOf).filter(Boolean))].sort(), [mappings.data]);
  const allTags = useMemo(() => [...new Set((mappings.data ?? []).flatMap(tagsOf))].sort(), [mappings.data]);
  const scenarioNames = (scenarios.data ?? []).map((s) => s.name);
  const scenarioStates = (scenarios.data ?? []).find((s) => s.name === stub.scenarioName)?.possibleStates ?? ['Started'];

  const r = stub.request ?? {};
  const urlKey = urlKeyOf(r) ?? 'urlPath';
  const meta = wmui(stub);
  const templateVars = urlKey === 'urlPathTemplate' ? [...String(r.urlPathTemplate ?? '').matchAll(/\{([^}]+)\}/g)].map((m) => m[1]) : [];
  const reqCt = headerValue(Object.fromEntries(Object.entries(r.headers ?? {}).map(([k, v]) => [k, String((v as ContentPattern).equalTo ?? (v as ContentPattern).contains ?? '')])), 'content-type');
  const visibleHeaders = Object.fromEntries(Object.entries(r.headers ?? {}).filter(([k]) => k !== DISABLED_HEADER));

  // Optional request sections: shown when present or added by the user.
  const [shown, setShown] = useState<Set<string>>(new Set());
  const has = (k: string) => shown.has(k) || r[k] !== undefined;
  const show = (k: string) => setShown(new Set([...shown, k]));
  const hide = (k: string) => {
    const n = new Set(shown);
    n.delete(k);
    setShown(n);
    setReq({ [k]: undefined });
  };
  const optional: { key: string; label: string }[] = [
    { key: 'queryParameters', label: 'Query parameters' },
    { key: 'headers', label: 'Headers' },
    { key: 'cookies', label: 'Cookies' },
    { key: 'bodyPatterns', label: 'Body' },
    { key: 'formParameters', label: 'Form parameters' },
    { key: 'multipartPatterns', label: 'Multipart' },
    { key: 'basicAuthCredentials', label: 'Basic auth' },
    { key: 'host', label: 'Host' },
    { key: 'port', label: 'Port' },
    { key: 'scheme', label: 'Scheme' },
    { key: 'clientIp', label: 'Client IP' },
    { key: 'customMatcher', label: 'Custom matcher' },
  ];

  const listeners: ServeEventListenerDef[] = Array.isArray(stub.serveEventListeners) ? stub.serveEventListeners : [];
  const extraMeta = Object.fromEntries(Object.entries(stub.metadata ?? {}).filter(([k]) => k !== GUI_NS && k !== NS));

  return (
    <div className="scroll p-5 grow">
      <div className="col" style={{ maxWidth: 1080 }}>
        {r.headers?.[DISABLED_HEADER] && (
          <Callout kind="warning">This stub is <b>disabled</b>: it requires a <code className="code-inline">{DISABLED_HEADER}</code> header no client sends. Enable it from the ⋯ menu.</Callout>
        )}
        <Section title="General">
          <div className="grid-2">
            <Field label="Name">
              <input className="input" value={stub.name ?? ''} placeholder="e.g. Get user — happy path" onChange={(e) => up((d) => void (d.name = e.target.value || undefined))} />
            </Field>
            <Field label="Folder" hint="Stored in metadata so every UI and teammate sees it">
              <input className="input" list="wmui-folders" value={folderOf(stub)} placeholder="users/profile" onChange={(e) => onChange(withFolder(stub, e.target.value))} />
              <datalist id="wmui-folders">
                {allFolders.map((f) => (
                  <option key={f} value={f} />
                ))}
              </datalist>
            </Field>
          </div>
          <Field label="Description">
            <textarea className="textarea" rows={2} value={meta.description ?? ''} placeholder="What is this stub for? Links to tickets, contract notes…" onChange={(e) => onChange(withWmui(stub, { description: e.target.value || undefined }))} />
          </Field>
          <div className="grid-3" style={{ gridTemplateColumns: '2fr 1fr 1fr' }}>
            <Field label="Tags">
              <TagsInput value={tagsOf(stub)} onChange={(tags) => onChange(withWmui(stub, { tags: tags.length ? tags : undefined }))} suggestions={allTags} />
            </Field>
            <Field label="Priority" hint="1 = highest, default 5">
              <input className="input" type="number" min={1} value={stub.priority ?? ''} placeholder="5" onChange={(e) => up((d) => void (d.priority = e.target.value ? Number(e.target.value) : undefined))} />
            </Field>
            <Field label="Persistence" hint={builtin ? 'Saved as a file in mocks/mappings/' : "Write to the server's mappings/ on save"}>
              <Checkbox
                checked={stub.persistent ?? builtin}
                onChange={(v) => up((d) => void (d.persistent = builtin ? v : v || undefined))}
                label={builtin ? 'Save to project' : 'Persistent'}
                title={builtin ? 'Unticked: the stub lives in memory only and is gone when the mock server restarts' : undefined}
              />
            </Field>
          </div>
        </Section>

        <Section title="Request">
          <div className="row items-start">
            <Field label="Method" className="shrink-0">
              <MethodSelect value={r.method ?? 'ANY'} onChange={(method) => setReq({ method })} />
            </Field>
            <Field label="URL match" className="shrink-0">
              <select
                className="select"
                style={{ width: 150 }}
                value={urlKey}
                onChange={(e) => {
                  const v = r[urlKey];
                  const patch: Partial<RequestPattern> = {};
                  for (const k of URL_KEYS) patch[k] = undefined;
                  patch[e.target.value as (typeof URL_KEYS)[number]] = (v as string) ?? '/';
                  if (e.target.value !== 'urlPathTemplate') patch.pathParameters = undefined;
                  setReq(patch);
                }}
              >
                {URL_KEYS.map((k) => (
                  <option key={k} value={k}>
                    {URL_KEY_LABEL[k]}
                  </option>
                ))}
              </select>
            </Field>
            <Field
              label="URL"
              className="grow"
              hint={
                urlKey === 'url'
                  ? 'Exact path + query string'
                  : urlKey === 'urlPath'
                    ? 'Exact path; match query params below'
                    : urlKey === 'urlPathTemplate'
                      ? 'RFC 6570 template, e.g. /users/{id} — available as {{request.path.id}}'
                      : 'Java regular expression'
              }
            >
              <input className="input mono" value={String(r[urlKey] ?? '')} placeholder={urlKey === 'urlPathTemplate' ? '/api/users/{id}' : urlKey.includes('Pattern') ? '/api/users/[0-9]+' : '/api/users'} onChange={(e) => setReq({ [urlKey]: e.target.value })} />
            </Field>
          </div>
          {templateVars.length > 0 && (
            <Sub label="Path parameters">
              <PatternMapEditor
                value={Object.fromEntries(templateVars.map((v) => [v, r.pathParameters?.[v] ?? { matches: '.+' }]))}
                onChange={(pathParameters) => setReq({ pathParameters })}
                fixedKeys
              />
            </Sub>
          )}
          {has('queryParameters') && (
            <Sub label="Query parameters" onRemove={() => hide('queryParameters')}>
              {urlKey === 'url' || urlKey === 'urlPattern' ? <div className="text-warning xs">With “{URL_KEY_LABEL[urlKey]}” the query string is part of the URL — switch to “Path equals” to match parameters individually.</div> : null}
              <PatternMapEditor value={r.queryParameters} onChange={(queryParameters) => setReq({ queryParameters })} addLabel="Add parameter" />
            </Sub>
          )}
          {has('headers') && (
            <Sub label="Headers" onRemove={() => hide('headers')}>
              <PatternMapEditor
                value={visibleHeaders}
                onChange={(h) => setReq({ headers: r.headers?.[DISABLED_HEADER] ? { ...(h ?? {}), [DISABLED_HEADER]: r.headers[DISABLED_HEADER] } : h })}
                keySuggestions={REQUEST_HEADERS}
                addLabel="Add header"
              />
            </Sub>
          )}
          {has('cookies') && (
            <Sub label="Cookies" onRemove={() => hide('cookies')}>
              <PatternMapEditor value={r.cookies} onChange={(cookies) => setReq({ cookies })} addLabel="Add cookie" />
            </Sub>
          )}
          {has('bodyPatterns') && (
            <Sub label="Body — all patterns must match" onRemove={() => hide('bodyPatterns')}>
              <BodyPatternsEditor value={r.bodyPatterns} onChange={(bodyPatterns) => setReq({ bodyPatterns })} contentType={reqCt} />
            </Sub>
          )}
          {has('formParameters') && (
            <Sub label="Form parameters (application/x-www-form-urlencoded)" onRemove={() => hide('formParameters')}>
              <PatternMapEditor value={r.formParameters} onChange={(formParameters) => setReq({ formParameters })} addLabel="Add field" />
            </Sub>
          )}
          {has('multipartPatterns') && (
            <Sub label="Multipart parts" onRemove={() => hide('multipartPatterns')}>
              <MultipartEditor value={r.multipartPatterns ?? []} onChange={(multipartPatterns) => setReq({ multipartPatterns: multipartPatterns.length ? multipartPatterns : undefined })} />
            </Sub>
          )}
          {has('basicAuthCredentials') && (
            <Sub label="Basic auth credentials" onRemove={() => hide('basicAuthCredentials')}>
              <div className="grid-2">
                <input className="input" placeholder="username" value={r.basicAuthCredentials?.username ?? ''} onChange={(e) => setReq({ basicAuthCredentials: { username: e.target.value, password: r.basicAuthCredentials?.password ?? '' } })} />
                <input className="input" placeholder="password" value={r.basicAuthCredentials?.password ?? ''} onChange={(e) => setReq({ basicAuthCredentials: { username: r.basicAuthCredentials?.username ?? '', password: e.target.value } })} />
              </div>
            </Sub>
          )}
          {(has('host') || has('clientIp')) && (
            <div className="grid-2">
              {has('host') && (
                <Sub label="Host" onRemove={() => hide('host')}>
                  <div className="matcher-row no-key">
                    <InlinePattern value={r.host} onChange={(host) => setReq({ host })} />
                  </div>
                </Sub>
              )}
              {has('clientIp') && (
                <Sub label="Client IP" onRemove={() => hide('clientIp')}>
                  <div className="matcher-row no-key">
                    <InlinePattern value={r.clientIp} onChange={(clientIp) => setReq({ clientIp })} />
                  </div>
                </Sub>
              )}
            </div>
          )}
          {(has('port') || has('scheme')) && (
            <div className="grid-2">
              {has('port') && (
                <Sub label="Port" onRemove={() => hide('port')}>
                  <input className="input" type="number" value={r.port ?? ''} onChange={(e) => setReq({ port: e.target.value ? Number(e.target.value) : undefined })} />
                </Sub>
              )}
              {has('scheme') && (
                <Sub label="Scheme" onRemove={() => hide('scheme')}>
                  <select className="select" value={r.scheme ?? 'http'} onChange={(e) => setReq({ scheme: e.target.value as 'http' | 'https' })}>
                    <option>http</option>
                    <option>https</option>
                  </select>
                </Sub>
              )}
            </div>
          )}
          {has('customMatcher') && (
            <Sub label="Custom matcher (extension)" onRemove={() => hide('customMatcher')}>
              <div className="grid-2" style={{ gridTemplateColumns: '1fr 2fr' }}>
                <input className="input mono" placeholder="matcher-name" value={r.customMatcher?.name ?? ''} onChange={(e) => setReq({ customMatcher: { ...r.customMatcher, name: e.target.value } })} />
                <JsonField value={r.customMatcher?.parameters} onChange={(parameters) => setReq({ customMatcher: { name: r.customMatcher?.name ?? '', parameters: parameters as Record<string, unknown> } })} placeholder='{"param": "value"}' />
              </div>
            </Sub>
          )}
          <div className="row-sm wrap">
            <span className="subtle xs">Match on:</span>
            {optional
              .filter((o) => !has(o.key))
              .map((o) => (
                <button key={o.key} className="btn btn-sm btn-ghost" onClick={() => show(o.key)}>
                  <Plus /> {o.label}
                </button>
              ))}
          </div>
        </Section>

        <Section title="Response">
          <ResponseSection value={stub.response ?? {}} onChange={(response) => up((d) => void (d.response = response))} />
        </Section>

        <Section title="Scenario (stateful behaviour)" defaultOpen={!!stub.scenarioName} count={stub.scenarioName ? stub.scenarioName : undefined}>
          <div className="subtle xs">
            A stub in a scenario only matches when the scenario is in the required state, and can move it to a new state after serving. All scenarios start in{' '}
            <code className="code-inline">Started</code>.
          </div>
          <div className="grid-3">
            <Field label="Scenario name">
              <input className="input" list="wmui-scenarios" value={stub.scenarioName ?? ''} placeholder="Checkout flow" onChange={(e) => up((d) => void (d.scenarioName = e.target.value || undefined))} />
              <datalist id="wmui-scenarios">
                {scenarioNames.map((n) => (
                  <option key={n} value={n} />
                ))}
              </datalist>
            </Field>
            <Field label="Required state">
              <input className="input" list="wmui-states" value={stub.requiredScenarioState ?? ''} placeholder="Started" disabled={!stub.scenarioName} onChange={(e) => up((d) => void (d.requiredScenarioState = e.target.value || undefined))} />
            </Field>
            <Field label="New state after serving">
              <input className="input" list="wmui-states" value={stub.newScenarioState ?? ''} placeholder="(unchanged)" disabled={!stub.scenarioName} onChange={(e) => up((d) => void (d.newScenarioState = e.target.value || undefined))} />
            </Field>
            <datalist id="wmui-states">
              {scenarioStates.map((n) => (
                <option key={n} value={n} />
              ))}
            </datalist>
          </div>
        </Section>

        <Section title="Webhooks & serve event listeners" defaultOpen={listeners.length > 0 || !!stub.postServeActions} count={listeners.length || undefined}>
          <WebhooksEditor
            value={listeners}
            onChange={(l) => up((d) => void (d.serveEventListeners = l.length ? l : undefined))}
          />
          {stub.postServeActions && (
            <Callout kind="warning">
              <div className="row">
                <span className="grow">
                  Uses deprecated <code className="code-inline">postServeActions</code>.
                </span>
                <button
                  className="btn btn-sm"
                  onClick={() =>
                    up((d) => {
                      const psa = d.postServeActions;
                      const arr: ServeEventListenerDef[] = Array.isArray(psa) ? psa : Object.entries(psa ?? {}).map(([name, parameters]) => ({ name, parameters: parameters as Record<string, unknown> }));
                      d.serveEventListeners = [...(d.serveEventListeners ?? []), ...arr];
                      delete d.postServeActions;
                    })
                  }
                >
                  Convert to serveEventListeners
                </button>
              </div>
            </Callout>
          )}
        </Section>

        <Section title="Other metadata" defaultOpen={false} count={Object.keys(extraMeta).length || undefined}>
          <div className="subtle xs">
            Free-form JSON stored on the stub. Find or delete stubs by metadata via the Verify page. Folder, tags and description are managed above (<code className="code-inline">{GUI_NS}</code>,{' '}
            <code className="code-inline">{NS}</code>).
          </div>
          <JsonField
            value={Object.keys(extraMeta).length ? extraMeta : undefined}
            minHeight={80}
            onChange={(v) =>
              up((d) => {
                const keep = Object.fromEntries(Object.entries(d.metadata ?? {}).filter(([k]) => k === GUI_NS || k === NS));
                const next = { ...keep, ...((v as Record<string, unknown>) ?? {}) };
                d.metadata = Object.keys(next).length ? next : undefined;
              })
            }
            placeholder='{"owner": "team-payments", "ticket": "PAY-123"}'
          />
        </Section>
      </div>
    </div>
  );
}

function JsonField({ value, onChange, placeholder, minHeight = 40 }: { value: unknown; onChange(v: unknown): void; placeholder?: string; minHeight?: number }) {
  const [text, setText] = useState(value === undefined ? '' : prettyJson(value));
  const [err, setErr] = useState<string>();
  return (
    <div className="col-sm">
      <div className="editor-shell">
        <CodeEditor
          value={text}
          minimal
          autoHeight={{ min: minHeight, max: 300 }}
          onChange={(t) => {
            setText(t);
            if (!t.trim()) {
              setErr(undefined);
              return onChange(undefined);
            }
            const r = tryParseJson(t);
            setErr(r.ok ? undefined : r.error);
            if (r.ok) onChange(r.value);
          }}
        />
      </div>
      {err ? <div className="text-danger xs">{err}</div> : !text && placeholder ? <div className="subtle xs mono">e.g. {placeholder}</div> : null}
    </div>
  );
}

function MultipartEditor({ value, onChange }: { value: NonNullable<RequestPattern['multipartPatterns']>; onChange(v: NonNullable<RequestPattern['multipartPatterns']>): void }) {
  return (
    <div className="col-sm">
      {value.map((mp, i) => (
        <div key={i} className="fieldset" style={{ padding: 8 }}>
          <div className="col-sm">
            <div className="row-sm">
              <input className="input sm" style={{ width: 200 }} placeholder="part name (optional)" value={mp.name ?? ''} onChange={(e) => onChange(value.map((x, j) => (j === i ? { ...x, name: e.target.value || undefined } : x)))} />
              <select className="select sm" style={{ width: 170 }} value={mp.matchingType ?? 'ANY'} onChange={(e) => onChange(value.map((x, j) => (j === i ? { ...x, matchingType: e.target.value as 'ALL' | 'ANY' } : x)))}>
                <option value="ANY">any part matches</option>
                <option value="ALL">all parts match</option>
              </select>
              <span className="spacer" />
              <button className="icon-btn sm danger" onClick={() => onChange(value.filter((_, j) => j !== i))}>
                <X />
              </button>
            </div>
            <span className="field-label">Part headers</span>
            <PatternMapEditor value={mp.headers} onChange={(headers) => onChange(value.map((x, j) => (j === i ? { ...x, headers } : x)))} keySuggestions={['Content-Disposition', 'Content-Type']} />
            <span className="field-label">Part body</span>
            <BodyPatternsEditor value={mp.bodyPatterns} onChange={(bodyPatterns) => onChange(value.map((x, j) => (j === i ? { ...x, bodyPatterns } : x)))} />
          </div>
        </div>
      ))}
      <div>
        <button className="btn btn-sm btn-ghost" onClick={() => onChange([...value, { matchingType: 'ANY', headers: { 'Content-Disposition': { contains: 'name="file"' } } }])}>
          <Plus /> Add part
        </button>
      </div>
    </div>
  );
}

interface WebhookParams {
  method?: string;
  url?: string;
  headers?: Record<string, string>;
  body?: string;
  base64Body?: string;
  delay?: { type: 'fixed'; milliseconds: number } | { type: 'uniform'; lower: number; upper: number };
}

function WebhooksEditor({ value, onChange }: { value: ServeEventListenerDef[]; onChange(v: ServeEventListenerDef[]): void }) {
  const setAt = (i: number, l: ServeEventListenerDef) => onChange(value.map((x, j) => (j === i ? l : x)));
  return (
    <div className="col-sm">
      <div className="subtle xs">
        Webhooks fire an outbound HTTP call after the stub is served (from the WireMock server; the built-in engine keeps them on the stub but does not send them). Body and URL support templating via{' '}
        <code className="code-inline">{'{{originalRequest.…}}'}</code>.
      </div>
      {value.map((l, i) =>
        l.name === 'webhook' ? (
          <WebhookRow key={i} params={(l.parameters ?? {}) as WebhookParams} onChange={(p) => setAt(i, { ...l, parameters: p as Record<string, unknown> })} onRemove={() => onChange(value.filter((_, j) => j !== i))} />
        ) : (
          <div key={i} className="fieldset" style={{ padding: 8 }}>
            <div className="row-sm">
              <input className="input sm mono" style={{ width: 220 }} value={l.name} onChange={(e) => setAt(i, { ...l, name: e.target.value })} />
              <span className="subtle xs grow">custom listener (extension)</span>
              <button className="icon-btn sm danger" onClick={() => onChange(value.filter((_, j) => j !== i))}>
                <X />
              </button>
            </div>
            <JsonField value={l.parameters} onChange={(p) => setAt(i, { ...l, parameters: p as Record<string, unknown> })} />
          </div>
        ),
      )}
      <div className="row-sm">
        <button className="btn btn-sm btn-ghost" onClick={() => onChange([...value, { name: 'webhook', parameters: { method: 'POST', url: 'http://localhost:9000/callback', headers: { 'Content-Type': 'application/json' }, body: '{"ok": true}' } }])}>
          <Plus /> Webhook
        </button>
        <button className="btn btn-sm btn-ghost" onClick={() => onChange([...value, { name: 'my-listener', parameters: {} }])}>
          <Plus /> Custom listener
        </button>
      </div>
    </div>
  );
}

function WebhookRow({ params, onChange, onRemove }: { params: WebhookParams; onChange(p: WebhookParams): void; onRemove(): void }) {
  const set = (patch: Partial<WebhookParams>) => {
    const n = { ...params, ...patch };
    for (const k of Object.keys(n) as (keyof WebhookParams)[]) if (n[k] === undefined) delete n[k];
    onChange(n);
  };
  const delay = params.delay;
  return (
    <div className="fieldset" style={{ padding: 8 }}>
      <div className="col-sm">
        <div className="row-sm">
          <select className="select sm" style={{ width: 100 }} value={params.method ?? 'POST'} onChange={(e) => set({ method: e.target.value })}>
            {['POST', 'PUT', 'PATCH', 'GET', 'DELETE'].map((m) => (
              <option key={m}>{m}</option>
            ))}
          </select>
          <input className="input sm mono grow" placeholder="https://callback.example.com/hook" value={params.url ?? ''} onChange={(e) => set({ url: e.target.value })} />
          <span className="subtle xs">delay</span>
          <input
            className="input sm"
            style={{ width: 80 }}
            type="number"
            placeholder="0"
            value={delay?.type === 'fixed' ? delay.milliseconds : ''}
            onChange={(e) => set({ delay: e.target.value ? { type: 'fixed', milliseconds: Number(e.target.value) } : undefined })}
          />
          <span className="subtle xs">ms</span>
          <button className="icon-btn sm danger" onClick={onRemove}>
            <X />
          </button>
        </div>
        <span className="field-label">Headers</span>
        <StringMapEditor value={params.headers} onChange={(h) => set({ headers: h as Record<string, string> | undefined })} />
        <span className="field-label">Body</span>
        <div className="editor-shell">
          <CodeEditor value={params.body ?? ''} onChange={(body) => set({ body: body || undefined })} language="handlebars" autoHeight={{ min: 50, max: 240 }} minimal />
        </div>
      </div>
    </div>
  );
}

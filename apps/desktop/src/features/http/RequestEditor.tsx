import { useEffect, useState } from 'react';
import clsx from 'clsx';
import { BookOpen, Braces, Code2, Copy, CopyPlus, FileCode, FlaskConical, FolderOpen, Heading, History, ListFilter, Lock, MoreHorizontal, Save, Send, Ban, KeyRound, Settings2 } from 'lucide-react';
import { openLocalHistory } from '@/features/history/LocalHistoryDialog';
import { GenerateCodeModal } from '@/features/import';
import { GraphqlSchemaPanel } from '@/features/proto';
import { ScriptsEditor } from '@/features/scripts';
import { scriptCount } from '@/lib/api/scripts';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { Menu } from '@/components/ui/Menu';
import { Field, Switch, copyText } from '@/components/ui/misc';
import { ResponsiveTabs, type TabDef } from '@/components/ui/ResponsiveTabs';
import { LayoutToggle, ReqResSplit } from '@/components/layout/ReqResSplit';
import { toast } from '@/components/ui/toast';
import { VarInput } from '@/components/VarInput';
import { api, errorMessage } from '@/lib/ipc';
import type { HttpRequestFile, KeyValue } from '@/lib/types';
import { useActiveViewRef, useViewState } from '@/store/nav';
import { useVariables } from '@/store/variables';
import { isDirty, useWorkspace, type RequestTab } from '@/store/workspace';
import { AuthEditor, type TokenActions } from './AuthEditor';
import { TestsEditor } from './TestsEditor';
import { responseContext } from './aiContext';
import { BodyEditor } from './BodyEditor';
import { COMMON_HEADERS, KeyValueEditor } from './KeyValueEditor';
import { ResponsePanel } from './ResponsePanel';

const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'];
type Sub = 'params' | 'headers' | 'body' | 'auth' | 'tests' | 'scripts' | 'settings' | 'docs';

/** Splits `?a=1&b=2` typed into the URL bar into params (Postman-style two-way sync). */
function pullQuery(url: string, params: KeyValue[]): { url: string; params: KeyValue[] } {
  const i = url.indexOf('?');
  if (i < 0) return { url, params };
  const q = url.slice(i + 1);
  const added = q
    .split('&')
    .filter(Boolean)
    .map((p) => {
      const [k, ...v] = p.split('=');
      const dec = (s: string) => {
        try {
          return decodeURIComponent(s.replace(/\+/g, ' '));
        } catch {
          return s;
        }
      };
      return { name: dec(k), value: dec(v.join('=')) };
    });
  return { url: url.slice(0, i), params: [...params, ...added] };
}

export function RequestEditor({ tab }: { tab: RequestTab }) {
  const project = useWorkspace((s) => s.project!);
  const update = useWorkspace((s) => s.updateRequest);
  const save = useWorkspace((s) => s.save);
  const send = useWorkspace((s) => s.send);
  const cancel = useWorkspace((s) => s.cancel);
  const rename = useWorkspace((s) => s.rename);
  const activeEnv = useWorkspace((s) => s.activeEnv());
  const vars = useVariables(tab.path);
  // The selected sub-tab survives switching workbench tabs.
  const [sub, setSub] = useViewState<Sub>('request.sub', 'params');
  const activeView = useActiveViewRef();
  const [codegenOpen, setCodegenOpen] = useState(false);
  const [name, setName] = useState(tab.draft.name);
  const d = tab.draft;
  const set = (fn: (d: HttpRequestFile) => HttpRequestFile) => update(tab.id, fn);

  useEffect(() => setName(tab.draft.name), [tab.draft.name, tab.path]);

  // Ctrl/Cmd+Enter sends, Ctrl/Cmd+S saves — scoped to the active editor.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Only the editor of the active view (two can be visible in a split).
      if (!(e.ctrlKey || e.metaKey) || !activeView.current) return;
      if (e.key === 'Enter') {
        e.preventDefault();
        send(tab.id);
      } else if (e.key.toLowerCase() === 's') {
        e.preventDefault();
        save(tab.id);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [tab.id, send, save, activeView]);

  const counts: Partial<Record<Sub, number>> = {
    params: (d.params ?? []).filter((p) => p.enabled !== false).length,
    headers: (d.headers ?? []).filter((p) => p.enabled !== false).length,
    tests: (d.tests ?? []).length + (d.captures ?? []).length,
    scripts: scriptCount(d.scripts),
  };
  const tokens: TokenActions = {
    get: (force) => api.oauth2Token(project.root, tab.path, d, activeEnv?.id, force),
    clear: () => api.oauth2Clear(project.root, tab.path, d, activeEnv?.id),
  };
  const bodyType = d.body?.type ?? 'none';
  const authType = d.auth?.type ?? 'inherit';
  const subTabs: TabDef<Sub>[] = [
    { key: 'params', label: 'Params', icon: <ListFilter />, badge: counts.params || undefined },
    { key: 'headers', label: 'Headers', icon: <Heading />, badge: counts.headers || undefined },
    { key: 'body', label: 'Body', icon: <Braces />, dot: bodyType !== 'none' },
    { key: 'auth', label: 'Auth', icon: <Lock />, dot: authType !== 'inherit' && authType !== 'none' },
    { key: 'tests', label: 'Tests', icon: <FlaskConical />, badge: counts.tests || undefined },
    { key: 'scripts', label: 'Scripts', icon: <FileCode />, badge: counts.scripts || undefined },
    { key: 'settings', label: 'Settings', icon: <Settings2 /> },
    { key: 'docs', label: 'Docs', icon: <BookOpen />, dot: !!d.docs },
  ];

  const curl = async (includeSecrets: boolean) => {
    try {
      const cmd = await api.copyAsCurl(project.root, tab.path, d, activeEnv?.id, includeSecrets);
      await copyText(cmd, includeSecrets ? 'cURL copied (includes secret values)' : 'cURL copied (secrets masked)');
    } catch (e) {
      toast.error('Could not build cURL', errorMessage(e));
    }
  };

  return (
    <div className="request-editor">
      <div className="request-head">
        <input
          className="name-input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => name.trim() && name !== d.name && rename(tab.path, name.trim())}
          onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
          aria-label="Request name"
        />
        <span className="subtle xs mono truncate req-path" title={`${project.root}/http/${tab.path}`}>
          http/{tab.path}
        </span>
        <span className="spacer" />
        {isDirty(tab) && (
          <button className="btn btn-sm" onClick={() => save(tab.id)} title="Save (Ctrl+S)">
            <Save /> Save
          </button>
        )}
        <LayoutToggle />
        <Menu
          align="right"
          items={[
            { label: 'Copy as cURL (secrets masked)', icon: <Copy />, onSelect: () => curl(false) },
            { label: 'Copy as cURL with secrets', icon: <KeyRound />, onSelect: () => curl(true) },
            { label: 'Generate code…', icon: <Code2 />, onSelect: () => setCodegenOpen(true) },
            { separator: true },
            {
              label: 'Duplicate',
              icon: <CopyPlus />,
              onSelect: async () => {
                const p = await api.duplicateRequest(project.root, tab.path);
                await useWorkspace.getState().refresh();
                await useWorkspace.getState().openRequest(p, { preview: false });
              },
            },
            { label: 'Local history…', icon: <History />, onSelect: () => openLocalHistory({ root: project.root, path: `http/${tab.path}`, title: tab.draft.name }) },
            { label: 'Reveal file', icon: <FolderOpen />, onSelect: () => revealItemInDir(`${project.root}/http/${tab.path}`).catch((e) => toast.error('Could not reveal', String(e))) },
          ]}
          trigger={(p) => (
            <button className="icon-btn" {...p} aria-label="More actions">
              <MoreHorizontal />
            </button>
          )}
        />
        {codegenOpen && <GenerateCodeModal open path={tab.path} request={d} env={activeEnv?.id} onClose={() => setCodegenOpen(false)} />}
      </div>

      <div className="url-bar">
        <select className={clsx('select method-select', `m-${d.method}`)} value={d.method} onChange={(e) => set((x) => ({ ...x, method: e.target.value }))} aria-label="Method">
          {[...new Set([...METHODS, d.method])].map((m) => (
            <option key={m}>{m}</option>
          ))}
        </select>
        <VarInput
          value={d.url}
          vars={vars}
          placeholder="{{baseUrl}}/path   ·   paste a cURL command to import it"
          className="url-input"
          onChange={(url) => {
            if (/^\s*curl\s/i.test(url)) {
              api
                .importCurl(url)
                .then((imported) => {
                  set((x) => ({ ...imported, id: x.id, name: x.name }));
                  toast.success('Imported cURL command');
                })
                .catch((e) => toast.error('Could not parse cURL', errorMessage(e)));
              return;
            }
            set((x) => {
              const next = pullQuery(url, x.params ?? []);
              return { ...x, url: next.url, params: next.params };
            });
          }}
          onEnter={() => send(tab.id)}
        />
        {tab.sendId ? (
          <button className="btn btn-send" onClick={() => cancel(tab.id)} title="Cancel">
            <Ban /> <span className="btn-label">Cancel</span>
          </button>
        ) : (
          <button className="btn btn-primary btn-send" onClick={() => send(tab.id)} title="Send (Ctrl+Enter)">
            <Send /> <span className="btn-label">Send</span>
          </button>
        )}
      </div>

      <ReqResSplit
        request={
          <>
          <ResponsiveTabs tabs={subTabs} value={sub} onChange={setSub} label="Request" />
          <div className="request-sub">
            {sub === 'params' && (
              <KeyValueEditor rows={d.params ?? []} onChange={(params) => set((x) => ({ ...x, params }))} vars={vars} namePlaceholder="Query parameter" />
            )}
            {sub === 'headers' && (
              <KeyValueEditor rows={d.headers ?? []} onChange={(headers) => set((x) => ({ ...x, headers }))} vars={vars} namePlaceholder="Header" nameSuggestions={COMMON_HEADERS} />
            )}
            {sub === 'body' &&
              (d.body?.type === 'graphql' ? (
                <div className="row min-h-0" style={{ alignItems: 'stretch', gap: 'var(--space-3)', height: '100%' }}>
                  <div className="grow min-h-0" style={{ minWidth: 0 }}>
                    <BodyEditor body={d.body} onChange={(body) => set((x) => ({ ...x, body }))} vars={vars} projectRoot={project.root} />
                  </div>
                  <div style={{ width: 300, flexShrink: 0, minHeight: 0 }}>
                    <GraphqlSchemaPanel root={project.root} path={tab.path} request={d} onInsert={({ query, variables, operationName }) => set((x) => ({ ...x, body: { type: 'graphql', query, variables, operationName } }))} />
                  </div>
                </div>
              ) : (
                <BodyEditor body={d.body ?? { type: 'none' }} onChange={(body) => set((x) => ({ ...x, body }))} vars={vars} projectRoot={project.root} />
              ))}
            {sub === 'auth' && <AuthEditor auth={d.auth ?? { type: 'inherit' }} onChange={(auth) => set((x) => ({ ...x, auth }))} vars={vars} tokens={tokens} />}
            {sub === 'tests' && (
              <TestsEditor
                tests={d.tests ?? []}
                captures={d.captures ?? []}
                onTests={(tests) => set((x) => ({ ...x, tests: tests.length ? tests : undefined }))}
                onCaptures={(captures) => set((x) => ({ ...x, captures: captures.length ? captures : undefined }))}
                vars={vars}
                sample={responseContext(tab.result)}
                path={tab.path}
              />
            )}
            {sub === 'scripts' && <ScriptsEditor scripts={d.scripts} onChange={(scripts) => set((x) => ({ ...x, scripts }))} modelKey={tab.id} />}
            {sub === 'settings' && <SettingsTab draft={d} set={set} />}
            {sub === 'docs' && (
              <Field label="Notes (Markdown, saved with the request)">
                <textarea className="textarea" style={{ minHeight: 240 }} value={d.docs ?? ''} onChange={(e) => set((x) => ({ ...x, docs: e.target.value || undefined }))} />
              </Field>
            )}
          </div>
          </>
        }
        response={<ResponsePanel result={tab.result} sending={!!tab.sendId} onCancel={() => cancel(tab.id)} stream={tab.stream} progress={tab.progress} root={project.root} path={tab.path} />}
      />
    </div>
  );
}

function SettingsTab({ draft, set }: { draft: HttpRequestFile; set(fn: (d: HttpRequestFile) => HttpRequestFile): void }) {
  const s = draft.settings ?? { timeoutMs: 30000, followRedirects: true, verifyTls: true };
  const patch = (p: Partial<typeof s>) => set((x) => ({ ...x, settings: { ...s, ...p } }));
  return (
    <div className="col-lg" style={{ maxWidth: 480 }}>
      <Field label="Timeout (ms)" hint="0 disables the timeout.">
        <input className="input" type="number" min={0} value={s.timeoutMs} onChange={(e) => patch({ timeoutMs: Math.max(0, Number(e.target.value) || 0) })} />
      </Field>
      <Switch checked={s.followRedirects} onChange={(followRedirects) => patch({ followRedirects })} label="Follow redirects (up to 10)" />
      <Switch checked={s.verifyTls} onChange={(verifyTls) => patch({ verifyTls })} label="Verify TLS certificates" />
      <p className="subtle xs">Certificates are checked against your operating system’s trust store, so corporate CAs work out of the box.</p>
      <Field label="Proxy" hint="Empty = system proxy settings (HTTP_PROXY / OS). “none” bypasses proxies. Supports http://, https:// and socks5://.">
        <input className="input mono" value={s.proxy ?? ''} placeholder="system" onChange={(e) => patch({ proxy: e.target.value.trim() || undefined })} />
      </Field>
      <Field label="Client certificate (mutual TLS)" hint="PEM files; paths inside the project are stored relative to it. {{variables}} allowed.">
        <div className="col-sm">
          <input
            className="input mono"
            value={s.clientCert?.cert ?? ''}
            placeholder="certs/client.pem"
            onChange={(e) => patch({ clientCert: e.target.value.trim() ? { cert: e.target.value, key: s.clientCert?.key } : undefined })}
          />
          <input
            className="input mono"
            value={s.clientCert?.key ?? ''}
            placeholder="certs/client.key (optional when the PEM contains the key)"
            disabled={!s.clientCert}
            onChange={(e) => s.clientCert && patch({ clientCert: { cert: s.clientCert.cert, key: e.target.value.trim() || undefined } })}
          />
        </div>
      </Field>
    </div>
  );
}

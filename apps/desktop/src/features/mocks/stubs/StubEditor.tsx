import { useEffect, useMemo, useState } from 'react';
import { Save, Undo2, Copy, Trash2, MoreHorizontal, Ban, CheckCircle2, Shuffle, Braces, Code2, Download, AlertTriangle, ArrowRightLeft, FilePlus2, HardDrive, Send } from 'lucide-react';
import type { StubMapping } from '@/features/mocks/lib/types';
import { useCapabilities, useMappings, useWm } from '@/features/mocks/lib/queries';
import { useStubTabs, type StubTab } from '@/features/mocks/store/tabs';
import { usePrefs } from '@/features/mocks/store/prefs';
import { useTargets } from '@/features/mocks/store/target';
import { openInRequests } from './openInRequests';
import { cleanForSave, displayName, isDisabled, isProxy, hasParkedProxy, prettyJson, setDisabled, stubId, toggleProxy, tryParseJson, stripIdentity } from '@/features/mocks/lib/stub-utils';
import { Segmented, CopyButton, Callout, copyText, Empty, Spinner, MOD } from '@/features/mocks/components/ui';
import { Menu } from '@/components/ui/Menu';
import { CodeEditor, CodeDiff } from '@/components/editor/CodeEditor';
import { stubModelPath } from '@/features/mocks/lib/monacoSchema';
import { curlCreateStub, curlForStub, javaDsl } from '@/features/mocks/lib/codegen';
import { downloadJson, safeFileName } from '@/features/mocks/lib/download';
import { useStubActions } from './actions';
import { StubForm } from './StubForm';
import { StubActivity } from './StubActivity';

type View = 'form' | 'json' | 'activity' | 'code' | 'diff';

export function StubEditor({ tab }: { tab: StubTab }) {
  const { sid, server, client } = useWm();
  const caps = useCapabilities();
  const mappings = useMappings();
  const ws = useStubTabs();
  const editorMode = usePrefs((s) => s.editorMode);
  const setPref = usePrefs((s) => s.set);
  const [view, setView] = useState<View>(editorMode);
  const [saving, setSaving] = useState(false);
  const actions = useStubActions();
  const targets = useTargets();

  const serverStub = useMemo(() => (tab.stubId ? mappings.data?.find((m) => stubId(m) === tab.stubId) : undefined), [mappings.data, tab.stubId]);
  const serverJson = serverStub ? prettyJson(cleanForSave(serverStub)) : undefined;

  // Track the server version we started editing from, to detect concurrent changes.
  useEffect(() => {
    if (serverJson && !tab.draft && tab.baseline !== serverJson) ws.update(sid, tab.key, { baseline: serverJson, title: displayName(serverStub!), method: serverStub!.request.method });
  }, [serverJson, tab.draft, tab.baseline, sid, tab.key, ws, serverStub]);

  const stub: StubMapping | undefined = tab.draft ?? serverStub;
  const dirty = !!tab.draft || !!tab.draftText;
  const conflict = !!tab.draft && !!serverJson && !!tab.baseline && serverJson !== tab.baseline;
  const [jsonText, setJsonText] = useState<string>(() => tab.draftText ?? (stub ? prettyJson(stub) : ''));
  const [jsonError, setJsonError] = useState<string | undefined>();

  useEffect(() => {
    // Keep the JSON view in sync when the form (or server) changes the stub.
    if (view !== 'json' && stub && !tab.draftText) setJsonText(prettyJson(stub));
  }, [stub, view, tab.draftText]);

  if (!stub) {
    if (mappings.isPending)
      return (
        <div className="empty">
          <Spinner />
        </div>
      );
    return (
      <Empty icon={<AlertTriangle />} title="This stub no longer exists on the server">
        It may have been deleted or the server was reset.
        {tab.baseline && (
          <div className="mt-4">
            <button
              className="btn btn-primary"
              onClick={() => {
                const s = JSON.parse(tab.baseline!);
                ws.update(sid, tab.key, { isNew: true, stubId: undefined, draft: s });
              }}
            >
              <FilePlus2 /> Restore as new stub
            </button>
          </div>
        )}
      </Empty>
    );
  }

  const setDraft = (next: StubMapping) => {
    const same = serverJson && prettyJson(cleanForSave(next)) === serverJson;
    ws.update(sid, tab.key, { draft: same && !tab.isNew ? undefined : next, draftText: undefined, title: displayName(next), method: next.request?.method });
  };

  const onJsonChange = (text: string) => {
    setJsonText(text);
    const r = tryParseJson<StubMapping>(text);
    if (r.ok && r.value && typeof r.value === 'object') {
      setJsonError(undefined);
      if (!r.value.request) r.value.request = {};
      if (!r.value.response) r.value.response = {};
      setDraft(r.value);
    } else if (!r.ok) {
      setJsonError(r.error);
      ws.update(sid, tab.key, { draftText: text });
    }
  };

  const save = async () => {
    if (tab.draftText) return; // invalid JSON
    if (!dirty && !tab.isNew) return;
    setSaving(true);
    try {
      if (tab.isNew) {
        const created = await actions.create(stub);
        if (created) ws.update(sid, tab.key, { isNew: false, stubId: stubId(created), draft: undefined, draftText: undefined, baseline: prettyJson(cleanForSave(created)), title: displayName(created), method: created.request.method, pinned: true });
      } else {
        const saved = await actions.update(stub);
        if (saved) ws.update(sid, tab.key, { draft: undefined, draftText: undefined, baseline: prettyJson(cleanForSave(saved)) });
      }
    } finally {
      setSaving(false);
    }
  };

  const revert = () => {
    ws.update(sid, tab.key, { draft: undefined, draftText: undefined, baseline: serverJson });
    setJsonError(undefined);
    if (serverStub) setJsonText(prettyJson(serverStub));
  };

  const quick = async (fn: (s: StubMapping) => StubMapping, label: string) => {
    const next = fn(stub);
    if (tab.isNew || dirty) return setDraft(next);
    await actions.update(next, { label });
  };

  const otherServers = targets.filter((t) => t.id !== server?.id);
  const disabled = isDisabled(stub);

  return (
    <div className="page" onKeyDown={(e) => (e.ctrlKey || e.metaKey) && e.key === 's' && (e.preventDefault(), save())}>
      <div className="toolbar" style={{ gap: 6 }}>
        <Segmented<View>
          value={view}
          onChange={(v) => {
            setView(v);
            if (v === 'form' || v === 'json') setPref('editorMode', v);
          }}
          options={[
            { value: 'form', label: 'Form' },
            { value: 'json', label: 'JSON' },
            ...(tab.isNew ? [] : [{ value: 'activity' as View, label: 'Activity' }]),
            { value: 'code', label: 'Code' },
            ...(dirty && !tab.isNew ? [{ value: 'diff' as View, label: 'Diff' }] : []),
          ]}
        />
        {!tab.isNew && (
          <span className="subtle xs mono row-sm" title="Stub id">
            {tab.stubId?.slice(0, 8)}
            <CopyButton text={tab.stubId ?? ''} title="Copy id" />
          </span>
        )}
        {disabled && <span className="badge warning">DISABLED</span>}
        {server.kind === 'builtin' && stub.persistent === false && (
          <span className="badge warning" title="Not saved to mocks/mappings: lost when the mock server restarts">
            IN MEMORY
          </span>
        )}
        {stub.persistent && (
          <span className="badge outline" title="Persistent: saved to the mappings folder">
            <HardDrive /> PERSISTENT
          </span>
        )}
        <div className="spacer" />
        {dirty && <span className="subtle xs">Unsaved changes</span>}
        <button className="btn btn-sm btn-ghost" onClick={revert} disabled={!dirty || tab.isNew} title="Discard edits">
          <Undo2 /> Revert
        </button>
        <button className="btn btn-sm btn-primary" onClick={save} disabled={saving || !!tab.draftText || (!dirty && !tab.isNew) || server?.readOnly} title={`Save (${MOD}+S)`}>
          {saving ? <Spinner /> : <Save />} {tab.isNew ? 'Create' : 'Save'}
        </button>
        <Menu
          align="right"
          items={() => [
            { label: 'Duplicate', icon: <Copy />, disabled: tab.isNew, onSelect: () => actions.duplicate(stub) },
            { label: 'Duplicate into new tab', icon: <FilePlus2 />, onSelect: () => ws.open(sid, { isNew: true, title: `${displayName(stub)} (copy)`, draft: { ...stripIdentity(stub), name: `${displayName(stub)} (copy)` } }) },
            disabled
              ? { label: 'Enable stub', icon: <CheckCircle2 />, onSelect: () => quick((s) => setDisabled(s, false), 'Enable stub') }
              : { label: 'Disable stub', icon: <Ban />, onSelect: () => quick((s) => setDisabled(s, true), 'Disable stub') },
            ...(isProxy(stub) || hasParkedProxy(stub) ? [{ label: isProxy(stub) ? 'Switch to mock response' : 'Switch back to proxy', icon: <Shuffle />, onSelect: () => quick(toggleProxy, 'Toggle proxy') }] : []),
            { separator: true },
            { label: 'Copy JSON', icon: <Braces />, onSelect: () => copyText(prettyJson(stub), 'Stub JSON copied') },
            { label: 'Open in Requests', icon: <Send />, disabled: !client, onSelect: () => openInRequests(stub, client?.base ?? '') },
            { label: 'Copy request as cURL', icon: <Code2 />, onSelect: () => copyText(curlForStub(client?.base ?? server?.url ?? '', stub), 'cURL copied') },
            { label: 'Copy create-stub cURL (admin API)', icon: <Code2 />, onSelect: () => copyText(curlCreateStub(client?.admin ?? '', stripIdentity(stub)), 'cURL copied') },
            { label: 'Copy Java DSL', icon: <Code2 />, onSelect: () => copyText(javaDsl(stub), 'Java copied') },
            { label: 'Download JSON file', icon: <Download />, onSelect: () => downloadJson(`${safeFileName(stub.name || stub.id || 'stub')}.json`, stub) },
            ...(otherServers.length && !tab.isNew
              ? [{ heading: 'Copy to server' }, ...otherServers.slice(0, 8).map((t) => ({ label: t.name, icon: <ArrowRightLeft />, onSelect: () => actions.copyTo(t, [stub]) }))]
              : []),
            { separator: true },
            {
              label: tab.isNew ? 'Discard new stub' : 'Delete stub',
              icon: <Trash2 />,
              danger: true,
              onSelect: () => (tab.isNew ? ws.close(sid, tab.key) : actions.remove([stub])),
            },
          ]}
          trigger={(p) => (
            <button className="icon-btn" {...p} aria-label="More actions">
              <MoreHorizontal />
            </button>
          )}
        />
      </div>
      {conflict && (
        <div className="p-4" style={{ paddingBottom: 0 }}>
          <Callout kind="warning">
            <div className="row wrap">
              <span className="grow">
                <b>Changed on the server</b> since you started editing (another user, a test, or a recording). Saving will overwrite those changes.
              </span>
              <button className="btn btn-sm" onClick={() => setView('diff')}>
                View diff
              </button>
              <button className="btn btn-sm" onClick={revert}>
                Discard mine & reload
              </button>
              <button className="btn btn-sm" onClick={() => ws.update(sid, tab.key, { baseline: serverJson })}>
                Keep mine
              </button>
            </div>
          </Callout>
        </div>
      )}
      <div className="grow min-h-0" style={{ display: 'flex', flexDirection: 'column' }}>
        {view === 'form' && <StubForm stub={stub} onChange={setDraft} onSave={save} />}
        {view === 'json' && (
          <div className="col p-4 grow min-h-0">
            {jsonError && (
              <div className="text-danger xs mono" role="alert">
                {jsonError}
              </div>
            )}
            <div className="editor-shell fill">
              <CodeEditor value={jsonText} onChange={onJsonChange} language="json" path={stubModelPath(caps.schema, tab.key)} onSave={save} />
            </div>
            <div className="subtle xs">Validated against the WireMock {caps.schema}.x stub schema — press Ctrl+Space for completions.</div>
          </div>
        )}
        {view === 'activity' && tab.stubId && <StubActivity stub={stub} onApplyFix={(s) => { setDraft(s); setView('form'); }} />}
        {view === 'code' && <CodeView stub={stub} baseUrl={client?.base ?? server?.url ?? ''} admin={client?.admin ?? ''} />}
        {view === 'diff' && (
          <div className="col p-4 grow min-h-0">
            <div className="row subtle xs">
              <span className="grow">Left: server · Right: your edits</span>
            </div>
            <div className="editor-shell fill">
              <CodeDiff original={serverJson ?? ''} modified={prettyJson(cleanForSave(stub))} />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function CodeView({ stub, baseUrl, admin }: { stub: StubMapping; baseUrl: string; admin: string }) {
  const [lang, setLang] = useState<'java' | 'curl' | 'admin' | 'json'>('java');
  const code = lang === 'java' ? javaDsl(stub) : lang === 'curl' ? curlForStub(baseUrl, stub) : lang === 'admin' ? curlCreateStub(admin, stripIdentity(stub)) : prettyJson(stub);
  return (
    <div className="col p-4 grow min-h-0">
      <div className="row">
        <Segmented<typeof lang>
          value={lang}
          onChange={setLang}
          options={[
            { value: 'java', label: 'Java DSL' },
            { value: 'curl', label: 'cURL (call stub)' },
            { value: 'admin', label: 'cURL (create stub)' },
            { value: 'json', label: 'JSON file' },
          ]}
        />
        <div className="spacer" />
        <CopyButton text={code} label="Copy" />
      </div>
      <div className="editor-shell fill">
        <CodeEditor value={code} readOnly language={lang === 'java' ? 'java' : lang === 'json' ? 'json' : 'shell'} />
      </div>
    </div>
  );
}

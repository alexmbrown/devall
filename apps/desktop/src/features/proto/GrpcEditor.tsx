import { useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { Ban, FileCode2, FolderOpen, Info, Play, RefreshCw, Square, Wand2, X } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { Callout, CopyButton, Empty, Field, Spinner, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { VarInput } from '@/components/VarInput';
import { uid } from '@/lib/id';
import { errorMessage } from '@/lib/ipc';
import { protoApi, type GrpcDescription, type GrpcFile, type GrpcStatus, type MethodInfo } from '@/lib/api/proto';
import { usePrefs } from '@/store/prefs';
import { useVariables } from '@/store/variables';
import { KeyValueEditor } from '@/features/http/KeyValueEditor';
import { Composer } from './Composer';
import { MessageLog } from './MessageLog';
import { EditorFrame, KindBadge, LoadState, ProtoHead, StatusPill, confirmProtected, num, useEditorKeys, useProtoFile, useSticky } from './shared';
import { beginConnect, connectFailed, eventSink, liveKey, useLive, useLiveState } from './store';

type Sub = 'message' | 'metadata' | 'schema' | 'settings' | 'docs';

function methodKind(m?: MethodInfo): string {
  if (!m) return '';
  if (m.clientStreaming && m.serverStreaming) return 'bidirectional streaming';
  if (m.clientStreaming) return 'client streaming';
  if (m.serverStreaming) return 'server streaming';
  return 'unary';
}

/** Project-relative path for files inside the project (so they work for teammates). */
function relToRoot(root: string, p: string): string {
  const norm = (s: string) => s.replace(/\\/g, '/');
  const r = norm(root).replace(/\/$/, '');
  return norm(p).startsWith(`${r}/`) ? norm(p).slice(r.length + 1) : p;
}

const isBlankMessage = (s: string) => !s.trim() || s.replace(/\s/g, '') === '{}';

/**
 * Editor for `*.grpc.yaml`: service/method pickers from server reflection or `.proto` files,
 * request templates, metadata, deadlines and a streaming log. Client/bidi-streaming calls can be
 * kept open to send more messages.
 */
export function GrpcEditor({ root, path }: { root: string; path: string }) {
  const { draft, update, dirty, save, error } = useProtoFile<GrpcFile>(root, path);
  const key = liveKey(root, path);
  const live = useLiveState(key, 'grpc');
  const env = usePrefs((s) => s.activeEnv[root]);
  const vars = useVariables(path);
  const [sub, setSub] = useState<Sub>('message');
  const [desc, setDesc] = useSticky<GrpcDescription | null>(`grpc-desc:${key}`, () => null);
  const [describing, setDescribing] = useState(false);
  const [describeError, setDescribeError] = useState<string>();
  const [keepOpen, setKeepOpen] = useSticky(`grpc-keep:${key}`, () => true);

  const method = useMemo(() => desc?.services.find((s) => s.name === draft?.service)?.methods.find((m) => m.name === draft?.method), [desc, draft?.service, draft?.method]);
  const running = live.status === 'connecting' || live.status === 'open';
  // Without loaded descriptors we can't tell the method kind; allow sending (the server decides).
  const streamingOpen = live.status === 'open' && !!live.sessionId && keepOpen && (method ? method.clientStreaming : true);

  const describe = async (quiet = false) => {
    if (!draft) return;
    setDescribing(true);
    setDescribeError(undefined);
    try {
      const d = await protoApi.grpcDescribe(root, path, draft, env);
      setDesc(d);
      for (const w of d.warnings) toast.info('gRPC reflection', w);
      if (!d.services.length && !quiet) toast.info('No services found');
    } catch (e) {
      setDescribeError(errorMessage(e));
    } finally {
      setDescribing(false);
    }
  };

  // .proto files compile locally (no network), so load them as soon as the file opens.
  const autoDescribed = useRef(false);
  useEffect(() => {
    if (!draft || autoDescribed.current || desc) return;
    autoDescribed.current = true;
    if (draft.useReflection === false && (draft.protoFiles ?? []).length) describe(true);
  }, [draft]);

  const invoke = async () => {
    if (!draft || running) return;
    if (!(await confirmProtected(root))) return;
    const callId = uid();
    beginConnect(key, 'grpc');
    useLive.getState().patch(key, 'grpc', { sessionId: callId });
    try {
      const status = await protoApi.grpcCall(callId, root, path, draft, env, keepOpen, eventSink(key));
      useLive.getState().patch(key, 'grpc', { grpcStatus: status, status: 'closed', sessionId: undefined });
    } catch (e) {
      connectFailed(key, 'grpc', errorMessage(e));
    }
  };
  const sendMore = () => {
    if (!draft || !live.sessionId) return;
    protoApi.grpcSend(live.sessionId, draft.message).catch((e) => toast.error('Send failed', errorMessage(e)));
  };
  const endStream = () => live.sessionId && protoApi.grpcEnd(live.sessionId).catch((e) => toast.error('Could not end the stream', errorMessage(e)));
  const cancel = () => live.sessionId && protoApi.grpcCancel(live.sessionId);
  useEditorKeys(save, () => (streamingOpen ? sendMore() : invoke()));

  if (!draft) return <LoadState error={error} />;
  const set = (fn: (d: GrpcFile) => GrpcFile) => update(fn);
  const useReflection = draft.useReflection !== false;
  const presets = draft.messages ?? [];
  const setPresets = (messages: typeof presets) => set((d) => ({ ...d, messages: messages.length ? messages : undefined }));

  const pickMethod = (service: string, name: string) => {
    const m = desc?.services.find((s) => s.name === service)?.methods.find((x) => x.name === name);
    set((d) => {
      const prev = desc?.services.find((s) => s.name === d.service)?.methods.find((x) => x.name === d.method);
      const replace = m && (isBlankMessage(d.message) || (prev && d.message === prev.template));
      return { ...d, service, method: name, message: replace ? m.template : d.message };
    });
  };

  const services = desc?.services ?? [];
  const serviceOptions = [...new Set([...services.map((s) => s.name), ...(draft.service ? [draft.service] : [])])];
  const methodOptions = [...new Set([...(services.find((s) => s.name === draft.service)?.methods.map((m) => m.name) ?? []), ...(draft.method ? [draft.method] : [])])];

  const addProtoFiles = async () => {
    const picked = await openDialog({ multiple: true, directory: false, defaultPath: root, filters: [{ name: 'Protocol Buffers', extensions: ['proto'] }] });
    const list = (Array.isArray(picked) ? picked : picked ? [picked] : []).map((p) => relToRoot(root, p));
    if (list.length) set((d) => ({ ...d, protoFiles: [...new Set([...(d.protoFiles ?? []), ...list])], useReflection: false }));
  };
  const addImportPath = async () => {
    const picked = await openDialog({ directory: true, multiple: false, defaultPath: root });
    if (typeof picked === 'string') set((d) => ({ ...d, importPaths: [...new Set([...(d.importPaths ?? []), relToRoot(root, picked)])] }));
  };

  return (
    <EditorFrame
      head={
        <ProtoHead
          root={root}
          path={path}
          name={draft.name}
          dirty={dirty}
          onSave={save}
          status={<StatusPill status={live.status} text={live.status === 'open' ? 'Call in progress' : live.status === 'connecting' ? 'Connecting…' : live.grpcStatus ? `Last call: ${live.grpcStatus.codeName}` : 'Idle'} />}
        />
      }
      bar={
        <>
          <KindBadge kind="grpc" />
          <VarInput value={draft.url} vars={vars} placeholder="grpc://localhost:50051   ·   grpcs:// for TLS" className="url-input" onChange={(url) => set((d) => ({ ...d, url }))} onEnter={invoke} />
          <div className="grpc-picker">
            <select className="select" value={draft.service} onChange={(e) => pickMethod(e.target.value, services.find((s) => s.name === e.target.value)?.methods[0]?.name ?? '')} aria-label="Service" title={draft.service || 'Service'}>
              <option value="">{services.length ? 'Service…' : 'Load services →'}</option>
              {serviceOptions.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </select>
            <select className="select" value={draft.method} onChange={(e) => pickMethod(draft.service, e.target.value)} aria-label="Method" title={draft.method || 'Method'}>
              <option value="">Method…</option>
              {methodOptions.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </div>
          <button className="icon-btn" onClick={() => describe()} title={useReflection ? 'Load services via server reflection' : 'Reload the .proto files'} aria-label="Load services" disabled={describing}>
            {describing ? <Spinner /> : <RefreshCw />}
          </button>
          {live.status === 'open' ? (
            <button className="btn btn-send" onClick={cancel} title="Cancel the call">
              <Ban /> Cancel
            </button>
          ) : (
            <button className="btn btn-primary btn-send" onClick={invoke} disabled={live.status === 'connecting'} title="Invoke (Ctrl+Enter)">
              {live.status === 'connecting' ? <Spinner /> : <Play />} Invoke
            </button>
          )}
        </>
      }
      tabs={[
        { key: 'message', label: 'Message' },
        { key: 'metadata', label: 'Metadata', count: (draft.metadata ?? []).filter((m) => m.enabled !== false).length },
        { key: 'schema', label: useReflection ? 'Schema · reflection' : 'Schema · protos', count: useReflection ? 0 : (draft.protoFiles ?? []).length },
        { key: 'settings', label: 'Settings', dot: !!draft.insecure || !!draft.deadlineMs },
        { key: 'docs', label: 'Docs', dot: !!draft.docs },
      ]}
      sub={sub}
      onSub={setSub}
      log={
        <MessageLog
          entries={live.log}
          onClear={() => useLive.getState().clear(key)}
          header={
            <>
              {live.error && (
                <Callout kind="danger" className="m-3">
                  {live.error}
                </Callout>
              )}
              {live.grpcStatus && live.status !== 'open' && <StatusBar status={live.grpcStatus} />}
            </>
          }
          empty={
            <Empty icon={<FileCode2 />} title="No calls yet">
              Pick a method and press <span className="kbd">Ctrl</span> + <span className="kbd">Enter</span>. Streamed messages, headers and trailers show up here.
            </Empty>
          }
        />
      }
    >
      {describeError && (
        <Callout kind="danger" className="mb-4">
          <div className="strong">Could not load services</div>
          <div className="mono xs break">{describeError}</div>
        </Callout>
      )}
      {sub === 'message' && (
        <Composer
          value={draft.message}
          onChange={(message) => set((d) => ({ ...d, message }))}
          onSend={streamingOpen ? sendMore : invoke}
          canSend={streamingOpen || !running}
          sendLabel={streamingOpen ? 'Send message' : 'Invoke'}
          sendTitle={streamingOpen ? 'Send this message on the open stream (Ctrl+Enter)' : 'Invoke (Ctrl+Enter)'}
          presets={presets}
          onLoadPreset={(i) => set((d) => ({ ...d, message: presets[i].body }))}
          onSavePreset={(name) => setPresets([...presets, { name, body: draft.message, format: 'json' }])}
          onUpdatePreset={(i) => setPresets(presets.map((x, j) => (j === i ? { ...x, body: draft.message } : x)))}
          onDeletePreset={(i) => setPresets(presets.filter((_, j) => j !== i))}
          modelPath={`proto/${path}/grpc-message.json`}
          fields={
            <div className="row-sm w-full">
              {method ? (
                <span className="subtle xs mono truncate" title={method.path}>
                  {methodKind(method)} · {method.inputType} → {method.outputType}
                </span>
              ) : (
                <span className="subtle xs">{desc ? 'Pick a method to get a request template.' : 'Load services (↻) for method pickers and request templates.'}</span>
              )}
              <span className="spacer" />
              {method?.clientStreaming && (
                <Switch checked={keepOpen} onChange={setKeepOpen} label="Keep stream open" title="Send more messages after invoking, then End stream. Off: a JSON array is sent as the whole stream." />
              )}
              {method && (
                <button className="btn btn-sm btn-ghost" onClick={() => set((d) => ({ ...d, message: method.template }))} title="Replace the message with an example of every field">
                  <Wand2 /> Template
                </button>
              )}
            </div>
          }
          actions={
            streamingOpen ? (
              <button className="btn btn-sm" onClick={endStream} title="Half-close the request stream">
                <Square /> End stream
              </button>
            ) : undefined
          }
          hint={method?.clientStreaming && !keepOpen ? 'a JSON array sends one message per item' : undefined}
        />
      )}
      {sub === 'metadata' && (
        <div className="col">
          <KeyValueEditor rows={draft.metadata ?? []} onChange={(metadata) => set((d) => ({ ...d, metadata: metadata.length ? metadata : undefined }))} vars={vars} namePlaceholder="Metadata key" />
          <p className="subtle xs">Keys are lower-cased. Keys ending in -bin take base64 values. Reflection requests carry the same metadata (useful for auth).</p>
        </div>
      )}
      {sub === 'schema' && (
        <div className="col-lg" style={{ maxWidth: 640 }}>
          <Switch checked={useReflection} onChange={(v) => set((d) => ({ ...d, useReflection: v ? undefined : false }))} label="Use server reflection" />
          <p className="subtle xs">
            Reflection asks the server for its schema (grpc.reflection v1, falling back to v1alpha). Turn it off to use .proto files instead — they are compiled on your machine, no protoc needed.
          </p>
          <Field label=".proto files" hint="Paths inside the project are stored relative to it. Imports resolve against each file's folder, the import paths below and the project root; Google's well-known types are built in.">
            <div className="col-sm">
              <div className="chip-list">
                {(draft.protoFiles ?? []).map((f) => (
                  <span key={f} className="chip" title={f}>
                    {f}
                    <button className="icon-btn sm" onClick={() => set((d) => ({ ...d, protoFiles: (d.protoFiles ?? []).filter((x) => x !== f) }))} aria-label={`Remove ${f}`}>
                      <X />
                    </button>
                  </span>
                ))}
                {!(draft.protoFiles ?? []).length && <span className="subtle xs">None</span>}
              </div>
              <div className="row-sm">
                <button className="btn btn-sm" onClick={addProtoFiles}>
                  <FileCode2 /> Add .proto files…
                </button>
              </div>
            </div>
          </Field>
          <Field label="Import paths">
            <div className="col-sm">
              <div className="chip-list">
                {(draft.importPaths ?? []).map((f) => (
                  <span key={f} className="chip" title={f}>
                    {f}
                    <button className="icon-btn sm" onClick={() => set((d) => ({ ...d, importPaths: (d.importPaths ?? []).filter((x) => x !== f) }))} aria-label={`Remove ${f}`}>
                      <X />
                    </button>
                  </span>
                ))}
                {!(draft.importPaths ?? []).length && <span className="subtle xs">None</span>}
              </div>
              <div className="row-sm">
                <button className="btn btn-sm" onClick={addImportPath}>
                  <FolderOpen /> Add folder…
                </button>
              </div>
            </div>
          </Field>
          <div className="row-sm">
            <button className="btn btn-sm" onClick={() => describe()} disabled={describing}>
              {describing ? <Spinner /> : <RefreshCw />} {useReflection ? 'Load services from the server' : 'Compile and list services'}
            </button>
            {desc && (
              <span className="subtle xs">
                {desc.services.length} service{desc.services.length === 1 ? '' : 's'} · {desc.services.reduce((n, s) => n + s.methods.length, 0)} methods ({desc.source === 'reflection' ? 'reflection' : '.proto files'})
              </span>
            )}
          </div>
          {desc && desc.services.length > 0 && (
            <div className="list" style={{ border: '1px solid var(--border)', borderRadius: 'var(--radius)' }}>
              {desc.services.flatMap((s) =>
                s.methods.map((m) => (
                  <button key={m.path} className={clsx('list-item', draft.service === s.name && draft.method === m.name && 'on')} onClick={() => pickMethod(s.name, m.name)}>
                    <span className="mono xs grow truncate">{m.path}</span>
                    <span className="meta">{methodKind(m)}</span>
                  </button>
                )),
              )}
            </div>
          )}
        </div>
      )}
      {sub === 'settings' && (
        <div className="col-lg" style={{ maxWidth: 480 }}>
          <Switch checked={!draft.insecure} onChange={(v) => set((d) => ({ ...d, insecure: v ? undefined : true }))} label="Verify TLS certificates (grpcs:// / https://)" />
          <p className="subtle xs">Certificates are checked against your operating system’s trust store. Turn this off for local self-signed servers.</p>
          <Field label="Deadline (ms)" hint="Sent as grpc-timeout and enforced locally; 0 = none. Calls past it end with DEADLINE_EXCEEDED.">
            <input className="input" type="number" min={0} step={500} value={draft.deadlineMs ?? 0} onChange={(e) => set((d) => ({ ...d, deadlineMs: num(e.target.value) || undefined }))} />
          </Field>
        </div>
      )}
      {sub === 'docs' && (
        <Field label="Notes (Markdown, saved with the file)">
          <textarea className="textarea" style={{ minHeight: 240 }} value={draft.docs ?? ''} onChange={(e) => set((d) => ({ ...d, docs: e.target.value || undefined }))} />
        </Field>
      )}
    </EditorFrame>
  );
}

function StatusBar({ status }: { status: GrpcStatus }) {
  const [open, setOpen] = useState(false);
  const ok = status.code === 0;
  return (
    <>
    <div className="response-status">
      <span className={clsx('badge', ok ? 'success' : 'danger')} title={`gRPC status ${status.code}`}>
        {status.code} {status.codeName}
      </span>
      {status.message && (
        <span className="mono xs truncate" title={status.message}>
          {status.message}
        </span>
      )}
      <span className="subtle xs nowrap">{status.durationMs < 1000 ? `${Math.round(status.durationMs)} ms` : `${(status.durationMs / 1000).toFixed(2)} s`}</span>
      <span className="subtle xs nowrap" title="messages sent / received">
        ↑{status.messagesSent} ↓{status.messagesReceived}
      </span>
      <span className="spacer" />
      {status.details !== undefined && (
        <button className="btn btn-sm btn-ghost" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
          <Info size={12} /> Error details
        </button>
      )}
    </div>
    {open && status.details !== undefined && (
      <div className="plog-detail" style={{ paddingLeft: 'var(--space-5)', paddingTop: 'var(--space-3)' }}>
        <div className="row-sm">
          <span className="subtle xs">google.rpc.Status details</span>
          <span className="spacer" />
          <CopyButton text={JSON.stringify(status.details, null, 2)} />
        </div>
        <pre className="code">{JSON.stringify(status.details, null, 2)}</pre>
      </div>
    )}
    </>
  );
}

import { useEffect, useRef, useState } from 'react';
import { Plus, Radio, X } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Callout, Checkbox, Empty, Field, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { VarInput } from '@/components/VarInput';
import { errorMessage } from '@/lib/ipc';
import { protoApi, type SioEmit, type SioFile, type SioSettings } from '@/lib/api/proto';
import { usePrefs } from '@/store/prefs';
import { useVariables } from '@/store/variables';
import { COMMON_HEADERS, KeyValueEditor } from '@/features/http/KeyValueEditor';
import { Composer } from './Composer';
import { MessageLog } from './MessageLog';
import { ConnectButton, EditorFrame, KindBadge, LoadState, ProtoHead, StatusPill, confirmProtected, num, useEditorKeys, useProtoFile, useSticky } from './shared';
import { beginConnect, connectFailed, connected, eventSink, liveKey, useLive, useLiveState } from './store';

type Sub = 'emit' | 'listen' | 'auth' | 'headers' | 'settings' | 'docs';

const DEFAULT_SETTINGS: SioSettings = { verifyTls: true, connectTimeoutMs: 10000 };

interface EmitDraft {
  event: string;
  args: string;
  ack: boolean;
}

const toPreset = (name: string, e: EmitDraft): SioEmit => ({ name, event: e.event, args: e.args, ...(e.ack ? { ack: true } : {}) });

/** Editor for `*.sio.yaml`: connect to a namespace with auth, listen to events, emit with acks. */
export function SioEditor({ root, path }: { root: string; path: string }) {
  const { draft, update, dirty, save, error } = useProtoFile<SioFile>(root, path);
  const key = liveKey(root, path);
  const live = useLiveState(key, 'socketio');
  const env = usePrefs((s) => s.activeEnv[root]);
  const vars = useVariables(path);
  const [sub, setSub] = useState<Sub>('emit');
  const [emit, setEmit] = useSticky<EmitDraft>(`sio:${key}`, () => ({ event: '', args: '', ack: false }));
  const [newEvent, setNewEvent] = useState('');
  const seeded = useRef(false);
  useEffect(() => {
    if (!draft || seeded.current) return;
    seeded.current = true;
    const first = draft.emits?.[0];
    if (!emit.event && !emit.args && first) setEmit({ event: first.event, args: first.args, ack: !!first.ack });
  }, [draft, emit.event, emit.args, setEmit]);

  const open = live.status === 'open' && !!live.sessionId;
  const connect = async () => {
    if (!draft || live.status === 'connecting' || live.status === 'open') return;
    if (!(await confirmProtected(root))) return;
    beginConnect(key, 'socketio');
    try {
      connected(key, 'socketio', await protoApi.sioConnect(root, path, draft, env, eventSink(key)));
    } catch (e) {
      connectFailed(key, 'socketio', errorMessage(e));
    }
  };
  const disconnect = () => {
    if (live.sessionId) protoApi.sioClose(live.sessionId).catch((e) => toast.error('Close failed', errorMessage(e)));
    else useLive.getState().patch(key, 'socketio', { status: 'closed' });
  };
  const send = () => {
    if (!open) return toast.info('Not connected', 'Connect first (Ctrl+Enter).');
    if (!emit.event.trim()) return toast.info('Event name required', 'Enter the event to emit.');
    protoApi.sioEmit(live.sessionId!, emit.event, emit.args, emit.ack).catch((e) => toast.error('Emit failed', errorMessage(e)));
  };
  useEditorKeys(save, () => (open ? send() : connect()));

  if (!draft) return <LoadState error={error} />;
  const set = (fn: (d: SioFile) => SioFile) => update(fn);
  const settings = { ...DEFAULT_SETTINGS, ...draft.settings };
  const listen = draft.listen ?? [];
  const setListen = (l: string[]) => set((d) => ({ ...d, listen: l.length ? l : undefined }));
  const presets = draft.emits ?? [];
  const setPresets = (p: SioEmit[]) => set((d) => ({ ...d, emits: p.length ? p : undefined }));
  const addEvent = () => {
    const e = newEvent.trim();
    if (e && !listen.includes(e)) setListen([...listen, e]);
    setNewEvent('');
  };

  return (
    <EditorFrame
      head={<ProtoHead root={root} path={path} name={draft.name} dirty={dirty} onSave={save} status={<StatusPill status={live.status} />} />}
      bar={
        <>
          <KindBadge kind="socketio" />
          <VarInput value={draft.url} vars={vars} placeholder="http://localhost:3000   ·   add /namespace to the path" className="url-input" onChange={(url) => set((d) => ({ ...d, url }))} onEnter={connect} />
          <ConnectButton status={live.status} onConnect={connect} onDisconnect={disconnect} />
        </>
      }
      tabs={[
        { key: 'emit', label: 'Emit' },
        { key: 'listen', label: 'Listen', count: listen.length },
        { key: 'auth', label: 'Auth', dot: !!draft.auth || !!draft.query?.length },
        { key: 'headers', label: 'Headers', count: (draft.headers ?? []).filter((h) => h.enabled !== false).length },
        { key: 'settings', label: 'Settings' },
        { key: 'docs', label: 'Docs', dot: !!draft.docs },
      ]}
      sub={sub}
      onSub={setSub}
      log={
        <MessageLog
          entries={live.log}
          onClear={() => useLive.getState().clear(key)}
          header={live.error ? <Callout kind="danger" className="m-3">{live.error}</Callout> : undefined}
          empty={
            <Empty icon={<Radio />} title={open ? 'Connected — waiting for events' : 'Not connected'}>
              Events are labelled with their name; arguments show as a JSON array. Acknowledgements arrive as “ack #id”.
            </Empty>
          }
        />
      }
    >
      {sub === 'emit' && (
        <Composer
          value={emit.args}
          onChange={(args) => setEmit((x) => ({ ...x, args }))}
          onSend={send}
          canSend={open}
          sendLabel="Emit"
          sendTitle="Emit (Ctrl+Enter)"
          presets={presets}
          onLoadPreset={(i) => setEmit({ event: presets[i].event, args: presets[i].args, ack: !!presets[i].ack })}
          onSavePreset={(name) => setPresets([...presets, toPreset(name, emit)])}
          onUpdatePreset={(i) => setPresets(presets.map((x, j) => (j === i ? toPreset(x.name, emit) : x)))}
          onDeletePreset={(i) => setPresets(presets.filter((_, j) => j !== i))}
          modelPath={`proto/${path}/sio-args.json`}
          hint="arguments: a JSON array is spread"
          fields={
            <>
              <Field label="Event" className="grow-field">
                <VarInput value={emit.event} vars={vars} placeholder="message" onChange={(event) => setEmit((x) => ({ ...x, event }))} onEnter={send} />
              </Field>
              <Checkbox checked={emit.ack} onChange={(ack) => setEmit((x) => ({ ...x, ack }))} label="Request ack" title="Ask the server to acknowledge; the reply shows as “ack #id”" />
            </>
          }
        />
      )}
      {sub === 'listen' && (
        <div className="col" style={{ maxWidth: 560 }}>
          <p className="subtle xs" style={{ margin: 0 }}>
            Events to show. Empty = every event. Other events are mentioned once in the log. Changes apply on the next connect.
          </p>
          <div className="chip-list">
            {listen.map((e) => (
              <span key={e} className="chip">
                {e}
                <button className="icon-btn sm" onClick={() => setListen(listen.filter((x) => x !== e))} aria-label={`Stop listening to ${e}`}>
                  <X />
                </button>
              </span>
            ))}
            {!listen.length && <span className="subtle xs">All events</span>}
          </div>
          <div className="row-sm">
            <input className="input mono" value={newEvent} placeholder="event name" onChange={(e) => setNewEvent(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && addEvent()} aria-label="Event to listen to" />
            <button className="btn btn-sm" onClick={addEvent} disabled={!newEvent.trim()}>
              <Plus /> Listen
            </button>
          </div>
        </div>
      )}
      {sub === 'auth' && (
        <div className="col-lg">
          <Field label="Auth payload (JSON object)" hint="Sent with the namespace CONNECT (socket.io-client's `auth` option). Use {{secret.NAME}} inside strings.">
            <div className="editor-shell" style={{ height: 160 }}>
              <CodeEditor value={draft.auth ?? ''} onChange={(auth) => set((d) => ({ ...d, auth: auth.trim() ? auth : undefined }))} language="json" path={`proto/${path}/sio-auth.json`} minimal />
            </div>
          </Field>
          <Field label="Query parameters" hint="Added to the handshake URL (socket.io-client's `query` option).">
            <KeyValueEditor rows={draft.query ?? []} onChange={(query) => set((d) => ({ ...d, query: query.length ? query : undefined }))} vars={vars} namePlaceholder="Parameter" />
          </Field>
        </div>
      )}
      {sub === 'headers' && (
        <KeyValueEditor rows={draft.headers ?? []} onChange={(headers) => set((d) => ({ ...d, headers: headers.length ? headers : undefined }))} vars={vars} namePlaceholder="Header" nameSuggestions={COMMON_HEADERS} />
      )}
      {sub === 'settings' && (
        <div className="col-lg" style={{ maxWidth: 480 }}>
          <Field label="Path" hint="Engine.IO endpoint path on the server (socket.io-client's `path`).">
            <input className="input mono" value={draft.path ?? '/socket.io/'} onChange={(e) => set((d) => ({ ...d, path: e.target.value && e.target.value !== '/socket.io/' ? e.target.value : undefined }))} />
          </Field>
          <Switch checked={settings.verifyTls} onChange={(verifyTls) => set((d) => ({ ...d, settings: { ...settings, verifyTls } }))} label="Verify TLS certificates (https://)" />
          <Field label="Connect timeout (ms)">
            <input className="input" type="number" min={0} value={settings.connectTimeoutMs} onChange={(e) => set((d) => ({ ...d, settings: { ...settings, connectTimeoutMs: num(e.target.value, 10000) } }))} />
          </Field>
          <p className="subtle xs">Socket.IO 3/4 servers over the WebSocket transport (long-polling-only servers aren’t supported).</p>
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

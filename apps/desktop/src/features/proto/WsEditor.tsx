import { useEffect, useRef, useState } from 'react';
import { Plug } from 'lucide-react';
import { Callout, Empty, Field, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { VarInput } from '@/components/VarInput';
import { errorMessage } from '@/lib/ipc';
import { protoApi, type MessageFormat, type WsFile, type WsSettings } from '@/lib/api/proto';
import { usePrefs } from '@/store/prefs';
import { useVariables } from '@/store/variables';
import { COMMON_HEADERS, KeyValueEditor } from '@/features/http/KeyValueEditor';
import { Composer } from './Composer';
import { MessageLog } from './MessageLog';
import { CommaListInput, ConnectButton, EditorFrame, KindBadge, LoadState, ProtoHead, StatusPill, confirmProtected, num, useEditorKeys, useProtoFile, useSticky } from './shared';
import { beginConnect, connectFailed, connected, eventSink, liveKey, useLive, useLiveState } from './store';
import { DocsField } from '@/features/http/DocsField';

type Sub = 'message' | 'headers' | 'settings' | 'docs';

const DEFAULT_SETTINGS: WsSettings = { verifyTls: true, connectTimeoutMs: 10000, pingIntervalMs: 0 };

/** Editor for `*.ws.yaml`: connect, compose and send text/JSON/binary frames, live log. */
export function WsEditor({ root, path }: { root: string; path: string }) {
  const { draft, update, dirty, save, error } = useProtoFile<WsFile>(root, path);
  const key = liveKey(root, path);
  const live = useLiveState(key, 'ws');
  const env = usePrefs((s) => s.activeEnv[root]);
  const vars = useVariables(path);
  const [sub, setSub] = useState<Sub>('message');
  const [msg, setMsg] = useSticky<{ body: string; format: MessageFormat }>(`ws:${key}`, () => ({ body: '', format: 'text' }));
  // Start the composer with the first saved message.
  const seeded = useRef(false);
  useEffect(() => {
    if (!draft || seeded.current) return;
    seeded.current = true;
    const first = draft.messages?.[0];
    if (!msg.body && first) setMsg({ body: first.body, format: first.format ?? 'text' });
  }, [draft, msg.body, setMsg]);

  const connect = async () => {
    if (!draft || live.status === 'connecting' || live.status === 'open') return;
    if (!(await confirmProtected(root))) return;
    beginConnect(key, 'ws');
    try {
      connected(key, 'ws', await protoApi.wsConnect(root, path, draft, env, eventSink(key)));
    } catch (e) {
      connectFailed(key, 'ws', errorMessage(e));
    }
  };
  const disconnect = () => {
    if (live.sessionId) protoApi.wsClose(live.sessionId).catch((e) => toast.error('Close failed', errorMessage(e)));
    else useLive.getState().patch(key, 'ws', { status: 'closed' });
  };
  const send = () => {
    if (live.status !== 'open' || !live.sessionId) return toast.info('Not connected', 'Connect first (Ctrl+Enter).');
    protoApi.wsSend(live.sessionId, msg.body, msg.format).catch((e) => toast.error('Send failed', errorMessage(e)));
  };
  useEditorKeys(save, () => (live.status === 'open' ? send() : connect()));

  if (!draft) return <LoadState error={error} />;
  const set = (fn: (d: WsFile) => WsFile) => update(fn);
  const settings = { ...DEFAULT_SETTINGS, ...draft.settings };
  const patchSettings = (p: Partial<WsSettings>) => set((d) => ({ ...d, settings: { ...settings, ...p } }));
  const presets = draft.messages ?? [];
  const setPresets = (messages: typeof presets) => set((d) => ({ ...d, messages: messages.length ? messages : undefined }));

  return (
    <EditorFrame
      head={<ProtoHead root={root} path={path} name={draft.name} dirty={dirty} onSave={save} status={<StatusPill status={live.status} />} />}
      bar={
        <>
          <KindBadge kind="ws" />
          <VarInput value={draft.url} vars={vars} placeholder="ws://localhost:8080/ws   ·   {{baseUrl}} works too (http → ws)" className="url-input" onChange={(url) => set((d) => ({ ...d, url }))} onEnter={connect} />
          <ConnectButton status={live.status} onConnect={connect} onDisconnect={disconnect} />
        </>
      }
      tabs={[
        { key: 'message', label: 'Message' },
        { key: 'headers', label: 'Headers', count: (draft.headers ?? []).filter((h) => h.enabled !== false).length + (draft.subprotocols?.length ?? 0) },
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
            <Empty icon={<Plug />} title={live.status === 'open' ? 'Connected — nothing received yet' : 'Not connected'}>
              Press <span className="kbd">Ctrl</span> + <span className="kbd">Enter</span> to {live.status === 'open' ? 'send the message' : 'connect'}. Frames you send and receive show up here.
            </Empty>
          }
        />
      }
    >
      {sub === 'message' && (
        <Composer
          value={msg.body}
          onChange={(body) => setMsg((m) => ({ ...m, body }))}
          format={msg.format}
          onFormat={(format) => setMsg((m) => ({ ...m, format }))}
          onSend={send}
          canSend={live.status === 'open'}
          presets={presets}
          onLoadPreset={(i) => setMsg({ body: presets[i].body, format: presets[i].format ?? 'text' })}
          onSavePreset={(name) => setPresets([...presets, { name, body: msg.body, ...(msg.format !== 'text' ? { format: msg.format } : {}) }])}
          onUpdatePreset={(i) => setPresets(presets.map((x, j) => (j === i ? { name: x.name, body: msg.body, ...(msg.format !== 'text' ? { format: msg.format } : {}) } : x)))}
          onDeletePreset={(i) => setPresets(presets.filter((_, j) => j !== i))}
          modelPath={`proto/${path}/ws-message`}
          hint="{{variables}} are resolved when sent"
        />
      )}
      {sub === 'headers' && (
        <div className="col-lg">
          <KeyValueEditor rows={draft.headers ?? []} onChange={(headers) => set((d) => ({ ...d, headers: headers.length ? headers : undefined }))} vars={vars} namePlaceholder="Header" nameSuggestions={COMMON_HEADERS} />
          <Field label="Subprotocols" hint="Offered in Sec-WebSocket-Protocol, comma-separated (e.g. graphql-transport-ws, mqtt).">
            <CommaListInput value={draft.subprotocols ?? []} onChange={(list) => set((d) => ({ ...d, subprotocols: list.length ? list : undefined }))} placeholder="none" />
          </Field>
        </div>
      )}
      {sub === 'settings' && (
        <div className="col-lg" style={{ maxWidth: 480 }}>
          <Switch checked={settings.verifyTls} onChange={(verifyTls) => patchSettings({ verifyTls })} label="Verify TLS certificates (wss://)" />
          <p className="subtle xs">Certificates are checked against your operating system’s trust store. Turn this off for local self-signed servers.</p>
          <Field label="Connect timeout (ms)">
            <input className="input" type="number" min={0} value={settings.connectTimeoutMs} onChange={(e) => patchSettings({ connectTimeoutMs: num(e.target.value, 10000) })} />
          </Field>
          <Field label="Keep-alive ping every (ms)" hint="0 = off. Some proxies drop idle connections after 60 s.">
            <input className="input" type="number" min={0} step={1000} value={settings.pingIntervalMs} onChange={(e) => patchSettings({ pingIntervalMs: num(e.target.value) })} />
          </Field>
        </div>
      )}
      {sub === 'docs' && (
        <Field label="Notes (Markdown, saved with the file)">
          <DocsField value={draft.docs} onChange={(docs) => set((d) => ({ ...d, docs }))} />
        </Field>
      )}
    </EditorFrame>
  );
}

import { useEffect, useRef, useState } from 'react';
import { BellOff, BellRing, Plus, Radio, Trash2 } from 'lucide-react';
import { Callout, Checkbox, Empty, Field, Segmented, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { VarInput } from '@/components/VarInput';
import { errorMessage } from '@/lib/ipc';
import { protoApi, type MessageFormat, type MqttFile, type MqttLastWill, type MqttPublish, type MqttSubscription } from '@/lib/api/proto';
import { usePrefs } from '@/store/prefs';
import { useVariables } from '@/store/variables';
import type { VarInfo } from '@/lib/types';
import { Composer } from './Composer';
import { MessageLog } from './MessageLog';
import { ConnectButton, EditorFrame, KindBadge, LoadState, ProtoHead, StatusPill, confirmProtected, num, useEditorKeys, useProtoFile, useSticky } from './shared';
import { beginConnect, connectFailed, connected, eventSink, liveKey, useLive, useLiveState } from './store';

type Sub = 'publish' | 'subscriptions' | 'connection' | 'docs';

interface PubDraft {
  topic: string;
  payload: string;
  format: MessageFormat;
  qos: number;
  retain: boolean;
}

function QosSelect({ value, onChange, label = 'QoS' }: { value: number; onChange(q: number): void; label?: string }) {
  return (
    <select className="select sm" value={value} onChange={(e) => onChange(Number(e.target.value))} aria-label={label} title="0 = at most once · 1 = at least once · 2 = exactly once">
      <option value={0}>QoS 0</option>
      <option value={1}>QoS 1</option>
      <option value={2}>QoS 2</option>
    </select>
  );
}

const toPreset = (name: string, p: PubDraft): MqttPublish => ({
  name,
  topic: p.topic,
  payload: p.payload,
  ...(p.format !== 'text' ? { format: p.format } : {}),
  ...(p.qos ? { qos: p.qos } : {}),
  ...(p.retain ? { retain: true } : {}),
});

/** Editor for `*.mqtt.yaml`: connect (3.1.1/5, TCP/TLS/WebSocket), subscribe, publish, live log. */
export function MqttEditor({ root, path }: { root: string; path: string }) {
  const { draft, update, dirty, save, error } = useProtoFile<MqttFile>(root, path);
  const key = liveKey(root, path);
  const live = useLiveState(key, 'mqtt');
  const env = usePrefs((s) => s.activeEnv[root]);
  const vars = useVariables(path);
  const [sub, setSub] = useState<Sub>('publish');
  const [pub, setPub] = useSticky<PubDraft>(`mqtt:${key}`, () => ({ topic: '', payload: '', format: 'text', qos: 0, retain: false }));
  const seeded = useRef(false);
  useEffect(() => {
    if (!draft || seeded.current) return;
    seeded.current = true;
    const first = draft.publishes?.[0];
    if (!pub.topic && !pub.payload && first) setPub({ topic: first.topic, payload: first.payload, format: first.format ?? 'text', qos: first.qos ?? 0, retain: !!first.retain });
  }, [draft, pub.topic, pub.payload, setPub]);

  const open = live.status === 'open' && !!live.sessionId;
  const connect = async () => {
    if (!draft || live.status === 'connecting' || live.status === 'open') return;
    if (!(await confirmProtected(root))) return;
    beginConnect(key, 'mqtt');
    try {
      connected(key, 'mqtt', await protoApi.mqttConnect(root, path, draft, env, eventSink(key)));
    } catch (e) {
      connectFailed(key, 'mqtt', errorMessage(e));
    }
  };
  const disconnect = () => {
    if (live.sessionId) protoApi.mqttDisconnect(live.sessionId).catch((e) => toast.error('Disconnect failed', errorMessage(e)));
    useLive.getState().patch(key, 'mqtt', { status: 'closed', sessionId: undefined });
  };
  const publish = () => {
    if (!open) return toast.info('Not connected', 'Connect first (Ctrl+Enter).');
    if (!pub.topic.trim()) return toast.info('Topic required', 'Enter the topic to publish to.');
    protoApi.mqttPublish(live.sessionId!, pub.topic, pub.payload, pub.qos, pub.retain, pub.format).catch((e) => toast.error('Publish failed', errorMessage(e)));
  };
  useEditorKeys(save, () => (open ? publish() : connect()));

  if (!draft) return <LoadState error={error} />;
  const set = (fn: (d: MqttFile) => MqttFile) => update(fn);
  const subs = draft.subscriptions ?? [];
  const setSubs = (s: MqttSubscription[]) => set((d) => ({ ...d, subscriptions: s.length ? s : undefined }));
  const presets = draft.publishes ?? [];
  const setPresets = (p: MqttPublish[]) => set((d) => ({ ...d, publishes: p.length ? p : undefined }));
  const will = draft.lastWill;
  const setWill = (w: Partial<MqttLastWill> | null) =>
    set((d) => {
      if (!w) return { ...d, lastWill: undefined };
      const next = { topic: '', payload: '', ...d.lastWill, ...w };
      return { ...d, lastWill: next.topic || next.payload ? next : undefined };
    });

  return (
    <EditorFrame
      head={<ProtoHead root={root} path={path} name={draft.name} dirty={dirty} onSave={save} status={<StatusPill status={live.status} />} />}
      bar={
        <>
          <KindBadge kind="mqtt" />
          <VarInput value={draft.brokerUrl} vars={vars} placeholder="mqtt://localhost:1883 · mqtts:// · ws://host:8083/mqtt · wss://" className="url-input" onChange={(brokerUrl) => set((d) => ({ ...d, brokerUrl }))} onEnter={connect} />
          <ConnectButton status={live.status} onConnect={connect} onDisconnect={disconnect} />
        </>
      }
      tabs={[
        { key: 'publish', label: 'Publish' },
        { key: 'subscriptions', label: 'Subscriptions', count: subs.length },
        { key: 'connection', label: 'Connection', dot: !!draft.username || !!will || draft.mqttVersion === '5' },
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
            <Empty icon={<Radio />} title={open ? 'Connected — waiting for messages' : 'Not connected'}>
              Enabled subscriptions are subscribed on connect. Received messages are labelled with their topic; filter by topic above.
            </Empty>
          }
        />
      }
    >
      {sub === 'publish' && (
        <Composer
          value={pub.payload}
          onChange={(payload) => setPub((p) => ({ ...p, payload }))}
          format={pub.format}
          onFormat={(format) => setPub((p) => ({ ...p, format }))}
          onSend={publish}
          canSend={open}
          sendLabel="Publish"
          sendTitle="Publish (Ctrl+Enter)"
          presets={presets}
          onLoadPreset={(i) => {
            const x = presets[i];
            setPub({ topic: x.topic, payload: x.payload, format: x.format ?? 'text', qos: x.qos ?? 0, retain: !!x.retain });
          }}
          onSavePreset={(name) => setPresets([...presets, toPreset(name, pub)])}
          onUpdatePreset={(i) => setPresets(presets.map((x, j) => (j === i ? toPreset(x.name, pub) : x)))}
          onDeletePreset={(i) => setPresets(presets.filter((_, j) => j !== i))}
          modelPath={`proto/${path}/mqtt-payload`}
          hint="{{variables}} are resolved when published"
          fields={
            <>
              <Field label="Topic" className="grow-field">
                <VarInput value={pub.topic} vars={vars} placeholder="devices/42/state" onChange={(topic) => setPub((p) => ({ ...p, topic }))} onEnter={publish} />
              </Field>
              <QosSelect value={pub.qos} onChange={(qos) => setPub((p) => ({ ...p, qos }))} />
              <Checkbox checked={pub.retain} onChange={(retain) => setPub((p) => ({ ...p, retain }))} label="Retain" title="The broker keeps the last retained message per topic for new subscribers" />
            </>
          }
        />
      )}
      {sub === 'subscriptions' && <Subscriptions subs={subs} onChange={setSubs} vars={vars} sessionId={open ? live.sessionId : undefined} />}
      {sub === 'connection' && (
        <div className="col-lg" style={{ maxWidth: 560 }}>
          <Field label="Protocol version">
            <Segmented
              value={draft.mqttVersion ?? '3.1.1'}
              onChange={(v) => set((d) => ({ ...d, mqttVersion: v === '3.1.1' ? undefined : v }))}
              options={[
                { value: '3.1.1', label: 'MQTT 3.1.1' },
                { value: '5', label: 'MQTT 5' },
              ]}
            />
          </Field>
          <Field label="Client id" hint="Empty = a random devall-… id per connection. Two clients with the same id kick each other off.">
            <VarInput value={draft.clientId ?? ''} vars={vars} placeholder="random" onChange={(clientId) => set((d) => ({ ...d, clientId: clientId || undefined }))} />
          </Field>
          <div className="grid-2">
            <Field label="Username">
              <VarInput value={draft.username ?? ''} vars={vars} placeholder="none" onChange={(username) => set((d) => ({ ...d, username: username || undefined }))} />
            </Field>
            <Field label="Password">
              <VarInput value={draft.password ?? ''} vars={vars} placeholder="{{secret.MQTT_PASSWORD}}" onChange={(password) => set((d) => ({ ...d, password: password || undefined }))} />
            </Field>
          </div>
          {!!draft.password && !draft.password.includes('{{') && (
            <Callout kind="warning">This password is stored in the file in plain text. Use a secret instead, e.g. {'{{secret.MQTT_PASSWORD}}'} (Environments → Secrets).</Callout>
          )}
          <div className="grid-2">
            <Field label="Keep alive (s)">
              <input className="input" type="number" min={0} value={draft.keepAlive ?? 60} onChange={(e) => set((d) => ({ ...d, keepAlive: num(e.target.value, 60) === 60 ? undefined : num(e.target.value, 60) }))} />
            </Field>
            <Field label="Connect timeout (ms)">
              <input
                className="input"
                type="number"
                min={0}
                value={draft.connectTimeoutMs ?? 10000}
                onChange={(e) => set((d) => ({ ...d, connectTimeoutMs: num(e.target.value, 10000) === 10000 ? undefined : num(e.target.value, 10000) }))}
              />
            </Field>
          </div>
          <Switch checked={draft.cleanSession !== false} onChange={(v) => set((d) => ({ ...d, cleanSession: v ? undefined : false }))} label={draft.mqttVersion === '5' ? 'Clean start' : 'Clean session'} />
          <Switch checked={draft.verifyTls !== false} onChange={(v) => set((d) => ({ ...d, verifyTls: v ? undefined : false }))} label="Verify TLS certificates (mqtts:// / wss://)" />
          <Field label="Last will" hint="Published by the broker if this client disconnects unexpectedly. Empty topic = none.">
            <div className="col-sm">
              <div className="row-sm">
                <VarInput value={will?.topic ?? ''} vars={vars} placeholder="clients/devall/status" onChange={(topic) => setWill({ topic })} />
                <QosSelect value={will?.qos ?? 0} onChange={(qos) => setWill({ qos: qos || undefined })} label="Last will QoS" />
                <Checkbox checked={!!will?.retain} onChange={(retain) => setWill({ retain: retain || undefined })} label="Retain" />
              </div>
              <VarInput value={will?.payload ?? ''} vars={vars} placeholder="offline" onChange={(payload) => setWill({ payload })} />
            </div>
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

function Subscriptions({ subs, onChange, vars, sessionId }: { subs: MqttSubscription[]; onChange(s: MqttSubscription[]): void; vars: VarInfo[]; sessionId?: string }) {
  const patch = (i: number, p: Partial<MqttSubscription>) => onChange(subs.map((s, j) => (j === i ? { ...s, ...p } : s)));
  const subscribe = (s: MqttSubscription) => sessionId && protoApi.mqttSubscribe(sessionId, s.topic, s.qos ?? 0).catch((e) => toast.error('Subscribe failed', errorMessage(e)));
  const unsubscribe = (s: MqttSubscription) => sessionId && protoApi.mqttUnsubscribe(sessionId, s.topic).catch((e) => toast.error('Unsubscribe failed', errorMessage(e)));
  return (
    <div className="col">
      <p className="subtle xs" style={{ margin: 0 }}>
        Checked filters are subscribed automatically on connect. <span className="mono">+</span> matches one level, <span className="mono">#</span> the rest.
      </p>
      {subs.map((s, i) => (
        <div key={i} className="sub-row">
          <Checkbox checked={s.enabled !== false} onChange={(v) => patch(i, { enabled: v ? undefined : false })} title="Subscribe on connect" />
          <VarInput value={s.topic} vars={vars} placeholder="sensors/+/temperature" onChange={(topic) => patch(i, { topic })} />
          <QosSelect value={s.qos ?? 0} onChange={(qos) => patch(i, { qos: qos || undefined })} />
          <div className="row-sm">
            <button className="icon-btn sm" onClick={() => subscribe(s)} disabled={!sessionId || !s.topic.trim()} title="Subscribe now" aria-label="Subscribe now">
              <BellRing />
            </button>
            <button className="icon-btn sm" onClick={() => unsubscribe(s)} disabled={!sessionId || !s.topic.trim()} title="Unsubscribe" aria-label="Unsubscribe">
              <BellOff />
            </button>
            <button className="icon-btn sm" onClick={() => onChange(subs.filter((_, j) => j !== i))} title="Remove" aria-label="Remove subscription">
              <Trash2 />
            </button>
          </div>
        </div>
      ))}
      <div>
        <button className="btn btn-sm" onClick={() => onChange([...subs, { topic: '' }])}>
          <Plus /> Add subscription
        </button>
      </div>
    </div>
  );
}

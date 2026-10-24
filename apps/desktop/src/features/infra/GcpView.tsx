import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { ArrowUpFromLine, Download, Eye, Folder, FileText, Pause, Play, Plus, RefreshCw, ScrollText, Send, Trash2 } from 'lucide-react';
import { Callout, Checkbox, Empty, Field, SearchInput, Segmented, Spinner, Switch } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { errorMessage } from '@/lib/ipc';
import { uid } from '@/lib/id';
import {
  infraApi,
  type Bucket,
  type DsEntity,
  type FsDocument,
  type GcpStatus,
  type GcpTarget,
  type LogEntryRow,
  type ObjectList,
  type ReceivedMessage,
  type Subscription,
  type Topic,
} from '@/lib/api/infra';
import { KeyValueEditor } from '@/features/http/KeyValueEditor';
import type { KeyValue } from '@/lib/types';
import { fmtBytes } from './store';
import { useOpenInLogViewer } from './logViewer';
import { GcpSecretsPanel } from './GcpSecretsPanel';
import { SidePane } from '@/components/layout/SidePane';

type Service = 'pubsub' | 'firestore' | 'datastore' | 'storage' | 'logging' | 'secrets';
/** Real-cloud only (no emulator). */
const CLOUD_ONLY: Service[] = ['logging', 'secrets'];

const SETTINGS_KEY = 'devall.infra.gcp';
interface GcpSettings {
  project: string;
  /** Per service: emulator host, or '' for the real cloud. */
  hosts: Partial<Record<Service, string>>;
  cloud: Partial<Record<Service, boolean>>;
  allowWrites: boolean;
  service: Service;
}

function loadSettings(): GcpSettings {
  try {
    return { project: 'demo-project', hosts: {}, cloud: {}, allowWrites: false, service: 'pubsub', ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}') };
  } catch {
    return { project: 'demo-project', hosts: {}, cloud: {}, allowWrites: false, service: 'pubsub' };
  }
}

const LABEL: Record<Service, string> = { pubsub: 'Pub/Sub', firestore: 'Firestore', datastore: 'Datastore', storage: 'Storage', logging: 'Logging', secrets: 'Secret Manager' };

export function GcpView() {
  const [s, setS] = useState<GcpSettings>(loadSettings);
  const [status, setStatus] = useState<GcpStatus | null>(null);
  const update = (patch: Partial<GcpSettings>) => {
    const next = { ...s, ...patch };
    setS(next);
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(next));
    } catch {
      /* storage unavailable */
    }
  };
  const refreshStatus = useCallback(() => {
    infraApi.gcp.status().then(setStatus, (e) => toast.error('GCP status failed', errorMessage(e)));
  }, []);
  useEffect(refreshStatus, [refreshStatus]);

  const svc = s.service;
  const emu = status?.emulators.find((e) => e.kind === svc);
  const cloud = CLOUD_ONLY.includes(svc) || !!s.cloud[svc];
  const host = s.hosts[svc] ?? emu?.host ?? '';
  const target: GcpTarget = useMemo(
    () => ({ project: s.project.trim(), emulator: cloud ? undefined : host, allowWrites: cloud ? s.allowWrites : true }),
    [s.project, cloud, host, s.allowWrites],
  );
  const targetKey = `${svc}|${target.project}|${target.emulator ?? 'cloud'}|${target.allowWrites}`;

  return (
    <div className="infra-view">
      <div className="toolbar">
        <Segmented value={svc} onChange={(v) => update({ service: v })} options={(Object.keys(LABEL) as Service[]).map((k) => ({ value: k, label: LABEL[k] }))} />
        <span className="grow" />
        <label className="row-sm small">
          Project
          <input className="input sm mono gcp-project" value={s.project} onChange={(e) => update({ project: e.target.value })} placeholder={status?.gcloud.project ?? 'my-project'} />
        </label>
        {status?.gcloud.project && status.gcloud.project !== s.project && (
          <button className="btn btn-sm btn-ghost" onClick={() => update({ project: status.gcloud.project! })} title="Use the gcloud default project">
            use {status.gcloud.project}
          </button>
        )}
      </div>
      <div className="toolbar gcp-target">
        {!CLOUD_ONLY.includes(svc) ? (
          <>
            <Segmented
              value={cloud ? 'cloud' : 'emulator'}
              onChange={(v) => update({ cloud: { ...s.cloud, [svc]: v === 'cloud' } })}
              options={[
                { value: 'emulator', label: 'Emulator' },
                { value: 'cloud', label: 'Google Cloud' },
              ]}
            />
            {!cloud && (
              <>
                <input className="input sm mono gcp-host" value={host} onChange={(e) => update({ hosts: { ...s.hosts, [svc]: e.target.value } })} aria-label="Emulator host" />
                <span className={clsx('dot', emu?.reachable && emu.host === host ? 'ok' : 'err')} />
                <span className="xs subtle">
                  {emu?.reachable && emu.host === host ? 'emulator running' : 'not reachable'}
                  {emu?.fromEnv ? ' · from env' : ''}
                </span>
              </>
            )}
          </>
        ) : (
          <span className="xs subtle">
            {svc === 'logging'
              ? 'Cloud Logging has no emulator; it uses your gcloud login and is read-only.'
              : 'Secret Manager has no emulator; it uses your gcloud login and is read-only (metadata until you import).'}
          </span>
        )}
        {cloud && (
          <>
            <span className="xs subtle truncate">
              gcloud: {status?.gcloud.account ?? (status?.gcloud.path ? 'not logged in' : 'not installed')}
            </span>
            {!CLOUD_ONLY.includes(svc) && <Switch checked={s.allowWrites} onChange={(v) => update({ allowWrites: v })} label="Allow writes" title="Real projects are read-only until you allow writes" />}
          </>
        )}
        <span className="grow" />
        <button className="icon-btn sm" onClick={refreshStatus} title="Re-check emulators" aria-label="Re-check emulators">
          <RefreshCw />
        </button>
      </div>
      {!target.project ? (
        <Empty title="Enter a project id">Emulators accept any project id.</Empty>
      ) : svc === 'pubsub' ? (
        <PubSubPanel key={targetKey} target={target} />
      ) : svc === 'firestore' ? (
        <FirestorePanel key={targetKey} target={target} />
      ) : svc === 'datastore' ? (
        <DatastorePanel key={targetKey} target={target} />
      ) : svc === 'storage' ? (
        <StoragePanel key={targetKey} target={target} />
      ) : svc === 'secrets' ? (
        <GcpSecretsPanel key={targetKey} target={target} />
      ) : (
        <LoggingPanel key={targetKey} target={target} />
      )}
    </div>
  );
}

function Failure({ error, onRetry }: { error: string; onRetry?(): void }) {
  return (
    <div className="p-5">
      <Callout kind="danger">
        <div className="break pre-line">{error}</div>
        {onRetry && (
          <button className="btn btn-sm mt-2" onClick={onRetry}>
            <RefreshCw /> Retry
          </button>
        )}
      </Callout>
    </div>
  );
}

// ---------- Pub/Sub ----------

function PubSubPanel({ target }: { target: GcpTarget }) {
  const [topics, setTopics] = useState<Topic[] | null>(null);
  const [subs, setSubs] = useState<Subscription[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [sel, setSel] = useState<{ kind: 'topic' | 'sub'; name: string } | null>(null);
  const load = useCallback(async () => {
    try {
      const [t, su] = await Promise.all([infraApi.gcp.topics(target), infraApi.gcp.subscriptions(target)]);
      setTopics(t);
      setSubs(su);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [target]);
  useEffect(() => {
    void load();
  }, [load]);

  const createTopic = async () => {
    const name = await promptDialog({ title: 'New topic', label: 'Topic id', placeholder: 'orders' });
    if (!name) return;
    try {
      await infraApi.gcp.createTopic(target, name);
      void load();
      setSel({ kind: 'topic', name });
    } catch (e) {
      toast.error('Could not create the topic', errorMessage(e));
    }
  };
  const createSub = async (topic: string) => {
    const name = await promptDialog({ title: `New subscription on ${topic}`, label: 'Subscription id', initial: `${topic}-sub` });
    if (!name) return;
    try {
      await infraApi.gcp.createSubscription(target, name, topic);
      void load();
      setSel({ kind: 'sub', name });
    } catch (e) {
      toast.error('Could not create the subscription', errorMessage(e));
    }
  };
  const remove = async (kind: 'topic' | 'sub', name: string) => {
    const ok = await confirmDialog({ title: `Delete ${kind === 'topic' ? 'topic' : 'subscription'} ${name}?`, confirmText: 'Delete', danger: true });
    if (!ok) return;
    try {
      if (kind === 'topic') await infraApi.gcp.deleteTopic(target, name);
      else await infraApi.gcp.deleteSubscription(target, name);
      setSel(null);
      void load();
    } catch (e) {
      toast.error('Delete failed', errorMessage(e));
    }
  };

  if (error && !topics) return <Failure error={error} onRetry={load} />;
  if (!topics)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  return (
    <div className="split">
      <SidePane className="gcp-side">
        <div className="toolbar">
          <span className="strong small grow">Topics</span>
          <button className="icon-btn sm" onClick={createTopic} title="New topic" aria-label="New topic">
            <Plus />
          </button>
          <button className="icon-btn sm" onClick={load} title="Refresh" aria-label="Refresh">
            <RefreshCw />
          </button>
        </div>
        <div className="list">
          {topics.length === 0 && <div className="p-4 xs subtle">No topics yet.</div>}
          {topics.map((t) => (
            <div key={t.name}>
              <div className={clsx('list-item', sel?.kind === 'topic' && sel.name === t.name && 'on')} onClick={() => setSel({ kind: 'topic', name: t.name })}>
                <Send size={13} className="subtle" />
                <span className="grow truncate">{t.name}</span>
                <button className="icon-btn sm" title="New subscription" aria-label="New subscription" onClick={(e) => (e.stopPropagation(), createSub(t.name))}>
                  <Plus />
                </button>
              </div>
              {subs
                .filter((su) => su.topic === t.name)
                .map((su) => (
                  <div key={su.name} className={clsx('list-item sub-item', sel?.kind === 'sub' && sel.name === su.name && 'on')} onClick={() => setSel({ kind: 'sub', name: su.name })}>
                    <span className="grow truncate">{su.name}</span>
                    {su.pushEndpoint && <span className="meta">push</span>}
                  </div>
                ))}
            </div>
          ))}
          {subs.filter((su) => !topics.some((t) => t.name === su.topic)).length > 0 && <div className="list-group-header">Orphaned subscriptions</div>}
          {subs
            .filter((su) => !topics.some((t) => t.name === su.topic))
            .map((su) => (
              <div key={su.name} className={clsx('list-item', sel?.kind === 'sub' && sel.name === su.name && 'on')} onClick={() => setSel({ kind: 'sub', name: su.name })}>
                <span className="grow truncate">{su.name}</span>
              </div>
            ))}
        </div>
      </SidePane>
      <div className="pane fill">
        {!sel ? (
          <Empty icon={<Send />} title="Pick a topic or subscription">
            Publish with attributes to a topic; pull, ack or live-tail a subscription. The emulator has no UI of its own — this is it.
          </Empty>
        ) : sel.kind === 'topic' ? (
          <PublishPanel key={sel.name} target={target} topic={sel.name} onDelete={() => remove('topic', sel.name)} />
        ) : (
          <SubscriptionPanel key={sel.name} target={target} sub={subs.find((x) => x.name === sel.name)} name={sel.name} onDelete={() => remove('sub', sel.name)} />
        )}
      </div>
    </div>
  );
}

function PublishPanel({ target, topic, onDelete }: { target: GcpTarget; topic: string; onDelete(): void }) {
  const [data, setData] = useState('{\n  "id": 1\n}');
  const [attrs, setAttrs] = useState<KeyValue[]>([]);
  const [orderingKey, setOrderingKey] = useState('');
  const [base64, setBase64] = useState(false);
  const [sent, setSent] = useState<string[]>([]);
  const publish = async () => {
    try {
      const attributes = Object.fromEntries(attrs.filter((a) => a.name && a.enabled !== false).map((a) => [a.name, a.value]));
      const ids = await infraApi.gcp.publish(target, topic, [{ data, base64, attributes, orderingKey: orderingKey || undefined }]);
      setSent((prev) => [...ids, ...prev].slice(0, 20));
      toast.success(`Published to ${topic}`, `message id ${ids.join(', ')}`);
    } catch (e) {
      toast.error('Publish failed', errorMessage(e));
    }
  };
  return (
    <div className="gcp-panel">
      <div className="toolbar">
        <span className="strong small">Publish to {topic}</span>
        <span className="grow" />
        <Checkbox checked={base64} onChange={setBase64} label="Payload is base64" />
        <button className="btn btn-sm btn-primary" onClick={publish}>
          <Send /> Publish
        </button>
        <button className="icon-btn sm danger" onClick={onDelete} title="Delete topic" aria-label="Delete topic">
          <Trash2 />
        </button>
      </div>
      <div className="gcp-editor">
        <CodeEditor value={data} onChange={setData} language={base64 ? 'plaintext' : 'json'} path={`inmemory://infra/pubsub-${topic}.json`} onSave={publish} />
      </div>
      <div className="p-4 col">
        <Field label="Attributes">
          <KeyValueEditor rows={attrs} onChange={setAttrs} vars={[]} namePlaceholder="key" valuePlaceholder="value" />
        </Field>
        <Field label="Ordering key" hint="Optional; the topic must have message ordering enabled for subscribers to see order.">
          <input className="input mono" value={orderingKey} onChange={(e) => setOrderingKey(e.target.value)} />
        </Field>
        {sent.length > 0 && <div className="xs subtle">Recent message ids: {sent.join(', ')}</div>}
      </div>
    </div>
  );
}

function SubscriptionPanel({ target, sub, name, onDelete }: { target: GcpTarget; sub?: Subscription; name: string; onDelete(): void }) {
  const [messages, setMessages] = useState<ReceivedMessage[]>([]);
  const [ack, setAck] = useState(true);
  const [tailing, setTailing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const streamRef = useRef<string | null>(null);
  const openInViewer = useOpenInLogViewer();

  const stopTail = () => {
    if (streamRef.current) void infraApi.streamStop(streamRef.current);
    streamRef.current = null;
    setTailing(false);
  };
  useEffect(() => stopTail, []);

  const pull = async () => {
    try {
      const got = await infraApi.gcp.pull(target, name, 50, ack);
      setMessages((prev) => [...got, ...prev].slice(0, 1000));
      if (got.length === 0) toast.info('No messages waiting');
    } catch (e) {
      toast.error('Pull failed', errorMessage(e));
    }
  };
  const tail = async () => {
    if (tailing) return stopTail();
    const id = `pubsub-${uid()}`;
    streamRef.current = id;
    setTailing(true);
    setError(null);
    try {
      await infraApi.gcp.tail(id, target, name, ack, (m) => {
        if (m.type === 'error') setError(m.message);
        else if (m.messages.length) {
          setError(null);
          setMessages((prev) => [...m.messages.slice().reverse(), ...prev].slice(0, 1000));
        }
      });
    } catch (e) {
      setTailing(false);
      toast.error('Tail failed', errorMessage(e));
    }
  };
  const toViewer = () =>
    openInViewer({
      name: `pubsub ${name}`,
      text: messages
        .slice()
        .reverse()
        .map((m) => JSON.stringify({ ts: m.publishTime, messageId: m.messageId, attributes: m.attributes, data: m.text ?? m.dataBase64 }))
        .join('\n'),
    });

  return (
    <div className="gcp-panel">
      <div className="toolbar">
        <span className="strong small">{name}</span>
        {sub && <span className="xs subtle">topic {sub.topic} · ack deadline {sub.ackDeadlineSeconds}s{sub.filter ? ` · filter ${sub.filter}` : ''}</span>}
        <span className="grow" />
        <Checkbox checked={ack} onChange={setAck} label="Ack" title="Acknowledge received messages (otherwise they are redelivered after the ack deadline)" />
        <button className="btn btn-sm" onClick={pull} disabled={tailing}>
          <Download /> Pull
        </button>
        <button className={clsx('btn btn-sm', tailing ? 'btn-subtle' : 'btn-primary')} onClick={tail}>
          {tailing ? <Pause /> : <Play />} {tailing ? 'Stop tail' : 'Live tail'}
        </button>
        <button className="btn btn-sm" onClick={toViewer} disabled={!messages.length}>
          <ScrollText /> Open in log viewer
        </button>
        <button className="icon-btn sm danger" onClick={onDelete} title="Delete subscription" aria-label="Delete subscription">
          <Trash2 />
        </button>
      </div>
      {error && <div className="log-error small">{error}</div>}
      {messages.length === 0 ? (
        <Empty title={tailing ? 'Waiting for messages…' : 'No messages pulled yet'}>{tailing ? <Spinner /> : 'Pull once or start a live tail.'}</Empty>
      ) : (
        <div className="scroll grow">
          <table className="table infra-table">
            <thead>
              <tr>
                <th>Published</th>
                <th>Id</th>
                <th>Attributes</th>
                <th>Data</th>
              </tr>
            </thead>
            <tbody>
              {messages.map((m) => (
                <tr key={`${m.messageId}-${m.ackId}`} className={clsx('clickable', open === m.messageId && 'on')} onClick={() => setOpen(open === m.messageId ? null : m.messageId)}>
                  <td className="xs subtle nowrap">{m.publishTime?.replace('T', ' ').replace(/\.\d+Z$/, 'Z')}</td>
                  <td className="mono xs">{m.messageId}</td>
                  <td className="xs">
                    {Object.entries(m.attributes).map(([k, v]) => (
                      <span key={k} className="tag mono">
                        {k}={v}
                      </span>
                    ))}
                  </td>
                  <td className={clsx('mono xs', open === m.messageId ? 'pre-wrap' : 'truncate cell-msg')}>{m.text ?? `<${m.size} bytes binary>`}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// ---------- Firestore ----------

function FirestorePanel({ target }: { target: GcpTarget }) {
  const [path, setPath] = useState<string>('');
  const [collections, setCollections] = useState<string[] | null>(null);
  const [docs, setDocs] = useState<FsDocument[]>([]);
  const [next, setNext] = useState<string | undefined>();
  const [error, setError] = useState<string | null>(null);
  const [doc, setDoc] = useState<FsDocument | null>(null);
  // `path` is '' (root), a collection path (odd segments) or a document path (even segments).
  const segments = path ? path.split('/') : [];
  const isCollection = segments.length % 2 === 1;

  useEffect(() => {
    let live = true;
    setError(null);
    setDoc(null);
    const run = async () => {
      try {
        if (isCollection) {
          const page = await infraApi.gcp.firestoreDocuments(target, path);
          if (!live) return;
          setDocs(page.documents);
          setNext(page.nextPageToken);
          setCollections(null);
        } else {
          const cols = await infraApi.gcp.firestoreCollections(target, path);
          if (!live) return;
          setCollections(cols);
          setDocs([]);
        }
      } catch (e) {
        if (live) setError(errorMessage(e));
      }
    };
    void run();
    return () => {
      live = false;
    };
  }, [target, path, isCollection]);

  const more = async () => {
    if (!next) return;
    const page = await infraApi.gcp.firestoreDocuments(target, path, next);
    setDocs((d) => [...d, ...page.documents]);
    setNext(page.nextPageToken);
  };

  return (
    <div className="gcp-panel">
      <div className="toolbar">
        <div className="breadcrumbs small">
          <button className="btn btn-sm btn-ghost" onClick={() => setPath('')}>
            (root)
          </button>
          {segments.map((s, i) => (
            <span key={i} className="row-sm">
              /
              <button className="btn btn-sm btn-ghost mono" onClick={() => setPath(segments.slice(0, i + 1).join('/'))}>
                {s}
              </button>
            </span>
          ))}
        </div>
      </div>
      {error ? (
        <Failure error={error} />
      ) : (
        <div className="split">
          <SidePane className="gcp-side">
            <div className="list">
              {collections?.map((c) => (
                <div key={c} className="list-item" onClick={() => setPath(path ? `${path}/${c}` : c)}>
                  <Folder size={13} className="subtle" />
                  <span className="grow truncate">{c}</span>
                </div>
              ))}
              {collections?.length === 0 && <div className="p-4 xs subtle">No sub-collections.</div>}
              {isCollection &&
                docs.map((d) => (
                  <div key={d.path} className={clsx('list-item', doc?.path === d.path && 'on')} onClick={() => setDoc(d)} onDoubleClick={() => setPath(d.path)}>
                    <FileText size={13} className="subtle" />
                    <span className="grow truncate mono">{d.id}</span>
                  </div>
                ))}
              {isCollection && docs.length === 0 && <div className="p-4 xs subtle">No documents.</div>}
              {next && (
                <div className="p-4">
                  <button className="btn btn-sm w-full" onClick={more}>
                    Load more
                  </button>
                </div>
              )}
            </div>
          </SidePane>
          <div className="pane fill">
            {doc ? (
              <div className="gcp-panel">
                <div className="toolbar">
                  <span className="mono small strong">{doc.path}</span>
                  <span className="xs subtle">updated {doc.updateTime}</span>
                  <span className="grow" />
                  <button className="btn btn-sm" onClick={() => setPath(doc.path)}>
                    <Folder /> Sub-collections
                  </button>
                </div>
                <CodeEditor value={JSON.stringify(doc.fields, null, 2)} language="json" readOnly path={`inmemory://infra/fs-${doc.path}.json`} />
              </div>
            ) : (
              <Empty icon={<FileText />} title={isCollection ? 'Pick a document' : 'Pick a collection'}>
                Values are shown as plain JSON (integers, maps, arrays, timestamps, references).
              </Empty>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ---------- Datastore ----------

function DatastorePanel({ target }: { target: GcpTarget }) {
  const [namespace, setNamespace] = useState('');
  const [kinds, setKinds] = useState<string[] | null>(null);
  const [kind, setKind] = useState<string | null>(null);
  const [entities, setEntities] = useState<DsEntity[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    infraApi.gcp.datastoreKinds(target, namespace || undefined).then(
      (k) => (setKinds(k), setError(null)),
      (e) => setError(errorMessage(e)),
    );
  }, [target, namespace]);
  useEffect(() => {
    if (!kind) return;
    setEntities(null);
    infraApi.gcp.datastoreEntities(target, kind, namespace || undefined, 200).then(setEntities, (e) => setError(errorMessage(e)));
  }, [target, kind, namespace]);
  if (error) return <Failure error={error} />;
  return (
    <div className="split">
      <SidePane className="gcp-side">
        <div className="toolbar">
          <input className="input sm" value={namespace} onChange={(e) => setNamespace(e.target.value)} placeholder="namespace (default)" aria-label="Namespace" />
        </div>
        <div className="list">
          {!kinds ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : kinds.length === 0 ? (
            <div className="p-4 xs subtle">No kinds.</div>
          ) : (
            kinds.map((k) => (
              <div key={k} className={clsx('list-item', kind === k && 'on')} onClick={() => setKind(k)}>
                <span className="grow truncate">{k}</span>
              </div>
            ))
          )}
        </div>
      </SidePane>
      <div className="pane fill">
        {!kind ? (
          <Empty title="Pick a kind" />
        ) : !entities ? (
          <div className="empty">
            <Spinner />
          </div>
        ) : (
          <CodeEditor value={JSON.stringify(entities, null, 2)} language="json" readOnly path={`inmemory://infra/ds-${kind}.json`} />
        )}
      </div>
    </div>
  );
}

// ---------- Storage ----------

function StoragePanel({ target }: { target: GcpTarget }) {
  const [buckets, setBuckets] = useState<Bucket[] | null>(null);
  const [bucket, setBucket] = useState<string | null>(null);
  const [prefix, setPrefix] = useState('');
  const [list, setList] = useState<ObjectList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ name: string; text: string } | null>(null);
  const [q, setQ] = useState('');

  useEffect(() => {
    infraApi.gcp.buckets(target).then(
      (b) => (setBuckets(b), setError(null)),
      (e) => setError(errorMessage(e)),
    );
  }, [target]);
  const loadObjects = useCallback(async () => {
    if (!bucket) return;
    setList(null);
    try {
      setList(await infraApi.gcp.objects(target, bucket, prefix));
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [target, bucket, prefix]);
  useEffect(() => {
    void loadObjects();
  }, [loadObjects]);

  const download = async (name: string) => {
    const dest = await saveDialog({ defaultPath: name.split('/').pop() });
    if (!dest || !bucket) return;
    try {
      const n = await infraApi.gcp.download(target, bucket, name, dest);
      toast.success(`Downloaded ${fmtBytes(n)}`, dest);
    } catch (e) {
      toast.error('Download failed', errorMessage(e));
    }
  };
  const showPreview = async (name: string) => {
    if (!bucket) return;
    try {
      setPreview({ name, text: await infraApi.gcp.preview(target, bucket, name) });
    } catch (e) {
      toast.error('Preview failed', errorMessage(e));
    }
  };
  const upload = async () => {
    if (!bucket) return;
    const picked = await openDialog({ multiple: false });
    if (!picked || Array.isArray(picked)) return;
    const base = String(picked).split(/[\\/]/).pop() ?? 'upload';
    const name = await promptDialog({ title: 'Upload as', label: 'Object name', initial: `${prefix}${base}` });
    if (!name) return;
    try {
      await infraApi.gcp.upload(target, bucket, name, String(picked));
      toast.success(`Uploaded ${name}`);
      void loadObjects();
    } catch (e) {
      toast.error('Upload failed', errorMessage(e));
    }
  };

  if (error) return <Failure error={error} />;
  const crumbs = prefix.split('/').filter(Boolean);
  return (
    <div className="split">
      <SidePane className="gcp-side">
        <div className="toolbar">
          <span className="strong small grow">Buckets</span>
        </div>
        <div className="list">
          {!buckets ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : buckets.length === 0 ? (
            <div className="p-4 xs subtle">No buckets.</div>
          ) : (
            buckets.map((b) => (
              <div key={b.name} className={clsx('list-item', bucket === b.name && 'on')} onClick={() => (setBucket(b.name), setPrefix(''), setPreview(null))}>
                <span className="grow truncate">{b.name}</span>
                {b.location && <span className="meta">{b.location}</span>}
              </div>
            ))
          )}
        </div>
      </SidePane>
      <div className="pane fill">
        {!bucket ? (
          <Empty title="Pick a bucket">Works with fake-gcs-server (or another GCS emulator) and, read-only by default, with real buckets.</Empty>
        ) : (
          <div className="gcp-panel">
            <div className="toolbar">
              <div className="breadcrumbs small">
                <button className="btn btn-sm btn-ghost" onClick={() => setPrefix('')}>
                  {bucket}
                </button>
                {crumbs.map((c, i) => (
                  <span key={i} className="row-sm">
                    /
                    <button className="btn btn-sm btn-ghost mono" onClick={() => setPrefix(`${crumbs.slice(0, i + 1).join('/')}/`)}>
                      {c}
                    </button>
                  </span>
                ))}
              </div>
              <SearchInput value={q} onChange={setQ} placeholder="Filter…" />
              <span className="grow" />
              <button className="btn btn-sm" onClick={upload} title={target.emulator || target.allowWrites ? 'Upload a file' : 'Enable writes to upload to a real bucket'} disabled={!target.emulator && !target.allowWrites}>
                <ArrowUpFromLine /> Upload
              </button>
              <button className="icon-btn sm" onClick={loadObjects} title="Refresh" aria-label="Refresh">
                <RefreshCw />
              </button>
            </div>
            {!list ? (
              <div className="empty">
                <Spinner />
              </div>
            ) : (
              <div className="scroll grow">
                <table className="table infra-table">
                  <thead>
                    <tr>
                      <th>Name</th>
                      <th>Size</th>
                      <th>Type</th>
                      <th>Updated</th>
                      <th style={{ width: 70 }} />
                    </tr>
                  </thead>
                  <tbody>
                    {list.prefixes
                      .filter((p) => !q || p.toLowerCase().includes(q.toLowerCase()))
                      .map((p) => (
                        <tr key={p} className="clickable" onClick={() => setPrefix(p)}>
                          <td className="small">
                            <Folder size={13} className="subtle" /> {p.slice(prefix.length)}
                          </td>
                          <td />
                          <td />
                          <td />
                          <td />
                        </tr>
                      ))}
                    {list.objects
                      .filter((o) => !q || o.name.toLowerCase().includes(q.toLowerCase()))
                      .map((o) => (
                        <tr key={o.name}>
                          <td className="mono xs">{o.name.slice(prefix.length)}</td>
                          <td className="small nowrap">{fmtBytes(o.size)}</td>
                          <td className="xs subtle">{o.contentType}</td>
                          <td className="xs subtle nowrap">{o.updated?.replace('T', ' ').slice(0, 19)}</td>
                          <td>
                            <div className="row-actions">
                              <button className="icon-btn sm" title="Preview" aria-label="Preview" onClick={() => showPreview(o.name)}>
                                <Eye />
                              </button>
                              <button className="icon-btn sm" title="Download…" aria-label="Download" onClick={() => download(o.name)}>
                                <Download />
                              </button>
                            </div>
                          </td>
                        </tr>
                      ))}
                  </tbody>
                </table>
                {list.nextPageToken && <div className="xs subtle p-4">More objects exist; narrow with a folder.</div>}
              </div>
            )}
            {preview && (
              <div className="storage-preview">
                <div className="toolbar">
                  <span className="mono small strong grow truncate">{preview.name}</span>
                  <button className="icon-btn sm" onClick={() => setPreview(null)} aria-label="Close preview">
                    ×
                  </button>
                </div>
                <CodeEditor value={preview.text} language={preview.name.endsWith('.json') ? 'json' : preview.name.match(/\.ya?ml$/) ? 'yaml' : 'plaintext'} readOnly path={`inmemory://infra/gcs-${preview.name}`} />
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

// ---------- Logging ----------

const SEV_CLASS: Record<string, string> = { ERROR: 'text-danger', CRITICAL: 'text-danger', ALERT: 'text-danger', EMERGENCY: 'text-danger', WARNING: 'text-warning' };

function LoggingPanel({ target }: { target: GcpTarget }) {
  const [filter, setFilter] = useState('severity>=WARNING');
  const [applied, setApplied] = useState(filter);
  const [entries, setEntries] = useState<LogEntryRow[]>([]);
  const [tailing, setTailing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [mode, setMode] = useState<'tail' | 'query'>('tail');
  const openInViewer = useOpenInLogViewer();

  useEffect(() => {
    let live = true;
    setEntries([]);
    setError(null);
    if (mode === 'query') {
      infraApi.gcp.logs(target, applied, 300).then(
        (e) => live && setEntries(e),
        (e) => live && setError(errorMessage(e)),
      );
      return () => {
        live = false;
      };
    }
    if (!tailing) return;
    const id = `logging-${uid()}`;
    infraApi.gcp
      .logsTail(id, target, applied, (m) => {
        if (!live) return;
        if (m.type === 'error') setError(m.message);
        else if (m.entries.length) {
          setError(null);
          setEntries((prev) => [...prev, ...m.entries].slice(-5000));
        }
      })
      .catch((e) => live && setError(errorMessage(e)));
    return () => {
      live = false;
      void infraApi.streamStop(id);
    };
  }, [target, applied, tailing, mode]);

  const rows = mode === 'query' ? entries : entries.slice().reverse();

  return (
    <div className="gcp-panel">
      <div className="toolbar">
        <Segmented
          value={mode}
          onChange={setMode}
          options={[
            { value: 'tail', label: 'Live tail' },
            { value: 'query', label: 'Query' },
          ]}
        />
        <input
          className="input sm mono grow"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && setApplied(filter)}
          placeholder='resource.type="k8s_container" AND severity>=ERROR'
          aria-label="Logging filter"
        />
        <button className="btn btn-sm" onClick={() => setApplied(filter)}>
          Apply
        </button>
        {mode === 'tail' && (
          <button className={clsx('btn btn-sm', tailing ? 'btn-subtle' : 'btn-primary')} onClick={() => setTailing(!tailing)}>
            {tailing ? <Pause /> : <Play />} {tailing ? 'Stop' : 'Start tail'}
          </button>
        )}
        <button
          className="btn btn-sm"
          disabled={!entries.length}
          onClick={() => openInViewer({ name: `Cloud Logging ${target.project}`, text: entries.map((e) => JSON.stringify({ ts: e.ts, level: e.severity, msg: e.text, resource: e.resource, log: e.log, ...(e.json && typeof e.json === 'object' ? { payload: e.json } : {}) })).join('\n') })}
        >
          <ScrollText /> Open in log viewer
        </button>
      </div>
      {error && <div className="log-error small break">{error}</div>}
      {rows.length === 0 ? (
        <Empty title={mode === 'tail' ? (tailing ? 'Waiting for entries…' : 'Start a live tail') : 'No entries'}>
          {mode === 'tail' ? 'Polls entries:list every 3 s with your gcloud credentials (read-only), starting one minute back.' : 'Newest 300 entries matching the filter.'}
        </Empty>
      ) : (
        <div className="scroll grow">
          <table className="table infra-table logging-table">
            <tbody>
              {rows.map((e) => (
                <tr key={e.insertId} className={clsx('clickable', open === e.insertId && 'on')} onClick={() => setOpen(open === e.insertId ? null : e.insertId)}>
                  <td className="xs subtle nowrap">{e.ts?.replace('T', ' ').slice(0, 23)}</td>
                  <td className={clsx('xs strong nowrap', SEV_CLASS[e.severity])}>{e.severity}</td>
                  <td className="xs subtle nowrap truncate cell-max">{e.resource}</td>
                  <td className={clsx('mono xs', open === e.insertId ? 'pre-wrap' : 'truncate cell-msg')}>{open === e.insertId && e.json ? JSON.stringify(e.json, null, 2) : e.text}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

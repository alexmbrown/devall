// Typed wrappers + TS mirrors for the infra commands in src-tauri/src/modules/infra.rs
// (crate: crates/devall-infra). Keep in sync with the Rust types.
import { Channel, invoke } from '@tauri-apps/api/core';

// ---------- shared streams ----------

export type LogStreamKind = 'stdout' | 'stderr';
export interface LogLine {
  ts?: string;
  stream: LogStreamKind;
  text: string;
}
export type LogMsg = { type: 'lines'; lines: LogLine[] } | { type: 'end'; error: string | null };

export type ExecEvent = { type: 'output'; data: string } | { type: 'exit'; code: number | null; error: string | null };

// ---------- Docker ----------

export interface SocketCandidate {
  host: string;
  source: string;
  exists: boolean;
}

export interface EngineInfo {
  host: string;
  source: string;
  engine: string;
  version?: string;
  apiVersion?: string;
  os?: string;
  arch?: string;
}

export interface PortMapping {
  privatePort: number;
  publicPort?: number;
  ip?: string;
  protocol: string;
  url?: string;
}

export interface ContainerRow {
  id: string;
  shortId: string;
  name: string;
  image: string;
  command?: string;
  created?: number;
  state: string;
  status: string;
  health?: string;
  ports: PortMapping[];
  composeProject?: string;
  composeService?: string;
  networks: string[];
}

export interface ImageRow {
  id: string;
  shortId: string;
  tags: string[];
  created: number;
  size: number;
  containers: number;
}

export interface VolumeRow {
  name: string;
  driver: string;
  mountpoint: string;
  created?: string;
  composeProject?: string;
}

export interface NetworkRow {
  id: string;
  name: string;
  driver?: string;
  scope?: string;
  internal: boolean;
  composeProject?: string;
}

export interface ComposeProject {
  name: string;
  workingDir?: string;
  configFiles: string[];
  services: string[];
  containers: number;
  running: number;
}

export type ComposeAction = 'up' | 'down' | 'stop' | 'start' | 'restart' | 'pull';

export interface DockerLogOptions {
  follow?: boolean;
  tail?: number;
  timestamps?: boolean;
  sinceSeconds?: number;
}

export interface StatsSample {
  ts: number;
  cpuPercent: number;
  onlineCpus: number;
  memUsed: number;
  memLimit: number;
  memPercent: number;
  netRx: number;
  netTx: number;
  blockRead: number;
  blockWrite: number;
  pids: number;
}

// ---------- Kubernetes ----------

export interface KubeOptions {
  kubeconfig: string[];
  wslFallback: boolean;
}

export type AuthSummary =
  | { type: 'none' }
  | { type: 'token' }
  | { type: 'clientCertificate' }
  | { type: 'basic' }
  | { type: 'exec'; command: string; args: string[]; resolved?: string; found: boolean; installHint?: string }
  | { type: 'authProvider'; name: string };

export interface ContextInfo {
  name: string;
  cluster: string;
  user?: string;
  namespace?: string;
  server?: string;
  auth: AuthSummary;
  current: boolean;
}

export interface KubeContexts {
  current?: string;
  contexts: ContextInfo[];
  files: string[];
}

export interface ClusterInfo {
  context: string;
  defaultNamespace: string;
  version: string;
  platform: string;
}

export interface KindRef {
  group: string;
  version: string;
  kind: string;
  plural: string;
  namespaced: boolean;
}

export interface KindInfo extends KindRef {
  category: string;
}

export type Health = 'ok' | 'warn' | 'err' | 'idle';

export interface PortInfo {
  port: number;
  name?: string;
  protocol: string;
}

export interface ResourceRow {
  uid: string;
  name: string;
  namespace?: string;
  created?: string;
  cells: string[];
  health: Health;
  containers: string[];
  ports: PortInfo[];
}

export interface ResourceList {
  columns: string[];
  rows: ResourceRow[];
}

export type WatchMsg =
  | { type: 'reset'; columns: string[]; rows: ResourceRow[] }
  | { type: 'upsert'; row: ResourceRow }
  | { type: 'delete'; uid: string }
  | { type: 'error'; message: string };

export interface SecretEntry {
  key: string;
  value?: string;
  size: number;
}

export interface ConditionRow {
  type: string;
  status: string;
  reason?: string;
  message?: string;
  lastTransition?: string;
}

export interface ContainerInfo {
  name: string;
  image: string;
  ready: boolean;
  restarts: number;
  state: string;
  reason?: string;
  message?: string;
  init: boolean;
}

export interface EventRow {
  type: string;
  reason: string;
  message: string;
  count: number;
  lastSeen?: string;
  source?: string;
}

export interface Describe {
  kind: string;
  name: string;
  namespace?: string;
  uid: string;
  created?: string;
  labels: Record<string, string>;
  annotations: Record<string, string>;
  owners: string[];
  fields: [string, string][];
  conditions: ConditionRow[];
  containers: ContainerInfo[];
  events: EventRow[];
}

export interface PodLogOptions {
  container?: string;
  follow?: boolean;
  previous?: boolean;
  tail?: number;
  timestamps?: boolean;
  sinceSeconds?: number;
}

// ---------- port forwards (connections/*.forward.yaml) ----------

export type ForwardSource = 'kubernetes' | 'docker';
export type TargetKind = 'pod' | 'service' | 'deployment' | 'statefulset' | 'daemonset' | 'replicaset' | 'container';

export interface EnvBinding {
  /** Environment id; omitted = whichever environment is active. */
  environment?: string;
  variable: string;
  /** `{port}`, `{host}`, `{remotePort}`, `{name}`, `{namespace}`, `{target}`, `{context}`. */
  template?: string;
}

export interface ForwardFile {
  version?: number;
  name: string;
  source?: ForwardSource;
  context?: string;
  namespace?: string;
  target: { kind: TargetKind; name: string };
  remotePort: number;
  localPort?: number;
  bindEnvironment?: EnvBinding;
  [extra: string]: unknown;
}

export interface ForwardEntry {
  id: string;
  path: string;
  file?: ForwardFile;
  error?: string;
}

export type ForwardState = 'starting' | 'active' | 'reconnecting' | 'error' | 'stopped';

export interface ForwardStatus {
  id: string;
  root: string;
  name: string;
  source: ForwardSource;
  state: ForwardState;
  localPort?: number;
  url?: string;
  backend?: string;
  namespace?: string;
  context?: string;
  connections: number;
  activeConnections: number;
  reconnects: number;
  lastError?: string;
  startedAt: number;
  binding?: { environment?: string; variable: string; value: string };
  saved: boolean;
  spec: ForwardFile;
}

export interface ForwardListing {
  saved: ForwardEntry[];
  running: ForwardStatus[];
}

export interface OverrideEntry {
  root: string;
  environment?: string;
  variable: string;
  value: string;
  forwardId: string;
  forwardName: string;
  since: number;
}

// ---------- GCP ----------

/** Secret Manager secret (metadata only). */
export interface GcpSecret {
  name: string;
  fullName: string;
  createTime: string | null;
  labels: Record<string, string>;
  replication: string | null;
}

export interface GcpSecretVersion {
  name: string;
  fullName: string;
  createTime: string | null;
  state: 'ENABLED' | 'DISABLED' | 'DESTROYED' | string;
}

export interface SecretImportItem {
  secret: string;
  /** Empty / `latest` = the latest version. */
  version?: string;
  /** `{{secret.NAME}}` in the environment. */
  name: string;
}

/** `keychain`: values go to the OS keychain; `reference`: a `gcp-sm://` entry in `secretSources`. */
export type SecretImportMode = 'keychain' | 'reference';

export interface SecretImportResult {
  environment: string;
  imported: string[];
  skipped: [string, string][];
  warnings: string[];
}

export interface GcpTarget {
  project: string;
  /** Emulator host; omit for the real Google Cloud API with your gcloud login. */
  emulator?: string;
  allowWrites?: boolean;
}

export interface EmulatorStatus {
  kind: 'pubsub' | 'firestore' | 'datastore' | 'storage';
  host: string;
  fromEnv: boolean;
  reachable: boolean;
}

export interface GcpStatus {
  emulators: EmulatorStatus[];
  gcloud: { path?: string; account?: string; project?: string };
}

export interface Topic {
  name: string;
  fullName: string;
  labels: Record<string, string>;
}

export interface Subscription {
  name: string;
  fullName: string;
  topic: string;
  ackDeadlineSeconds: number;
  pushEndpoint?: string;
  filter?: string;
}

export interface PublishMessage {
  data: string;
  base64?: boolean;
  attributes?: Record<string, string>;
  orderingKey?: string;
}

export interface ReceivedMessage {
  ackId: string;
  messageId: string;
  publishTime?: string;
  text?: string;
  dataBase64: string;
  size: number;
  attributes: Record<string, string>;
  orderingKey?: string;
  deliveryAttempt?: number;
}

export type PubSubTailMsg = { type: 'messages'; messages: ReceivedMessage[] } | { type: 'error'; message: string };

export interface FsDocument {
  id: string;
  path: string;
  fields: unknown;
  createTime?: string;
  updateTime?: string;
}

export interface FsPage {
  documents: FsDocument[];
  nextPageToken?: string;
}

export interface DsEntity {
  key: string;
  properties: unknown;
}

export interface Bucket {
  name: string;
  location?: string;
  storageClass?: string;
  created?: string;
}

export interface ObjectInfo {
  name: string;
  size: number;
  contentType?: string;
  updated?: string;
  md5?: string;
}

export interface ObjectList {
  prefixes: string[];
  objects: ObjectInfo[];
  nextPageToken?: string;
}

export interface LogEntryRow {
  ts?: string;
  severity: string;
  text: string;
  log?: string;
  resource?: string;
  insertId: string;
  json?: unknown;
}

export type LoggingTailMsg = { type: 'entries'; entries: LogEntryRow[] } | { type: 'error'; message: string };

// ---------- wrappers ----------

function channel<T>(onEvent: (m: T) => void): Channel<T> {
  const ch = new Channel<T>();
  ch.onmessage = onEvent;
  return ch;
}

export const infraApi = {
  // streams shared by every tab
  streamStop: (streamId: string) => invoke<boolean>('infra_stream_stop', { streamId }),
  streamStopAll: () => invoke<number>('infra_stream_stop_all'),
  execInput: (execId: string, data: string) => invoke<boolean>('infra_exec_input', { execId, data }),
  execResize: (execId: string, cols: number, rows: number) => invoke<boolean>('infra_exec_resize', { execId, cols, rows }),

  docker: {
    candidates: () => invoke<SocketCandidate[]>('docker_candidates'),
    connect: (host?: string) => invoke<EngineInfo>('docker_connect', { host }),
    info: () => invoke<EngineInfo>('docker_info'),
    containers: (all = true) => invoke<ContainerRow[]>('docker_containers', { all }),
    images: () => invoke<ImageRow[]>('docker_images'),
    volumes: () => invoke<VolumeRow[]>('docker_volumes'),
    networks: () => invoke<NetworkRow[]>('docker_networks'),
    composeProjects: () => invoke<ComposeProject[]>('docker_compose_projects'),
    inspect: (id: string) => invoke<unknown>('docker_inspect', { id }),
    start: (id: string) => invoke<void>('docker_start', { id }),
    stop: (id: string) => invoke<void>('docker_stop', { id }),
    restart: (id: string) => invoke<void>('docker_restart', { id }),
    remove: (id: string, force = false) => invoke<void>('docker_remove', { id, force }),
    compose: (project: ComposeProject, action: ComposeAction) => invoke<string>('docker_compose', { project, action }),
    logs: (streamId: string, id: string, options: DockerLogOptions, onEvent: (m: LogMsg) => void) =>
      invoke<void>('docker_logs', { streamId, id, options, onEvent: channel(onEvent) }),
    stats: (streamId: string, id: string, onEvent: (s: StatsSample) => void) => invoke<void>('docker_stats', { streamId, id, onEvent: channel(onEvent) }),
    exec: (execId: string, id: string, onEvent: (e: ExecEvent) => void, command?: string[], tty = true) =>
      invoke<void>('docker_exec', { execId, id, command, tty, onEvent: channel(onEvent) }),
  },

  kube: {
    options: () => invoke<KubeOptions>('kube_options'),
    setOptions: (options: KubeOptions) => invoke<void>('kube_set_options', { options }),
    contexts: () => invoke<KubeContexts>('kube_contexts'),
    test: (context: string) => invoke<ClusterInfo>('kube_test', { context }),
    namespaces: (context: string) => invoke<string[]>('kube_namespaces', { context }),
    kinds: (context: string, custom = false) => invoke<KindInfo[]>('kube_kinds', { context, custom }),
    list: (context: string, kind: KindRef, namespace?: string, labelSelector?: string) =>
      invoke<ResourceList>('kube_list', { context, kind, namespace, labelSelector }),
    watch: (streamId: string, context: string, kind: KindRef, namespace: string | undefined, onEvent: (m: WatchMsg) => void, labelSelector?: string) =>
      invoke<void>('kube_watch', { streamId, context, kind, namespace, labelSelector, onEvent: channel(onEvent) }),
    yaml: (context: string, kind: KindRef, namespace: string | undefined, name: string, reveal = false) =>
      invoke<string>('kube_yaml', { context, kind, namespace, name, reveal }),
    secretValues: (context: string, namespace: string, name: string) => invoke<SecretEntry[]>('kube_secret_values', { context, namespace, name }),
    describe: (context: string, kind: KindRef, namespace: string | undefined, name: string) => invoke<Describe>('kube_describe', { context, kind, namespace, name }),
    delete: (context: string, kind: KindRef, namespace: string | undefined, name: string) => invoke<void>('kube_delete', { context, kind, namespace, name }),
    restart: (context: string, kind: KindRef, namespace: string | undefined, name: string) => invoke<void>('kube_restart', { context, kind, namespace, name }),
    scale: (context: string, kind: KindRef, namespace: string | undefined, name: string, replicas: number) =>
      invoke<void>('kube_scale', { context, kind, namespace, name, replicas }),
    logs: (streamId: string, context: string, namespace: string, pod: string, options: PodLogOptions, onEvent: (m: LogMsg) => void) =>
      invoke<void>('kube_logs', { streamId, context, namespace, pod, options, onEvent: channel(onEvent) }),
    exec: (execId: string, context: string, namespace: string, pod: string, container: string | undefined, onEvent: (e: ExecEvent) => void, command?: string[], tty = true) =>
      invoke<void>('kube_exec', { execId, context, namespace, pod, container, command, tty, onEvent: channel(onEvent) }),
  },

  forward: {
    list: (root: string) => invoke<ForwardListing>('forward_list', { root }),
    /** A saved forward by id, or an ad-hoc spec (or a saved id run with an edited spec). */
    start: (root: string, id: string | undefined, spec?: ForwardFile) => invoke<ForwardStatus>('forward_start', { root, id, spec }),
    stop: (root: string, id: string) => invoke<boolean>('forward_stop', { root, id }),
    /** Creates (no id) or overwrites `connections/<id>.forward.yaml`; returns the id. */
    save: (root: string, id: string | undefined, spec: ForwardFile, runningId?: string) => invoke<string>('forward_save', { root, id, spec, runningId }),
    delete: (root: string, id: string) => invoke<void>('forward_delete', { root, id }),
    activeOverrides: (root: string, env?: string) => invoke<OverrideEntry[]>('forward_active_overrides', { root, env }),
    saveToEnvironment: (root: string, id: string, envId?: string) => invoke<string>('forward_save_to_environment', { root, id, envId }),
  },

  gcp: {
    status: () => invoke<GcpStatus>('gcp_status'),
    topics: (target: GcpTarget) => invoke<Topic[]>('gcp_pubsub_topics', { target }),
    subscriptions: (target: GcpTarget) => invoke<Subscription[]>('gcp_pubsub_subscriptions', { target }),
    createTopic: (target: GcpTarget, name: string) => invoke<Topic>('gcp_pubsub_create_topic', { target, name }),
    deleteTopic: (target: GcpTarget, name: string) => invoke<void>('gcp_pubsub_delete_topic', { target, name }),
    createSubscription: (target: GcpTarget, name: string, topic: string, ackDeadlineSeconds?: number) =>
      invoke<Subscription>('gcp_pubsub_create_subscription', { target, name, topic, ackDeadlineSeconds }),
    deleteSubscription: (target: GcpTarget, name: string) => invoke<void>('gcp_pubsub_delete_subscription', { target, name }),
    publish: (target: GcpTarget, topic: string, messages: PublishMessage[]) => invoke<string[]>('gcp_pubsub_publish', { target, topic, messages }),
    pull: (target: GcpTarget, subscription: string, max = 20, ack = false) => invoke<ReceivedMessage[]>('gcp_pubsub_pull', { target, subscription, max, ack }),
    tail: (streamId: string, target: GcpTarget, subscription: string, ack: boolean, onEvent: (m: PubSubTailMsg) => void) =>
      invoke<void>('gcp_pubsub_tail', { streamId, target, subscription, ack, onEvent: channel(onEvent) }),
    firestoreCollections: (target: GcpTarget, parent = '', database?: string) => invoke<string[]>('gcp_firestore_collections', { target, database, parent }),
    firestoreDocuments: (target: GcpTarget, collection: string, pageToken?: string, pageSize = 50, database?: string) =>
      invoke<FsPage>('gcp_firestore_documents', { target, database, collection, pageSize, pageToken }),
    datastoreKinds: (target: GcpTarget, namespace?: string) => invoke<string[]>('gcp_datastore_kinds', { target, namespace }),
    datastoreEntities: (target: GcpTarget, kind: string, namespace?: string, limit = 100) => invoke<DsEntity[]>('gcp_datastore_entities', { target, namespace, kind, limit }),
    buckets: (target: GcpTarget) => invoke<Bucket[]>('gcp_storage_buckets', { target }),
    objects: (target: GcpTarget, bucket: string, prefix = '', pageToken?: string) => invoke<ObjectList>('gcp_storage_objects', { target, bucket, prefix, pageToken }),
    download: (target: GcpTarget, bucket: string, object: string, dest: string) => invoke<number>('gcp_storage_download', { target, bucket, object, dest }),
    preview: (target: GcpTarget, bucket: string, object: string) => invoke<string>('gcp_storage_preview', { target, bucket, object }),
    upload: (target: GcpTarget, bucket: string, object: string, src: string) => invoke<ObjectInfo>('gcp_storage_upload', { target, bucket, object, src }),
    logs: (target: GcpTarget, filter?: string, limit = 200) => invoke<LogEntryRow[]>('gcp_logging_list', { target, filter, limit }),
    secrets: (target: GcpTarget) => invoke<GcpSecret[]>('gcp_secrets', { target }),
    secretVersions: (target: GcpTarget, secret: string) => invoke<GcpSecretVersion[]>('gcp_secret_versions', { target, secret }),
    /** Imports into an environment; values (keychain mode) never come back to the UI. */
    secretsImport: (root: string, envId: string, target: GcpTarget, items: SecretImportItem[], mode: SecretImportMode) =>
      invoke<SecretImportResult>('gcp_secrets_import', { root, envId, target, items, mode }),
    logsTail: (streamId: string, target: GcpTarget, filter: string, onEvent: (m: LoggingTailMsg) => void, everySecs = 3) =>
      invoke<void>('gcp_logging_tail', { streamId, target, filter, everySecs, onEvent: channel(onEvent) }),
  },
};

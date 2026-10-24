import { Channel, invoke } from '@tauri-apps/api/core';
import type { HttpRequestFile, KeyValue } from '@/lib/types';

// Mirrors of crates/devall-proto (model.rs, event.rs, grpc, graphql) and
// apps/desktop/src-tauri/src/modules/proto.rs. Keep in sync.

export type ProtoKind = 'ws' | 'grpc' | 'mqtt' | 'socketio';
export type MessageFormat = 'text' | 'json' | 'binary-base64';

export interface SavedMessage {
  name: string;
  body: string;
  format?: MessageFormat; // omitted = text
}

interface FileBase {
  version?: number;
  id: string;
  name: string;
  docs?: string;
  [extra: string]: unknown;
}

export interface WsSettings {
  verifyTls: boolean;
  connectTimeoutMs: number;
  /** 0 = off */
  pingIntervalMs: number;
}

/** `http/**\/*.ws.yaml` */
export interface WsFile extends FileBase {
  url: string;
  headers?: KeyValue[];
  subprotocols?: string[];
  messages?: SavedMessage[];
  settings?: WsSettings;
}

/** `http/**\/*.grpc.yaml` */
export interface GrpcFile extends FileBase {
  /** grpc:// or http:// = plaintext, grpcs:// or https:// = TLS */
  url: string;
  service: string;
  method: string;
  /** JSON; an array sends several messages on client-streaming methods. */
  message: string;
  metadata?: KeyValue[];
  /** omitted = true */
  useReflection?: boolean;
  protoFiles?: string[];
  importPaths?: string[];
  insecure?: boolean;
  deadlineMs?: number;
  messages?: SavedMessage[];
}

export type MqttVersion = '3.1.1' | '5';

export interface MqttSubscription {
  topic: string;
  qos?: number;
  enabled?: boolean; // omitted = true
}

export interface MqttPublish {
  name: string;
  topic: string;
  payload: string;
  format?: MessageFormat;
  qos?: number;
  retain?: boolean;
}

export interface MqttLastWill {
  topic: string;
  payload: string;
  qos?: number;
  retain?: boolean;
}

/** `http/**\/*.mqtt.yaml` */
export interface MqttFile extends FileBase {
  brokerUrl: string;
  clientId?: string;
  username?: string;
  password?: string;
  /** seconds; omitted = 60 */
  keepAlive?: number;
  cleanSession?: boolean;
  mqttVersion?: MqttVersion;
  subscriptions?: MqttSubscription[];
  publishes?: MqttPublish[];
  lastWill?: MqttLastWill;
  verifyTls?: boolean;
  connectTimeoutMs?: number;
}

export interface SioEmit {
  name: string;
  event: string;
  /** JSON; an array is spread into arguments. */
  args: string;
  ack?: boolean;
}

export interface SioSettings {
  verifyTls: boolean;
  connectTimeoutMs: number;
}

/** `http/**\/*.sio.yaml` */
export interface SioFile extends FileBase {
  /** Its path is the namespace, e.g. http://localhost:3000/admin */
  url: string;
  path?: string;
  /** JSON object */
  auth?: string;
  headers?: KeyValue[];
  query?: KeyValue[];
  listen?: string[];
  emits?: SioEmit[];
  settings?: SioSettings;
}

export type ProtoFile = WsFile | GrpcFile | MqttFile | SioFile;

export type Direction = 'in' | 'out';

export type ProtoEvent =
  | { type: 'connecting'; at: number; target: string }
  | { type: 'opened'; at: number; detail?: string; headers?: [string, string][] }
  | {
      type: 'message';
      at: number;
      direction: Direction;
      /** event (Socket.IO), topic (MQTT), Service/Method (gRPC) */
      label?: string;
      /** text, or base64 when `binary` */
      body: string;
      binary: boolean;
      size: number;
      meta?: Record<string, unknown>;
    }
  | { type: 'metadata'; at: number; trailers: boolean; entries: [string, string][] }
  | { type: 'info'; at: number; message: string }
  | { type: 'error'; at: number; message: string }
  | { type: 'closed'; at: number; code?: number; reason?: string };

export interface MethodInfo {
  name: string;
  /** /package.Service/Method */
  path: string;
  clientStreaming: boolean;
  serverStreaming: boolean;
  inputType: string;
  outputType: string;
  /** Example request JSON with every field. */
  template: string;
}

export interface ServiceInfo {
  name: string;
  methods: MethodInfo[];
}

export interface GrpcDescription {
  source: 'reflection' | 'protoFiles';
  services: ServiceInfo[];
  warnings: string[];
}

export interface GrpcStatus {
  code: number;
  codeName: string;
  message: string;
  details?: unknown;
  headers: [string, string][];
  trailers: [string, string][];
  messagesSent: number;
  messagesReceived: number;
  durationMs: number;
}

export type GqlOperationKind = 'query' | 'mutation' | 'subscription';

export interface GqlInputValue {
  name: string;
  description?: string;
  typeRef: string;
  namedType: string;
  defaultValue?: string;
}

export interface GqlField {
  name: string;
  description?: string;
  args?: GqlInputValue[];
  typeRef: string;
  namedType: string;
  deprecated?: boolean;
  deprecationReason?: string;
}

export interface GqlType {
  name: string;
  kind: 'OBJECT' | 'INTERFACE' | 'UNION' | 'ENUM' | 'INPUT_OBJECT' | 'SCALAR';
  description?: string;
  fields?: GqlField[];
  inputFields?: GqlInputValue[];
  enumValues?: { name: string; description?: string; deprecated?: boolean }[];
  interfaces?: string[];
  possibleTypes?: string[];
}

export interface GqlOperation {
  kind: GqlOperationKind;
  field: string;
  description?: string;
  typeRef: string;
  deprecated?: boolean;
  query: string;
  /** JSON (may be empty) */
  variables: string;
}

export interface GraphqlSchema {
  queryType: string | null;
  mutationType: string | null;
  subscriptionType: string | null;
  types: GqlType[];
  operations: GqlOperation[];
}

export interface SessionInfo {
  id: string;
  kind: ProtoKind;
  root: string;
  path: string;
  open: boolean;
}

function channel(onEvent: (e: ProtoEvent) => void) {
  const ch = new Channel<ProtoEvent>();
  ch.onmessage = onEvent;
  return ch;
}

/** Typed wrappers over src-tauri/src/modules/proto.rs. Errors reject with a string. */
export const protoApi = {
  read: <T extends ProtoFile = ProtoFile>(root: string, path: string) => invoke<T>('proto_read', { root, path }),
  save: (root: string, path: string, item: ProtoFile) => invoke<void>('proto_save', { root, path, item }),
  create: (root: string, folder: string, kind: ProtoKind, name: string) => invoke<string>('proto_create', { root, folder, kind, name }),
  sessions: () => invoke<SessionInfo[]>('proto_sessions'),
  close: (session: string) => invoke<boolean>('proto_close', { session }),

  wsConnect: (root: string, path: string, item: WsFile, env: string | undefined, onEvent: (e: ProtoEvent) => void) =>
    invoke<string>('ws_connect', { root, path, item, env, onEvent: channel(onEvent) }),
  wsSend: (session: string, body: string, format: MessageFormat = 'text') => invoke<void>('ws_send', { session, message: { body, format } }),
  wsClose: (session: string, code?: number, reason?: string) => invoke<void>('ws_close', { session, code, reason }),

  grpcDescribe: (root: string, path: string, item: GrpcFile, env: string | undefined) => invoke<GrpcDescription>('grpc_describe', { root, path, item, env }),
  grpcCall: (callId: string, root: string, path: string, item: GrpcFile, env: string | undefined, keepOpen: boolean, onEvent: (e: ProtoEvent) => void) =>
    invoke<GrpcStatus>('grpc_call', { callId, root, path, item, env, keepOpen, onEvent: channel(onEvent) }),
  grpcSend: (callId: string, message: string) => invoke<void>('grpc_send', { callId, message }),
  grpcEnd: (callId: string) => invoke<void>('grpc_end', { callId }),
  grpcCancel: (callId: string) => invoke<boolean>('grpc_cancel', { callId }),

  mqttConnect: (root: string, path: string, item: MqttFile, env: string | undefined, onEvent: (e: ProtoEvent) => void) =>
    invoke<string>('mqtt_connect', { root, path, item, env, onEvent: channel(onEvent) }),
  mqttPublish: (session: string, topic: string, payload: string, qos: number, retain: boolean, format: MessageFormat = 'text') =>
    invoke<void>('mqtt_publish', { session, topic, payload, qos, retain, format }),
  mqttSubscribe: (session: string, topic: string, qos: number) => invoke<void>('mqtt_subscribe', { session, topic, qos }),
  mqttUnsubscribe: (session: string, topic: string) => invoke<void>('mqtt_unsubscribe', { session, topic }),
  mqttDisconnect: (session: string) => invoke<void>('mqtt_disconnect', { session }),

  sioConnect: (root: string, path: string, item: SioFile, env: string | undefined, onEvent: (e: ProtoEvent) => void) =>
    invoke<string>('sio_connect', { root, path, item, env, onEvent: channel(onEvent) }),
  /** Resolves with the ack id when `ack` is set. */
  sioEmit: (session: string, event: string, args: string, ack: boolean) => invoke<number | null>('sio_emit', { session, event, args, ack }),
  sioClose: (session: string) => invoke<void>('sio_close', { session }),

  graphqlIntrospect: (root: string, path: string, request: HttpRequestFile, env: string | undefined) =>
    invoke<GraphqlSchema>('graphql_introspect', { root, path, request, env }),
};

/** Protocol of a file under `http/` by extension (null for HTTP requests and unknown files). */
export function protoKindOf(path: string): ProtoKind | null {
  if (path.endsWith('.ws.yaml')) return 'ws';
  if (path.endsWith('.grpc.yaml')) return 'grpc';
  if (path.endsWith('.mqtt.yaml')) return 'mqtt';
  if (path.endsWith('.sio.yaml')) return 'socketio';
  return null;
}

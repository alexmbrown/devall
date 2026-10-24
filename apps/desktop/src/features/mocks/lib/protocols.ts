/**
 * Types and helpers for the built-in engine's non-HTTP mocks (crates/devall-mock: graphql/, ws.rs,
 * grpc.rs, mqtt.rs, messages.rs) and their admin API under `/__admin/devall/…`.
 */

export type MessageProtocol = 'websocket' | 'grpc' | 'mqtt';
export type ProtocolTab = 'graphql' | MessageProtocol;

export interface LoadError {
  file: string;
  error: string;
}

/** One frame / message / connection event (mirrors devall_mock::MessageEvent). */
export interface MessageEvent {
  seq: number;
  at: number;
  protocol: MessageProtocol;
  connection: string;
  direction: 'in' | 'out' | 'event';
  channel: string;
  body: string;
  binary: boolean;
  stub: string | null;
  meta: Record<string, unknown> | null;
}

export interface SchemaInfo {
  name: string;
  endpoint: string;
  files: string[];
  seed: number;
  listLength: number;
  summary: { types: number; query: string[]; mutation: string[]; subscription: string[] };
}

export interface GraphqlStatus {
  schemas: SchemaInfo[];
  errors: LoadError[];
}

export interface WsConnection {
  id: string;
  path: string;
  stubId: string;
  stubName: string | null;
  opened: number;
  clientIp: string;
}

export interface DefinitionInfo {
  id?: string;
  name: string | null;
  file: string;
  method?: string;
  definition: unknown;
}

export interface WebsocketStatus {
  stubs: DefinitionInfo[];
  connections: WsConnection[];
  errors: LoadError[];
}

export interface GrpcMethod {
  path: string;
  clientStreaming: boolean;
  serverStreaming: boolean;
  inputType: string;
  outputType: string;
  template: string;
}

export interface GrpcStatus {
  running: boolean;
  port: number | null;
  hasProtos: boolean;
  methods: GrpcMethod[];
  stubs: DefinitionInfo[];
  errors: LoadError[];
}

export interface MqttClient {
  clientId: string;
  subscriptions: string[];
  connectedAt: number;
  peer: string;
}

export interface MqttStatus {
  running: boolean;
  port: number | null;
  hasBehaviours: boolean;
  clients: MqttClient[];
  retained: { topic: string; payload: string; binary: boolean; qos: number }[];
  behaviours: unknown[];
  errors: LoadError[];
}

export interface ProtocolsStatus {
  graphql: GraphqlStatus;
  websocket: { stubs: number; connections: number; errors: LoadError[] };
  grpc: { running: boolean; port: number | null; hasProtos: boolean; methods: number; errors: LoadError[] };
  mqtt: { running: boolean; port: number | null; hasBehaviours: boolean; clients: number; errors: LoadError[] };
}

/** Folder of each protocol under the project. */
export const PROTOCOL_DIR: Record<ProtocolTab, string> = {
  graphql: 'mocks/graphql',
  websocket: 'mocks/websocket',
  grpc: 'mocks/grpc',
  mqtt: 'mocks/mqtt',
};

const STARTERS: Record<MessageProtocol, { name: string; value: unknown }> = {
  websocket: {
    name: 'echo.json',
    value: {
      name: 'Echo',
      request: { urlPath: '/ws/echo' },
      onConnect: [{ text: 'connected' }],
      messages: [{ match: { equalTo: 'ping' }, reply: [{ text: 'pong' }] }],
      echo: true,
    },
  },
  grpc: {
    name: 'say-hello.json',
    value: {
      name: 'Say hello',
      method: 'example.v1.Greeter/SayHello',
      request: { body: { name: 'Ada' } },
      response: { message: { message: "Hello {{jsonPath request.body '$.name'}}" } },
    },
  },
  mqtt: {
    name: 'devices.json',
    value: {
      name: 'Devices',
      retained: [{ topic: 'devices/1/state', payload: 'on', qos: 1 }],
      rules: [{ topic: 'devices/+/cmd', publish: [{ topic: 'devices/{{topicSegments.[1]}}/ack', json: { ok: true } }] }],
    },
  },
};

/** Starter content for a new file of each kind. */
export function templateFor(protocol: ProtocolTab, kind: 'stub' | 'schema' | 'proto' = 'stub'): { name: string; content: string } {
  if (protocol === 'graphql')
    return {
      name: 'schema.graphql',
      content: 'type Query {\n  user(id: ID!): User\n  users(first: Int = 10): [User!]!\n}\n\ntype User {\n  id: ID!\n  name: String!\n  email: String!\n}\n',
    };
  if (protocol === 'grpc' && kind === 'proto')
    return {
      name: 'service.proto',
      content:
        'syntax = "proto3";\n\npackage example.v1;\n\nservice Greeter {\n  rpc SayHello (HelloRequest) returns (HelloReply);\n}\n\nmessage HelloRequest { string name = 1; }\nmessage HelloReply { string message = 1; }\n',
    };
  const s = STARTERS[protocol];
  return { name: s.name, content: `${JSON.stringify(s.value, null, 2)}\n` };
}

/** MQTT topic filter syntax: `+` whole levels, `#` last level only. */
export function isTopicFilter(f: string): boolean {
  if (!f) return false;
  const levels = f.split('/');
  return levels.every((l, i) => (l === '#' ? i === levels.length - 1 : l === '+' || !/[+#]/.test(l)));
}

/** Problems a definition file would have when loaded (fast checks before saving). */
export function validateDefinition(protocol: MessageProtocol, text: string): string[] {
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch (e) {
    return [`Invalid JSON: ${e instanceof Error ? e.message : String(e)}`];
  }
  const listKey = protocol === 'mqtt' ? 'behaviours' : 'stubs';
  const wrapped = v && typeof v === 'object' && !Array.isArray(v) && Array.isArray((v as Record<string, unknown>)[listKey]);
  const items: unknown[] = Array.isArray(v) ? v : wrapped ? ((v as Record<string, unknown>)[listKey] as unknown[]) : [v];
  const out: string[] = [];
  items.forEach((it, i) => {
    const at = items.length > 1 ? `#${i + 1}: ` : '';
    if (!it || typeof it !== 'object' || Array.isArray(it)) {
      out.push(`${at}expected an object`);
      return;
    }
    const o = it as Record<string, unknown>;
    if (protocol === 'websocket') {
      if (!o.request || typeof o.request !== 'object') out.push(`${at}request is required (e.g. {"urlPath": "/ws"})`);
      if (o.messages !== undefined && !Array.isArray(o.messages)) out.push(`${at}messages must be an array of rules`);
    } else if (protocol === 'grpc') {
      if (typeof o.method !== 'string' || (!o.method.includes('/') && typeof o.service !== 'string')) out.push(`${at}method must look like "package.Service/Method"`);
      const r = (o.response ?? {}) as Record<string, unknown>;
      if (r.message === undefined && r.messages === undefined && (r.status === undefined || r.status === 'OK' || r.status === 0))
        out.push(`${at}response needs a message, messages or a non-OK status`);
    } else {
      for (const key of ['rules', 'onSubscribe']) {
        const list = o[key];
        if (list === undefined) continue;
        if (!Array.isArray(list)) {
          out.push(`${at}${key} must be an array`);
          continue;
        }
        list.forEach((r: unknown, j) => {
          const t = r && typeof r === 'object' ? (r as Record<string, unknown>).topic : undefined;
          if (typeof t !== 'string' || !isTopicFilter(t)) out.push(`${at}${key}[${j}].topic must be a topic filter`);
        });
      }
    }
  });
  return out;
}

/** One-line description of a journal entry. */
export function describeMessage(m: MessageEvent): string {
  if (m.direction === 'event') {
    const meta = m.meta ?? {};
    const ev = String(meta.event ?? 'event');
    if (ev === 'status') return `status ${String(meta.statusName ?? meta.status)}${meta.message ? ` — ${String(meta.message)}` : ''}`;
    if (ev === 'close') {
      const code = meta.code !== undefined && meta.code !== null ? ` (${String(meta.code)}${meta.reason ? ` ${String(meta.reason)}` : ''})` : '';
      return `closed${code} by ${String(meta.by ?? 'peer')}`;
    }
    if (ev === 'subscribe') return `subscribed (QoS ${String(meta.qos ?? 0)})`;
    return ev;
  }
  return m.binary ? `<${Math.round((m.body.length * 3) / 4)} bytes>` : m.body;
}

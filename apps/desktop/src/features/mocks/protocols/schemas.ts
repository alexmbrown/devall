/**
 * JSON Schemas for the WebSocket / gRPC / MQTT definition files, registered with Monaco so the
 * editors validate and complete them (opt-in through the model path, like the stub editor).
 */
import { monaco } from '@/components/editor/monaco';
import type { MessageProtocol } from '@/features/mocks/lib/protocols';

const matcher = {
  description: 'A WireMock value matcher: {"equalTo"}, {"contains"}, {"matches"}, {"equalToJson"}, {"matchesJsonPath"}, … (a string means equalTo)',
  anyOf: [{ type: 'string' }, { type: 'object' }],
};

const message = {
  description: 'A frame to send: text, JSON or base64 binary (text and JSON are response templates).',
  anyOf: [
    { type: 'string' },
    {
      type: 'object',
      properties: {
        text: { type: 'string' },
        json: {},
        binary: { type: 'string', description: 'Base64' },
        delayMs: { type: 'integer', minimum: 0 },
      },
      additionalProperties: false,
    },
  ],
};

const close = {
  type: 'object',
  properties: { code: { type: 'integer', minimum: 1000, maximum: 4999 }, reason: { type: 'string' }, afterMs: { type: 'integer', minimum: 0 } },
  additionalProperties: false,
};

const wsStub = {
  type: 'object',
  required: ['request'],
  properties: {
    id: { type: 'string' },
    name: { type: 'string' },
    priority: { type: 'integer' },
    request: { type: 'object', description: 'WireMock request pattern for the upgrade request (urlPath, urlPathPattern, queryParameters, headers, …)' },
    subprotocol: { type: 'string', description: 'Sec-WebSocket-Protocol to accept ("*" = the first one offered)' },
    onConnect: { type: 'array', items: message },
    messages: {
      type: 'array',
      description: 'Rules for incoming frames; the first matching rule answers',
      items: {
        type: 'object',
        properties: { match: matcher, reply: { type: 'array', items: message }, broadcast: { type: 'boolean' }, close },
        additionalProperties: false,
      },
    },
    periodic: {
      type: 'array',
      items: {
        type: 'object',
        required: ['intervalMs', 'message'],
        properties: { intervalMs: { type: 'integer', minimum: 10 }, initialDelayMs: { type: 'integer', minimum: 0 }, count: { type: 'integer', minimum: 0 }, message },
        additionalProperties: false,
      },
    },
    close,
    echo: { type: 'boolean', description: 'Echo frames no rule matched' },
  },
  additionalProperties: false,
};

const grpcStub = {
  type: 'object',
  required: ['method'],
  properties: {
    id: { type: 'string' },
    name: { type: 'string' },
    priority: { type: 'integer' },
    method: { type: 'string', description: 'package.Service/Method' },
    service: { type: 'string' },
    request: {
      type: 'object',
      properties: {
        body: { description: 'Matcher on the request message as proto3 JSON (an object = equalToJson ignoring extra fields)' },
        metadata: { type: 'object', additionalProperties: matcher },
      },
      additionalProperties: false,
    },
    response: {
      type: 'object',
      properties: {
        message: { type: 'object' },
        messages: { type: 'array', items: { type: 'object' } },
        streamIntervalMs: { type: 'integer', minimum: 0 },
        fixedDelayMilliseconds: { type: 'integer', minimum: 0 },
        status: {
          anyOf: [
            { type: 'integer', minimum: 0, maximum: 16 },
            {
              enum: [
                'OK',
                'CANCELLED',
                'UNKNOWN',
                'INVALID_ARGUMENT',
                'DEADLINE_EXCEEDED',
                'NOT_FOUND',
                'ALREADY_EXISTS',
                'PERMISSION_DENIED',
                'RESOURCE_EXHAUSTED',
                'FAILED_PRECONDITION',
                'ABORTED',
                'OUT_OF_RANGE',
                'UNIMPLEMENTED',
                'INTERNAL',
                'UNAVAILABLE',
                'DATA_LOSS',
                'UNAUTHENTICATED',
              ],
            },
          ],
        },
        statusMessage: { type: 'string' },
        headers: { type: 'object', additionalProperties: { type: 'string' } },
        trailers: { type: 'object', additionalProperties: { type: 'string' } },
      },
      additionalProperties: false,
    },
  },
  additionalProperties: false,
};

const publish = {
  type: 'object',
  required: ['topic'],
  properties: {
    topic: { type: 'string', description: 'No wildcards; a response template' },
    payload: { type: 'string' },
    json: {},
    qos: { enum: [0, 1, 2] },
    retain: { type: 'boolean' },
    delayMs: { type: 'integer', minimum: 0 },
  },
  additionalProperties: false,
};

const mqttBehaviour = {
  type: 'object',
  properties: {
    name: { type: 'string' },
    retained: { type: 'array', items: publish },
    onSubscribe: {
      type: 'array',
      items: { type: 'object', required: ['topic'], properties: { topic: { type: 'string' }, publish: { type: 'array', items: publish } }, additionalProperties: false },
    },
    periodic: {
      type: 'array',
      items: {
        type: 'object',
        required: ['intervalMs', 'publish'],
        properties: { intervalMs: { type: 'integer', minimum: 10 }, initialDelayMs: { type: 'integer', minimum: 0 }, count: { type: 'integer', minimum: 0 }, publish },
        additionalProperties: false,
      },
    },
    rules: {
      type: 'array',
      items: {
        type: 'object',
        required: ['topic'],
        properties: { topic: { type: 'string', description: 'Topic filter (+, #)' }, payload: matcher, publish: { type: 'array', items: publish } },
        additionalProperties: false,
      },
    },
  },
  additionalProperties: false,
};

const fileOf = (item: object, listKey: string) => ({ anyOf: [item, { type: 'array', items: item }, { type: 'object', required: [listKey], properties: { [listKey]: { type: 'array', items: item } } }] });

export const DEFINITION_SCHEMAS: Record<MessageProtocol, object> = {
  websocket: fileOf(wsStub, 'stubs'),
  grpc: fileOf(grpcStub, 'stubs'),
  mqtt: fileOf(mqttBehaviour, 'behaviours'),
};

/** The `graphql` request matcher (a /dev/all extension of WireMock request patterns). */
export const GRAPHQL_MATCHER_SCHEMA = {
  type: 'object',
  description: '/dev/all: match GraphQL requests (operationName, operationType, root field, query text, variables)',
  properties: {
    operationName: matcher,
    operationType: { enum: ['query', 'mutation', 'subscription'] },
    field: matcher,
    query: matcher,
    variables: { description: 'An object (these variables, others allowed) or a matcher on the variables JSON' },
  },
  additionalProperties: false,
};

const PREFIX = 'inmemory://devall-mocks/protocol/';
let registered = false;

/** Model path for a definition editor; registers the schemas on first use. */
export function definitionModelPath(protocol: MessageProtocol, key: string): string {
  if (!registered) {
    registered = true;
    const ours = (Object.keys(DEFINITION_SCHEMAS) as MessageProtocol[]).map((p) => ({
      uri: `https://devall.dev/schemas/mocks/${p}.json`,
      fileMatch: [`${PREFIX}${p}/*`],
      schema: DEFINITION_SCHEMAS[p],
    }));
    const current = monaco.json.jsonDefaults.diagnosticsOptions;
    const others = (current.schemas ?? []).filter((s) => !ours.some((o) => o.uri === s.uri));
    monaco.json.jsonDefaults.setDiagnosticsOptions({ ...current, validate: true, enableSchemaRequest: false, schemas: [...others, ...ours] });
  }
  return `${PREFIX}${protocol}/${key.replace(/[^\w.-]+/g, '_')}.json`;
}

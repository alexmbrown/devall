// Types mirroring the WireMock 3.x / 4.x Admin API JSON. Fields are optional because
// WireMock omits defaults; unknown fields are preserved via index signatures so
// round-tripping a stub never drops data the UI does not know about.

export type HttpMethod =
  | 'ANY'
  | 'GET'
  | 'POST'
  | 'PUT'
  | 'PATCH'
  | 'DELETE'
  | 'HEAD'
  | 'OPTIONS'
  | 'TRACE'
  | 'CONNECT'
  | 'QUERY'
  | (string & {});

/** WireMock 4: match any of / none of several methods. */
export type MethodSet = { oneOf: string[] } | { noneOf: string[] };
export type MethodPattern = HttpMethod | MethodSet;

/** A single "string value pattern" (equalTo, matches, contains, ...). */
export interface ContentPattern {
  equalTo?: string | number | boolean;
  caseInsensitive?: boolean;
  binaryEqualTo?: string;
  contains?: string;
  doesNotContain?: string;
  matches?: string;
  doesNotMatch?: string;
  absent?: boolean;
  equalToJson?: unknown;
  ignoreArrayOrder?: boolean;
  ignoreExtraElements?: boolean;
  matchesJsonPath?: string | { expression: string; [matcher: string]: unknown };
  matchesJsonSchema?: string | Record<string, unknown>;
  schemaVersion?: string;
  equalToXml?: string;
  enablePlaceholders?: boolean;
  placeholderOpeningDelimiterRegex?: string;
  placeholderClosingDelimiterRegex?: string;
  exemptedComparisons?: string[];
  namespaceAwareness?: 'STRICT' | 'NONE' | 'LEGACY';
  matchesXPath?: string | { expression: string; [matcher: string]: unknown };
  xPathNamespaces?: Record<string, string>;
  before?: string;
  after?: string;
  equalToDateTime?: string;
  actualFormat?: string;
  truncateExpected?: string;
  truncateActual?: string;
  expectedOffset?: number;
  expectedOffsetUnit?: string;
  and?: ContentPattern[];
  or?: ContentPattern[];
  not?: ContentPattern;
  hasExactly?: ContentPattern[];
  includes?: ContentPattern[];
  /** WireMock 4 number matchers: the value is parsed as a double. */
  equalToNumber?: number;
  greaterThanNumber?: number;
  greaterThanEqualNumber?: number;
  lessThanNumber?: number;
  lessThanEqualNumber?: number;
  [key: string]: unknown;
}

export interface MultipartPattern {
  name?: string;
  matchingType?: 'ALL' | 'ANY';
  headers?: Record<string, ContentPattern>;
  bodyPatterns?: ContentPattern[];
  [key: string]: unknown;
}

export interface RequestPattern {
  method?: MethodPattern;
  url?: string;
  urlPath?: string;
  urlPattern?: string;
  urlPathPattern?: string;
  urlPathTemplate?: string;
  scheme?: 'http' | 'https';
  host?: ContentPattern;
  port?: number;
  clientIp?: ContentPattern;
  pathParameters?: Record<string, ContentPattern>;
  queryParameters?: Record<string, ContentPattern>;
  formParameters?: Record<string, ContentPattern>;
  headers?: Record<string, ContentPattern>;
  cookies?: Record<string, ContentPattern>;
  basicAuthCredentials?: { username: string; password: string };
  bodyPatterns?: ContentPattern[];
  multipartPatterns?: MultipartPattern[];
  customMatcher?: { name: string; parameters?: Record<string, unknown> };
  [key: string]: unknown;
}

export type Fault =
  | 'EMPTY_RESPONSE'
  | 'MALFORMED_RESPONSE_CHUNK'
  | 'RANDOM_DATA_THEN_CLOSE'
  | 'CONNECTION_RESET_BY_PEER';

export type DelayDistribution =
  | { type: 'lognormal'; median: number; sigma: number; maxValue?: number }
  | { type: 'uniform'; lower: number; upper: number }
  | { type: 'fixed'; milliseconds: number };

export interface ResponseDefinition {
  status?: number;
  /** Ignored on the wire by WireMock 4 (Jetty 12 sends the standard reason phrase). */
  statusMessage?: string;
  headers?: Record<string, string | string[]>;
  additionalProxyRequestHeaders?: Record<string, string>;
  removeProxyRequestHeaders?: string[];
  body?: string;
  base64Body?: string;
  jsonBody?: unknown;
  bodyFileName?: string;
  fault?: Fault;
  fixedDelayMilliseconds?: number;
  delayDistribution?: DelayDistribution;
  chunkedDribbleDelay?: { numberOfChunks: number; totalDuration: number };
  proxyBaseUrl?: string;
  proxyUrlPrefixToRemove?: string;
  transformers?: string[];
  transformerParameters?: Record<string, unknown>;
  fromConfiguredStub?: boolean;
  /** WireMock 4 (beta.39+): upgrade the matched request to a WebSocket channel. */
  openWebsocketChannel?: boolean;
  /** WireMock 4 (beta.39+): answer with a server-sent events stream. */
  openSseChannel?: boolean;
  [key: string]: unknown;
}

export interface ServeEventListenerDef {
  name: string;
  requestPhases?: string[];
  parameters?: Record<string, unknown>;
}

export interface StubMapping {
  id?: string;
  /** 3.x alias for `id`. WireMock 4 neither returns nor honours it, so it is turned into `id` before sending. */
  uuid?: string;
  name?: string;
  request: RequestPattern;
  response: ResponseDefinition;
  persistent?: boolean;
  priority?: number;
  scenarioName?: string;
  requiredScenarioState?: string;
  newScenarioState?: string;
  postServeActions?: ServeEventListenerDef[] | Record<string, unknown>;
  serveEventListeners?: ServeEventListenerDef[];
  metadata?: Record<string, unknown>;
  insertionIndex?: number;
  [key: string]: unknown;
}

export interface MappingsResponse {
  mappings: StubMapping[];
  meta: { total: number };
}

export interface LoggedRequest {
  id?: string;
  url: string;
  absoluteUrl?: string;
  method: string;
  clientIp?: string;
  scheme?: string;
  host?: string;
  port?: number;
  headers?: Record<string, string | string[]>;
  cookies?: Record<string, unknown>;
  queryParams?: Record<string, { key: string; values: string[] }>;
  formParams?: Record<string, { key: string; values: string[] }>;
  pathParameters?: Record<string, string>;
  body?: string;
  bodyAsBase64?: string;
  browserProxyRequest?: boolean;
  loggedDate?: number;
  loggedDateString?: string;
  multiparts?: unknown[];
  protocol?: string;
  [key: string]: unknown;
}

export interface LoggedResponse {
  status: number;
  headers?: Record<string, string | string[]>;
  body?: string;
  bodyAsBase64?: string;
  fault?: string;
  [key: string]: unknown;
}

export interface ServeEvent {
  id: string;
  request: LoggedRequest;
  stubMapping?: StubMapping;
  responseDefinition?: ResponseDefinition & { fromConfiguredStub?: boolean };
  response?: LoggedResponse;
  wasMatched: boolean;
  timing?: {
    addedDelay?: number;
    processTime?: number;
    responseSendTime?: number;
    serveTime?: number;
    totalTime?: number;
  };
  subEvents?: { type: string; timeOffsetNanos?: number; data?: Record<string, unknown> }[];
  [key: string]: unknown;
}

export interface RequestsResponse {
  requests: ServeEvent[];
  meta: { total: number };
  requestJournalDisabled?: boolean;
}

export interface DiffLine {
  [key: string]: unknown;
}

/** Per-field result reported by the /dev/all engine (stock WireMock reports only `distance`). */
export interface ServerFieldResult {
  field: string;
  expected: string;
  actual?: string | null;
  matched: boolean;
  distance: number;
  note?: string;
}

export interface MatchResult {
  distance: number;
  fields?: ServerFieldResult[];
}

export interface NearMiss {
  request: LoggedRequest;
  stubMapping?: StubMapping;
  requestPattern?: RequestPattern;
  matchResult: MatchResult;
}

export interface NearMissesResponse {
  nearMisses: NearMiss[];
}

export interface Scenario {
  id: string;
  name: string;
  state: string;
  possibleStates: string[];
  mappings?: StubMapping[];
}

export interface ScenariosResponse {
  scenarios: Scenario[];
}

export interface GlobalSettings {
  fixedDelay?: number;
  delayDistribution?: DelayDistribution;
  proxyPassThrough?: boolean;
  extended?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface RecordFilters {
  urlPattern?: string;
  urlPathPattern?: string;
  url?: string;
  urlPath?: string;
  method?: string;
  headers?: Record<string, ContentPattern>;
  ids?: string[];
  allowNonProxied?: boolean;
}

export interface RecordSpec {
  targetBaseUrl?: string;
  filters?: RecordFilters;
  captureHeaders?: Record<string, { caseInsensitive?: boolean }>;
  requestBodyPattern?: {
    matcher: 'equalTo' | 'equalToJson' | 'equalToXml' | 'auto';
    ignoreArrayOrder?: boolean;
    ignoreExtraElements?: boolean;
    caseInsensitive?: boolean;
  };
  extractBodyCriteria?: { textSizeThreshold?: string; binarySizeThreshold?: string };
  outputFormat?: 'FULL' | 'IDS';
  persist?: boolean;
  repeatsAsScenarios?: boolean;
  transformers?: string[];
  transformerParameters?: Record<string, unknown>;
}

export interface RecordingStatus {
  status: 'NeverStarted' | 'Recording' | 'Stopped';
}

export interface HealthResponse {
  status?: string;
  message?: string;
  version?: string;
  uptimeInSeconds?: number;
  timestamp?: string;
}

// ---------------------------------------------------------------- WireMock 4 messaging (WebSocket / SSE)

/** `websocket` and `sse` are built in; extensions may register others (e.g. `fixed` channel providers). */
export type ChannelType = 'websocket' | 'sse' | (string & {});

export interface ChannelPattern {
  type?: ChannelType;
  initiatingRequestPattern?: RequestPattern;
  [key: string]: unknown;
}

export interface MessagePattern {
  channel?: RequestPattern;
  body?: ContentPattern;
  [key: string]: unknown;
}

export type MessageTrigger =
  | { type: 'message'; channel?: ChannelPattern; message?: MessagePattern; [key: string]: unknown }
  | { type: 'http-stub'; stubId: string; [key: string]: unknown }
  | { type: 'http-request'; requestPattern: RequestPattern; [key: string]: unknown };

/** Text (`data`) or base64 (`data` + `isBinary`) body; headers carry SSE `event` / `id` / `retry` / `comment`. */
export interface MessageDefinition {
  body?: { data?: string; isBinary?: boolean; [key: string]: unknown };
  headers?: Record<string, string | string[]>;
  [key: string]: unknown;
}

export type ChannelTarget =
  | { type: 'originating'; [key: string]: unknown }
  | { type: 'request-initiated'; channelType?: ChannelType; requestPattern?: RequestPattern; [key: string]: unknown };

export interface MessageAction {
  type: 'send' | (string & {});
  message?: MessageDefinition;
  channelTarget?: ChannelTarget;
  transformers?: string[];
  transformerParameters?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface MessageStubMapping {
  id?: string;
  name?: string;
  priority?: number;
  trigger?: MessageTrigger;
  actions?: MessageAction[];
  metadata?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface MessageMappingsResponse {
  messageMappings: MessageStubMapping[];
  meta?: { total: number };
}

export interface MessageChannel {
  id: string;
  /** WireMock 4.0.0-beta.40 reports SSE channels as "websocket" too; see `channelKind()`. */
  type: ChannelType;
  open?: boolean;
  initiatingRequest?: LoggedRequest;
  [key: string]: unknown;
}

export interface ChannelsResponse {
  channels: MessageChannel[];
  meta?: { total: number };
}

export interface ChannelProvidersResponse {
  channelProviders: Record<string, unknown>[];
  meta?: { total: number };
}

export interface SendChannelMessageRequest {
  type: ChannelType;
  initiatingRequest?: RequestPattern;
  message: MessageDefinition;
}

export interface SendChannelMessageResult {
  channels?: MessageChannel[];
  channelsMessaged?: number;
}

export interface MessageServeEvent {
  id: string;
  /** The server sends lower case ("sent" / "received"); the OpenAPI spec says upper case. */
  eventType?: string;
  channel?: MessageChannel;
  message?: { body?: string; base64Body?: string; headers?: Record<string, string | string[]> };
  stubMapping?: MessageStubMapping;
  wasMatched?: boolean;
  timestamp?: string;
  [key: string]: unknown;
}

export interface MessageServeEventsResponse {
  messageServeEvents: MessageServeEvent[];
  messageJournalDisabled?: boolean;
}

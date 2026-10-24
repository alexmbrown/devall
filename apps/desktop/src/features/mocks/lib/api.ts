import type {
  ContentPattern,
  GlobalSettings,
  HealthResponse,
  LoggedRequest,
  MappingsResponse,
  NearMissesResponse,
  RecordSpec,
  RecordingStatus,
  RequestPattern,
  RequestsResponse,
  ScenariosResponse,
  ServeEvent,
  StubMapping,
} from './types';
import { normalizeIdentity } from './stub-utils';
import type { GraphqlStatus, GrpcStatus, MessageEvent, MessageProtocol, MqttStatus, ProtocolsStatus, WebsocketStatus } from './protocols';
import { send, TransportError, type KeychainAuth, type TransportMode } from './transport';

export type AuthType = 'none' | 'basic' | 'bearer' | 'header';

/**
 * Admin API credentials. Non-secret parts (`username`, `headerName`) may live in
 * `mocks/servers.yaml`; the secret itself is either in the OS keychain (`secret` names it) or
 * typed for this session only (`value`, never persisted).
 */
export interface ServerAuth {
  type: AuthType;
  username?: string;
  headerName?: string;
  /** Keychain secret name (all-environments scope of the project). */
  secret?: string;
  /** Session-only password / token / header value. Wins over `secret`. */
  value?: string;
}

export interface ConnectionConfig {
  url: string;
  /** WireMock under a context path, e.g. `/mocks` → admin API at `/mocks/__admin`. */
  contextPath?: string;
  auth?: ServerAuth;
  headers?: { key: string; value: string }[];
  timeoutMs?: number;
  insecureTls?: boolean;
  /** Project root, for keychain lookups. */
  root?: string;
  transport?: TransportMode;
}

/** Normalised failure. `kind` lets the UI explain *why* a call failed. */
export class ApiError extends Error {
  constructor(
    message: string,
    public kind: 'network' | 'http' | 'timeout' | 'parse' | 'tls' | 'credentials' | 'blocked',
    public status?: number,
    public body?: string,
  ) {
    super(message);
  }
}

export function normalizeBaseUrl(url: string): string {
  let u = url.trim();
  if (!/^https?:\/\//i.test(u)) u = `http://${u}`;
  return u.replace(/\/+$/, '');
}

export function normalizeContextPath(p?: string): string {
  const s = (p ?? '').trim().replace(/\/+$/, '').replace(/\/__admin$/, '');
  if (!s) return '';
  return s.startsWith('/') ? s : `/${s}`;
}

export function isLocalHost(hostname: string): boolean {
  const h = hostname.replace(/^\[|\]$/g, '').toLowerCase();
  return h === 'localhost' || h.endsWith('.localhost') || h === '::1' || /^127\./.test(h);
}

export function basicHeader(username: string, password: string): string {
  return `Basic ${btoa(unescape(encodeURIComponent(`${username}:${password}`)))}`;
}

/** Headers for session-only credentials (keychain credentials are added by the proxy). */
export function authHeaders(auth?: ServerAuth): Record<string, string> {
  if (!auth || auth.type === 'none' || auth.value === undefined) return {};
  if (auth.type === 'basic') return { Authorization: basicHeader(auth.username ?? '', auth.value) };
  if (auth.type === 'bearer') return { Authorization: `Bearer ${auth.value}` };
  if (auth.type === 'header' && auth.headerName) return { [auth.headerName]: auth.value };
  return {};
}

const enc = encodeURIComponent;
const utf8 = new TextEncoder();

export class WireMockClient {
  /** Base URL including the context path. */
  readonly base: string;
  /** Admin API base, `${base}/__admin`. */
  readonly admin: string;
  private headers: Record<string, string>;
  private keychainAuth?: KeychainAuth;
  private timeoutMs: number;
  private root?: string;
  private insecureTls?: boolean;
  private transport: TransportMode;

  constructor(cfg: ConnectionConfig) {
    this.base = normalizeBaseUrl(cfg.url) + normalizeContextPath(cfg.contextPath);
    this.admin = `${this.base}/__admin`;
    this.headers = {};
    for (const h of cfg.headers ?? []) if (h.key) this.headers[h.key] = h.value;
    Object.assign(this.headers, authHeaders(cfg.auth));
    const a = cfg.auth;
    if (a && a.type !== 'none' && a.value === undefined && a.secret) this.keychainAuth = { type: a.type, username: a.username, headerName: a.headerName, secret: a.secret };
    this.timeoutMs = cfg.timeoutMs ?? 15000;
    this.root = cfg.root;
    this.insecureTls = cfg.insecureTls;
    this.transport = cfg.transport ?? 'auto';
  }

  async raw(method: string, path: string, body?: unknown, opts: { text?: boolean; binary?: boolean; contentType?: string } = {}): Promise<unknown> {
    const headers: Record<string, string> = { ...this.headers };
    let payload: Uint8Array | undefined;
    if (body !== undefined) {
      if (body instanceof Uint8Array) {
        payload = body;
        headers['Content-Type'] = opts.contentType ?? 'application/octet-stream';
      } else if (body instanceof ArrayBuffer) {
        payload = new Uint8Array(body);
        headers['Content-Type'] = opts.contentType ?? 'application/octet-stream';
      } else if (typeof body === 'string') {
        payload = utf8.encode(body);
        headers['Content-Type'] = opts.contentType ?? 'text/plain';
      } else {
        payload = utf8.encode(JSON.stringify(body));
        headers['Content-Type'] = 'application/json';
      }
    }
    let res;
    try {
      res = await send(
        { baseUrl: this.base, method, path: `/__admin${path}`, headers, body: payload, timeoutMs: this.timeoutMs, keychainAuth: this.keychainAuth, root: this.root, insecureTls: this.insecureTls },
        this.transport,
      );
    } catch (e) {
      if (e instanceof TransportError) throw new ApiError(e.message, e.kind);
      throw new ApiError(e instanceof Error ? e.message : String(e), 'network');
    }
    if (res.status < 200 || res.status >= 300) {
      const text = new TextDecoder().decode(res.body);
      let message = `${res.status} ${res.statusText}`.trim();
      try {
        const j = JSON.parse(text);
        const errs = j.errors as { title?: string; detail?: string }[] | undefined;
        if (errs?.length) message = errs.map((x) => [x.title, x.detail].filter(Boolean).join(': ')).join('; ');
      } catch {
        if (text) message += ` — ${text.slice(0, 300)}`;
      }
      throw new ApiError(message, 'http', res.status, text);
    }
    if (opts.binary) return res.body;
    const text = new TextDecoder().decode(res.body);
    if (opts.text) return text;
    if (!text) return undefined;
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  }

  private get<T>(p: string) {
    return this.raw('GET', p) as Promise<T>;
  }
  private post<T>(p: string, b?: unknown) {
    return this.raw('POST', p, b ?? {}) as Promise<T>;
  }
  private put<T>(p: string, b?: unknown) {
    return this.raw('PUT', p, b) as Promise<T>;
  }
  private del<T>(p: string) {
    return this.raw('DELETE', p) as Promise<T>;
  }

  // ---- system
  health() {
    return this.get<HealthResponse>('/health');
  }
  async version(): Promise<string | undefined> {
    try {
      const v = await this.get<{ version: string }>('/version');
      return v?.version;
    } catch (e) {
      // 2.x servers have no /version; fall back to health or unknown.
      if (e instanceof ApiError && e.status === 404) return undefined;
      throw e;
    }
  }
  /** Lightweight connectivity probe that works on WireMock 2.x, 3.x and 4.x. */
  async ping(): Promise<{ ok: true; version?: string; latencyMs: number }> {
    const t0 = performance.now();
    try {
      const h = await this.health();
      return { ok: true, version: h?.version, latencyMs: Math.round(performance.now() - t0) };
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        await this.get('/mappings?limit=1');
        return { ok: true, latencyMs: Math.round(performance.now() - t0) };
      }
      throw e;
    }
  }
  resetAll() {
    return this.post('/reset');
  }
  shutdown() {
    return this.post('/shutdown');
  }
  async getSettings(): Promise<GlobalSettings> {
    const r = await this.get<{ settings: GlobalSettings }>('/settings');
    return r?.settings ?? {};
  }
  /** WireMock replaces the whole settings object, so always send the merged result. */
  async updateSettings(patch: Partial<GlobalSettings>): Promise<GlobalSettings> {
    const current = await this.getSettings();
    const next: GlobalSettings = { ...current, ...patch };
    for (const k of Object.keys(next)) if (next[k] === undefined || next[k] === null) delete next[k];
    await this.raw('POST', '/settings', next);
    return next;
  }
  openApiSpec() {
    return this.get<Record<string, unknown>>('/docs/swagger');
  }

  // ---- /dev/all engine extensions
  devallInfo() {
    return this.get<DevallInfo>('/devall/info');
  }
  devallReload() {
    return this.post<{ stubCount: number; loadErrors: { file: string; error: string }[] }>('/devall/reload');
  }
  chaos() {
    return this.get<ChaosConfig>('/devall/chaos');
  }
  setChaos(c: ChaosConfig) {
    return this.put<ChaosConfig>('/devall/chaos', c);
  }

  // ---- /dev/all: GraphQL, WebSocket, gRPC, MQTT (built-in engine only)
  protocols() {
    return this.get<ProtocolsStatus>('/devall/protocols');
  }
  graphql() {
    return this.get<GraphqlStatus>('/devall/graphql');
  }
  graphqlExecute(body: { schema?: string; query: string; variables?: Record<string, unknown>; operationName?: string }) {
    return this.post<Record<string, unknown>>('/devall/graphql/execute', body);
  }
  websocket() {
    return this.get<WebsocketStatus>('/devall/websocket');
  }
  websocketSend(body: { connectionId?: string; path?: string; text?: string; json?: unknown; binary?: string }) {
    return this.post<{ sent: number }>('/devall/websocket/send', body);
  }
  websocketClose(body: { connectionId?: string; path?: string; code?: number; reason?: string }) {
    return this.post<{ closed: number }>('/devall/websocket/close', body);
  }
  grpc() {
    return this.get<GrpcStatus>('/devall/grpc');
  }
  grpcStart(port?: number) {
    return this.post<{ port: number }>('/devall/grpc/start', port ? { port } : {});
  }
  grpcStop() {
    return this.post<void>('/devall/grpc/stop');
  }
  mqtt() {
    return this.get<MqttStatus>('/devall/mqtt');
  }
  mqttStart(port?: number) {
    return this.post<{ port: number }>('/devall/mqtt/start', port ? { port } : {});
  }
  mqttStop() {
    return this.post<void>('/devall/mqtt/stop');
  }
  mqttPublish(body: { topic: string; payload?: string; qos?: number; retain?: boolean }) {
    return this.post<{ delivered: number }>('/devall/mqtt/publish', body);
  }
  messages(protocol?: MessageProtocol, limit = 500) {
    const q = new URLSearchParams({ limit: String(limit) });
    if (protocol) q.set('protocol', protocol);
    return this.get<{ messages: MessageEvent[] }>(`/devall/messages?${q}`);
  }
  clearMessages(protocol?: MessageProtocol) {
    return this.del<void>(`/devall/messages${protocol ? `?protocol=${protocol}` : ''}`);
  }

  // ---- mappings
  mappings(limit?: number, offset?: number) {
    const q = new URLSearchParams();
    if (limit) q.set('limit', String(limit));
    if (offset) q.set('offset', String(offset));
    const qs = q.toString();
    return this.get<MappingsResponse>(`/mappings${qs ? `?${qs}` : ''}`);
  }
  mapping(id: string) {
    return this.get<StubMapping>(`/mappings/${enc(id)}`);
  }
  // Every write sends `id`, never only `uuid`: WireMock 4 ignores `uuid` and would assign a random id.
  createMapping(stub: StubMapping) {
    return this.post<StubMapping>('/mappings', normalizeIdentity(stub));
  }
  updateMapping(id: string, stub: StubMapping) {
    return this.put<StubMapping>(`/mappings/${enc(id)}`, normalizeIdentity(stub));
  }
  deleteMapping(id: string) {
    return this.del(`/mappings/${enc(id)}`);
  }
  deleteAllMappings() {
    return this.del('/mappings');
  }
  resetMappings() {
    return this.post('/mappings/reset');
  }
  saveMappings() {
    return this.post('/mappings/save');
  }
  importMappings(mappings: StubMapping[], duplicatePolicy: 'OVERWRITE' | 'IGNORE' = 'OVERWRITE', deleteAllNotInImport = false) {
    return this.post('/mappings/import', { mappings: mappings.map(normalizeIdentity), importOptions: { duplicatePolicy, deleteAllNotInImport } });
  }
  findByMetadata(pattern: ContentPattern) {
    return this.post<MappingsResponse>('/mappings/find-by-metadata', pattern);
  }
  removeByMetadata(pattern: ContentPattern) {
    return this.post('/mappings/remove-by-metadata', pattern);
  }
  unmatchedMappings() {
    return this.get<MappingsResponse>('/mappings/unmatched');
  }
  removeUnmatchedMappings() {
    return this.del('/mappings/unmatched');
  }

  // ---- request journal
  requests(limit?: number, since?: string, opts: { unmatched?: boolean; matchingStub?: string } = {}) {
    const q = new URLSearchParams();
    if (limit) q.set('limit', String(limit));
    if (since) q.set('since', since);
    if (opts.unmatched) q.set('unmatched', 'true');
    if (opts.matchingStub) q.set('matchingStub', opts.matchingStub);
    const qs = q.toString();
    return this.get<RequestsResponse>(`/requests${qs ? `?${qs}` : ''}`);
  }
  request(id: string) {
    return this.get<ServeEvent>(`/requests/${enc(id)}`);
  }
  deleteRequest(id: string) {
    return this.del(`/requests/${enc(id)}`);
  }
  clearRequests() {
    return this.del('/requests');
  }
  countRequests(p: RequestPattern) {
    return this.post<{ count: number }>('/requests/count', p);
  }
  findRequests(p: RequestPattern) {
    return this.post<{ requests: LoggedRequest[] }>('/requests/find', p);
  }
  removeRequests(p: RequestPattern) {
    return this.post('/requests/remove', p);
  }
  unmatchedRequests() {
    return this.get<{ requests: LoggedRequest[] }>('/requests/unmatched');
  }
  unmatchedNearMisses() {
    return this.get<NearMissesResponse>('/requests/unmatched/near-misses');
  }
  nearMissesForRequest(r: Partial<LoggedRequest>) {
    return this.post<NearMissesResponse>('/near-misses/request', r);
  }
  nearMissesForPattern(p: RequestPattern) {
    return this.post<NearMissesResponse>('/near-misses/request-pattern', p);
  }

  // ---- recording
  startRecording(spec: RecordSpec) {
    return this.post('/recordings/start', spec);
  }
  stopRecording() {
    return this.post<MappingsResponse>('/recordings/stop');
  }
  recordingStatus() {
    return this.get<RecordingStatus>('/recordings/status');
  }
  snapshot(spec: RecordSpec) {
    return this.post<MappingsResponse>('/recordings/snapshot', spec);
  }

  // ---- scenarios
  scenarios() {
    return this.get<ScenariosResponse>('/scenarios');
  }
  resetScenarios() {
    return this.post('/scenarios/reset');
  }
  /** Set a scenario's state; omit `state` to reset that single scenario to "Started". */
  setScenarioState(name: string, state?: string) {
    return this.put(`/scenarios/${enc(name)}/state`, state === undefined ? undefined : { state });
  }

  // ---- files (__files)
  async files() {
    // WireMock on Windows reports paths with backslashes.
    return ((await this.get<string[]>('/files')) ?? []).map((f) => f.replace(/\\/g, '/'));
  }
  fileText(path: string) {
    return this.raw('GET', `/files/${path.split('/').map(enc).join('/')}`, undefined, { text: true }) as Promise<string>;
  }
  fileBytes(path: string) {
    return this.raw('GET', `/files/${path.split('/').map(enc).join('/')}`, undefined, { binary: true }) as Promise<Uint8Array>;
  }
  putFile(path: string, content: string | Uint8Array) {
    return this.raw('PUT', `/files/${path.split('/').map(enc).join('/')}`, content, {
      contentType: typeof content === 'string' ? 'text/plain' : 'application/octet-stream',
    });
  }
  deleteFile(path: string) {
    return this.del(`/files/${path.split('/').map(enc).join('/')}`);
  }
}

/** `GET /__admin/devall/info` on the built-in engine. */
export interface DevallInfo {
  engine: string;
  version: string;
  port: number;
  projectRoot: string;
  stubCount: number;
  running: boolean;
  globalTemplating: boolean;
  loadErrors: { file: string; error: string }[];
}

export type ChaosFault = 'CONNECTION_RESET_BY_PEER' | 'EMPTY_RESPONSE' | 'MALFORMED_RESPONSE_CHUNK' | 'RANDOM_DATA_THEN_CLOSE';

/** `GET/PUT /__admin/devall/chaos`. Rates are probabilities in 0..1. */
export interface ChaosConfig {
  enabled: boolean;
  latencyMs: number;
  jitterMs: number;
  errorRate: number;
  errorStatuses: number[];
  faultRate: number;
  faults: ChaosFault[];
  urlPattern?: string;
}

export function isDevallVersion(v?: string): boolean {
  return !!v && /devall/i.test(v);
}

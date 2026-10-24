// Typed wrappers + TS mirrors for the traffic-inspector commands in
// src-tauri/src/modules/proxy.rs (crate: crates/devall-proxy). Keep in sync with the Rust types.
import { Channel, invoke } from '@tauri-apps/api/core';

// ---------- captured traffic ----------

export type ExchangeId = number;
/** Header name/value pairs in wire order (lower-case names). */
export type HeaderList = [string, string][];

export type ExchangeState = 'pending' | 'paused' | 'receiving' | 'complete' | 'failed' | 'aborted' | 'blocked' | 'tunnel' | 'webSocket';
export type Phase = 'request' | 'response';

export interface ExchangeSummary {
  id: ExchangeId;
  /** Unix epoch ms. */
  startedAt: number;
  method: string;
  url: string;
  scheme: string;
  /** host[:port] */
  host: string;
  /** Path and query. */
  path: string;
  httpVersion: string;
  status: number | null;
  state: ExchangeState;
  contentType: string | null;
  requestSize: number;
  responseSize: number | null;
  durationMs: number | null;
  /** Names of matched rules. */
  rules: string[];
  /** e.g. edited, mocked, mapLocal, mapRemote, throttled, replay, bypassed, imported, breakpointTimeout, tlsRejected, closed */
  tags: string[];
  paused: Phase | null;
  error: string | null;
  wsFrames: number;
  clientAddr: string | null;
  mappedUrl: string | null;
  /** OpenAPI violations found by a Validate rule (0 = none / not checked). */
  violations: number;
}

/** One OpenAPI problem with an exchange (crates/devall-openapi conform.rs). */
export interface SpecViolation {
  kind: import('./openapi').DriftKind;
  severity: import('./openapi').Severity;
  operation: string | null;
  message: string;
  details: string[];
}

/** Findings of a "Validate against OpenAPI" rule. */
export interface SpecValidation {
  rule: string;
  spec: string;
  blocked: boolean;
  request: SpecViolation[];
  response: SpecViolation[];
  responseChecked: boolean;
  responseBodySkipped: boolean;
}

export interface BodyView {
  size: number;
  captured: number;
  truncated: boolean;
  evicted: boolean;
  encoding: string | null;
  decoded: boolean;
  decodeError: string | null;
  contentType: string | null;
  text: string | null;
  base64: string | null;
}

export interface MessageView {
  method: string | null;
  url: string | null;
  status: number | null;
  reason: string | null;
  httpVersion: string;
  headers: HeaderList;
  body: BodyView;
}

export interface Timing {
  blockedMs?: number;
  dnsMs?: number;
  connectMs?: number;
  tlsMs?: number;
  waitMs?: number;
  receiveMs?: number;
  totalMs?: number;
  connectionReused?: boolean;
}

export interface WsFrame {
  direction: 'send' | 'receive';
  atMs: number;
  opcode: 'text' | 'binary' | 'ping' | 'pong' | 'close';
  size: number;
  text?: string;
  base64?: string;
  truncated?: boolean;
}

export interface ExchangeDetail extends ExchangeSummary {
  request: MessageView;
  response: MessageView | null;
  timing: Timing;
  serverAddr: string | null;
  clientProcess: string | null;
  /** WebSocket frames (`wsFrames` is the total count, frames beyond the cap are not kept). */
  frames: WsFrame[];
  validation: SpecValidation | null;
}

export interface PausedExchange {
  id: ExchangeId;
  phase: Phase;
  method: string;
  url: string;
  status: number | null;
  headers: HeaderList;
  body: BodyView;
  /** Unix epoch ms when it resumes on its own. */
  deadline: number;
}

/** Unset fields keep their value. Setting `body`/`bodyBase64` sends it uncompressed with a fresh Content-Length. */
export interface BreakpointEdit {
  method?: string;
  url?: string;
  status?: number;
  headers?: HeaderList;
  body?: string;
  bodyBase64?: string;
}

export type ProxyEvent =
  | { type: 'exchange'; exchange: ExchangeSummary }
  | { type: 'paused'; paused: PausedExchange }
  | { type: 'resumed'; id: ExchangeId }
  | { type: 'removed'; ids: ExchangeId[] }
  | { type: 'cleared' }
  | { type: 'status'; running: boolean; port: number | null; error: string | null }
  | { type: 'resync' };

export type ContentKind = 'json' | 'html' | 'script' | 'css' | 'image' | 'font' | 'media' | 'xml' | 'text' | 'webSocket' | 'other';

export interface ExchangeFilter {
  text?: string;
  methods?: string[];
  /** 1–5, 0 = no response. */
  statusClasses?: number[];
  host?: string;
  contentKinds?: ContentKind[];
  hideStatic?: boolean;
  onlyPaused?: boolean;
  limit?: number;
}

// ---------- status & settings ----------

export interface ProxyStatus {
  running: boolean;
  port: number | null;
  address: string | null;
  startedAt: number | null;
  exchanges: number;
  paused: number;
  rules: number;
  caSha256: string;
}

export interface ProxyConfig {
  version: number;
  port: number;
  bypass: string[];
  ignoreTlsErrors: string[];
  maxExchanges: number;
  maxBodyBytes: number;
  maxTotalBodyBytes: number;
  upstreamHttp2: boolean;
  systemRoots: boolean;
  extraRootsPem: string[];
  breakpointTimeoutSecs: number;
  upstreamTimeoutSecs: number;
  maxWsFrames: number;
}

// ---------- CA & trust ----------

export interface CaInfo {
  commonName: string;
  certPath: string;
  dir: string;
  createdAt: number;
  notAfter: number;
  sha256: string;
  sha1: string;
  spkiSha256: string;
  pem: string;
}

export interface TrustCommand {
  label: string;
  program: string;
  args: string[];
  display: string;
  runnable: boolean;
  prompt: string | null;
}

export interface TrustNote {
  title: string;
  body: string;
  command: string | null;
}

export interface TrustInfo {
  os: 'windows' | 'macos' | 'linux';
  installed: boolean | null;
  install: TrustCommand | null;
  uninstall: TrustCommand | null;
  manual: TrustNote[];
}

export interface EnvSnippet {
  shell: 'bash' | 'fish' | 'powershell' | 'cmd';
  label: string;
  text: string;
}

export type BrowserKind = 'chrome' | 'edge' | 'brave' | 'chromium';

export interface BrowserInfo {
  kind: BrowserKind;
  name: string;
  path: string;
}

export interface CaView {
  ca: CaInfo;
  trust: TrustInfo;
  env: EnvSnippet[];
  browsers: BrowserInfo[];
}

export interface CommandOutput {
  success: boolean;
  code: number | null;
  stdout: string;
  stderr: string;
}

export interface Launched {
  kind: BrowserKind;
  path: string;
  args: string[];
  profileDir: string;
  pid: number;
}

// ---------- rules ----------

export interface NameValue {
  name: string;
  value: string;
}

export interface RuleMatch {
  methods?: string[];
  /** Glob over the host (`*.example.com`); with `:port` also matches the port. */
  host?: string;
  /** Glob over the path without query. */
  path?: string;
  /** Regex searched in path + query. */
  pathRegex?: string;
  header?: { name: string; value?: string };
}

export type RuleAction =
  | { type: 'breakpoint'; request?: boolean; response?: boolean }
  | { type: 'mapLocal'; file?: string; body?: string; status?: number; contentType?: string; headers?: NameValue[] }
  | { type: 'mapRemote'; scheme?: string; host?: string; port?: number; pathFrom?: string; pathTo?: string; preserveHost?: boolean }
  | { type: 'mockResponse'; status?: number; headers?: NameValue[]; body?: string; delayMs?: number }
  | { type: 'setHeader'; target?: 'request' | 'response'; name: string; value?: string }
  | { type: 'removeHeader'; target?: 'request' | 'response'; name: string }
  | { type: 'throttle'; latencyMs?: number; kbps?: number }
  | { type: 'block'; mode?: 'forbidden' | 'close' }
  /** Check traffic against an OpenAPI spec (absolute path). `block`: 422 for bad requests, 502 for bad responses. */
  | { type: 'validateSpec'; spec: string; mode?: 'flag' | 'block'; allHosts?: boolean };

export type RuleActionType = RuleAction['type'];

export interface Rule {
  id: string;
  name: string;
  /** Omitted = true. */
  enabled?: boolean;
  match: RuleMatch;
  actions: RuleAction[];
}

// ---------- exports ----------

export interface StubOptions {
  matchQuery?: boolean;
  matchHeaders?: string[];
  body?: 'none' | 'equalToJson' | 'equalTo';
  ignoreExtraElements?: boolean;
  generalize?: boolean;
  includeSensitive?: boolean;
  folder?: string;
}

export interface StubsResult {
  written: { file: string; name: string }[];
  skipped: string[];
}

export interface OpenApiOptions {
  title?: string;
  version?: string;
  includeExamples?: boolean;
  minSiblings?: number;
}

export interface LoadExport {
  /** Relative to load/. */
  loadFile: string;
  /** Relative to http/. */
  requests: string[];
}

export interface SessionEntry {
  file: string;
  size: number;
  modified: number;
}

// ---------- commands ----------

export const proxyApi = {
  start: (port?: number) => invoke<ProxyStatus>('proxy_start', { port }),
  stop: () => invoke<ProxyStatus>('proxy_stop'),
  status: () => invoke<ProxyStatus>('proxy_status'),
  settings: () => invoke<ProxyConfig>('proxy_settings_get'),
  saveSettings: (settings: ProxyConfig) => invoke<ProxyConfig>('proxy_settings_save', { settings }),

  caInfo: (check = false) => invoke<CaView>('proxy_ca_info', { check }),
  caInstall: () => invoke<CommandOutput>('proxy_ca_install'),
  caUninstall: () => invoke<CommandOutput>('proxy_ca_uninstall'),
  caRegenerate: () => invoke<CaInfo>('proxy_ca_regenerate'),

  subscribe: (onEvents: (events: ProxyEvent[]) => void) => {
    const ch = new Channel<ProxyEvent[]>();
    ch.onmessage = onEvents;
    return invoke<void>('proxy_subscribe', { onEvent: ch });
  },
  unsubscribe: () => invoke<void>('proxy_unsubscribe'),

  list: (filter?: ExchangeFilter) => invoke<ExchangeSummary[]>('proxy_list', { filter }),
  get: (id: ExchangeId) => invoke<ExchangeDetail | null>('proxy_get', { id }),
  clear: () => invoke<void>('proxy_clear'),
  remove: (ids: ExchangeId[]) => invoke<number>('proxy_remove', { ids }),
  paused: () => invoke<PausedExchange[]>('proxy_paused'),

  rules: () => invoke<Rule[]>('proxy_rules_get'),
  saveRules: (rules: Rule[]) => invoke<Rule[]>('proxy_rules_save', { rules }),
  exportRules: () => invoke<string>('proxy_rules_export'),
  importRules: (text: string, replace = false) => invoke<Rule[]>('proxy_rules_import', { text, replace }),

  resume: (id: ExchangeId, edited?: BreakpointEdit) => invoke<void>('proxy_breakpoint_resume', { id, edited }),
  abort: (id: ExchangeId) => invoke<void>('proxy_breakpoint_abort', { id }),
  replay: (id: ExchangeId, edits?: BreakpointEdit) => invoke<ExchangeId>('proxy_replay', { id, edits }),
  copyCurl: (id: ExchangeId, includeSensitive = false) => invoke<string>('proxy_copy_curl', { id, includeSensitive }),

  /** Returns the HAR text, or the path when `path` is given. */
  exportHar: (ids: ExchangeId[], includeSensitive: boolean, path?: string) => invoke<string>('proxy_export_har', { ids, includeSensitive, path }),
  toStubs: (root: string, ids: ExchangeId[], options?: StubOptions) => invoke<StubsResult>('proxy_to_stubs', { root, ids, options }),
  toRequests: (root: string, ids: ExchangeId[], folder?: string) => invoke<string[]>('proxy_to_requests', { root, ids, folder }),
  toOpenApi: (root: string, ids: ExchangeId[], options?: OpenApiOptions) => invoke<string>('proxy_to_openapi', { root, ids, options }),
  saveOpenApi: (root: string, path: string, yaml: string) => invoke<string>('proxy_save_openapi', { root, path, yaml }),
  toLoad: (root: string, ids: ExchangeId[], name: string) => invoke<LoadExport>('proxy_to_load', { root, ids, name }),

  sessions: (root: string) => invoke<SessionEntry[]>('proxy_sessions', { root }),
  saveSession: (root: string, ids: ExchangeId[], name?: string, includeSensitive = false) => invoke<string>('proxy_save_session', { root, ids, name, includeSensitive }),
  loadSession: (root: string, file: string) => invoke<ExchangeId[]>('proxy_load_session', { root, file }),
  deleteSession: (root: string, file: string) => invoke<void>('proxy_delete_session', { root, file }),
  /** Imports HAR text (e.g. from browser dev tools) into the capture. */
  importHar: (text: string) => invoke<ExchangeId[]>('proxy_import_har', { text }),

  launchBrowser: (kind: BrowserKind, url?: string) => invoke<Launched>('proxy_launch_browser', { kind, url }),
};

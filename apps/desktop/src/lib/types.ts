import type { ScriptReport, Scripts } from './api/scripts';
// Mirrors of the Rust types crossing the IPC boundary (crates/devall-core/src/model.rs,
// crates/devall-http, apps/desktop/src-tauri/src/commands.rs). Keep in sync.

export interface KeyValue {
  name: string;
  value: string;
  enabled?: boolean; // omitted = true
  description?: string;
}

export type ApiKeyLocation = 'header' | 'query';

export type Auth =
  | { type: 'inherit' }
  | { type: 'none' }
  | { type: 'basic'; username: string; password: string }
  | { type: 'bearer'; token: string; prefix?: string }
  | { type: 'apiKey'; key: string; value: string; location: ApiKeyLocation }
  | ({ type: 'oauth2' } & OAuth2Config)
  | { type: 'awsSigV4'; accessKeyId: string; secretAccessKey: string; sessionToken?: string; region: string; service: string }
  | { type: 'digest'; username: string; password: string }
  | { type: 'ntlm'; username: string; password: string; domain?: string; workstation?: string }
  /** A WebAssembly plugin's auth provider (`contributes.auth`); config values accept `{{vars}}`. */
  | { type: 'plugin'; plugin: string; provider: string; config?: Record<string, string> };

export type OAuth2Grant = 'clientCredentials' | 'authorizationCode' | 'password';

export interface OAuth2Config {
  grant?: OAuth2Grant;
  authUrl?: string;
  tokenUrl: string;
  clientId: string;
  clientSecret?: string;
  scope?: string;
  audience?: string;
  username?: string;
  password?: string;
  usePkce?: boolean;
  redirectPort?: number;
  clientAuth?: 'basic' | 'body';
  tokenPrefix?: string;
  extraParams?: Record<string, string>;
}

export type CaptureSource =
  | { from: 'status' }
  | { from: 'header'; header: string }
  | { from: 'jsonPath'; path: string }
  | { from: 'regex'; pattern: string; group?: number }
  | { from: 'cookie'; cookie: string }
  | { from: 'body' };

export type Capture = { name: string; enabled?: boolean } & CaptureSource;

export type AssertTarget =
  | { assert: 'status' }
  | { assert: 'header'; name: string }
  | { assert: 'jsonPath'; path: string }
  | { assert: 'body' }
  | { assert: 'responseTime' }
  | { assert: 'size' }
  | { assert: 'contentType' }
  /** Body matches a committed snapshot file (relative to the project root); `ignore` = JSONPaths to mask. */
  | { assert: 'snapshot'; file: string; ignore?: string[] };

export type AssertOp =
  | 'eq'
  | 'ne'
  | 'lt'
  | 'lte'
  | 'gt'
  | 'gte'
  | 'contains'
  | 'notContains'
  | 'startsWith'
  | 'endsWith'
  | 'matches'
  | 'exists'
  | 'notExists'
  | 'isEmpty'
  | 'isNotEmpty'
  | 'isType'
  | 'oneOf';

export type Assertion = AssertTarget & { op: AssertOp; value?: string; enabled?: boolean };

export interface AssertionResult {
  label: string;
  passed: boolean;
  actual: string | null;
  message: string | null;
}

export interface CaptureResult {
  name: string;
  value: string | null;
  error: string | null;
}

export interface MultipartPart {
  name: string;
  value: string;
  file?: string;
  contentType?: string;
  enabled?: boolean;
}

export type Body =
  | { type: 'none' }
  | { type: 'json'; content: string }
  | { type: 'text'; content: string; contentType?: string }
  | { type: 'xml'; content: string }
  | { type: 'form'; fields: KeyValue[] }
  | { type: 'multipart'; parts: MultipartPart[] }
  | { type: 'graphql'; query: string; variables?: string; operationName?: string }
  | { type: 'binary'; path: string; contentType?: string };

export type BodyType = Body['type'];

export interface RequestSettings {
  timeoutMs: number;
  followRedirects: boolean;
  verifyTls: boolean;
  /** Proxy URL, 'none', or absent for the system proxy. */
  proxy?: string;
  clientCert?: { cert: string; key?: string };
}

export interface HttpRequestFile {
  version?: number;
  id: string;
  name: string;
  method: string;
  url: string;
  params?: KeyValue[];
  headers?: KeyValue[];
  auth?: Auth;
  body?: Body;
  settings?: RequestSettings;
  captures?: Capture[];
  tests?: Assertion[];
  /** Pre-request / post-response JavaScript (`pm.*` API). */
  scripts?: Scripts;
  docs?: string;
  [extra: string]: unknown;
}

export interface FolderFile {
  version?: number;
  name?: string;
  order?: string[];
  headers?: KeyValue[];
  auth?: Auth;
  variables?: Record<string, string>;
  /** Run before/after every request in this folder (outermost folder first). */
  scripts?: Scripts;
  [extra: string]: unknown;
}

export interface EnvironmentFile {
  version?: number;
  name: string;
  /** Preset (`local`, `dev`, `test`, `staging`, `prod`) or a custom hex colour. */
  color?: string;
  /** Short chip text (defaults to the preset's name). */
  label?: string;
  protected?: boolean;
  variables?: Record<string, string>;
  secrets?: string[];
  /** Secret-manager references by secret name (e.g. op://vault/item/field). Safe to commit. */
  secretSources?: Record<string, string>;
  [extra: string]: unknown;
}

export interface EnvironmentEntry {
  id: string;
  file: EnvironmentFile;
}

export interface ProjectManifest {
  version?: number;
  id: string;
  name: string;
  description?: string;
  variables?: Record<string, string>;
  [extra: string]: unknown;
}

export interface ProjectInfo {
  root: string;
  manifest: ProjectManifest;
  environments: EnvironmentEntry[];
  isScratch: boolean;
}

export interface RecentProject {
  path: string;
  name: string;
  openedAt: number;
}

/** A project on this device with its `devall.yaml` id (`recent_project_ids`). */
export interface RecentProjectId extends RecentProject {
  id: string;
  /** The scratch pad (never on the recent list). */
  scratch: boolean;
}

/** A `devall:` link from outside the app, checked in Rust (`modules::deep_link::Incoming`). */
export interface IncomingLink {
  /** Canonical, with its `?project=` qualifier, when it passed the checks. */
  link?: string;
  /** Why it was refused. */
  error?: string;
  /** What arrived, shortened. */
  raw: string;
}

export interface AppInfo {
  version: string;
  platform: string;
  scratchPath: string;
  keychainAvailable: boolean;
  configDir: string;
}

export type TreeNode =
  | { kind: 'folder'; name: string; path: string; children: TreeNode[] }
  | { kind: 'request'; name: string; path: string; method: string; id: string; protocol: 'http' | 'ws' | 'grpc' | 'mqtt' | 'socketio'; error?: string };

/** A `{{name}}` / `{{secret.name}}` reference found in a project file (`env_usages`). */
export interface VarUsage {
  name: string;
  /** Relative to the project root. */
  path: string;
  kind: 'request' | 'folder' | 'realtime' | 'mock' | 'note' | 'load' | 'data' | 'connection' | 'query' | 'webhook' | 'environment' | 'project' | 'other';
  line: number;
  column: number;
  excerpt: string;
}

export interface SecretStatus {
  name: string;
  source: string | null;
}

export interface VarInfo {
  name: string;
  value: string | null;
  source: string;
}

export interface Timings {
  ttfbMs: number;
  downloadMs: number;
  totalMs: number;
}

export interface ResponseView {
  status: number;
  statusText: string;
  httpVersion: string;
  headers: [string, string][];
  bodyTruncated: boolean;
  url: string;
  redirected: boolean;
  remoteAddr: string | null;
  timings: Timings;
  bodySize: number;
  bodyText: string | null;
  bodyBase64: string | null;
  contentType: string | null;
}

export type HttpError = { kind: 'invalidUrl' | 'invalidRequest' | 'connect' | 'timeout' | 'request' | 'body'; message: string };

export interface SentSummary {
  method: string;
  url: string;
  headers: [string, string][];
  body: string | null;
}

export interface SendResult {
  response: ResponseView | null;
  error: HttpError | null;
  cancelled: boolean;
  sent: SentSummary | null;
  unresolved: string[];
  secretErrors: string[];
  tests: AssertionResult[];
  captures: CaptureResult[];
  historyId: string | null;
  at: number;
  /** Script console output and errors. */
  scripts?: ScriptReport;
}

export interface SseEvent {
  event: string;
  data: string;
  id: string | null;
  retry: number | null;
}

export type StreamMsg = { type: 'sse'; event: SseEvent } | { type: 'progress'; bytes: number };

export interface RunItemResult {
  iteration: number;
  path: string;
  name: string;
  method: string;
  url: string;
  status: number | null;
  error: string | null;
  durationMs: number;
  size: number;
  tests: AssertionResult[];
  captures: CaptureResult[];
  passed: boolean;
  scripts?: ScriptReport;
  /** A pre-request script called `pm.execution.skipRequest()`. */
  skipped?: boolean;
}

export interface RunSummary {
  total: number;
  passed: number;
  failed: number;
  testsPassed: number;
  testsFailed: number;
  durationMs: number;
  cancelled: boolean;
  items: RunItemResult[];
  variables: Record<string, string>;
}

export type RunEvent = { type: 'started'; total: number } | { type: 'item'; result: RunItemResult };

export interface RunRequest {
  folder: string;
  paths?: string[];
  env?: string;
  iterations?: number;
  bail?: boolean;
  delayMs?: number;
  failOnHttpError?: boolean;
  data?: string;
  keepVariables?: boolean;
  /** Record missing / accept changed response snapshots. */
  updateSnapshots?: boolean;
}

export interface HistorySummary {
  id: string;
  requestPath: string;
  requestName: string;
  environment: string | null;
  method: string;
  url: string;
  at: number;
  status: number | null;
  error: string | null;
  durationMs: number;
  size: number;
  testsPassed: number;
  testsFailed: number;
}

export interface HistoryDetail {
  entry: Omit<HistorySummary, 'testsPassed' | 'testsFailed'> & {
    contentType: string | null;
    sent: SentSummary;
    responseHeaders: [string, string][];
    tests: AssertionResult[];
    bodyTruncated: boolean;
  };
  bodyText: string | null;
  bodyBase64: string | null;
  previousId: string | null;
}

export interface CookieInfo {
  name: string;
  value: string;
  domain: string;
  path: string;
  secure: boolean;
  httpOnly: boolean;
  expires: string | null;
}

export interface TokenInfo {
  preview: string;
  expiresAt: number | null;
  scope: string | null;
  hasRefreshToken: boolean;
}

/** What “Move to project” copies (paths: items under http/, notes under notes/, environment ids). */
export interface TransferSelection {
  items?: string[];
  environments?: string[];
  notes?: string[];
}

export interface TransferOptions {
  /** Folder under the destination's http/ (empty = root). */
  targetFolder?: string;
  /** Recreate the parent folders of single requests (keeps inherited headers/auth). */
  keepFolders?: boolean;
  includeVariables?: boolean;
  /** Move the originals to the trash afterwards. */
  removeSource?: boolean;
}

export interface TransferReport {
  items: { from: string; to: string }[];
  requests: number;
  environments: { from: string; to: string; name: string; secrets: string[] }[];
  notes: { from: string; to: string }[];
  variablesAdded: string[];
  secretRefs: string[];
  warnings: string[];
  firstRequest: string | null;
  removedSource: boolean;
}

/** A keychain value that can follow moved items (names only — values never reach the UI). */
export interface SecretCopy {
  fromEnv: string;
  toEnv: string;
  name: string;
  exists: boolean;
}

export interface TransferResult {
  report: TransferReport;
  secrets: SecretCopy[];
}

/** utilities/regex/*.regex.yaml */
export interface RegexFile {
  version?: number;
  name: string;
  description?: string;
  pattern: string;
  flags?: string;
  flavor?: 'rust' | 'pcre-like';
  replacement?: string;
  shouldMatch?: string[];
  shouldNotMatch?: string[];
  [extra: string]: unknown;
}

export interface RegexEntry {
  path: string;
  name: string;
  pattern: string;
  cases: number;
  error?: string;
}

export type { ArgInfo, OpInfo } from './api/utils';

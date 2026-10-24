// Typed wrappers + TS mirrors for the webhook inbox and tunnel commands in
// src-tauri/src/modules/webhooks.rs (crate: crates/devall-webhooks). Keep in sync with the Rust types.
import { Channel, invoke } from '@tauri-apps/api/core';

/** `[name, value]` in received order, duplicates preserved. */
export type HeaderPair = [string, string];

// ---------- inbox files (webhooks/<slug>.webhook.yaml) ----------

export type ProviderKind = 'none' | 'stripe' | 'github' | 'shopify' | 'slack' | 'twilio' | 'standardWebhooks' | 'genericHmac';
export type HmacAlgorithm = 'sha256' | 'sha1' | 'sha512';
export type SignatureEncoding = 'hex' | 'base64';

export interface GenericHmac {
  header: string;
  algorithm?: HmacAlgorithm;
  encoding?: SignatureEncoding;
  /** e.g. `sha256=` */
  prefix?: string;
}

export interface ResponseSpec {
  /** Default 200. */
  status?: number;
  headers?: Record<string, string>;
  body?: string;
  delayMs?: number;
}

export interface ForwardSpec {
  /** May use `{{variables}}`. */
  url: string;
  /** Default true. */
  enabled?: boolean;
  preserveHost?: boolean;
  /** Default 10000. */
  timeoutMs?: number;
  /** Answer the sender with the local app's response. */
  passThrough?: boolean;
}

export interface InboxFile {
  version?: number;
  name: string;
  path: string;
  provider?: ProviderKind;
  genericHmac?: GenericHmac;
  /** `{{secret.NAME}}` — never a literal secret. */
  secret?: string;
  response?: ResponseSpec;
  forward?: ForwardSpec;
  /** Exact public URL the provider calls (Twilio signs it). */
  publicUrl?: string;
  /** Default 300. */
  toleranceSeconds?: number;
  description?: string;
  [extra: string]: unknown;
}

export interface Issue {
  field: string;
  message: string;
}

export interface InboxItem {
  /** File stem under `webhooks/`. */
  id: string;
  file: InboxFile | null;
  error: string | null;
  issues: Issue[];
  localUrl: string | null;
  lanUrl: string | null;
  publicUrl: string | null;
  events: number;
}

export interface ProviderInfo {
  kind: ProviderKind;
  label: string;
  signatureHeaders: string[];
  secretName: string | null;
  secretHint: string;
  algorithm: string;
  timestamped: boolean;
  docsUrl: string | null;
}

// ---------- receiver ----------

export interface ReceiverInfo {
  port: number;
  bindAll: boolean;
  localUrl: string;
  lanUrl: string | null;
  startedAt: number;
}

export interface ReceiverStatus {
  running: boolean;
  info: ReceiverInfo | null;
  /** Tunnel URL exposing the receiver this session. */
  publicBase: string | null;
  env: string | null;
  defaultPort: number;
}

// ---------- events ----------

export type VerifyStatus = 'valid' | 'mismatch' | 'expired' | 'missingHeader' | 'malformed' | 'missingSecret';

export interface VerifyResult {
  provider: ProviderKind;
  valid: boolean;
  status: VerifyStatus;
  reason: string;
  /** Masked. */
  expected: string | null;
  received: string | null;
  computedOver: string | null;
  signedUrl: string | null;
  timestamp: number | null;
  timestampAgeSeconds: number | null;
  toleranceSeconds: number | null;
  hints: string[];
  checkedAt: number;
}

export interface ForwardResult {
  url: string;
  at: number;
  status: number | null;
  statusText: string;
  headers: HeaderPair[];
  bodyExcerpt: string | null;
  bodySize: number;
  durationMs: number;
  error: string | null;
}

export type EventSource = 'local' | 'relay' | 'replay' | 'import';

export interface WebhookEvent {
  id: string;
  inbox: string | null;
  receivedAt: number;
  source: EventSource;
  method: string;
  url: string;
  path: string;
  query: string;
  headers: HeaderPair[];
  bodySize: number;
  bodyTruncated: boolean;
  contentType: string | null;
  remoteAddr: string | null;
  publicUrl: string | null;
  label: string | null;
  provider: ProviderKind | null;
  responseStatus: number | null;
  verification: VerifyResult | null;
  forward: ForwardResult | null;
  replayOf: string | null;
  /** Id at the source (relay event id) for relayed/imported events. */
  externalId: string | null;
}

export interface EventView extends WebhookEvent {
  bodyText: string | null;
  bodyBase64: string | null;
}

export interface SignatureBadge {
  valid: boolean;
  status: VerifyStatus;
  reason: string;
}

export interface EventSummary {
  id: string;
  inbox: string | null;
  receivedAt: number;
  source: EventSource;
  method: string;
  path: string;
  query: string;
  label: string | null;
  bodySize: number;
  contentType: string | null;
  remoteAddr: string | null;
  responseStatus: number | null;
  signature: SignatureBadge | null;
  forwardStatus: number | null;
  forwardError: string | null;
  forwardMs: number | null;
  replayOf: string | null;
}

export interface EventQuery {
  /** Inbox id; `''` = events that matched no inbox; absent = all. */
  inbox?: string;
  search?: string;
  offset?: number;
  limit?: number;
}

export interface EventPage {
  events: EventSummary[];
  total: number;
}

export type StoreNotice =
  | { type: 'added'; event: EventSummary }
  | { type: 'updated'; event: EventSummary }
  | { type: 'deleted'; id: string }
  | { type: 'cleared'; inbox: string | null }
  | { type: 'error'; message: string };

export interface Retention {
  maxEvents?: number | null;
  maxAgeDays?: number | null;
}

export interface ReplayEdits {
  method?: string;
  /** Replaces all headers. */
  headers?: HeaderPair[];
  body?: string;
  bodyBase64?: string;
}

export interface ReplayRequest {
  /** Defaults to the inbox's forward URL, then the original URL. */
  target?: string;
  edits?: ReplayEdits;
  /** Fresh signature headers with the current timestamp. */
  resign: boolean;
  env?: string;
}

// ---------- tunnels ----------

export type TunnelKind = 'cloudflared' | 'ngrok' | 'tailscale';
export type TunnelState = 'starting' | 'online' | 'error' | 'stopped';

export interface ToolInfo {
  kind: TunnelKind;
  label: string;
  installed: boolean;
  path: string | null;
  version: string | null;
  /** ngrok: authtoken configured; tailscale: logged in; cloudflared: null (no account needed). */
  ready: boolean | null;
  hint: string | null;
  installHint: string;
  notes: string;
  docsUrl: string;
  predictedUrl: string | null;
}

export interface TunnelStatus {
  id: string;
  kind: TunnelKind;
  port: number;
  state: TunnelState;
  publicUrl: string | null;
  connected: boolean;
  message: string | null;
  hint: string | null;
  pid: number | null;
  startedAt: number;
  exitCode: number | null;
  log: string[];
}

export type CloudflaredMode = 'quick' | 'named' | 'token';

/** Bring-your-own tunnel settings (`<app data>/tunnels.json`; credentials are in the keychain). */
export interface TunnelSettings {
  ngrok: {
    program?: string | null;
    configFile?: string | null;
    domain?: string | null;
    region?: string | null;
    basicAuthUser?: string | null;
    oauthProvider?: string | null;
    oauthAllowEmails: string[];
    oauthAllowDomains: string[];
    extraArgs: string;
  };
  cloudflared: {
    program?: string | null;
    mode: CloudflaredMode;
    tunnelName?: string | null;
    hostname?: string | null;
    configFile?: string | null;
    extraArgs: string;
  };
  tailscale: {
    program?: string | null;
    httpsPort?: number | null;
    path?: string | null;
    extraArgs: string;
  };
}

export type TunnelSecretName = 'ngrok-authtoken' | 'ngrok-basic-auth-password' | 'cloudflared-token';

export interface TunnelSettingsView {
  settings: TunnelSettings;
  /** Which credentials are stored in the keychain (never values). */
  secrets: Partial<Record<TunnelSecretName, boolean>>;
  keychainAvailable: boolean;
}

export interface TunnelTest {
  ok: boolean;
  publicUrl: string | null;
  message: string;
  hint: string | null;
}

export const webhooksApi = {
  providers: () => invoke<ProviderInfo[]>('wh_providers'),
  list: (root: string) => invoke<InboxItem[]>('wh_inboxes_list', { root }),
  read: (root: string, id: string) => invoke<InboxFile>('wh_inboxes_read', { root, id }),
  save: (root: string, id: string, inbox: InboxFile) => invoke<Issue[]>('wh_inboxes_save', { root, id, inbox }),
  create: (root: string, name: string, provider: ProviderKind) => invoke<string>('wh_inboxes_create', { root, name, provider }),
  remove: (root: string, id: string) => invoke<void>('wh_inboxes_delete', { root, id }),

  receiverStart: (root: string, opts: { port?: number; bindAll?: boolean; env?: string }) =>
    invoke<ReceiverStatus>('wh_receiver_start', { root, port: opts.port, bindAll: opts.bindAll, env: opts.env }),
  receiverStop: (root: string) => invoke<ReceiverStatus>('wh_receiver_stop', { root }),
  receiverStatus: (root: string) => invoke<ReceiverStatus>('wh_receiver_status', { root }),
  setEnv: (root: string, env: string | undefined) => invoke<void>('wh_set_env', { root, env }),

  events: (root: string, query: EventQuery) => invoke<EventPage>('wh_events', { root, page: query }),
  event: (root: string, id: string) => invoke<EventView | null>('wh_event', { root, id }),
  subscribe: (root: string, onEvent: (n: StoreNotice) => void) => {
    const ch = new Channel<StoreNotice>();
    ch.onmessage = onEvent;
    return invoke<string>('wh_subscribe', { root, onEvent: ch });
  },
  unsubscribe: (id: string) => invoke<boolean>('wh_unsubscribe', { id }),
  verify: (root: string, id: string, env?: string) => invoke<VerifyResult>('wh_verify', { root, id, env }),
  replay: (root: string, id: string, r: ReplayRequest) => invoke<EventView>('wh_replay', { root, id, target: r.target, edits: r.edits, resign: r.resign, env: r.env }),
  copyCurl: (root: string, id: string, r: ReplayRequest) => invoke<string>('wh_copy_curl', { root, id, target: r.target, edits: r.edits, resign: r.resign, env: r.env }),
  saveAsRequest: (root: string, id: string, folder?: string, target?: string) => invoke<string>('wh_save_as_request', { root, id, folder, target }),
  saveFixture: (root: string, id: string) => invoke<string>('wh_save_fixture', { root, id }),
  deleteEvent: (root: string, id: string) => invoke<boolean>('wh_delete', { root, id }),
  clear: (root: string, inbox?: string) => invoke<number>('wh_clear', { root, inbox }),
  retention: (root: string) => invoke<Retention>('wh_retention', { root }),
  setRetention: (root: string, retention: Retention) => invoke<number>('wh_set_retention', { root, retention }),

  tunnelDetect: () => invoke<ToolInfo[]>('tunnel_detect'),
  tunnelStart: (kind: TunnelKind, port: number, root: string | undefined, onStatus: (s: TunnelStatus) => void, program?: string) => {
    const ch = new Channel<TunnelStatus>();
    ch.onmessage = onStatus;
    return invoke<TunnelStatus>('tunnel_start', { kind, port, root, program, onStatus: ch });
  },
  tunnelStop: (id: string) => invoke<void>('tunnel_stop', { id }),
  tunnelStatus: () => invoke<TunnelStatus[]>('tunnel_status'),
  tunnelSettings: () => invoke<TunnelSettingsView>('tunnel_settings_get'),
  tunnelSettingsSave: (settings: TunnelSettings) => invoke<void>('tunnel_settings_save', { settings }),
  /** Stores a credential in the OS keychain; `null`/'' removes it. */
  tunnelSecretSet: (name: TunnelSecretName, value: string | null) => invoke<void>('tunnel_secret_set', { name, value }),
  /** Starts the tool briefly with the saved settings to check them. */
  tunnelTest: (kind: TunnelKind) => invoke<TunnelTest>('tunnel_test', { kind }),
  /** The command a tunnel would run for `port`, credentials masked. */
  tunnelCommand: (kind: TunnelKind, port: number) => invoke<string>('tunnel_command', { kind, port }),
};

/** Header value lookup (case-insensitive). */
export function headerValue(headers: HeaderPair[], name: string): string | undefined {
  const n = name.toLowerCase();
  return headers.find(([k]) => k.toLowerCase() === n)?.[1];
}

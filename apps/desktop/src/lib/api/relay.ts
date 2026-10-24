import { Channel, invoke } from '@tauri-apps/api/core';

/** Mirrors `crates/devall-relay` + `src-tauri/src/modules/relay.rs`. Errors reject with a string. */

export type ConnState = 'connecting' | 'connected' | 'reconnecting' | 'stopped' | 'failed';
export type RelayMode = 'hosted' | 'selfHosted';
export type HeaderList = [string, string][];

export interface Limits {
  sessionSeconds: number | null;
  bandwidthBytesPerMonth: number | null;
  maxRequestBodyBytes: number | null;
  requestTimeoutMs: number;
  maxTunnels: number | null;
  interstitial: boolean;
  inboxRetentionSeconds: number;
  inboxEventsPerDay: number | null;
  inboxMaxBuffered: number | null;
  inboxMaxBodyBytes: number;
  maxInboxes: number | null;
  reservedSubdomains: number | null;
  ipRequestsPerMinute: number | null;
  // Batch D (absent on older relays)
  customDomains?: number | null;
  tcpTunnels?: boolean;
  tcpStreamsPerTunnel?: number | null;
  hostedMocks?: number | null;
  mockMaxStubs?: number | null;
  mockMaxBytes?: number | null;
  mockRequestsPerDay?: number | null;
  mockJournalSize?: number;
  shares?: number | null;
  shareMaxBytes?: number;
  shareTtlSeconds?: number;
  monitors?: number | null;
  monitorMinIntervalMinutes?: number;
  monitorMaxRequests?: number;
  monitorHistory?: number;
  // Cloud load-generator regions (0 = not on this plan).
  loadRegions?: number;
  loadMaxDurationSeconds?: number;
  loadMaxRps?: number | null;
  loadRunsPerDay?: number | null;
  loadPrivateTargets?: boolean;
}

export interface Usage {
  bandwidthBytes: number;
  requests: number;
  periodStart: string;
  periodEnd: string;
}

export interface Quota {
  kind: 'bandwidth' | 'session' | 'inbox_daily' | 'inbox_buffer' | 'rate' | string;
  used: number;
  limit: number;
  resetAt?: string | null;
  message: string;
}

export interface TunnelStatus {
  state: ConnState;
  local: string;
  publicUrl: string | null;
  /** `publicUrl` plus the custom domains serving the tunnel. */
  publicUrls?: string[];
  subdomain: string | null;
  plan: string | null;
  sessionExpiresAt: string | null;
  limits: Limits | null;
  usage: Usage | null;
  quota: Quota | null;
  requests: number;
  bytesIn: number;
  bytesOut: number;
  activeStreams: number;
  websockets: number;
  reconnects: number;
  lastError: string | null;
  connectedAtMs: number | null;
}

export interface RequestLog {
  stream: number;
  atMs: number;
  method: string;
  path: string;
  status: number | null;
  durationMs: number;
  bytesIn: number;
  bytesOut: number;
  clientIp: string | null;
  websocket: boolean;
  error: string | null;
}

export type TunnelEvent = ({ type: 'status' } & TunnelStatus) | ({ type: 'request' } & RequestLog);

export interface InboxResponse {
  status: number;
  headers: HeaderList;
  body: string;
}

export interface InboxState {
  state: ConnState;
  url: string | null;
  inboxId: string | null;
  plan: string | null;
  buffered: number;
  delivered: number;
  duplicates: number;
  failures: number;
  todayCount: number;
  dailyLimit: number | null;
  retentionSeconds: number | null;
  lastEventAt: string | null;
  lastError: string | null;
  quota: Quota | null;
  response: InboxResponse | null;
  reconnects: number;
}

export interface InboxDelivery {
  id: string;
  method: string;
  path: string;
  receivedAt: string;
  bodySize: number;
  duplicate: boolean;
  atMs: number;
}

export type InboxUpdate = ({ type: 'status' } & InboxState) | ({ type: 'delivered' } & InboxDelivery);

/**
 * A webhook captured by the hosted inbox (what devall-webhooks `ingest` receives).
 * Delivery is at-least-once: dedupe by `id`.
 */
export interface InboxEvent {
  id: string;
  seq: number;
  inboxId: string;
  method: string;
  /** Exact public URL the sender called. */
  url: string;
  path: string;
  headers: HeaderList;
  /** Raw body, byte-for-byte. */
  bodyBase64: string;
  bodySize: number;
  receivedAt: string;
  sourceIp: string | null;
}

export interface AccountInfo {
  uid: string;
  email: string | null;
  plan: 'free' | 'supporter' | 'selfhost' | string;
  selfHosted: boolean;
  domain: string;
  limits: Limits;
  usage: Usage;
  tunnels: { subdomain: string; url: string; since: string }[];
  inboxes: { projectKey: string; inboxId: string; url: string }[];
  reservedSubdomains: string[];
  upgradeUrl: string | null;
  /** Custom domains serving this account's tunnels (see `cloud.ts`). */
  customDomains?: import('./cloud').CustomDomain[];
  customDomainTarget?: string | null;
  hostedMocks?: { name: string; url: string }[];
}

export interface InboxView {
  root: string;
  state: InboxState;
}

export interface RelayStatus {
  mode: RelayMode;
  endpoint: string;
  signedIn: boolean;
  hostedConfigured: boolean;
  keychainAvailable: boolean;
  tunnel: TunnelStatus | null;
  inboxes: InboxView[];
}

export interface RelaySettings {
  mode: RelayMode;
  selfHostedEndpoint: string;
  hasToken: boolean;
  hostedEndpoint: string;
  hostedConfigured: boolean;
}

export interface SignedIn {
  uid: string | null;
  email: string | null;
}

export const relayApi = {
  status: () => invoke<RelayStatus>('relay_status'),
  /** Opens the browser; resolves after the loopback callback (up to 10 minutes). */
  signIn: () => invoke<SignedIn>('relay_sign_in'),
  signOut: () => invoke<void>('relay_sign_out'),
  account: () => invoke<AccountInfo>('relay_account'),
  tunnelStart: (port: number, subdomain: string | undefined, rewriteHost: boolean, onEvent: (e: TunnelEvent) => void) => {
    const ch = new Channel<TunnelEvent>();
    ch.onmessage = onEvent;
    return invoke<TunnelStatus>('relay_tunnel_start', { port, subdomain, rewriteHost, onEvent: ch });
  },
  tunnelStop: () => invoke<void>('relay_tunnel_stop'),
  inboxConnect: (root: string, onEvent: (e: InboxUpdate) => void) => {
    const ch = new Channel<InboxUpdate>();
    ch.onmessage = onEvent;
    return invoke<InboxState>('relay_inbox_connect', { root, onEvent: ch });
  },
  inboxDisconnect: (root: string) => invoke<void>('relay_inbox_disconnect', { root }),
  settingsGet: () => invoke<RelaySettings>('relay_settings_get'),
  /** `token` is stored in the OS keychain; omit it to keep the stored one. */
  settingsSet: (mode: RelayMode, endpoint?: string, token?: string) => invoke<RelaySettings>('relay_settings_set', { mode, endpoint, token }),
  /** Opens the Supporter page in the browser and returns its URL. */
  upgrade: () => invoke<string>('relay_upgrade_url'),
};

/** Splits a TunnelEvent into its payload (the `type` tag is dropped). */
export function eventPayload<T extends { type: string }>(e: T): Omit<T, 'type'> {
  const { type: _type, ...rest } = e;
  void _type;
  return rest;
}

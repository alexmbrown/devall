import { Channel, invoke } from '@tauri-apps/api/core';
import type { ConnState, HeaderList, Limits, Quota, Usage } from './relay';

/**
 * Hosted extras (Group 2, Batch D) — mirrors `src-tauri/src/modules/cloud.rs` and the relay's
 * `/v1/*` API (relay/PROTOCOL.md §6). Errors reject with a string.
 */

export interface CustomDomainRecord {
  type: 'CNAME' | 'TXT' | string;
  name: string;
  value: string;
  /** `routing` (required), `ownership` / `certificate` (only if the CNAME can't come first). */
  purpose: string;
}

export interface CustomDomain {
  hostname: string;
  subdomain: string;
  status: 'pending' | 'active' | 'error' | 'static' | string;
  sslStatus: string | null;
  records: CustomDomainRecord[];
  errors: string[];
  source: 'hosted' | 'static' | string;
  updatedAt: string | null;
}

export interface TcpTunnelStatus {
  state: ConnState;
  local: string;
  name: string | null;
  plan: string | null;
  sessionExpiresAt: string | null;
  limits: Limits | null;
  usage: Usage | null;
  quota: Quota | null;
  hasSecret: boolean;
  connections: number;
  totalConnections: number;
  bytesIn: number;
  bytesOut: number;
  reconnects: number;
  lastError: string | null;
  connectedAtMs: number | null;
}

export interface TcpConnLog {
  stream: number;
  atMs: number;
  clientIp: string | null;
  via: string;
  durationMs: number;
  bytesIn: number;
  bytesOut: number;
  error: string | null;
}

export type TcpTunnelEvent = ({ type: 'status' } & TcpTunnelStatus) | ({ type: 'connection' } & TcpConnLog);

export interface TcpConnectStatus {
  name: string;
  listen: string;
  running: boolean;
  connections: number;
  totalConnections: number;
  bytesIn: number;
  bytesOut: number;
  lastError: string | null;
}

export type TcpConnectEvent = ({ type: 'status' } & TcpConnectStatus) | ({ type: 'connection' } & TcpConnLog);

export interface TcpView {
  publisher: TcpTunnelStatus | null;
  secret: string | null;
  consumers: TcpConnectStatus[];
}

export interface MockWarning {
  stub: string;
  feature: string;
  message: string;
}

export interface PublishPlan {
  stubs: number;
  bytes: number;
  files: string[];
  skippedFiles: [string, string][];
  warnings: MockWarning[];
  blocking: string[];
}

export interface MockPlanView {
  suggestedName: string;
  plan: PublishPlan;
  loadErrors: string[];
}

export interface MockPublished {
  name: string;
  url: string;
  hostUrl: string | null;
  pathUrl: string | null;
  stubs: number;
  bytes: number;
  files: number;
  warnings: MockWarning[];
  createdAt: string | null;
  updatedAt: string | null;
}

export interface HostedMock {
  name: string;
  url: string;
  hostUrl: string;
  pathUrl: string;
  stubs: number;
  bytes: number;
  createdAt: string;
  updatedAt: string;
  requestsToday: number;
  settings: { cors: boolean; globalTemplating: boolean };
}

export interface MockJournalEntry {
  id: string;
  at: string;
  method: string;
  url: string;
  headers: HeaderList;
  body: string;
  bodySize: number;
  clientIp: string | null;
  matched: boolean;
  stubId: string | null;
  stubName: string | null;
  status: number;
  durationMs: number;
}

export interface ShareInfo {
  id: string;
  title: string;
  url: string;
  createdAt: string;
  expiresAt: string;
  bytes: number;
}

export interface ShareCreated {
  id: string;
  url: string;
  createdAt: string;
  expiresAt: string;
  bytes: number;
}

export type AssertTarget = 'status' | 'header' | 'jsonPath' | 'body' | 'responseTime' | 'size' | 'contentType';

export interface MonitorAssertion {
  assert: AssertTarget;
  name?: string;
  path?: string;
  op: string;
  value?: string | null;
}

export interface MonitorRequest {
  name: string;
  method: string;
  url: string;
  headers: HeaderList;
  body?: string | null;
  basicAuth?: { username: string; password: string } | null;
  timeoutMs?: number;
  followRedirects?: boolean;
  assertions: MonitorAssertion[];
}

export interface MonitorAlerts {
  webhookUrl?: string | null;
  slackWebhookUrl?: string | null;
  onRecovery?: boolean;
}

export interface MonitorCheck {
  name: string;
  method: string;
  url: string;
  status: number | null;
  durationMs: number;
  ok: boolean;
  failures: string[];
  error: string | null;
}

export interface MonitorRun {
  at: string;
  ok: boolean;
  durationMs: number;
  checks: MonitorCheck[];
  trigger: 'schedule' | 'manual';
}

export interface MonitorView {
  id: string;
  name: string;
  intervalMinutes: number;
  effectiveIntervalMinutes: number;
  requests: MonitorRequest[];
  alerts: MonitorAlerts;
  secretNames: string[];
  paused: boolean;
  pausedReason: string | null;
  createdAt: string;
  updatedAt: string;
  lastRunAt: string | null;
  lastOk: boolean | null;
  lastAlertError: string | null;
  uptime: number | null;
  runs: number;
  recent: { at: string; ok: boolean; durationMs: number }[];
  last: MonitorRun | null;
}

export interface MonitorLimits {
  monitors: number | null;
  monitorMinIntervalMinutes: number;
  monitorMaxRequests: number;
  monitorHistory: number;
  secretsAvailable: boolean;
}

export interface MonitorInput {
  name: string;
  intervalMinutes: number;
  requests: MonitorRequest[];
  alerts?: MonitorAlerts | null;
  paused?: boolean;
}

export interface MonitorCandidate {
  path: string;
  name: string;
  method: string;
}

export interface MonitorDraft {
  requests: MonitorRequest[];
  secretNames: string[];
  warnings: [string, string][];
}

export const MONITOR_INTERVALS = [5, 10, 15, 30, 60, 180, 360, 720, 1440] as const;

function channel<T>(fn: (e: T) => void): Channel<T> {
  const ch = new Channel<T>();
  ch.onmessage = fn;
  return ch;
}

const call = <T>(method: string, path: string, body?: unknown) => invoke<T>('relay_api', { method, path, body });
const enc = encodeURIComponent;

export const cloudApi = {
  // raw TCP tunnels
  tcpStatus: () => invoke<TcpView>('relay_tcp_status'),
  tcpStart: (port: number, host: string | undefined, subdomain: string | undefined, useSecret: boolean, onEvent: (e: TcpTunnelEvent) => void) =>
    invoke<TcpView>('relay_tcp_start', { port, host, subdomain, useSecret, onEvent: channel(onEvent) }),
  tcpStop: () => invoke<void>('relay_tcp_stop'),
  /** The access secret teammates use (kept in the OS keychain); `rotate` makes a new one. */
  tcpSecret: (rotate: boolean) => invoke<string>('relay_tcp_secret', { rotate }),
  tcpConnect: (name: string, localPort: number, secret: string | undefined, onEvent: (e: TcpConnectEvent) => void) =>
    invoke<TcpConnectStatus>('relay_tcp_connect', { name, localPort, secret, onEvent: channel(onEvent) }),
  tcpDisconnect: (name: string) => invoke<void>('relay_tcp_disconnect', { name }),

  // custom domains
  domains: () => call<{ target: string; domains: CustomDomain[] }>('GET', '/v1/domains'),
  domainAdd: (hostname: string, subdomain: string) => invoke<CustomDomain>('relay_domain_add', { hostname, subdomain }),
  domainRemove: (hostname: string) => invoke<unknown>('relay_domain_remove', { hostname }),
  domainRefresh: (hostname: string) => invoke<CustomDomain>('relay_domain_refresh', { hostname }),

  // hosted mocks
  mockPlan: (root: string, cors: boolean, globalTemplating: boolean) => invoke<MockPlanView>('relay_mock_plan', { root, cors, globalTemplating }),
  mockPublish: (root: string, name: string, cors: boolean, globalTemplating: boolean) => invoke<MockPublished>('relay_mock_publish', { root, name, cors, globalTemplating }),
  mocks: () => call<{ mocks: HostedMock[]; limits: Partial<Limits> }>('GET', '/v1/mocks'),
  mockUnpublish: (name: string) => call<unknown>('DELETE', `/v1/mocks/${enc(name)}`),
  mockJournal: (name: string, limit = 50) => call<{ requests: MockJournalEntry[] }>('GET', `/v1/mocks/${enc(name)}/requests?limit=${limit}`),
  mockJournalClear: (name: string) => call<unknown>('DELETE', `/v1/mocks/${enc(name)}/requests`),
  mockResetScenarios: (name: string) => call<unknown>('POST', `/v1/mocks/${enc(name)}/scenarios/reset`),

  // shared snapshots
  shareCreate: (root: string, historyId: string, opts: { title?: string; maskHeaders: boolean; includeResponseBody: boolean; ttlSeconds?: number }) =>
    invoke<ShareCreated>('relay_share_create', { root, historyId, ...opts }),
  shares: () => call<{ shares: ShareInfo[]; limits: { shares: number | null; shareMaxBytes: number; shareTtlSeconds: number } }>('GET', '/v1/shares'),
  shareRevoke: (id: string) => call<unknown>('DELETE', `/v1/shares/${enc(id)}`),

  // monitors
  monitors: () => call<{ monitors: MonitorView[]; limits: MonitorLimits }>('GET', '/v1/monitors'),
  monitorResults: (id: string, limit = 50) => call<{ results: MonitorRun[] }>('GET', `/v1/monitors/${enc(id)}/results?limit=${limit}`),
  monitorRun: (id: string) => call<MonitorRun>('POST', `/v1/monitors/${enc(id)}/run`),
  monitorDelete: (id: string) => call<unknown>('DELETE', `/v1/monitors/${enc(id)}`),
  /** Replaces a monitor's settings without touching its stored secrets. */
  monitorUpdate: (id: string, input: MonitorInput) => call<MonitorView>('PUT', `/v1/monitors/${enc(id)}`, input),
  monitorCandidates: (root: string) => invoke<MonitorCandidate[]>('relay_monitor_candidates', { root }),
  monitorPrepare: (root: string, paths: string[], env: string | undefined) => invoke<MonitorDraft>('relay_monitor_prepare', { root, paths, env }),
  /** Only the names in `uploadSecrets` are read from the keychain and uploaded (encrypted at rest on the relay). */
  monitorSave: (root: string, id: string | undefined, input: MonitorInput, env: string | undefined, uploadSecrets: string[], removeSecrets: string[]) =>
    invoke<MonitorView>('relay_monitor_save', { root, id, input, env, uploadSecrets, removeSecrets }),
};

/** "every 5 min", "hourly", "every 6 h", "daily". */
export function intervalLabel(m: number): string {
  if (m === 60) return 'hourly';
  if (m === 1440) return 'daily';
  if (m % 60 === 0) return `every ${m / 60} h`;
  return `every ${m} min`;
}

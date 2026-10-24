// Typed wrappers + TS mirrors for the load-testing commands in
// src-tauri/src/modules/load.rs (crate: crates/devall-load). Keep in sync with the Rust types.
import { Channel, invoke } from '@tauri-apps/api/core';

// ---------- test file (load/**/*.load.yaml) ----------

/** `30s`, `2m`, `1m30s`, `500ms`, `1.5s`, `1h`. */
export type DurationText = string;

export interface WeightedRequest {
  /** Request file relative to `http/`. */
  path: string;
  /** Omitted = 1. */
  weight?: number;
}

/** Exactly one of `requests` (weighted mix) or `flow` (ordered steps) is set. */
export interface Scenario {
  requests?: WeightedRequest[];
  flow?: string[];
}

export interface Stage {
  duration: DurationText;
  /** VUs (rampingVus) or iterations per timeUnit (rampingArrivalRate). */
  target: number;
}

export type Executor =
  | { type: 'constantVus'; vus: number; duration: DurationText }
  | { type: 'rampingVus'; startVus?: number; stages: Stage[] }
  | { type: 'constantArrivalRate'; rate: number; timeUnit?: DurationText; duration: DurationText; preAllocatedVus: number; maxVus?: number }
  | { type: 'rampingArrivalRate'; startRate?: number; timeUnit?: DurationText; stages: Stage[]; preAllocatedVus: number; maxVus?: number };

export type ExecutorType = Executor['type'];

export type ThinkTime = DurationText | { min: DurationText; max: DurationText };

export type Check =
  | { type: 'status'; expect: string[] }
  | { type: 'maxDuration'; duration: DurationText }
  | { type: 'bodyContains'; text: string };

export interface HttpSettings {
  timeoutMs?: number;
  maxConnections?: number;
  http2?: boolean;
  insecure?: boolean;
}

/** `mock` sends the load to the project's built-in mock server (a "mock baseline"). */
export type LoadTarget = 'requests' | 'mock';

export interface LoadTestFile {
  version?: number;
  name: string;
  description?: string;
  environment?: string;
  /** Omitted = `requests` (the saved requests' own hosts). */
  target?: LoadTarget;
  scenario: Scenario;
  executor: Executor;
  thinkTime?: ThinkTime;
  thresholds?: string[];
  checks?: Check[];
  http?: HttpSettings;
  [extra: string]: unknown;
}

export interface Issue {
  /** Dotted field path, e.g. `executor.stages[1].duration`. */
  field: string;
  message: string;
}

export interface LoadTestEntry {
  /** Relative to `load/`. */
  path: string;
  name: string;
  executor?: ExecutorType;
  runs: number;
  error?: string;
}

// ---------- results ----------

/** Milliseconds. */
export interface Percentiles {
  p50: number;
  p75: number;
  p90: number;
  p95: number;
  p99: number;
  p999: number;
  min: number;
  max: number;
  mean: number;
}

export interface ErrorKinds {
  connect: number;
  timeout: number;
  other: number;
}

export interface Totals {
  requests: number;
  failed: number;
  /** Fraction. */
  errorRate: number;
  iterations: number;
  droppedIterations: number;
  rps: number;
  iterationsPerSec: number;
  bytesIn: number;
  bytesOut: number;
  checksPassed: number;
  checksFailed: number;
  /** Fraction. */
  checksRate: number;
  maxVus: number;
}

export interface RequestStats {
  path: string;
  name: string;
  method: string;
  requests: number;
  failed: number;
  errorRate: number;
  rps: number;
  latency: Percentiles;
  statusCodes: Record<string, number>;
}

export interface TimelinePoint {
  /** Bucket start, seconds from the start. */
  t: number;
  requests: number;
  failed: number;
  rps: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
  vus: number;
  dropped: number;
}

export interface ThresholdResult {
  expr: string;
  ok: boolean;
  actual?: number;
  limit: number;
  /** `ms`, `%`, `/s` or empty. */
  unit: string;
  message: string;
}

export interface GitInfo {
  branch?: string;
  commit: string;
}

/** Set on runs that targeted the project's mock server instead of the real hosts. */
export interface MockBaseline {
  url: string;
  /** Hosts the requests normally go to. */
  replacedHosts?: string[];
}

export interface LoadSummary {
  version: number;
  runId: string;
  test: string;
  testPath: string;
  environment?: string;
  mockBaseline?: MockBaseline;
  startedAt: string;
  durationMs: number;
  plannedDurationMs: number;
  aborted: boolean;
  abortReason?: string;
  thresholdsPassed: boolean;
  executor: Executor;
  /** Latency measured from intended start (coordinated-omission corrected). */
  openModel: boolean;
  totals: Totals;
  latency: Percentiles;
  /** Open model: from the actual send. */
  serviceTime?: Percentiles;
  statusCodes: Record<string, number>;
  errorKinds: ErrorKinds;
  errorSamples?: string[];
  requests: RequestStats[];
  thresholds: ThresholdResult[];
  warnings?: string[];
  bucketSeconds: number;
  timeline: TimelinePoint[];
  git?: GitInfo;
  /** For distributed runs, the coordinator's machine. */
  machine: { os: string; cpus: number };
  /** Distributed runs: the agents that generated the load (percentiles come from their merged histograms). */
  agents?: AgentRun[];
}

export interface AgentRun {
  address: string;
  weight: number;
  /** Fraction of the load (0..1). */
  share: number;
  ok: boolean;
  error?: string;
  version?: string;
  machine: { os: string; cpus: number };
  requests: number;
  failed: number;
  /** The agent's own p95 (ms). */
  p95: number;
  abortedReason?: string;
}

/** A load agent (`devall load agent`) and its relative share of the load. */
export interface AgentTarget {
  /** `host:port`, `host` (port 7777), `https://…` or `dns:<headless service>:7777`. */
  address: string;
  weight: number;
}

export interface AgentInfo {
  version: string;
  protocol: number;
  machine: { os: string; cpus: number };
  busy: boolean;
  runId?: string;
}

export interface AgentCheck {
  address: string;
  info?: AgentInfo;
  error?: string;
}

export interface DistributedRun {
  agents: AgentTarget[];
  /** Omit to use the token stored in the OS keychain. */
  token?: string;
  rememberToken?: boolean;
  /** Explicit consent to send resolved `{{secret.X}}` values to the agents. */
  allowSecrets: boolean;
}

/** A run from the relay's cloud load-generator regions (Supporter / self-hosted relay). */
export interface CloudRun {
  /** Region ids, e.g. `weur`, `enam` (Durable Object location hints — best-effort placement). */
  regions: string[];
  /** Explicit consent to send resolved `{{secret.X}}` values to the relay's edge. */
  allowSecrets: boolean;
}

/** What a cloud run of a test would send (nothing is sent to prepare it). */
export interface CloudPrepare {
  /** `scheme://host[:port]` of every request: each must be verified on the relay first. */
  origins: string[];
  secretRequests: string[];
  warnings: string[];
  /** Why the test can't run from cloud regions (closed-model executor, mock target…). */
  error: string | null;
  plannedDurationMs: number;
}

export interface CloudTarget {
  key: string;
  origin: string;
  hostname: string;
  port: number;
  status: 'pending' | 'verified' | 'expired';
  token: string;
  /** Serve the token (just the token, HTTP 200, no redirect) here… */
  fileUrl: string;
  /** …or as a DNS TXT record here (covers every port of the host). */
  dnsName: string;
  verifiedAt: string | null;
  expiresAt: string | null;
  method: 'http' | 'dns' | null;
  lastError: string | null;
}

export interface CloudLoadInfo {
  /** False when the relay operator switched cloud load off (kill switch). */
  enabled: boolean;
  regions: { id: string; name: string }[];
  limits: { loadRegions: number; loadMaxDurationSeconds: number; loadMaxRps: number | null; loadRunsPerDay: number | null; loadPrivateTargets: boolean };
  runsToday: number;
  targets: CloudTarget[];
}

/** The relay target that covers `origin` (same host:port, or a DNS verification of the host). */
export function cloudTargetFor(targets: CloudTarget[], origin: string): CloudTarget | undefined {
  let u: URL;
  try {
    u = new URL(origin);
  } catch {
    return undefined;
  }
  const port = Number(u.port || (u.protocol === 'https:' ? 443 : 80));
  const host = u.hostname.toLowerCase();
  return targets.find((t) => t.hostname === host && t.port === port) ?? targets.find((t) => t.hostname === host && t.method === 'dns');
}

export type RunPhase = 'running' | 'stopping' | 'finished';

export interface LoadSnapshot {
  runId: string;
  phase: RunPhase;
  elapsedMs: number;
  plannedDurationMs: number;
  vus: number;
  vusAllocated: number;
  requests: number;
  failed: number;
  iterations: number;
  droppedIterations: number;
  rps: number;
  latency: Percentiles;
  bytesIn: number;
  bytesOut: number;
  statusCodes: Record<string, number>;
  errorKinds: ErrorKinds;
  /** Buckets completed since the previous snapshot — append them. */
  points: TimelinePoint[];
  bucketSeconds: number;
}

export interface Confirmation {
  reason: string;
  hosts: string[];
}

export interface Preflight {
  environment: string | null;
  openModel: boolean;
  plannedDurationMs: number;
  peakRate: number | null;
  expectedIterations: number | null;
  maxVus: number;
  hosts: string[];
  unresolved: string[];
  secretErrors: string[];
  warnings: string[];
  requiresConfirmation: Confirmation | null;
  /** Requests using `{{secret.X}}` — distributing them sends resolved values to agents. */
  secretRequests?: string[];
}

export type LoadRunResult =
  | { type: 'completed'; summary: LoadSummary; file: string | null; writeError: string | null }
  | { type: 'requiresConfirmation'; confirmation: Confirmation; preflight: Preflight };

export interface RunEntry {
  /** Relative to the project root. */
  file: string;
  runId: string;
  startedAt: string;
  durationMs: number;
  environment?: string;
  /** The run targeted the project's mock server. */
  mockBaseline?: boolean;
  aborted: boolean;
  thresholdsPassed: boolean;
  requests: number;
  rps: number;
  errorRate: number;
  p50: number;
  p95: number;
  p99: number;
  git?: GitInfo;
}

export type Better = 'lower' | 'higher' | 'neither';
export type Verdict = 'regression' | 'improvement' | 'unchanged';

export interface MetricDelta {
  key: string;
  label: string;
  unit: string;
  base: number;
  target: number;
  delta: number;
  deltaPct: number | null;
  better: Better;
  verdict: Verdict;
}

export interface RequestDelta {
  path: string;
  name: string;
  metrics: MetricDelta[];
}

export interface ThresholdDelta {
  expr: string;
  baseOk: boolean | null;
  targetOk: boolean | null;
}

export interface RunRef {
  runId: string;
  startedAt: string;
  environment?: string;
  mockBaseline?: boolean;
  commit?: string;
  aborted: boolean;
}

export interface Comparison {
  base: RunRef;
  target: RunRef;
  metrics: MetricDelta[];
  requests: RequestDelta[];
  thresholds: ThresholdDelta[];
  regressions: number;
  improvements: number;
  warnings: string[];
  /** Present when exactly one run is a mock baseline. */
  baseline?: BaselineBreakdown;
}

/** A real run against a mock baseline: the mock run's latency is client/tooling overhead, the rest is server + network. */
export interface BaselineBreakdown {
  /** Which run is the mock baseline. */
  mockSide: 'base' | 'target';
  rows: BreakdownRow[];
  requests: { path: string; name: string; realP95: number; mockP95: number; serverP95: number }[];
}

export interface BreakdownRow {
  key: string;
  label: string;
  real: number;
  mock: number;
  /** real − mock (≥ 0): server + network. */
  server: number;
  /** mock / real, 0..1. */
  clientShare: number;
}

// ---------- commands ----------

export const loadApi = {
  list: (root: string) => invoke<LoadTestEntry[]>('load_list', { root }),
  read: (root: string, path: string) => invoke<LoadTestFile>('load_read', { root, path }),
  /** Saves (drafts allowed) and returns validation issues. */
  save: (root: string, path: string, test: LoadTestFile) => invoke<Issue[]>('load_save', { root, path, test }),
  validate: (test: LoadTestFile) => invoke<Issue[]>('load_validate', { test }),
  create: (root: string, name: string, request?: string) => invoke<string>('load_create', { root, name, request }),
  remove: (root: string, path: string) => invoke<void>('load_delete', { root, path }),
  preflight: (root: string, path: string, test?: LoadTestFile, env?: string, againstMock?: boolean) => invoke<Preflight>('load_preflight', { root, path, test, env, againstMock }),
  /**
   * Runs the saved test; snapshots arrive ~4×/s. `env` is used when the test doesn't pin one.
   * `againstMock` starts the project's mock server if needed and runs against it (mock baseline).
   */
  run: (root: string, path: string, runId: string, env: string | undefined, confirm: boolean, onProgress: (s: LoadSnapshot) => void, againstMock?: boolean) => {
    const channel = new Channel<LoadSnapshot>();
    channel.onmessage = onProgress;
    return invoke<LoadRunResult>('load_run', { root, path, runId, env, confirm, againstMock, onProgress: channel });
  },
  stop: (runId: string) => invoke<boolean>('load_stop', { runId }),
  /** Runs the saved test across load agents and merges their results. */
  runDistributed: (root: string, path: string, runId: string, env: string | undefined, confirm: boolean, d: DistributedRun, onProgress: (s: LoadSnapshot) => void) => {
    const channel = new Channel<LoadSnapshot>();
    channel.onmessage = onProgress;
    return invoke<LoadRunResult>('load_run_distributed', { root, path, runId, env, confirm, agents: d.agents, token: d.token, rememberToken: d.rememberToken, allowSecrets: d.allowSecrets, onProgress: channel });
  },
  agentsCheck: (agents: AgentTarget[], token?: string) => invoke<AgentCheck[]>('load_agents_check', { agents, token }),
  cloudPrepare: (root: string, path: string, env?: string) => invoke<CloudPrepare>('load_cloud_prepare', { root, path, env }),
  cloudInfo: () => invoke<CloudLoadInfo>('load_cloud_info'),
  /** Adds the target (or returns it) with its verification token. */
  cloudTarget: (target: string) => invoke<CloudTarget>('load_cloud_target', { target }),
  cloudVerify: (target: string) => invoke<CloudTarget>('load_cloud_verify', { target }),
  cloudRemove: (target: string) => invoke<void>('load_cloud_remove', { target }),
  /** Runs the saved test from cloud regions; progress is elapsed time only (the relay answers at the end). */
  runCloud: (root: string, path: string, runId: string, env: string | undefined, c: CloudRun, onProgress: (s: LoadSnapshot) => void) => {
    const channel = new Channel<LoadSnapshot>();
    channel.onmessage = onProgress;
    return invoke<LoadRunResult>('load_cloud_run', { root, path, runId, env, regions: c.regions, allowSecrets: c.allowSecrets, onProgress: channel });
  },
  /** `''` forgets the stored token, a value stores it, `undefined` only reports whether one is stored. */
  agentsToken: (token?: string) => invoke<boolean>('load_agents_token', { token }),
  running: () => invoke<string[]>('load_running'),
  runs: (root: string, path: string) => invoke<RunEntry[]>('load_runs', { root, path }),
  readRun: (root: string, file: string) => invoke<LoadSummary>('load_read_run', { root, file }),
  deleteRun: (root: string, file: string) => invoke<void>('load_delete_run', { root, file }),
  /** `b` (target) compared against `a` (base). */
  compare: (root: string, a: string, b: string, tolerance?: number) => invoke<Comparison>('load_compare', { root, a, b, tolerance }),
  schema: () => invoke<unknown>('load_schema'),
};

// Typed wrappers + TS mirrors for the log viewer / event store commands in
// src-tauri/src/modules/logs.rs (crate: crates/devall-logs). Keep in sync with the Rust types.
import { Channel, invoke } from '@tauri-apps/api/core';

// ---------- entries ----------

export type Level = 'trace' | 'debug' | 'info' | 'warn' | 'error' | 'fatal' | 'unknown';
/** Order of `counts`/`levels` arrays in histograms and patterns. */
export const LEVELS: Level[] = ['trace', 'debug', 'info', 'warn', 'error', 'fatal', 'unknown'];

export type LogFormat = 'json' | 'docker' | 'cri' | 'logfmt' | 'accessLog' | 'syslog' | 'syslog5424' | 'klog' | 'prefixed' | 'plain';

export interface LogEntry {
  /** Position in the view (filters keep the original position). */
  index: number;
  /** 1-based first physical line (events: sequence number). */
  lineNo: number;
  lineCount: number;
  /** Unix ms. */
  ts: number | null;
  level: Level;
  message: string;
  fields: Record<string, unknown>;
  raw: string;
  format: LogFormat;
  correlationId?: string;
  /** Source file in merged views; module for events. */
  source?: string;
}

/** Query + level toggles + time range. */
export interface Filter {
  query: string;
  /** Empty = all. */
  levels: Level[];
  /** Inclusive Unix ms. */
  from?: number | null;
  /** Exclusive Unix ms. */
  to?: number | null;
}

export interface Page {
  total: number;
  viewTotal: number;
  offset: number;
  entries: LogEntry[];
  indexing: boolean;
  elapsedMs: number;
}

export interface QueryError {
  message: string;
  /** Character offsets. */
  start: number;
  end: number;
}

export interface Histogram {
  start: number;
  end: number;
  bucketMs: number;
  /** `counts` follow {@link LEVELS}. */
  buckets: { t: number; counts: number[] }[];
  total: number;
  withoutTs: number;
  max: number;
}

export interface Pattern {
  id: string;
  template: string;
  count: number;
  share: number;
  /** Follows {@link LEVELS}. */
  levels: number[];
  firstTs: number | null;
  lastTs: number | null;
  sample: string;
  sampleIndex: number;
  sparkline: number[];
  /** Query selecting the pattern (`msg~"…"`). */
  query: string;
}

export interface PatternResult {
  patterns: Pattern[];
  total: number;
  sampled: boolean;
  sparkStart: number;
  sparkBucketMs: number;
  templates: number;
}

export interface ValueCount {
  value: string;
  count: number;
}

export interface TopValues {
  field: string;
  scanned: number;
  present: number;
  distinct: number;
  values: ValueCount[];
  numeric: { min: number; max: number; avg: number; p50: number; p95: number } | null;
  sampled: boolean;
}

export interface FieldInfo {
  name: string;
  count: number;
  distinct: number;
  numeric: boolean;
  top: ValueCount[];
}

// ---------- sources ----------

export interface FileInfo {
  name: string;
  path: string;
  size: number;
  dataSize: number;
  encoding: 'utf8' | 'utf8Bom' | 'utf16Le' | 'utf16Be';
  compressed: boolean;
  format: LogFormat;
  formatLabel: string;
  tsMode: boolean;
  lines: number;
  entries: number;
  firstTs: number | null;
  lastTs: number | null;
  indexing: boolean;
  followable: boolean;
  indexBytes: number;
  error?: string;
}

export type SourceKind = { type: 'file'; path: string } | { type: 'merge'; paths: string[] } | { type: 'stream'; name: string };

export interface SourceInfo {
  handle: string;
  kind: SourceKind;
  name: string;
  files: FileInfo[];
  entries: number;
  indexing: boolean;
  followable: boolean;
}

export type Phase = 'starting' | 'decompressing' | 'transcoding' | 'indexing' | 'ready' | 'failed';

export interface Progress {
  phase: Phase;
  bytesDone: number;
  bytesTotal: number;
  lines: number;
  entries: number;
  elapsedMs: number;
  error?: string;
}

export interface OpenProgress {
  handle: string;
  /** Index into the merged paths (0 for single files). */
  file: number;
  progress: Progress;
}

export interface OpenOpts {
  /** Read zone-less timestamps as UTC (default: local time). */
  utc?: boolean;
  format?: LogFormat;
  name?: string;
}

export type TailMsg =
  | { type: 'appended'; total: number; lines: number; entries: LogEntry[] }
  | { type: 'reset'; reason: 'truncated' | 'rotated'; total: number }
  | { type: 'missing' }
  | { type: 'error'; message: string };

// ---------- SQL ----------

export interface SqlTarget {
  /** Exposes table `logs`. */
  handle?: string;
  /** Exposes table `events`. */
  root?: string;
}

export type SqlSource =
  | { type: 'file'; name: string; path: string }
  | { type: 'text'; name: string; text: string; format: 'json' | 'csv' }
  | { type: 'history'; name: string; id: string };

export interface SqlResult {
  columns: string[];
  rows: unknown[][];
  truncated: boolean;
  elapsedMs: number;
  backend: 'duckdb' | 'sqlite';
  statements: number;
}

export interface TableInfo {
  name: string;
  columns: [string, string][];
}

// ---------- events ----------

/** Well-known kinds; modules may add their own `<module>.<what>`. */
export const EVENT_KINDS = [
  'http.send',
  'http.run',
  'mock.request',
  'mock.started',
  'mock.stopped',
  'webhook.received',
  'load.run',
  'forward.started',
  'forward.stopped',
  'notebook.run',
  'db.query',
  'docker.event',
  'k8s.event',
  'app.note',
] as const;

export interface DevEvent {
  id: string;
  /** Unix ms. */
  at: number;
  kind: string;
  source: string;
  level: Level;
  title: string;
  correlationId?: string;
  /** Request file under `http/`. */
  requestPath?: string;
  data: unknown;
  durationMs?: number;
  seq: number;
}

export interface EventFilter {
  since?: number;
  until?: number;
  /** Exact kinds or prefixes (`http.*`). */
  kinds?: string[];
  sources?: string[];
  levels?: Level[];
  correlationId?: string;
  requestPath?: string;
  text?: string;
  afterSeq?: number;
  limit?: number;
  newestFirst?: boolean;
}

export interface EventStats {
  count: number;
  firstAt: number | null;
  lastAt: number | null;
  kinds: [string, number][];
}

export type EventTailMsg = { type: 'events'; events: DevEvent[] } | { type: 'lagged'; skipped: number };

// ---------- helpers ----------

/** Quotes a value for the query language (`"` and `\` escaped). */
export function quoteValue(v: string): string {
  return `"${v.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

/** `field=value` clause; bare when the value is a simple token. */
export function fieldClause(field: string, value: unknown, op = '='): string {
  const s = typeof value === 'string' ? value : JSON.stringify(value);
  const bare = /^[A-Za-z0-9_.\-/@:]+$/.test(s) && !s.includes('*');
  return `${field}${op}${bare ? s : quoteValue(s)}`;
}

function channel<T>(fn: (m: T) => void): Channel<T> {
  const ch = new Channel<T>();
  ch.onmessage = fn;
  return ch;
}

export const logsApi = {
  open: (path: string, onProgress: (p: OpenProgress) => void, options?: OpenOpts) =>
    invoke<SourceInfo>('logs_open', { path, options, onProgress: channel(onProgress) }),
  openText: (name: string, text: string) => invoke<SourceInfo>('logs_open_text', { name, text }),
  append: (handle: string, text: string) => invoke<number>('logs_append', { handle, text }),
  merge: (paths: string[], onProgress: (p: OpenProgress) => void, options?: OpenOpts) =>
    invoke<SourceInfo>('logs_merge', { paths, options, onProgress: channel(onProgress) }),
  sources: () => invoke<SourceInfo[]>('logs_sources'),
  info: (handle: string) => invoke<SourceInfo>('logs_info', { handle }),
  close: (handle: string) => invoke<boolean>('logs_close', { handle }),
  checkQuery: (query: string) => invoke<QueryError | null>('logs_check_query', { query }),

  query: (handle: string, filter: Filter, offset: number, limit: number) => invoke<Page>('logs_query', { handle, filter, offset, limit }),
  entry: (handle: string, index: number) => invoke<LogEntry | null>('logs_entry', { handle, index }),
  locate: (handle: string, filter: Filter, ts: number) => invoke<number | null>('logs_locate', { handle, filter, ts }),
  histogram: (handle: string, filter: Filter, buckets?: number) => invoke<Histogram>('logs_histogram', { handle, filter, buckets }),
  patterns: (handle: string, filter: Filter, maxEntries?: number) => invoke<PatternResult>('logs_patterns', { handle, filter, maxEntries }),
  topValues: (handle: string, filter: Filter, field: string, limit?: number) => invoke<TopValues>('logs_top_values', { handle, filter, field, limit }),
  fields: (handle: string, filter: Filter) => invoke<FieldInfo[]>('logs_fields', { handle, filter }),

  sql: (target: SqlTarget, sql: string, sources?: SqlSource[], maxRows?: number) => invoke<SqlResult>('logs_sql', { target, sql, sources, maxRows }),
  sqlTables: (target: SqlTarget) => invoke<[string, TableInfo[]]>('logs_sql_tables', { target }),

  tail: (handle: string, onLines: (m: TailMsg) => void, intervalMs?: number) => invoke<string>('logs_tail', { handle, onLines: channel(onLines), intervalMs }),
  untail: (tailId: string) => invoke<boolean>('logs_untail', { tailId }),
};

export const eventsApi = {
  query: (root: string, filter: Filter, offset: number, limit: number) => invoke<Page>('events_query', { root, filter, offset, limit }),
  list: (root: string, filter: EventFilter) => invoke<DevEvent[]>('events_list', { root, filter }),
  get: (root: string, id: string) => invoke<DevEvent | null>('events_get', { root, id }),
  correlated: (root: string, correlationId: string) => invoke<DevEvent[]>('events_correlated', { root, correlationId }),
  histogram: (root: string, filter: Filter, buckets?: number) => invoke<Histogram>('events_histogram', { root, filter, buckets }),
  patterns: (root: string, filter: Filter) => invoke<PatternResult>('events_patterns', { root, filter }),
  topValues: (root: string, filter: Filter, field: string, limit?: number) => invoke<TopValues>('events_top_values', { root, filter, field, limit }),
  locate: (root: string, filter: Filter, ts: number) => invoke<number | null>('events_locate', { root, filter, ts }),
  stats: (root: string) => invoke<EventStats>('events_stats', { root }),
  record: (root: string, event: Partial<DevEvent> & { kind: string; title: string }) => invoke<DevEvent>('events_record', { root, event }),
  /** Live events (batched). Stop with `logsApi.untail(id)`. */
  tail: (root: string, onEvent: (m: EventTailMsg) => void) => invoke<string>('events_tail', { root, onEvent: channel(onEvent) }),
  clear: (root: string) => invoke<number>('events_clear', { root }),
};

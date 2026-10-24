import { Channel, invoke } from '@tauri-apps/api/core';
import type { DataSchema } from '@/lib/api/data';

// Mirrors of crates/devall-db (model.rs, store.rs, sql/*, redisdb.rs, mongo.rs, session.rs) and
// the commands in src-tauri/src/modules/db.rs. Keep in sync.

export type DbKind = 'postgres' | 'mysql' | 'sqlite' | 'redis' | 'mongodb' | 'sqlserver';
export type Dialect = 'postgres' | 'mysql' | 'sqlite' | 'sqlserver';
export type TlsMode = 'disable' | 'prefer' | 'require' | 'verify-full';
export type HostKeyCheck = 'ask' | 'strict' | 'off';

export interface TlsConfig {
  mode?: TlsMode;
  /** PEM file (project-relative, absolute or ~/…). */
  caCert?: string;
  [extra: string]: unknown;
}

export interface SshConfig {
  host: string;
  port?: number | string;
  user: string;
  /** OpenSSH private key file. Without key and password the SSH agent is used. */
  privateKey?: string;
  /** Template, e.g. `{{secret.SSH_KEY_PASSPHRASE}}`. */
  passphrase?: string;
  /** Template, e.g. `{{secret.SSH_PASSWORD}}`. */
  password?: string;
  knownHostsCheck?: HostKeyCheck;
  [extra: string]: unknown;
}

/** `connections/<slug>.conn.yaml`. Passwords are `{{secret.X}}` templates only. */
export interface ConnectionFile {
  version?: number;
  name: string;
  kind: DbKind;
  url?: string;
  host?: string;
  port?: number | string;
  /** Database; SQLite: the file; Redis: the db index. */
  database?: string;
  user?: string;
  password?: string;
  tls?: TlsConfig;
  ssh?: SshConfig;
  readOnly: boolean;
  protected?: boolean;
  color?: string;
  options?: Record<string, string>;
  [extra: string]: unknown;
}

export interface ConnectionEntry {
  path: string;
  slug: string;
  name: string;
  kind?: DbKind;
  color?: string;
  readOnly: boolean;
  protected: boolean;
  target: string;
  ssh: boolean;
  error?: string;
}

export interface SessionInfo {
  id: string;
  name: string;
  kind: DbKind;
  dialect?: Dialect;
  readOnly: boolean;
  protected: boolean;
  color?: string;
  serverVersion: string;
  target: string;
  database?: string;
  tunnel?: string;
  connectedAt: number;
  warnings?: string[];
}

export interface OpenSession {
  root: string;
  path: string;
  env?: string;
  info: SessionInfo;
}

export interface HostKeyPrompt {
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
}

export type ConnectOutcome = { type: 'connected'; session: SessionInfo } | ({ type: 'hostKey' } & HostKeyPrompt);

export interface TestOutcome {
  ok: boolean;
  message: string;
  serverVersion?: string;
  elapsedMs: number;
  hostKey?: HostKeyPrompt;
  warnings?: string[];
}

// ---------- SQL ----------

export type ValueKind = 'bool' | 'int' | 'float' | 'decimal' | 'text' | 'json' | 'bytes' | 'date' | 'time' | 'timestamp' | 'uuid' | 'other';

export interface ColumnMeta {
  name: string;
  typeName: string;
  kind: ValueKind;
}

export interface StatementResult {
  index: number;
  sql: string;
  keyword: string;
  columns: ColumnMeta[];
  /** JSON values: big integers/decimals as strings, bytes as base64. */
  rows: unknown[][];
  rowsAffected?: number;
  more: boolean;
  offset: number;
  elapsedMs: number;
  notices?: string[];
  error?: string;
}

export interface QueryOutput {
  results: StatementResult[];
  elapsedMs: number;
  cancelled: boolean;
  totalStatements: number;
}

export type QueryProgress = { type: 'started'; index: number; total: number; sql: string } | { type: 'finished'; result: StatementResult };

export interface Statement {
  text: string;
  start: number;
  end: number;
  /** UTF-16 offsets (Monaco). */
  from: number;
  to: number;
}

export type TableKind = 'table' | 'view' | 'materializedView' | 'foreignTable';

export interface ColumnInfo {
  name: string;
  dataType: string;
  nullable: boolean;
  default?: string;
  primaryKey: boolean;
}

export interface TableNode {
  name: string;
  kind: TableKind;
  columns: ColumnInfo[];
  rowsEstimate?: number;
}

export interface SchemaTree {
  schemas: { name: string; tables: TableNode[] }[];
  defaultSchema?: string;
}

export interface IndexInfo {
  name: string;
  columns: string[];
  unique: boolean;
  primary: boolean;
  method?: string;
  definition?: string;
}

export interface ForeignKeyInfo {
  name: string;
  columns: string[];
  refSchema?: string;
  refTable: string;
  refColumns: string[];
  onUpdate?: string;
  onDelete?: string;
}

export interface TableDetails {
  schema?: string;
  name: string;
  kind: TableKind;
  columns: ColumnInfo[];
  primaryKey: string[];
  indexes: IndexInfo[];
  foreignKeys: ForeignKeyInfo[];
  ddl?: string;
}

export interface ExplainOutput {
  /** `postgres` | `mysql` | `sqlite` | `text` */
  format: string;
  plan: unknown;
  text?: string;
  rolledBack: boolean;
}

export type RowEdit =
  | { type: 'update'; key: Record<string, unknown>; values: Record<string, unknown> }
  | { type: 'insert'; values: Record<string, unknown> }
  | { type: 'delete'; key: Record<string, unknown> };

export interface EditRequest {
  schema?: string;
  table: string;
  edits: RowEdit[];
}

export interface ParamStatement {
  sql: string;
  params: unknown[];
  preview: string;
  expectOne: boolean;
}

export interface EditPreview {
  statements: ParamStatement[];
  script: string;
}

export interface SeedResult {
  rows: number;
  target: string;
  elapsedMs: number;
}

// ---------- Redis ----------

/** A key: text, or `{ b64 }` for keys that are not valid UTF-8. */
export type RedisKey = string | { b64: string };

export interface KeyInfo {
  key: string;
  keyB64?: string;
  type: string;
  /** ms to expiry; -1 none; -2 gone. */
  ttlMs: number;
}

export interface ScanPage {
  cursor: string;
  keys: KeyInfo[];
  done: boolean;
  dbSize: number;
}

export interface RField {
  field: string;
  value: string;
}

export type RedisValue =
  | { type: 'none' }
  | { type: 'string'; value: string; base64?: string; size: number; truncated: boolean }
  | { type: 'hash'; fields: RField[]; len: number; cursor: string }
  | { type: 'list'; items: string[]; len: number; offset: number }
  | { type: 'set'; members: string[]; len: number; cursor: string }
  | { type: 'zset'; members: { member: string; score: number | string }[]; len: number; offset: number }
  | { type: 'stream'; entries: { id: string; fields: RField[] }[]; len: number; groups: unknown; next?: string | null }
  | { type: 'json'; value: unknown }
  | { type: 'unsupported'; typeName: string };

export interface KeyValue {
  kind: string;
  ttlMs: number;
  value: RedisValue;
}

export type RedisEdit =
  | { type: 'setString'; key: RedisKey; value: string; ttlMs?: number | null }
  | { type: 'hashSet'; key: RedisKey; field: string; value: string }
  | { type: 'hashDelete'; key: RedisKey; field: string }
  | { type: 'listSet'; key: RedisKey; index: number; value: string }
  | { type: 'listPush'; key: RedisKey; value: string; head?: boolean }
  | { type: 'listRemove'; key: RedisKey; value: string; count?: number }
  | { type: 'setAdd'; key: RedisKey; member: string }
  | { type: 'setRemove'; key: RedisKey; member: string }
  | { type: 'zAdd'; key: RedisKey; member: string; score: number }
  | { type: 'zRemove'; key: RedisKey; member: string }
  | { type: 'streamAdd'; key: RedisKey; id?: string; fields: [string, string][] }
  | { type: 'streamDelete'; key: RedisKey; id: string }
  | { type: 'jsonSet'; key: RedisKey; path?: string; value: string }
  | { type: 'expire'; key: RedisKey; ttlMs?: number | null }
  | { type: 'rename'; key: RedisKey; newKey: string };

export interface CommandReply {
  command: string;
  text: string;
  value: unknown;
  elapsedMs: number;
  error?: string;
}

export interface InfoSection {
  name: string;
  entries: [string, string][];
}

export interface SlowlogEntry {
  id: number;
  timestamp: number;
  durationUs: number;
  command: string;
  client: string;
  clientName: string;
}

export interface PubSubMessage {
  channel: string;
  pattern?: string;
  payload: string;
  at: number;
}

// ---------- MongoDB ----------

export interface FindRequest {
  database: string;
  collection: string;
  /** Relaxed JSON / mongo-shell syntax. */
  filter?: string;
  projection?: string;
  sort?: string;
  skip?: number;
  limit?: number;
}

export interface AggregateRequest {
  database: string;
  collection: string;
  pipeline: string;
  limit?: number;
}

export interface Documents {
  docs: Record<string, unknown>[];
  more: boolean;
  elapsedMs: number;
}

export type MongoWrite =
  | { type: 'insert'; database: string; collection: string; documents: string }
  | { type: 'update'; database: string; collection: string; filter: string; update: string; many?: boolean; upsert?: boolean }
  | { type: 'replace'; database: string; collection: string; filter: string; replacement: string }
  | { type: 'delete'; database: string; collection: string; filter: string; many?: boolean };

// ---------- saved queries ----------

export type QueryKind = 'sql' | 'redis' | 'mongo';

export interface SavedQueryEntry {
  path: string;
  name: string;
  kind: QueryKind;
  connection?: string;
  error?: string;
}

export interface SavedQuery {
  path: string;
  kind: QueryKind;
  connection?: string;
  text: string;
}

/** `.mongo.json` saved query body. */
export interface MongoSavedQuery {
  connection?: string;
  type: 'find' | 'aggregate';
  database: string;
  collection: string;
  filter?: unknown;
  projection?: unknown;
  sort?: unknown;
  limit?: number;
  pipeline?: unknown[];
}

/** Typed wrappers over the `db_*` Tauri commands. Errors reject with a (secret-redacted) string. */
/** A table for "fake-data schema from database" (`schema` = the connection's default). */
export interface TableRef {
  schema?: string;
  name: string;
}

export interface FromDbOptions {
  /** Rows per table without foreign keys (tables with foreign keys get twice as many). */
  count?: number;
  /** Null rate (0–100) for nullable columns. */
  nullPercent?: number;
  seed?: number;
}

export interface FakeSchemaRequest {
  name?: string;
  /** SQL: empty = every table of the default schema. */
  tables?: TableRef[];
  /** MongoDB database (default: the connection's). */
  database?: string;
  /** MongoDB: empty = every collection. */
  collections?: string[];
  /** Documents sampled per MongoDB collection (default 50). */
  sample?: number;
  options?: FromDbOptions;
}

export interface FromDbResult {
  schema: DataSchema;
  warnings: string[];
}

export const dbApi = {
  // connection files
  list: (root: string) => invoke<ConnectionEntry[]>('db_connections_list', { root }),
  read: (root: string, path: string) => invoke<ConnectionFile>('db_connections_read', { root, path }),
  save: (root: string, path: string, connection: ConnectionFile) => invoke<void>('db_connections_save', { root, path, connection }),
  create: (root: string, name: string, kind: DbKind, connection?: ConnectionFile) => invoke<string>('db_connections_create', { root, name, kind, connection }),
  remove: (root: string, path: string) => invoke<void>('db_connections_delete', { root, path }),
  schema: () => invoke<unknown>('db_connection_schema'),

  // sessions
  test: (root: string, connection: ConnectionFile, env?: string, trustHostKey?: string) => invoke<TestOutcome>('db_test', { root, connection, env, trustHostKey }),
  connect: (root: string, path: string, env?: string, trustHostKey?: string) => invoke<ConnectOutcome>('db_connect', { root, path, env, trustHostKey }),
  disconnect: (session: string) => invoke<boolean>('db_disconnect', { session }),
  sessions: () => invoke<OpenSession[]>('db_sessions'),
  setReadOnly: (session: string, readOnly: boolean, confirm?: string) => invoke<SessionInfo>('db_set_readonly', { session, readOnly, confirm }),

  // SQL
  dbSchema: (session: string) => invoke<SchemaTree>('db_schema', { session }),
  table: (session: string, table: string, schema?: string) => invoke<TableDetails>('db_table', { session, schema, table }),
  split: (sql: string, dialect: Dialect) => invoke<Statement[]>('db_split', { sql, dialect }),
  query: (session: string, queryId: string, sql: string, opts: { limit?: number; offset?: number; onProgress?: (p: QueryProgress) => void } = {}) => {
    const onProgress = new Channel<QueryProgress>();
    if (opts.onProgress) onProgress.onmessage = opts.onProgress;
    return invoke<QueryOutput>('db_query', { session, queryId, sql, limit: opts.limit, offset: opts.offset, onProgress });
  },
  cancel: (queryId: string) => invoke<boolean>('db_cancel', { queryId }),
  explain: (session: string, sql: string, analyze: boolean) => invoke<ExplainOutput>('db_explain', { session, sql, analyze }),
  editPreview: (session: string, edit: EditRequest) => invoke<EditPreview>('db_edit_preview', { session, edit }),
  editApply: (session: string, edit: EditRequest) => invoke<number[]>('db_edit_apply', { session, edit }),
  insertRows: (session: string, table: string, rows: Record<string, unknown>[], schema?: string) => invoke<number>('db_insert_rows', { session, schema, table, rows }),
  seed: (session: string, root: string, dataPath: string, entity: string, table: string, schema?: string, count?: number) =>
    invoke<SeedResult>('db_seed', { session, root, dataPath, entity, schema, table, count }),
  /** Every entity of a data schema, parents first (each into its `table`). */
  seedAll: (session: string, root: string, dataPath: string, schema?: string, count?: number) => invoke<SeedResult[]>('db_seed_all', { session, root, dataPath, schema, count }),
  /** Tables/columns/keys as text for NL → SQL (never rows). */
  schemaSummary: (session: string, tables?: TableRef[], maxTables?: number) => invoke<string>('db_schema_summary', { session, tables, maxTables }),
  /** Field names/types of sampled documents for NL → MongoDB (values dropped). */
  mongoFields: (session: string, database: string, collection: string, sample?: number) => invoke<string>('db_mongo_fields', { session, database, collection, sample }),
  /** A fake-data schema from the connection's tables / sampled collections (not saved). */
  fakeSchema: (session: string, request: FakeSchemaRequest) => invoke<FromDbResult>('db_fake_schema', { session, request }),
  queryToJson: (session: string, sql: string, limit?: number) => invoke<string>('db_query_to_json', { session, sql, limit }),

  // Redis
  redisScan: (session: string, args: { cursor?: string; pattern?: string; typeFilter?: string; count?: number }) =>
    invoke<ScanPage>('db_redis_scan', { session, cursor: args.cursor, pattern: args.pattern, typeFilter: args.typeFilter, count: args.count }),
  redisGet: (session: string, key: RedisKey, page: { cursor?: string; offset?: number; count?: number } = {}) =>
    invoke<KeyValue>('db_redis_get', { session, key, cursor: page.cursor, offset: page.offset, count: page.count }),
  redisSet: (session: string, edit: RedisEdit) => invoke<CommandReply>('db_redis_set', { session, edit }),
  redisDelete: (session: string, keys: RedisKey[]) => invoke<number>('db_redis_delete', { session, keys }),
  redisCommand: (session: string, line: string) => invoke<CommandReply>('db_redis_command', { session, line }),
  redisScript: (session: string, script: string) => invoke<CommandReply[]>('db_redis_script', { session, script }),
  redisInfo: (session: string, section?: string) => invoke<InfoSection[]>('db_redis_info', { session, section }),
  redisSlowlog: (session: string, count?: number) => invoke<SlowlogEntry[]>('db_redis_slowlog', { session, count }),
  redisMemory: (session: string, key: RedisKey) => invoke<number | null>('db_redis_memory', { session, key }),
  redisSubscribe: (session: string, subId: string, channels: string[], patterns: string[], onMessage: (m: PubSubMessage) => void) => {
    const ch = new Channel<PubSubMessage>();
    ch.onmessage = onMessage;
    return invoke<void>('db_redis_subscribe', { session, subId, channels, patterns, onMessage: ch });
  },
  redisUnsubscribe: (subId: string) => invoke<boolean>('db_redis_unsubscribe', { subId }),
  redisPublish: (session: string, channel: string, message: string) => invoke<number>('db_redis_publish', { session, channel, message }),

  // MongoDB
  mongoDatabases: (session: string) => invoke<{ name: string; sizeOnDisk: number; empty: boolean }[]>('db_mongo_databases', { session }),
  mongoCollections: (session: string, database: string) => invoke<{ name: string; kind: string }[]>('db_mongo_collections', { session, database }),
  mongoFind: (session: string, request: FindRequest) => invoke<Documents>('db_mongo_find', { session, request }),
  mongoAggregate: (session: string, request: AggregateRequest) => invoke<Documents>('db_mongo_aggregate', { session, request }),
  mongoCount: (session: string, database: string, collection: string, filter?: string) => invoke<number>('db_mongo_count', { session, database, collection, filter }),
  mongoIndexes: (session: string, database: string, collection: string) => invoke<unknown[]>('db_mongo_indexes', { session, database, collection }),
  mongoExplain: (session: string, explain: { type: 'find'; request: FindRequest } | { type: 'aggregate'; request: AggregateRequest }, analyze: boolean) =>
    invoke<unknown>('db_mongo_explain', { session, explain, analyze }),
  mongoWrite: (session: string, op: MongoWrite) => invoke<unknown>('db_mongo_write', { session, op }),
  mongoCommand: (session: string, database: string, command: string) => invoke<unknown>('db_mongo_command', { session, database, command }),

  // saved queries
  queries: (root: string) => invoke<SavedQueryEntry[]>('db_queries_list', { root }),
  readQuery: (root: string, path: string) => invoke<SavedQuery>('db_queries_read', { root, path }),
  saveQuery: (root: string, path: string, text: string) => invoke<void>('db_queries_save', { root, path, text }),
  createQuery: (root: string, name: string, kind: QueryKind, opts: { folder?: string; connection?: string; text?: string } = {}) =>
    invoke<string>('db_queries_create', { root, folder: opts.folder, name, kind, connection: opts.connection, text: opts.text }),
  deleteQuery: (root: string, path: string) => invoke<void>('db_queries_delete', { root, path }),
};

export const DB_KINDS: { kind: DbKind; label: string; port?: number }[] = [
  { kind: 'postgres', label: 'PostgreSQL', port: 5432 },
  { kind: 'mysql', label: 'MySQL / MariaDB', port: 3306 },
  { kind: 'sqlite', label: 'SQLite' },
  { kind: 'redis', label: 'Redis / Valkey', port: 6379 },
  { kind: 'mongodb', label: 'MongoDB', port: 27017 },
  { kind: 'sqlserver', label: 'SQL Server / Azure SQL', port: 1433 },
];

export const dialectOf = (kind?: DbKind): Dialect | undefined => (kind === 'postgres' || kind === 'mysql' || kind === 'sqlite' || kind === 'sqlserver' ? kind : undefined);

import { Channel, invoke } from '@tauri-apps/api/core';
import type { Assertion, HttpRequestFile } from '@/lib/types';
import type { Priority } from '@/lib/api/todos';

// Mirrors of crates/devall-ai, crates/devall-mcp and the commands in
// src-tauri/src/modules/ai.rs. Keep in sync.

// ── Providers ─────────────────────────────────────────────────────────────────────────────────

export type ProviderKind = 'openAiCompatible' | 'ollama' | 'anthropic' | 'gemini';

export interface ProviderConfig {
  id: string;
  name: string;
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  /** An API key is stored in the OS keychain (never in ai.json). */
  hasApiKey?: boolean;
  /** Treat a LAN endpoint as local for redaction purposes. */
  trustedLocal?: boolean;
  /** Ask local reasoning models to skip thinking (default true). */
  disableThinking?: boolean;
  temperature?: number;
  maxTokens?: number;
}

export interface AiConfig {
  version: number;
  providers: ProviderConfig[];
  defaultProvider?: string;
  /** Allow notes and personal to-dos to be sent to cloud providers. */
  allowRemoteContext: boolean;
  /** Model for local semantic search (unset: the managed embedding model when it runs). */
  embeddings?: EmbeddingSettings;
}

/** Provider id of the managed embedding sidecar (`llama-server --embedding`). */
export const MANAGED_EMBED_ID = 'managed-embed';

export interface EmbeddingSettings {
  /** A configured provider id (Ollama / OpenAI-compatible) or `managed-embed`. */
  provider: string;
  /** Ollama tag, server model name, or catalog id for the managed sidecar. */
  model: string;
}

export interface ProvidersView {
  config: AiConfig;
  /** The managed llama.cpp sidecar when running (provider id `managed-llama`). */
  managed: ProviderConfig | null;
  keychainAvailable: boolean;
}

export const MANAGED_PROVIDER_ID = 'managed-llama';

export type ServerKind = 'ollama' | 'lmStudio' | 'llamaServer' | 'jan';

export interface DiscoveredModel {
  id: string;
  sizeBytes: number | null;
  details: string | null;
  warning: string | null;
}

export interface DiscoveredServer {
  kind: ServerKind;
  name: string;
  baseUrl: string;
  version: string | null;
  models: DiscoveredModel[];
  warnings: string[];
  suggested: ProviderConfig;
}

/** Presets for the "add provider" form. Cloud presets are clearly marked as such in the UI. */
export const PROVIDER_PRESETS: { label: string; kind: ProviderKind; baseUrl: string; model: string; needsKey: boolean; local: boolean }[] = [
  { label: 'Ollama', kind: 'ollama', baseUrl: 'http://127.0.0.1:11434', model: 'qwen3.5:9b', needsKey: false, local: true },
  { label: 'LM Studio', kind: 'openAiCompatible', baseUrl: 'http://127.0.0.1:1234/v1', model: 'gemma-4-12b-it', needsKey: false, local: true },
  { label: 'llama.cpp server', kind: 'openAiCompatible', baseUrl: 'http://127.0.0.1:8080/v1', model: 'local', needsKey: false, local: true },
  { label: 'vLLM', kind: 'openAiCompatible', baseUrl: 'http://127.0.0.1:8000/v1', model: 'Qwen/Qwen3.8-27B', needsKey: false, local: true },
  { label: 'OpenRouter', kind: 'openAiCompatible', baseUrl: 'https://openrouter.ai/api/v1', model: 'qwen/qwen3.8-27b', needsKey: true, local: false },
  { label: 'OpenAI', kind: 'openAiCompatible', baseUrl: 'https://api.openai.com/v1', model: '', needsKey: true, local: false },
  { label: 'Anthropic', kind: 'anthropic', baseUrl: 'https://api.anthropic.com', model: 'claude-opus-5-5', needsKey: true, local: false },
  { label: 'Google Gemini', kind: 'gemini', baseUrl: 'https://generativelanguage.googleapis.com', model: '', needsKey: true, local: false },
];

export function isLoopback(url: string): boolean {
  try {
    const h = new URL(url).hostname.replace(/^\[|\]$/g, '').toLowerCase();
    return h === 'localhost' || h.endsWith('.localhost') || h === '::1' || /^127\./.test(h);
  } catch {
    return false;
  }
}

export const isLocalProvider = (p: ProviderConfig) => !!p.trustedLocal || isLoopback(p.baseUrl);

// ── Hardware, catalog, sidecar ────────────────────────────────────────────────────────────────

export type GpuVendor = 'nvidia' | 'amd' | 'intel' | 'apple' | 'qualcomm' | 'other';
export interface Gpu {
  name: string;
  vendor: GpuVendor;
  vramBytes: number | null;
  driverVersion: string | null;
}
export interface Hardware {
  os: string;
  arch: string;
  totalRamBytes: number;
  cpu: string | null;
  appleSilicon: boolean;
  gpus: Gpu[];
  nvidiaSmi: boolean;
  notes: string[];
}
export type Tier = 't8' | 't16' | 't24' | 't32';
export type LlamaVariant =
  | 'macos-arm64'
  | 'macos-x64'
  | 'win-cpu-x64'
  | 'win-vulkan-x64'
  | 'win-cuda124-x64'
  | 'win-cuda134-x64'
  | 'win-cpu-arm64'
  | 'win-vulkan-arm64'
  | 'ubuntu-x64'
  | 'ubuntu-vulkan-x64'
  | 'ubuntu-arm64';
export interface VariantChoice {
  variant: LlamaVariant;
  fallbacks: LlamaVariant[];
  reason: string;
}
export interface Recommendation {
  tier: Tier;
  tierLabel: string;
  general: string;
  alternatives: string[];
  code: string | null;
  fim: string;
  embedding: string;
  notes: string[];
}
export interface HardwareView {
  hardware: Hardware;
  tier: Tier;
  tierLabel: string;
  recommendation: Recommendation;
  llama: VariantChoice;
  llamaDownloadBytes: number;
  /** Download key of the recommended llama.cpp runtime (`runtime:<asset suffix>`). */
  llamaKey: string;
}

export type ModelRole = 'general' | 'code' | 'fim' | 'embedding';
export interface ModelFileStatus {
  id: string;
  downloaded: boolean;
  path: string;
  partialBytes: number;
}
export interface CatalogEntry {
  id: string;
  name: string;
  family: string;
  role: ModelRole;
  license: string;
  hfRepo: string;
  hfFile: string;
  sizeBytes: number;
  sha256: string;
  quant: string;
  context: number;
  tiers: Tier[];
  thinking: boolean;
  fim: boolean;
  ollamaTag: string | null;
  minLlamaCppBuild: number;
  notes: string;
  status: ModelFileStatus;
  downloading: boolean;
}
export interface Installed {
  build: string;
  variant: LlamaVariant;
  server: string;
}
export interface LlamaAsset {
  variant: LlamaVariant;
  file: string;
  sizeBytes: number;
  sha256: string;
  runtime: [string, number, string] | null;
}
export interface CatalogView {
  models: CatalogEntry[];
  llamaBuild: string;
  installed: Installed[];
  runtimes: LlamaAsset[];
  server: string | null;
  /** Downloads in flight. */
  downloads: DownloadState[];
}

/** `done`, `paused` (cancelled, `.part` kept) and `error` are terminal; the rest are active. */
export type DownloadPhase = 'starting' | 'downloading' | 'verifying' | 'installing' | 'done' | 'paused' | 'error';

/** Live state of a model / runtime download (`ai://download` events, `ai_downloads`). */
export interface DownloadState {
  /** Model id, or `runtime:<asset suffix>` for the llama.cpp runtime. */
  key: string;
  phase: DownloadPhase;
  downloaded: number;
  total: number | null;
  bytesPerSec: number;
  /** Result path (done) or error text (error). */
  message: string | null;
}

export const DOWNLOAD_EVENT = 'ai://download';

export interface SidecarOptions {
  context: number;
  gpuLayers?: number | null;
  threads?: number | null;
  alias?: string | null;
  gpu: boolean;
}
export interface SidecarInfo {
  pid: number | null;
  port: number;
  baseUrl: string;
  model: string;
  modelPath: string;
  server: string;
  startedAt: number;
  logFile: string;
}
export interface SidecarStatus {
  running: boolean;
  info: SidecarInfo | null;
  server: string | null;
}

// ── Semantic search ───────────────────────────────────────────────────────────────────────────

export interface IndexStatus {
  exists: boolean;
  embedder?: string;
  embedderLabel?: string;
  dims: number;
  chunks: number;
  byKind: Record<string, number>;
  builtAt?: string;
  partial: boolean;
  /** New or changed chunks since the last build. */
  stale?: number;
  removed?: number;
  sizeBytes: number;
}

export interface EmbedSidecarStatus {
  running: boolean;
  model?: string;
  info?: SidecarInfo;
}

export interface IndexView {
  status: IndexStatus;
  settings?: EmbeddingSettings;
  /** `model @ endpoint` that would embed now. */
  embedder?: string;
  embedderLocal: boolean;
  /** The index was built with that model (otherwise search falls back to keywords). */
  embedderMatches: boolean;
  embedderError?: string;
  managed: EmbedSidecarStatus;
}

export type IndexProgress =
  | { type: 'scanned'; total: number; reused: number; toEmbed: number }
  | { type: 'embedded'; done: number; toEmbed: number }
  | { type: 'saved'; chunks: number };

export interface BuildOutcome {
  status: IndexStatus;
  embedded: number;
  reused: number;
  cancelled: boolean;
}

export type SearchKind = 'note' | 'request' | 'mock' | 'environment';

export interface SearchHit {
  kind: SearchKind;
  /** `notes/a.md`, `http/users/list.req.yaml`, `mocks/mappings/x.json`, `environments/x.env.yaml`. */
  source: string;
  title: string;
  /** Note heading, request method, stub id. */
  anchor?: string;
  snippet: string;
  score: number;
  similarity?: number;
}

export interface SearchResult {
  mode: 'hybrid' | 'keyword';
  hits: SearchHit[];
  note?: string;
}

// ── Features ──────────────────────────────────────────────────────────────────────────────────

export interface SourceRef {
  /** `S1`, `S2`… */
  id: string;
  /** `note`, `todos`, `requests`, `environments`. */
  kind: string;
  /** `notes/design/auth.md`, `todos (personal)`, `http/`, `environments/`. */
  source: string;
  title: string;
}

export interface Meta {
  provider: string;
  model: string;
  remote: boolean;
  redactions: number;
  repaired: boolean;
  schemaEnforced: boolean;
}

export interface AskAnswer {
  answer: string;
  citations: SourceRef[];
  sources: SourceRef[];
  meta: Meta;
}

export interface PayloadPreview {
  providerId: string;
  providerName: string;
  kind: ProviderKind;
  model: string;
  remote: boolean;
  method: string;
  url: string;
  headers: [string, string][];
  body: unknown;
}
export interface Preview {
  feature: string;
  payload: PayloadPreview;
  sources: SourceRef[];
  redactions: number;
  remote: boolean;
  notice: string;
}

export interface SampleCheck {
  sample: string;
  shouldMatch: boolean;
  matched: boolean;
}
export interface RegexSuggestion {
  pattern: string;
  flags: string;
  explanation: string;
  compiles: boolean;
  checks: SampleCheck[];
  valid: boolean;
  problem: string | null;
  meta: Meta;
}
export interface JsonPathSuggestion {
  path: string;
  explanation: string;
  matches: unknown[];
  count: number;
  valid: boolean;
  problem: string | null;
  meta: Meta;
}
export interface JqSuggestion {
  filter: string;
  explanation: string;
  /** The filter's output on the sample (or the reason it failed). */
  output: string;
  valid: boolean;
  problem: string | null;
  meta: Meta;
}
export interface RequestDraft {
  request: HttpRequestFile;
  explanation: string;
  variables: string[];
  unknownVariables: string[];
  problem: string | null;
  meta: Meta;
}
export interface MockData {
  items: unknown[];
  schema: unknown;
  inferredSchema: boolean;
  valid: boolean;
  errors: string[];
  meta: Meta;
}
export interface Fix {
  title: string;
  detail: string;
}
export interface ResponseExplanation {
  summary: string;
  likelyCauses: string[];
  suggestedFixes: Fix[];
  severity: 'info' | 'warning' | 'error';
  meta: Meta;
}
export interface ErrorExplanation {
  summary: string;
  rootCause: string;
  suggestedFixes: Fix[];
  relevantLines: string[];
  meta: Meta;
}
export interface SuggestedTest {
  assertion: Assertion;
  reason: string;
}
export interface TestSuggestions {
  tests: SuggestedTest[];
  dropped: string[];
  meta: Meta;
}
export interface NotesSummary {
  summary: string;
  keyPoints: string[];
  openQuestions: string[];
  citations: SourceRef[];
  sources: SourceRef[];
  meta: Meta;
}
/** A suggested task (shape of a to-do quick add). */
export interface TodoSuggestion {
  text: string;
  description: string;
  /** 1 (p1, most urgent) … 4 (none). */
  priority: Priority;
  /** Natural-language due date the to-do parser understands (`next fri`, `every mon`). */
  due: string | null;
  /** What `due` resolves to (YYYY-MM-DD). */
  dueDate: string | null;
  labels: string[];
  links: string[];
}
export interface TodoSuggestions {
  todos: TodoSuggestion[];
  meta: Meta;
}

/** A commit message for the staged diff (secrets masked before any model sees it). */
export interface CommitMessageSuggestion {
  subject: string;
  body: string;
  /** subject + blank line + body. */
  message: string;
  meta: Meta;
}

/** An HTTP exchange for "explain this response" / "suggest tests". Headers are `[name, value]`. */
export interface ResponseContext {
  method?: string;
  url?: string;
  requestHeaders?: [string, string][];
  requestBody?: string | null;
  status: number;
  statusText?: string;
  headers?: [string, string][];
  body?: string;
  durationMs?: number | null;
  failures?: string[];
  notes?: string | null;
}

/** A document gathered for "explain this failure" (mock stub, project event, pod events/logs). */
export interface FailureSource {
  /** `mockStub`, `mockNearMiss`, `event`, `podEvents`, `podLogs`. */
  kind: string;
  /** `mocks/mappings/x.json#<stub id>`, `event:<id>`, `pod:<context>/<namespace>/<name>`. */
  source: string;
  title: string;
  text: string;
}

/** A failed exchange plus gathered context (`ai_failure_context`). */
export interface FailureContext extends ResponseContext {
  /** Transport error when there was no response (status 0). */
  error?: string | null;
  sources: FailureSource[];
}

export interface FailureExplanation {
  summary: string;
  likelyCauses: string[];
  suggestedFixes: Fix[];
  severity: 'info' | 'warning' | 'error';
  citations: SourceRef[];
  /** Every source sent; `S1` is the exchange. */
  sources: SourceRef[];
  /** Gathered sources not sent (cloud provider without the project-context opt-in). */
  omitted: string[];
  meta: Meta;
}

export interface OperationName {
  key: string;
  operationId: string;
  summary: string;
  tags: string[];
}
export interface NamingProposal {
  operations: OperationName[];
  schemas: { body: string; name: string; location: string }[];
  tags: { name: string; description: string }[];
}
export interface OpenApiNaming {
  proposal: NamingProposal;
  /** The deterministic document with the proposal applied. */
  spec: unknown;
  changes: string[];
  problem: string | null;
  meta: Meta;
}
/** `ai_openapi_names`: the proposal applied, as YAML, plus lint before/after. */
export interface OpenApiNamingView extends OpenApiNaming {
  improved: string;
  lintBefore: import('@/lib/api/openapi').LintReport;
  lintAfter: import('@/lib/api/openapi').LintReport;
}

/** NL → SQL: checked with the database's EXPLAIN (never run). */
export interface SqlSuggestion {
  sql: string;
  explanation: string;
  validated: boolean;
  check: { readOnly?: boolean; keyword?: string; plan?: string } | null;
  problem: string | null;
  meta: Meta;
}
/** NL → MongoDB: checked with the server's explain (never run). */
export interface MongoSuggestion {
  type: 'find' | 'aggregate';
  filter: string;
  projection: string;
  sort: string;
  limit: number;
  pipeline: string;
  explanation: string;
  validated: boolean;
  check: { plan?: string } | null;
  problem: string | null;
  meta: Meta;
}
/** What the app can do itself for a suggested kubectl change (after the user confirms). */
export interface KubectlAction {
  type: 'delete' | 'scale' | 'restart';
  kind: { group: string; version: string; kind: string; plural: string; namespaced: boolean };
  name: string;
  namespace: string;
  replicas?: number;
}
/** NL → kubectl: parsed and dry-run on the cluster (never applied). */
export interface KubectlSuggestion {
  command: string;
  explanation: string;
  validated: boolean;
  check: {
    verb?: string;
    sub?: string | null;
    kind?: KubectlAction['kind'];
    name?: string | null;
    namespace?: string | null;
    mutating?: boolean;
    exists?: boolean;
    dryRun?: string | null;
    unverified?: string;
    action?: KubectlAction;
  } | null;
  problem: string | null;
  meta: Meta;
}

/** Feature name → input → output, as accepted by `ai_run` / `ai_preview`. */
export interface FeatureMap {
  ask: { input: { question: string }; output: AskAnswer };
  nlRegex: { input: { description: string; positive?: string[]; negative?: string[] }; output: RegexSuggestion };
  nlJsonPath: { input: { description: string; sampleJson: string }; output: JsonPathSuggestion };
  nlJq: { input: { description: string; sampleJson: string }; output: JqSuggestion };
  nlRequest: { input: { description: string }; output: RequestDraft };
  mockData: { input: { schemaOrSample: string; count?: number }; output: MockData };
  explainResponse: { input: ResponseContext; output: ResponseExplanation };
  explainError: { input: { text: string }; output: ErrorExplanation };
  explainFailure: { input: FailureContext; output: FailureExplanation };
  openapiNames: { input: { spec: string }; output: OpenApiNaming };
  nlSql: { input: { question: string; dialect: string; schema: string; allowWrites?: boolean; session?: string }; output: SqlSuggestion };
  nlMongo: { input: { question: string; database: string; collection: string; fields: string; session?: string }; output: MongoSuggestion };
  nlKubectl: { input: { question: string; context?: string; namespace?: string; kinds?: string }; output: KubectlSuggestion };
  suggestTests: { input: ResponseContext; output: TestSuggestions };
  summarizeNotes: { input: { paths?: string[] }; output: NotesSummary };
  todosFromText: { input: { text: string }; output: TodoSuggestions };
  commitMessage: { input: { diff: string; branch?: string; recent?: string[]; hint?: string }; output: CommitMessageSuggestion };
}
export type Feature = keyof FeatureMap;

// ── MCP ───────────────────────────────────────────────────────────────────────────────────────

export interface ConfigSnippets {
  claudeCode: string;
  mcpServersJson: string;
  vscodeJson: string;
  command: string[];
}
export interface HttpInfo {
  url: string;
  port: number;
  token: string;
}
export interface McpStatus {
  cli: string | null;
  snippets: ConfigSnippets;
  http: HttpInfo | null;
}

/** How the MCP server treats `send_request` writes (POST/PUT/PATCH/DELETE). */
export type McpWriteMode = 'readOnly' | 'confirm' | 'allow';

/** An MCP call waiting for the user's approval (every string already redacted). */
export interface ApprovalRequest {
  id: string;
  tool: string;
  requestPath: string;
  requestName: string;
  method: string;
  url: string;
  environmentId: string | null;
  environment: string | null;
  protected: boolean;
  reason: string;
  headers: [string, string][];
  bodyPreview: string | null;
  bodyTruncated: boolean;
  timeoutSecs: number;
  /** Unix ms. */
  createdAt: number;
}

export type ApprovalDecision = 'allowOnce' | 'allowSession' | 'deny';

// ── Commands ──────────────────────────────────────────────────────────────────────────────────

function channel<T>(fn?: (e: T) => void) {
  const ch = new Channel<T>();
  if (fn) ch.onmessage = fn;
  return ch;
}

export const aiApi = {
  providers: () => invoke<ProvidersView>('ai_providers'),
  /** `apiKey`: undefined keeps the stored key, '' deletes it, otherwise it goes to the OS keychain. */
  saveProvider: (provider: ProviderConfig, apiKey?: string, makeDefault?: boolean) => invoke<AiConfig>('ai_save_provider', { provider, apiKey, makeDefault }),
  removeProvider: (id: string) => invoke<AiConfig>('ai_remove_provider', { id }),
  settings: (s: { defaultProvider?: string; allowRemoteContext?: boolean }) => invoke<AiConfig>('ai_settings', s),
  testProvider: (arg: { id?: string; provider?: ProviderConfig; apiKey?: string }) => invoke<string[]>('ai_test_provider', arg),
  discover: () => invoke<DiscoveredServer[]>('ai_discover'),

  hardware: (refresh = false) => invoke<HardwareView>('ai_hardware', { refresh }),
  catalog: () => invoke<CatalogView>('ai_catalog'),
  /** Starts (or resumes) a model download in the background; progress arrives as `ai://download` events. */
  downloadModel: (model: string) => invoke<DownloadState>('ai_download_model', { model }),
  /** Downloads and installs llama.cpp in the background (key `runtime:<asset suffix>`). */
  installRuntime: (variant: LlamaVariant | undefined) => invoke<DownloadState>('ai_install_runtime', { variant }),
  /** Every download in flight (for views that mount mid-download). */
  downloads: () => invoke<DownloadState[]>('ai_downloads'),
  /** `key` is the model id, or `runtime:<asset suffix>` for the llama.cpp download. Pauses (keeps the partial file). */
  cancelDownload: (key: string) => invoke<boolean>('ai_cancel_download', { model: key }),
  deleteModel: (model: string) => invoke<void>('ai_delete_model', { model }),
  sidecarStart: (model: string, options?: Partial<SidecarOptions>) =>
    invoke<SidecarStatus>('ai_sidecar_start', { model, options: options ? { context: 16384, gpu: true, ...options } : undefined }),
  sidecarStop: () => invoke<void>('ai_sidecar_stop'),
  sidecarStatus: () => invoke<SidecarStatus>('ai_sidecar_status'),

  embeddingSettings: (provider: string, model?: string) => invoke<AiConfig>('ai_embedding_settings', { provider, model }),
  embedSidecarStart: (model: string) => invoke<EmbedSidecarStatus>('ai_embed_sidecar_start', { model }),
  embedSidecarStop: () => invoke<void>('ai_embed_sidecar_stop'),
  indexStatus: (root: string) => invoke<IndexView>('ai_index_status', { root }),
  indexBuild: (root: string, onProgress: (e: IndexProgress) => void, callId?: string) => invoke<BuildOutcome>('ai_index_build', { root, callId, onProgress: channel(onProgress) }),
  indexClear: (root: string) => invoke<void>('ai_index_clear', { root }),
  search: (root: string, query: string, limit = 20) => invoke<SearchResult>('ai_search', { root, query, limit }),

  preview: <F extends Feature>(root: string | undefined, feature: F, input: FeatureMap[F]['input'], provider?: string) =>
    invoke<Preview>('ai_preview', { root, feature, input, provider }),
  ask: (root: string, question: string, onToken: (text: string) => void, opts: { provider?: string; callId?: string } = {}) =>
    invoke<AskAnswer>('ai_ask', { root, question, provider: opts.provider, callId: opts.callId, onToken: channel<{ type: 'token'; text: string }>((e) => onToken(e.text)) }),
  cancel: (callId: string) => invoke<boolean>('ai_cancel', { callId }),
  /** Runs any feature by name (used by `AiAssistButton`). */
  run: <F extends Feature>(root: string | undefined, feature: F, input: FeatureMap[F]['input'], opts: { provider?: string; callId?: string } = {}) =>
    invoke<FeatureMap[F]['output']>('ai_run', { root, feature, input, provider: opts.provider, callId: opts.callId }),

  nlRegex: (description: string, positive: string[], negative: string[], provider?: string) => invoke<RegexSuggestion>('ai_nl_regex', { description, positive, negative, provider }),
  nlJsonPath: (description: string, sampleJson: string, provider?: string) => invoke<JsonPathSuggestion>('ai_nl_jsonpath', { description, sampleJson, provider }),
  nlJq: (description: string, sampleJson: string, provider?: string) => invoke<JqSuggestion>('ai_nl_jq', { description, sampleJson, provider }),
  nlRequest: (root: string | undefined, description: string, provider?: string) => invoke<RequestDraft>('ai_nl_request', { root, description, provider }),
  mockData: (schemaOrSample: string, count = 10, provider?: string) => invoke<MockData>('ai_mock_data', { schemaOrSample, count, provider }),
  explainResponse: (root: string | undefined, context: ResponseContext, provider?: string) => invoke<ResponseExplanation>('ai_explain_response', { root, context, provider }),
  explainError: (text: string, root?: string, provider?: string) => invoke<ErrorExplanation>('ai_explain_error', { root, text, provider }),
  /** "Improve names with AI" for an OpenAPI draft (applied as a patch, linted; nothing saved). */
  openapiNames: (root: string | undefined, spec: string, opts: { provider?: string; callId?: string } = {}) =>
    invoke<OpenApiNamingView>('ai_openapi_names', { root, spec, provider: opts.provider, callId: opts.callId }),
  /** Gathers the mock stub, related project events and pod events/logs for a failed request. */
  failureContext: (root: string | undefined, context: ResponseContext, error?: string | null, at?: number) =>
    invoke<FailureContext>('ai_failure_context', { root, context, error, at }),
  suggestTests: (root: string | undefined, sample: ResponseContext, provider?: string) => invoke<TestSuggestions>('ai_suggest_tests', { root, sample, provider }),
  summarizeNotes: (root: string, paths: string[] = [], provider?: string) => invoke<NotesSummary>('ai_summarize_notes', { root, paths, provider }),
  todosFromText: (text: string, root?: string, provider?: string) => invoke<TodoSuggestions>('ai_todos_from_text', { root, text, provider }),

  /** `localWrites` (default on): agents may write shared notes and this project's personal to-dos. */
  mcpStatus: (root: string, allowWrites = false, localWrites = true) => invoke<McpStatus>('mcp_status', { root, allowWrites, localWrites }),
  mcpConfigSnippet: (root: string, allowWrites = false, exe?: string, localWrites = true) => invoke<ConfigSnippets>('mcp_config_snippet', { root, allowWrites, exe, localWrites }),
  /** `confirmWrites`: each write waits for an in-app approval (protected environments always do). */
  mcpHttpStart: (root: string, allowWrites = false, confirmWrites = false, localWrites = true) => invoke<HttpInfo>('mcp_http_start', { root, allowWrites, confirmWrites, localWrites }),
  mcpApprovalsPending: () => invoke<ApprovalRequest[]>('mcp_approvals_pending'),
  mcpApprovalAnswer: (id: string, decision: ApprovalDecision) => invoke<boolean>('mcp_approval_answer', { id, decision }),
  mcpHttpStop: () => invoke<void>('mcp_http_stop'),
};

export function formatGiB(bytes: number | null | undefined): string {
  if (!bytes) return '—';
  const g = bytes / 1024 ** 3;
  return g >= 10 ? `${g.toFixed(0)} GB` : g >= 1 ? `${g.toFixed(1)} GB` : `${(bytes / 1024 ** 2).toFixed(0)} MB`;
}

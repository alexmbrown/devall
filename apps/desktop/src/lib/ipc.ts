import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  AppInfo,
  CookieInfo,
  HistoryDetail,
  HistorySummary,
  RunEvent,
  RunRequest,
  RunSummary,
  StreamMsg,
  TokenInfo,
  TransferOptions,
  TransferResult,
  TransferSelection,
  EnvironmentEntry,
  EnvironmentFile,
  FolderFile,
  HttpRequestFile,
  OpInfo,
  ProjectInfo,
  ProjectManifest,
  IncomingLink,
  RecentProject,
  RecentProjectId,
  RegexEntry,
  RegexFile,
  SecretCopy,
  SecretStatus,
  SendResult,
  VarUsage,
  TreeNode,
  VarInfo,
} from './types';

/** Typed wrappers over the Tauri commands in src-tauri/src/commands.rs. Errors reject with a string. */
export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  recentProjects: () => invoke<RecentProject[]>('recent_projects'),
  forgetRecent: (path: string) => invoke<void>('forget_recent', { path }),
  /** Recent projects (newest first) and the scratch pad with their `devall.yaml` ids. */
  recentProjectIds: () => invoke<RecentProjectId[]>('recent_project_ids'),
  /** `devall:` links from outside the app (OS deep links) waiting since the last call. */
  deepLinkTake: () => invoke<IncomingLink[]>('deep_link_take'),
  openProject: (path: string) => invoke<ProjectInfo>('open_project', { path }),
  createProject: (path: string, name: string) => invoke<ProjectInfo>('create_project', { path, name }),
  openScratch: () => invoke<ProjectInfo>('open_scratch'),
  saveManifest: (root: string, manifest: ProjectManifest) => invoke<void>('save_manifest', { root, manifest }),

  tree: (root: string) => invoke<TreeNode>('project_tree', { root }),
  readRequest: (root: string, path: string) => invoke<HttpRequestFile>('read_request', { root, path }),
  saveRequest: (root: string, path: string, request: HttpRequestFile) => invoke<void>('save_request', { root, path, request }),
  createRequest: (root: string, folder: string, request: HttpRequestFile) => invoke<string>('create_request', { root, folder, request }),
  duplicateRequest: (root: string, path: string) => invoke<string>('duplicate_request', { root, path }),
  createFolder: (root: string, parent: string, name: string) => invoke<string>('create_folder', { root, parent, name }),
  readFolder: (root: string, path: string) => invoke<FolderFile>('read_folder', { root, path }),
  saveFolder: (root: string, path: string, folder: FolderFile) => invoke<void>('save_folder', { root, path, folder }),
  renameItem: (root: string, path: string, name: string) => invoke<string>('rename_item', { root, path, name }),
  deleteItem: (root: string, path: string) => invoke<void>('delete_item', { root, path }),
  moveItem: (root: string, path: string, target: string, index?: number) => invoke<string>('move_item', { root, path, target, index }),

  environments: (root: string) => invoke<EnvironmentEntry[]>('list_environments', { root }),
  createEnvironment: (root: string, name: string) => invoke<string>('create_environment', { root, name }),
  saveEnvironment: (root: string, id: string, environment: EnvironmentFile) => invoke<void>('save_environment', { root, id, environment }),
  deleteEnvironment: (root: string, id: string) => invoke<void>('delete_environment', { root, id }),
  secretStatus: (root: string, env: string) => invoke<SecretStatus[]>('secret_status', { root, env }),
  setSecret: (root: string, env: string, name: string, value: string) => invoke<void>('set_secret', { root, env, name, value }),
  deleteSecret: (root: string, env: string, name: string) => invoke<void>('delete_secret', { root, env, name }),
  /** Where `names` (`baseUrl`, `secret.API_TOKEN`) are referenced in the project. */
  envUsages: (root: string, names: string[]) => invoke<VarUsage[]>('env_usages', { root, names }),
  variableCatalog: (root: string, path: string, env?: string) => invoke<VarInfo[]>('variable_catalog', { root, path, env }),

  send: (sendId: string, root: string, path: string, request: HttpRequestFile, env: string | undefined, onEvent: (m: StreamMsg) => void) => {
    const ch = new Channel<StreamMsg>();
    ch.onmessage = onEvent;
    return invoke<SendResult>('send_request', { sendId, root, path, request, env, onEvent: ch });
  },
  cancel: (sendId: string) => invoke<boolean>('cancel_request', { sendId }),
  runtimeVars: (root: string) => invoke<Record<string, string>>('runtime_vars', { root }),
  setRuntimeVars: (root: string, vars: Record<string, string>) => invoke<void>('runtime_vars_set', { root, vars }),
  historyList: (root: string, path?: string, limit = 100) => invoke<HistorySummary[]>('history_list', { root, path, limit }),
  /** Every request's responses, newest first (History page). */
  historySearch: (root: string, query?: string, offset = 0, limit = 200) => invoke<HistorySummary[]>('history_search', { root, query: query || undefined, offset, limit }),
  historyGet: (root: string, id: string) => invoke<HistoryDetail | null>('history_get', { root, id }),
  historyDelete: (root: string, id: string) => invoke<boolean>('history_delete', { root, id }),
  historyClear: (root: string, path?: string) => invoke<number>('history_clear', { root, path }),
  cookiesList: (root: string) => invoke<CookieInfo[]>('cookies_list', { root }),
  cookiesDelete: (root: string, domain: string, path: string, name: string) => invoke<boolean>('cookies_delete', { root, domain, path, name }),
  cookiesClear: (root: string) => invoke<void>('cookies_clear', { root }),
  oauth2Token: (root: string, path: string, request: HttpRequestFile, env: string | undefined, force: boolean) =>
    invoke<TokenInfo>('oauth2_token', { root, path, request, env, force }),
  oauth2Clear: (root: string, path: string, request: HttpRequestFile, env: string | undefined) => invoke<void>('oauth2_clear', { root, path, request, env }),
  runCollection: (runId: string, root: string, run: RunRequest, onEvent: (e: RunEvent) => void) => {
    const ch = new Channel<RunEvent>();
    ch.onmessage = onEvent;
    return invoke<RunSummary>('run_collection', { runId, root, run, onEvent: ch });
  },
  cancelRun: (runId: string) => invoke<boolean>('cancel_run', { runId }),
  runToJunit: (summary: RunSummary, suite: string) => invoke<string>('run_to_junit', { summary, suite }),
  copyAsCurl: (root: string, path: string, request: HttpRequestFile, env: string | undefined, includeSecrets: boolean) =>
    invoke<string>('copy_as_curl', { root, path, request, env, includeSecrets }),
  importCurl: (command: string) => invoke<HttpRequestFile>('import_curl', { command }),

  /** For paths the user picked in a file dialog. */
  readTextFile: (path: string) => invoke<string>('read_text_file', { path }),
  writeTextFile: (path: string, content: string) => invoke<void>('write_text_file', { path, content }),
  utilList: () => invoke<OpInfo[]>('util_list'),
  utilRun: (op: string, input: string, args: Record<string, string> = {}) => invoke<string>('util_run', { op, input, args }),

  /** Copies (or moves) items into another project; secret values stay put until `transferSecrets`. */
  transferToProject: (fromRoot: string, toRoot: string, selection: TransferSelection, options: TransferOptions) =>
    invoke<TransferResult>('transfer_to_project', { fromRoot, toRoot, selection, options }),
  transferSecrets: (fromRoot: string, toRoot: string, copies: SecretCopy[]) => invoke<number>('transfer_secrets', { fromRoot, toRoot, copies }),
  /** Writes a response snapshot; returns its path relative to the project root. */
  snapshotSave: (root: string, path: string, body: string, contentType: string | null, ignore: string[], file?: string) =>
    invoke<string>('snapshot_save', { root, path, body, contentType, ignore, file }),
  regexList: (root: string) => invoke<RegexEntry[]>('regex_list', { root }),
  regexRead: (root: string, path: string) => invoke<RegexFile>('regex_read', { root, path }),
  regexSave: (root: string, path: string | null, file: RegexFile) => invoke<string>('regex_save', { root, path, file }),
  regexDelete: (root: string, path: string) => invoke<void>('regex_delete', { root, path }),
};

export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export function errorMessage(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}

export interface ScanFinding {
  rule: string;
  description: string;
  line: number;
  column: number;
  excerpt: string;
}

export interface SecretProviderInfo {
  scheme: string;
  name: string;
  program: string;
  example: string;
}

export const scanApi = {
  text: (text: string) => invoke<ScanFinding[]>('scan_text', { text }),
  project: (root: string) => invoke<{ path: string; findings: ScanFinding[] }[]>('scan_project', { root }),
  providers: () => invoke<SecretProviderInfo[]>('secret_providers'),
  testProvider: (reference: string) => invoke<number>('secret_provider_test', { reference }),
};

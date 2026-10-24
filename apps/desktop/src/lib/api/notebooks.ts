// Typed wrappers + TS mirrors for the runnable-notebook commands in
// src-tauri/src/modules/notebooks.rs (crate: crates/devall-notebook). Keep in sync with the Rust types.
import { Channel, invoke } from '@tauri-apps/api/core';

/** Languages /dev/all can run (`devall_notebook::Language`). */
export type RunLanguage = 'shell' | 'powershell' | 'cmd' | 'javascript' | 'python';
export const RUN_LANGUAGES: RunLanguage[] = ['shell', 'powershell', 'cmd', 'javascript', 'python'];

export const LANGUAGE_LABELS: Record<RunLanguage, string> = {
  shell: 'Shell',
  powershell: 'PowerShell',
  cmd: 'cmd',
  javascript: 'JavaScript',
  python: 'Python',
};

/** Same aliases as `devall_core::notebook::normalize_language`. */
export function normalizeLanguage(lang: string | undefined | null): RunLanguage | null {
  switch ((lang ?? '').trim().toLowerCase()) {
    case 'shell':
    case 'bash':
    case 'sh':
    case 'zsh':
    case 'shellscript':
      return 'shell';
    case 'powershell':
    case 'pwsh':
    case 'ps1':
      return 'powershell';
    case 'cmd':
    case 'bat':
    case 'batch':
      return 'cmd';
    case 'javascript':
    case 'js':
    case 'node':
    case 'nodejs':
      return 'javascript';
    case 'python':
    case 'python3':
    case 'py':
      return 'python';
    default:
      return null;
  }
}

/** A MIME bundle: `text/plain`, `text/html` (sanitize!), `application/json`, `image/png` (base64), … */
export type MimeBundle = Record<string, unknown>;

export type CellEvent =
  | { type: 'started'; runId: string; language: RunLanguage; interpreter: string; program: string; version?: string | null; at: number }
  | { type: 'stdout'; text: string; plain: string; at: number }
  | { type: 'stderr'; text: string; plain: string; at: number }
  | { type: 'display'; data: MimeBundle; result: boolean; at: number }
  | { type: 'error'; ename: string; evalue: string; traceback: string[]; at: number }
  | { type: 'notice'; text: string; at: number }
  | { type: 'exit'; code: number | null; durationMs: number; interrupted: boolean; timedOut: boolean; at: number };

/** nbformat 4 output object. */
export type NbOutput =
  | { output_type: 'stream'; name: 'stdout' | 'stderr'; text: string | string[] }
  | { output_type: 'execute_result'; execution_count: number | null; data: MimeBundle; metadata: Record<string, unknown> }
  | { output_type: 'display_data'; data: MimeBundle; metadata: Record<string, unknown> }
  | { output_type: 'error'; ename: string; evalue: string; traceback: string[] };

export interface CellResult {
  runId: string;
  language: RunLanguage;
  interpreter?: string | null;
  exitCode: number | null;
  ok: boolean;
  interrupted: boolean;
  timedOut: boolean;
  durationMs: number;
  /** ANSI-free nbformat outputs, ready to save. */
  outputs: NbOutput[];
}

export interface CellRunOptions {
  runId?: string;
  /** `metadata.devall.templating` */
  templating?: boolean;
  /** Requires `allowSecrets` for this cell's current code. */
  secrets?: boolean;
  timeoutMs?: number;
  /** `metadata.devall.cwd`, relative to the project root. */
  cwd?: string;
  executionCount?: number;
}

export type InterpreterFlavor = 'bash' | 'sh' | 'pwsh' | 'windowsPowershell' | 'cmd' | 'node' | 'python';

export interface Interpreter {
  language: RunLanguage;
  flavor: InterpreterFlavor;
  name: string;
  program: string;
  args: string[];
  version?: string | null;
  /** Fallback explanation, e.g. shell cells running in PowerShell. */
  note?: string | null;
  custom: boolean;
}

export interface InterpreterStatus {
  language: RunLanguage;
  interpreter?: Interpreter | null;
  /** Why it's missing and how to fix it. */
  error?: string | null;
  customPath?: string | null;
}

export interface TrustStatus {
  trusted: boolean;
  hash: string;
  /** Cell ids with a valid secrets grant. */
  secretCells: string[];
}

export interface KernelInfo {
  language: RunLanguage;
  label: string;
}

const channel = (onEvent: (e: CellEvent) => void) => {
  const ch = new Channel<CellEvent>();
  ch.onmessage = onEvent;
  return ch;
};

export const notebooksApi = {
  /** Runs a notebook cell (the notebook must be trusted this session). `env` = active environment id. */
  runCell: (
    root: string,
    notebookPath: string,
    cellId: string,
    code: string,
    language: RunLanguage,
    env: string | undefined,
    options: CellRunOptions,
    onEvent: (e: CellEvent) => void,
  ) => invoke<CellResult>('nb_run_cell', { root, notebookPath, cellId, code, language, env, options, onEvent: channel(onEvent) }),
  /** Runs a fenced code block of a Markdown note (the block must be trusted). */
  runSnippet: (root: string, notePath: string, language: RunLanguage, code: string, env: string | undefined, runId: string, onEvent: (e: CellEvent) => void) =>
    invoke<CellResult>('nb_run_snippet', { root, notePath, language, code, env, runId, onEvent: channel(onEvent) }),
  interrupt: (runId: string) => invoke<boolean>('nb_interrupt', { runId }),
  restart: (root: string, notebookPath: string) => invoke<number>('nb_restart', { root, notebookPath }),
  shutdown: (root: string, notebookPath: string) => invoke<number>('nb_shutdown', { root, notebookPath }),
  kernels: (root: string, notebookPath: string) => invoke<KernelInfo[]>('nb_kernels', { root, notebookPath }),
  interpreters: (refresh = false) => invoke<InterpreterStatus[]>('nb_interpreters', { refresh }),
  setInterpreter: (language: RunLanguage, path: string | null) => invoke<InterpreterStatus>('nb_set_interpreter', { language, path }),
  /** Notebook: `content` = notebook JSON. Markdown code block: `content` = code, `language` required. */
  trustStatus: (root: string, path: string, content: string, language?: RunLanguage) => invoke<TrustStatus>('nb_trust_status', { root, path, content, language }),
  trust: (root: string, path: string, content: string, language?: RunLanguage) => invoke<TrustStatus>('nb_trust', { root, path, content, language }),
  allowSecrets: (root: string, path: string, cellId: string, code: string, language: RunLanguage, allow: boolean) =>
    invoke<void>('nb_allow_secrets', { root, path, cellId, code, language, allow }),
};

import { invoke } from '@tauri-apps/api/core';

/** `scripts:` on a request (`*.req.yaml`) or folder (`_folder.yaml`). Empty strings are omitted on disk. */
export interface Scripts {
  /** Runs before variables are resolved; may edit the request and set variables. */
  preRequest?: string;
  /** Runs after the response; may set variables and add `pm.test(...)` results. */
  postResponse?: string;
}

export type ScriptPhase = 'preRequest' | 'postResponse';
export type ScriptLogLevel = 'log' | 'info' | 'warn' | 'error' | 'debug';
export type ScriptErrorKind = 'syntax' | 'runtime' | 'timeout' | 'memory' | 'internal';
export type VarScope = 'variables' | 'environment' | 'collectionVariables' | 'globals';

/** Mirror of `devall_script::LogLine`. */
export interface ScriptLogLine {
  phase: ScriptPhase;
  /** `request`, `collection` or `folder <path>`. */
  source: string;
  level: ScriptLogLevel;
  message: string;
}

/** Mirror of `devall_script::ScriptError`. `line`/`column` are 1-based positions in that script. */
export interface ScriptError {
  phase: ScriptPhase;
  source: string;
  kind: ScriptErrorKind;
  message: string;
  line: number | null;
  column: number | null;
}

/** Mirror of `devall_script::ScriptTestResult` (same shape as an assertion result, plus extras). */
export interface ScriptTestResult {
  label: string;
  passed: boolean;
  actual: string | null;
  message: string | null;
  skipped: boolean;
  phase: ScriptPhase;
  source: string;
}

/** Mirror of `devall_script::VarChange` (`value: null` = unset). */
export interface VarChange {
  scope: VarScope;
  name: string;
  value: string | null;
}

/** Mirror of `devall_script::ScriptReport`: console output and errors of one send. */
export interface ScriptReport {
  logs: ScriptLogLine[];
  errors: ScriptError[];
  /** A pre-request script called `pm.execution.skipRequest()`. */
  skipped: boolean;
  scriptsRun: number;
}

export const PHASE_LABEL: Record<ScriptPhase, string> = { preRequest: 'Pre-request', postResponse: 'Post-response' };

/** Drops empty scripts so nothing is written to disk for them; `undefined` when both are empty. */
export function normalizeScripts(s: Scripts | undefined): Scripts | undefined {
  const pre = s?.preRequest ?? '';
  const post = s?.postResponse ?? '';
  const out: Scripts = {};
  if (pre.trim()) out.preRequest = pre;
  if (post.trim()) out.postResponse = post;
  return out.preRequest || out.postResponse ? out : undefined;
}

/** Number of non-blank scripts (for tab badges). */
export function scriptCount(s: Scripts | undefined): number {
  return (s?.preRequest?.trim() ? 1 : 0) + (s?.postResponse?.trim() ? 1 : 0);
}

/** `pre-request · folder api · line 3:5` */
export function errorLocation(e: Pick<ScriptError, 'phase' | 'source' | 'line' | 'column'>): string {
  const parts = [PHASE_LABEL[e.phase].toLowerCase(), e.source];
  if (e.line != null) parts.push(`line ${e.line}${e.column != null ? `:${e.column}` : ''}`);
  return parts.join(' · ');
}

/** Typed wrappers over `src-tauri/src/modules/scripts.rs`. */
export const scriptsApi = {
  /** Parses a script without running it; `null` when it compiles. */
  check: (code: string) => invoke<ScriptError | null>('script_check', { code }),
};

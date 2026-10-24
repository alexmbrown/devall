import { invoke } from '@tauri-apps/api/core';
import type { HttpRequestFile } from '@/lib/types';

// Mirrors of crates/devall-import (plan.rs, apply.rs, export/mod.rs, codegen/mod.rs) and the
// commands in src-tauri/src/modules/import.rs. Keep in sync.

export type ImportFormat =
  | 'postman'
  | 'postmanEnvironment'
  | 'insomnia'
  | 'bruno'
  | 'openCollection'
  | 'openApi'
  | 'swagger'
  | 'har'
  | 'httpFile'
  | 'hurl'
  | 'curl'
  /** A WebAssembly plugin's import format. */
  | 'plugin';

export interface ParseOptions {
  /** HAR: keep Cookie headers (session cookies are then extracted as secrets). */
  includeCookies?: boolean;
  /** HAR: keep scripts, styles, images, fonts and other static assets. */
  includeAssets?: boolean;
}

/** Pasted text, or a file/folder path. */
export interface ImportSource {
  text?: string;
  path?: string;
  options?: ParseOptions;
  /** A plugin importer (`<plugin>:<importer>`) instead of the built-in formats. */
  plugin?: string;
  /** The open project (project plugins). */
  root?: string;
}

/** A plugin importer that recognised the input. */
export interface PluginImportMatch {
  id: string;
  plugin: string;
  name: string;
  confidence: number;
}

export interface DetectResult {
  format: ImportFormat | null;
  label: string | null;
  isPath: boolean;
  isDir: boolean;
  /** Set when only a plugin importer recognised it. */
  plugin: PluginImportMatch | null;
}

export type SummaryNode =
  | { kind: 'folder'; name: string; hasSettings: boolean; children: SummaryNode[] }
  | { kind: 'request'; name: string; method: string; url: string };

export interface EnvSummary {
  name: string;
  variables: string[];
  secrets: string[];
}

export interface SecretCandidate {
  name: string;
  /** Environment name, or '' for every environment. */
  scope: string;
  source: string;
  hasValue: boolean;
}

export interface PlanSummary {
  name: string;
  format: ImportFormat;
  formatLabel: string;
  requestCount: number;
  folderCount: number;
  hasSettings: boolean;
  tree: SummaryNode[];
  environments: EnvSummary[];
  projectVariables: string[];
  secrets: SecretCandidate[];
  warnings: string[];
}

export interface ApplyOptions {
  /** Put everything in a new folder named after the collection (default true). */
  wrapInFolder?: boolean;
  folderName?: string | null;
  /** Update same-named items in place instead of creating copies (default false). */
  overwrite?: boolean;
  importEnvironments?: boolean;
  importVariables?: boolean;
  /** Turn credentials into {{secret.NAME}} references (default true). */
  extractSecrets?: boolean;
}

export interface DetectedSecret {
  /** Environment id, or '*' for every environment. */
  environment: string;
  name: string;
  /** May be empty when the export only declared the secret. */
  value: string;
  source: string;
}

export interface ApplyReport {
  folder: string;
  foldersCreated: number;
  foldersMerged: number;
  requestsCreated: number;
  requestsUpdated: number;
  environmentsCreated: string[];
  environmentsUpdated: string[];
  variablesAdded: string[];
  secrets: DetectedSecret[];
  warnings: string[];
  firstRequest: string | null;
}

export interface ExportFile {
  fileName: string;
  content: string;
}

export interface ExportOutput {
  content: string;
  fileName: string;
  /** Preview language: 'yaml' | 'json' | 'http' | 'hurl'. */
  language: string;
  warnings: string[];
  attachments: ExportFile[];
}

export type ExportFormat = 'openapi' | 'postman' | 'http' | 'hurl';

export type CodegenLanguage =
  | 'curl'
  | 'httpie'
  | 'powershell'
  | 'javascript-fetch'
  | 'node-axios'
  | 'python-requests'
  | 'python-httpx'
  | 'go'
  | 'rust-reqwest'
  | 'java-httpclient'
  | 'kotlin-okhttp'
  | 'csharp-httpclient'
  | 'php-curl'
  | 'ruby-net-http'
  | 'swift-urlsession'
  | 'hurl';

export interface LanguageInfo {
  id: CodegenLanguage;
  label: string;
  group: string;
  /** Monaco language id. */
  editor: string;
  extension: string;
}

/** Typed wrappers over src-tauri/src/modules/import.rs. Errors reject with a string. */
export const importApi = {
  detect: (textOrPath: string, root?: string | null) => invoke<DetectResult>('import_detect', { textOrPath, root: root ?? null }),
  preview: (source: ImportSource) => invoke<PlanSummary>('import_preview', { source }),
  apply: (root: string, source: ImportSource, targetFolder: string, options: ApplyOptions) =>
    invoke<ApplyReport>('import_apply', { root, source, targetFolder, options }),

  exportOpenapi: (root: string, folder?: string, env?: string) => invoke<ExportOutput>('export_openapi', { root, folder, env }),
  exportPostman: (root: string, folder?: string) => invoke<ExportOutput>('export_postman', { root, folder }),
  exportHttpFile: (root: string, folder?: string) => invoke<ExportOutput>('export_http_file', { root, folder }),
  exportHurl: (root: string, folder?: string) => invoke<ExportOutput>('export_hurl', { root, folder }),
  exportWrite: (dest: string, content: string) => invoke<void>('export_write', { dest, content }),

  codegenLanguages: () => invoke<LanguageInfo[]>('codegen_languages'),
  codegen: (root: string, path: string, request: HttpRequestFile, env: string | undefined, language: CodegenLanguage, includeSecrets: boolean) =>
    invoke<string>('codegen', { root, path, request, env, language, includeSecrets }),
  /** Code from a plugin codegen target (`<plugin>:<target>`). */
  codegenPlugin: (root: string, path: string, request: HttpRequestFile, env: string | undefined, target: string, includeSecrets: boolean) =>
    invoke<string>('codegen_plugin', { root, path, request, env, target, includeSecrets }),
};

export function exportFor(format: ExportFormat, root: string, folder?: string, env?: string): Promise<ExportOutput> {
  if (format === 'openapi') return importApi.exportOpenapi(root, folder, env);
  if (format === 'postman') return importApi.exportPostman(root, folder);
  if (format === 'hurl') return importApi.exportHurl(root, folder);
  return importApi.exportHttpFile(root, folder);
}

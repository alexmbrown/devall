import { invoke } from '@tauri-apps/api/core';
import type { ApplyReport } from './import';

// Mirrors of crates/devall-openapi (lint.rs, drift.rs), devall_import::sync and the commands in
// src-tauri/src/modules/openapi.rs. Keep in sync.

export type Severity = 'error' | 'warning' | 'info';

export interface LintFinding {
  rule: string;
  severity: Severity;
  message: string;
  /** e.g. `paths./users/{id}.get.responses.200` */
  location: string;
}

export interface LintReport {
  title: string;
  version: string;
  operations: number;
  errors: number;
  warnings: number;
  infos: number;
  findings: LintFinding[];
}

export type DriftKind =
  | 'undocumentedPath'
  | 'undocumentedMethod'
  | 'undocumentedStatus'
  | 'invalidResponseBody'
  | 'invalidRequestBody'
  | 'undocumentedContentType'
  | 'unverifiable';

export interface DriftFinding {
  kind: DriftKind;
  severity: Severity;
  sourceKind: 'mock' | 'traffic';
  /** Stub file or traffic origin (`inspector`, `history`, a HAR file name). */
  source: string;
  /** Stub id or exchange / history id. */
  refId: string | null;
  label: string;
  method: string;
  path: string;
  operation: string | null;
  status: number | null;
  message: string;
  details: string[];
  /** Requests showing this problem (traffic findings are grouped). */
  count: number;
}

export interface OperationCoverage {
  operation: string;
  operationId: string | null;
  mocks: number;
  traffic: number;
}

export interface DriftReport {
  title: string;
  operations: number;
  mocksChecked: number;
  trafficChecked: number;
  trafficSkipped: number;
  errors: number;
  warnings: number;
  infos: number;
  findings: DriftFinding[];
  coverage: OperationCoverage[];
}

export interface DriftOptions {
  mocks?: boolean;
  inspector?: boolean;
  /** Inspector exchange ids; empty = everything captured. */
  inspectorIds?: number[];
  history?: boolean;
  /** HAR file (absolute or project-relative). */
  har?: string;
  allHosts?: boolean;
}

export interface OpenApiLink {
  /** Project-relative path, absolute path or URL. */
  source: string;
  autoSync?: boolean;
}

export interface LinkedFolder {
  /** Relative to `http/`. */
  folder: string;
  name: string;
  link: OpenApiLink;
  projectPath: string | null;
  exists: boolean;
}

export interface SyncReport {
  folder: string;
  source: string;
  report: ApplyReport;
  /** Requests whose operation is no longer in the spec (not deleted). */
  stale: string[];
}

export interface AutoSyncResult {
  folder: string;
  report: SyncReport | null;
  error: string | null;
}

/** Typed wrappers over src-tauri/src/modules/openapi.rs. Errors reject with a string. */
export const openapiApi = {
  /** OpenAPI 3 files in the project (project-relative). */
  specs: (root: string) => invoke<string[]>('openapi_specs', { root }),
  lintFile: (root: string, path: string) => invoke<LintReport>('openapi_lint', { root, path }),
  lintText: (text: string) => invoke<LintReport>('openapi_lint', { text }),
  drift: (root: string, spec: string, options: DriftOptions) => invoke<DriftReport>('openapi_drift', { root, spec, options }),
  links: (root: string) => invoke<LinkedFolder[]>('openapi_links', { root }),
  link: (root: string, folder: string, source: string, autoSync: boolean, resync = false) =>
    invoke<[OpenApiLink, SyncReport | null]>('openapi_link', { root, folder, source, autoSync, resync }),
  unlink: (root: string, folder: string) => invoke<void>('openapi_unlink', { root, folder }),
  resync: (root: string, folder: string) => invoke<SyncReport>('openapi_resync', { root, folder }),
  autosync: (root: string, paths: string[]) => invoke<AutoSyncResult[]>('openapi_autosync', { root, paths }),
};

/** One-line summary of a re-sync for toasts. */
export function syncSummary(r: SyncReport): string {
  const parts = [`${r.report.requestsUpdated} updated`, `${r.report.requestsCreated} added`];
  if (r.stale.length) parts.push(`${r.stale.length} no longer in the spec`);
  return parts.join(' · ');
}

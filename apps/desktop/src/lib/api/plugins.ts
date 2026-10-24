import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import type { OpInfo } from './utils';

/**
 * WebAssembly plugins (crates/devall-plugins, src-tauri/src/modules/plugins.rs). Plugins add
 * utility ops, request hooks, import formats, codegen targets and auth providers; this module
 * mirrors their types and keeps the ready plugins' catalog for the open project.
 */

export type PluginScope = 'user' | 'project';
export type PluginStatus = 'ready' | 'disabled' | 'untrusted' | 'changed' | 'broken' | 'shadowed';

export interface PluginArgSpec {
  name: string;
  label: string;
  /** `text`, `secret`, `number`, `textarea`, `select:a|b`. */
  kind: string;
  default: string;
}

export interface PluginManifest {
  id: string;
  name: string;
  version: string;
  apiVersion: number;
  description?: string;
  author?: string;
  homepage?: string;
  wasm?: string;
  capabilities: { http?: { hosts: string[] }; 'secrets.read'?: string[]; 'fs.project.read'?: boolean };
  contributes: {
    utilities?: { id: string; name: string; description?: string }[];
    hooks?: { id: string; name: string; events: HookEvent[]; hosts?: string[] }[];
    importers?: { id: string; name: string; extensions?: string[] }[];
    codegen?: { id: string; name: string; language?: string }[];
    auth?: { id: string; name: string; description?: string; fields?: PluginArgSpec[] }[];
  };
}

export type HookEvent = 'preRequest' | 'postResponse';

export interface SecretStatus {
  name: string;
  granted: boolean;
  /** A value is stored in the keychain. */
  set: boolean;
}

/** Mirrors devall_plugins::PluginInfo. */
export interface PluginInfo {
  id: string;
  scope: PluginScope;
  dir: string;
  status: PluginStatus;
  error: string | null;
  manifest: PluginManifest | null;
  wasmSha256: string | null;
  approvedSha256: string | null;
  enabled: boolean;
  /** Capabilities as sentences (for prompts). */
  capabilities: string[];
  secrets: SecretStatus[];
  disabledHooks: string[];
}

export interface InstallPreview {
  manifest: PluginManifest;
  wasmSha256: string;
  wasmBytes: number;
  capabilities: string[];
  /** Version of the installed plugin this would replace. */
  replaces: string | null;
}

export interface UtilityInfo {
  /** `<plugin>:<op>`. */
  id: string;
  plugin: string;
  pluginName: string;
  name: string;
  description: string;
  category: string;
  output: 'text' | 'json';
  noInput: boolean;
  args: PluginArgSpec[];
  example: string | null;
}

export interface ImporterInfo {
  id: string;
  plugin: string;
  name: string;
  extensions: string[];
}

export interface CodegenInfo {
  id: string;
  plugin: string;
  name: string;
  language: string;
}

export interface AuthProviderInfo {
  plugin: string;
  pluginName: string;
  id: string;
  name: string;
  description: string;
  fields: PluginArgSpec[];
}

export interface HookInfo {
  plugin: string;
  id: string;
  name: string;
  events: HookEvent[];
  hosts: string[];
  enabled: boolean;
}

export interface Catalog {
  utilities: UtilityInfo[];
  importers: ImporterInfo[];
  codegen: CodegenInfo[];
  auth: AuthProviderInfo[];
  hooks: HookInfo[];
}

export interface PluginLogLine {
  level: string;
  message: string;
}

export const EMPTY_CATALOG: Catalog = { utilities: [], importers: [], codegen: [], auth: [], hooks: [] };

export const pluginsApi = {
  list: (root: string | null) => invoke<PluginInfo[]>('plugins_list', { root }),
  catalog: (root: string | null) => invoke<Catalog>('plugins_catalog', { root }),
  dir: () => invoke<string>('plugins_dir'),
  preview: (source: string) => invoke<InstallPreview>('plugins_preview', { source }),
  install: (source: string, sha256: string, grantSecrets: string[]) => invoke<PluginInfo>('plugins_install', { source, sha256, grantSecrets }),
  uninstall: (id: string) => invoke<void>('plugins_uninstall', { id }),
  trust: (root: string | null, scope: PluginScope, id: string, sha256: string, grantSecrets: string[]) =>
    invoke<PluginInfo>('plugins_trust', { root, scope, id, sha256, grantSecrets }),
  revoke: (root: string | null, scope: PluginScope, id: string) => invoke<void>('plugins_revoke', { root, scope, id }),
  setEnabled: (root: string | null, scope: PluginScope, id: string, enabled: boolean) => invoke<void>('plugins_set_enabled', { root, scope, id, enabled }),
  setHookEnabled: (root: string | null, scope: PluginScope, id: string, hook: string, enabled: boolean) =>
    invoke<void>('plugins_set_hook_enabled', { root, scope, id, hook, enabled }),
  setGrants: (root: string | null, scope: PluginScope, id: string, secrets: string[]) => invoke<void>('plugins_set_grants', { root, scope, id, secrets }),
  /** Stores (or with `null` deletes) a plugin secret in the OS keychain. */
  setSecret: (id: string, name: string, value: string | null) => invoke<void>('plugins_set_secret', { id, name, value }),
  runUtility: (root: string | null, op: string, input: string, args: Record<string, string>) =>
    invoke<{ output: string; logs: PluginLogLine[] }>('plugins_run_utility', { root, op, input, args }),
};

/** A plugin utility in the shape of a built-in op (Utilities page, command palette). */
export function utilityToOp(u: UtilityInfo): OpInfo {
  return {
    id: u.id,
    name: u.name,
    category: u.category || 'Plugins',
    description: u.description || `From the ${u.pluginName} plugin`,
    output: u.output === 'json' ? 'json' : 'text',
    noInput: u.noInput,
    args: u.args.map((a) => ({ name: a.name, label: a.label || a.name, kind: a.kind || 'text', default: a.default ?? '' })),
    network: false,
    examples: u.example ? [{ input: u.example, args: {} }] : [],
  };
}

/** Plugin contribution ids look like `<plugin>:<id>`. */
export function isPluginOp(id: string): boolean {
  return id.includes(':');
}

/** Short label for a status. */
export function statusLabel(s: PluginStatus): string {
  switch (s) {
    case 'ready':
      return 'Ready';
    case 'disabled':
      return 'Disabled';
    case 'untrusted':
      return 'Not trusted';
    case 'changed':
      return 'Changed — review';
    case 'broken':
      return 'Error';
    case 'shadowed':
      return 'Shadowed';
  }
}

interface CatalogState {
  /** Project the catalog was loaded for. */
  root: string | null;
  catalog: Catalog;
  loaded: boolean;
  refresh(root?: string | null): Promise<void>;
}

/** Contributions of the ready plugins for the open project (refreshed on project change and by the Plugins page). */
export const usePluginCatalog = create<CatalogState>((set, get) => ({
  root: null,
  catalog: EMPTY_CATALOG,
  loaded: false,
  async refresh(root) {
    const r = root === undefined ? get().root : root;
    try {
      const catalog = await pluginsApi.catalog(r);
      set({ root: r, catalog, loaded: true });
    } catch {
      set({ root: r, catalog: EMPTY_CATALOG, loaded: true });
    }
  },
}));

/** Utility ops contributed by plugins, as OpInfo. */
export function pluginOps(c: Catalog): OpInfo[] {
  return c.utilities.map(utilityToOp);
}

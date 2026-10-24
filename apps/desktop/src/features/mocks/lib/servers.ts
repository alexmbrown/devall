/**
 * Remote WireMock servers saved per project in `mocks/servers.yaml` (committed, shared with the
 * team). Only non-secret connection details live there; passwords / tokens / header values go to
 * the OS keychain under `MOCK_SERVER_<ID>_TOKEN` (all-environments scope) or are typed for the
 * current session only.
 */
import { api, isTauri } from '@/lib/ipc';
import type { AuthType } from './api';
import { parseYaml, stringifyYaml } from './yaml';

export type EnvKind = 'local' | 'dev' | 'test' | 'staging' | 'prod' | 'other';

export const ENV_LABEL: Record<EnvKind, string> = {
  local: 'Local',
  dev: 'Dev',
  test: 'Test',
  staging: 'Staging',
  prod: 'Prod',
  other: 'Other',
};

export interface RemoteServer {
  /** Stable slug, unique within the file. */
  id: string;
  name: string;
  url: string;
  /** WireMock under a context path, e.g. `/mocks`. */
  contextPath?: string;
  env?: EnvKind;
  /** Ask for confirmation before any write. */
  protected?: boolean;
  /** Never write from the UI. */
  readOnly?: boolean;
  auth?: { type: AuthType; username?: string; headerName?: string };
  /** Extra non-secret headers sent with every admin call. */
  headers?: { key: string; value: string }[];
  /** Accept self-signed / invalid TLS certificates (proxy transport only). */
  insecureTls?: boolean;
  notes?: string;
}

export const SERVERS_FILE = 'mocks/servers.yaml';

/** Keychain name holding the secret of server `id`. */
export function secretNameFor(id: string): string {
  return `MOCK_SERVER_${id.toUpperCase().replace(/[^A-Z0-9]+/g, '_').replace(/^_+|_+$/g, '')}_TOKEN`;
}

export function slugify(s: string): string {
  return (
    s
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 40) || 'server'
  );
}

export function uniqueId(base: string, taken: string[]): string {
  let id = slugify(base);
  let n = 2;
  const set = new Set(taken);
  while (set.has(id)) id = `${slugify(base)}-${n++}`;
  return id;
}

/** Header names that look like credentials: their values must not be stored in the file. */
export const SECRET_HEADER = /auth|token|key|secret|password|cookie|session/i;

const ENVS = Object.keys(ENV_LABEL);
const AUTHS: AuthType[] = ['none', 'basic', 'bearer', 'header'];

/** Validates and normalises the parsed file (unknown fields are dropped). */
export function readServers(data: unknown): { servers: RemoteServer[]; warnings: string[] } {
  const warnings: string[] = [];
  const list = (data as { servers?: unknown })?.servers;
  if (data == null) return { servers: [], warnings };
  if (!Array.isArray(list)) return { servers: [], warnings: ['servers.yaml: expected a "servers:" list'] };
  const out: RemoteServer[] = [];
  for (const [i, raw] of list.entries()) {
    const o = (raw ?? {}) as Record<string, unknown>;
    const url = typeof o.url === 'string' ? o.url.trim() : '';
    if (!url) {
      warnings.push(`servers[${i}]: missing url (skipped)`);
      continue;
    }
    const name = typeof o.name === 'string' && o.name.trim() ? o.name.trim() : url;
    const id = uniqueId(typeof o.id === 'string' && o.id ? o.id : name, out.map((s) => s.id));
    const s: RemoteServer = { id, name, url };
    if (typeof o.contextPath === 'string' && o.contextPath) s.contextPath = o.contextPath;
    if (typeof o.env === 'string' && ENVS.includes(o.env)) s.env = o.env as EnvKind;
    if (o.protected === true) s.protected = true;
    if (o.readOnly === true) s.readOnly = true;
    if (o.insecureTls === true) s.insecureTls = true;
    if (typeof o.notes === 'string' && o.notes) s.notes = o.notes;
    const a = o.auth as Record<string, unknown> | undefined;
    if (a && typeof a.type === 'string' && AUTHS.includes(a.type as AuthType) && a.type !== 'none') {
      s.auth = { type: a.type as AuthType };
      if (typeof a.username === 'string') s.auth.username = a.username;
      if (typeof a.headerName === 'string') s.auth.headerName = a.headerName;
      for (const k of ['password', 'token', 'value', 'headerValue']) if (a[k] !== undefined) warnings.push(`${id}: auth.${k} is ignored — secrets belong in the OS keychain, not in servers.yaml`);
    }
    if (Array.isArray(o.headers)) {
      const hs = (o.headers as { key?: unknown; value?: unknown }[])
        .filter((h) => typeof h?.key === 'string' && h.key)
        .map((h) => ({ key: String(h.key), value: h.value == null ? '' : String(h.value) }));
      if (hs.length) s.headers = hs;
    }
    out.push(s);
  }
  return { servers: out, warnings };
}

/** File content for `servers` (stable key order, no secrets, no UI state). */
export function serializeServers(servers: RemoteServer[]): string {
  const clean = servers.map((s) => ({
    id: s.id,
    name: s.name,
    url: s.url,
    contextPath: s.contextPath || undefined,
    env: s.env,
    readOnly: s.readOnly || undefined,
    protected: s.protected || undefined,
    insecureTls: s.insecureTls || undefined,
    auth: s.auth && s.auth.type !== 'none' ? { type: s.auth.type, username: s.auth.type === 'basic' ? s.auth.username : undefined, headerName: s.auth.type === 'header' ? s.auth.headerName : undefined } : undefined,
    headers: s.headers?.length ? s.headers.map((h) => ({ key: h.key, value: h.value })) : undefined,
    notes: s.notes || undefined,
  }));
  return `# Remote WireMock servers for the /dev/all mock UI. Secrets are never stored here:
# they live in the OS keychain (MOCK_SERVER_<ID>_TOKEN) or are entered per session.
${stringifyYaml({ version: 1, servers: clean })}`;
}

/** Problems that must block saving (credential-looking extra headers). */
export function secretProblems(servers: RemoteServer[]): string[] {
  const out: string[] = [];
  for (const s of servers) for (const h of s.headers ?? []) if (SECRET_HEADER.test(h.key) && h.value) out.push(`${s.name}: header "${h.key}" looks like a credential — use Authentication → Custom header instead (stored in the keychain).`);
  return out;
}

export function joinPath(root: string, rel: string): string {
  const sep = root.includes('\\') && !root.includes('/') ? '\\' : '/';
  return root.replace(/[\\/]+$/, '') + sep + rel.split('/').join(sep);
}

/** YAML → data, using the Rust parser when available (full YAML 1.2) and the built-in one otherwise. */
export async function parseYamlText(text: string): Promise<unknown> {
  if (isTauri) {
    try {
      return JSON.parse(await api.utilRun('yaml.toJson', text));
    } catch (e) {
      // Fall through to the TS parser only when the utility itself is unavailable.
      if (!/unknown|not found/i.test(String(e))) throw new Error(String(e));
    }
  }
  return parseYaml(text);
}

export async function loadServers(root: string): Promise<{ servers: RemoteServer[]; warnings: string[]; exists: boolean }> {
  let text: string;
  try {
    text = await api.readTextFile(joinPath(root, SERVERS_FILE));
  } catch {
    return { servers: [], warnings: [], exists: false };
  }
  const data = await parseYamlText(text);
  return { ...readServers(data), exists: true };
}

export async function saveServers(root: string, servers: RemoteServer[]): Promise<void> {
  await api.writeTextFile(joinPath(root, SERVERS_FILE), serializeServers(servers));
}

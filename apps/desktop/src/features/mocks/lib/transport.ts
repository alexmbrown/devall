/**
 * How admin API calls leave the app.
 *
 * - `proxy`: the `wiremock_admin` Tauri command (src-tauri/src/modules/wiremock_proxy.rs) sends the
 *   call from Rust. No CORS, no webview CSP, and credentials stored in the OS keychain are
 *   resolved there, so the webview never sees them.
 * - `fetch`: the webview calls the server directly. Works when the command is not registered
 *   (and in tests); the app's CSP must then allow the server's origin in `connect-src`, and the
 *   server must answer CORS (WireMock does; the built-in engine allows Tauri origins).
 *
 * `auto` uses the proxy when the command exists and falls back to fetch otherwise.
 */
import { invoke } from '@tauri-apps/api/core';
import { isTauri } from '@/lib/ipc';

export interface AdminRequest {
  /** Server base URL including any context path, e.g. `http://127.0.0.1:8080` or `https://h/mocks`. */
  baseUrl: string;
  method: string;
  /** Starts with `/__admin`. */
  path: string;
  headers: Record<string, string>;
  body?: Uint8Array;
  timeoutMs: number;
  /** Credentials kept in the OS keychain, resolved by the proxy. */
  keychainAuth?: KeychainAuth;
  /** Project root whose keychain scope holds `keychainAuth.secret`. */
  root?: string;
  insecureTls?: boolean;
}

export interface KeychainAuth {
  type: 'basic' | 'bearer' | 'header';
  username?: string;
  headerName?: string;
  /** Secret name in the project's all-environments keychain scope. */
  secret: string;
}

export interface AdminResponse {
  status: number;
  statusText: string;
  headers: Record<string, string>;
  body: Uint8Array;
}

/** Thrown by transports for failures that never produced an HTTP response. */
export class TransportError extends Error {
  constructor(
    message: string,
    public kind: 'network' | 'timeout' | 'tls' | 'credentials' | 'blocked',
  ) {
    super(message);
  }
}

export type TransportMode = 'auto' | 'proxy' | 'fetch';

let proxyAvailable: boolean | undefined = isTauri ? undefined : false;

/** Whether the `wiremock_admin` command is registered (undefined until the first call). */
export function proxyStatus(): boolean | undefined {
  return proxyAvailable;
}

export function b64encode(bytes: Uint8Array): string {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export function b64decode(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

interface ProxyResponse {
  status: number;
  statusText: string;
  headers: [string, string][];
  bodyBase64: string;
}

const MISSING = /not found|unknown command|not allowed|isn't allowed|denied/i;

async function viaProxy(req: AdminRequest): Promise<AdminResponse> {
  const r = await invoke<ProxyResponse>('wiremock_admin', {
    request: {
      root: req.root,
      baseUrl: req.baseUrl,
      method: req.method,
      path: req.path,
      headers: Object.entries(req.headers),
      bodyBase64: req.body ? b64encode(req.body) : undefined,
      auth: req.keychainAuth,
      timeoutMs: req.timeoutMs,
      insecureTls: req.insecureTls,
    },
  });
  return { status: r.status, statusText: r.statusText, headers: Object.fromEntries(r.headers.map(([k, v]) => [k.toLowerCase(), v])), body: b64decode(r.bodyBase64) };
}

function proxyError(e: unknown): TransportError {
  const m = typeof e === 'string' ? e : e instanceof Error ? e.message : JSON.stringify(e);
  if (/timed out|timeout/i.test(m)) return new TransportError(m, 'timeout');
  if (/certificate|tls|ssl|handshake/i.test(m)) return new TransportError(m, 'tls');
  if (/secret|keychain|credential/i.test(m)) return new TransportError(m, 'credentials');
  return new TransportError(m, 'network');
}

async function viaFetch(req: AdminRequest): Promise<AdminResponse> {
  if (req.keychainAuth)
    throw new TransportError(
      'This server’s credentials are stored in the OS keychain, which only the app’s Rust side can read. Register the wiremock_admin command (see wiremock_proxy.rs) or enter the credential for this session.',
      'credentials',
    );
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), req.timeoutMs);
  try {
    const res = await fetch(req.baseUrl.replace(/\/+$/, '') + req.path, {
      method: req.method,
      headers: req.headers,
      body: req.body ? new Blob([req.body.slice().buffer]) : undefined,
      signal: ctrl.signal,
      cache: 'no-store',
    });
    const headers: Record<string, string> = {};
    res.headers.forEach((v, k) => (headers[k.toLowerCase()] = v));
    return { status: res.status, statusText: res.statusText, headers, body: new Uint8Array(await res.arrayBuffer()) };
  } catch (e) {
    if ((e as Error).name === 'AbortError') throw new TransportError(`Timed out after ${req.timeoutMs} ms`, 'timeout');
    throw new TransportError(
      'Could not reach the server. It may be down or the URL wrong; when calling from the webview, the app’s content security policy or the server’s CORS answer can also block the call.',
      'blocked',
    );
  } finally {
    clearTimeout(timer);
  }
}

export async function send(req: AdminRequest, mode: TransportMode = 'auto'): Promise<AdminResponse> {
  if (mode === 'fetch' || (mode === 'auto' && proxyAvailable === false)) return viaFetch(req);
  try {
    const r = await viaProxy(req);
    proxyAvailable = true;
    return r;
  } catch (e) {
    const m = typeof e === 'string' ? e : e instanceof Error ? e.message : '';
    if (proxyAvailable !== true && MISSING.test(m) && /wiremock_admin/.test(m)) {
      proxyAvailable = false;
      if (mode === 'proxy') throw new TransportError('The wiremock_admin command is not registered in this build.', 'network');
      return viaFetch(req);
    }
    if (proxyAvailable === undefined && !isTauri) {
      proxyAvailable = false;
      return viaFetch(req);
    }
    throw proxyError(e);
  }
}

/** Whether the keychain holds `name` in the project's all-environments scope (undefined = can't tell). */
export async function keychainHas(root: string, name: string): Promise<boolean | undefined> {
  if (!isTauri) return undefined;
  try {
    return await invoke<boolean>('wiremock_secret_status', { root, name });
  } catch {
    return undefined;
  }
}

/** Writes bytes to a path the user picked (falls back to `undefined` when the command is missing). */
export async function writeBytes(path: string, bytes: Uint8Array): Promise<boolean> {
  if (!isTauri) return false;
  try {
    await invoke('wiremock_write_file', { path, dataBase64: b64encode(bytes) });
    return true;
  } catch (e) {
    const m = typeof e === 'string' ? e : e instanceof Error ? e.message : '';
    if (MISSING.test(m) && /wiremock_write_file/.test(m)) return false;
    throw e;
  }
}

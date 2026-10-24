// Minimal Workers KV client (Cloudflare REST API) used to push entitlements and reserved
// subdomains to the relay edge. Token scope needed: Account → Workers KV Storage → Edit.

export interface KvConfig {
  accountId: string;
  namespaceId: string;
  apiToken: string;
  /** Override for tests. */
  apiBase?: string;
}

export class KvError extends Error {
  override name = "KvError";
}

function valueUrl(cfg: KvConfig, key: string): string {
  const base = cfg.apiBase ?? "https://api.cloudflare.com/client/v4";
  return `${base}/accounts/${encodeURIComponent(cfg.accountId)}/storage/kv/namespaces/${encodeURIComponent(cfg.namespaceId)}/values/${encodeURIComponent(key)}`;
}

async function check(res: Response, what: string): Promise<void> {
  let body: { success?: boolean; errors?: { message?: string }[] } = {};
  try {
    body = (await res.json()) as typeof body;
  } catch {
    // non-JSON error page
  }
  if (!res.ok || body.success === false) {
    const msg = body.errors?.map((e) => e.message).filter(Boolean).join("; ") || `HTTP ${res.status}`;
    throw new KvError(`${what}: ${msg}`);
  }
}

export async function kvPut(cfg: KvConfig, key: string, value: string, fetchImpl: typeof fetch = fetch): Promise<void> {
  if (!cfg.accountId || !cfg.namespaceId || !cfg.apiToken) throw new KvError("Cloudflare KV is not configured");
  const res = await fetchImpl(valueUrl(cfg, key), {
    method: "PUT",
    headers: { authorization: `Bearer ${cfg.apiToken}`, "content-type": "text/plain" },
    body: value,
  });
  await check(res, `KV put ${key}`);
}

export async function kvDelete(cfg: KvConfig, key: string, fetchImpl: typeof fetch = fetch): Promise<void> {
  if (!cfg.accountId || !cfg.namespaceId || !cfg.apiToken) throw new KvError("Cloudflare KV is not configured");
  const res = await fetchImpl(valueUrl(cfg, key), { method: "DELETE", headers: { authorization: `Bearer ${cfg.apiToken}` } });
  if (res.status === 404) return;
  await check(res, `KV delete ${key}`);
}

// ---------- Cloudflare for SaaS: custom hostnames ----------
// Token scope needed: Zone → SSL and Certificates → Edit (on the tunnel zone).

export interface SaasConfig {
  zoneId: string;
  apiToken: string;
  apiBase?: string;
}

/** The fields of a custom hostname we read (Cloudflare API `custom_hostname` object). */
export interface CfCustomHostname {
  id: string;
  hostname: string;
  status?: string;
  ssl?: {
    status?: string;
    method?: string;
    validation_records?: { txt_name?: string; txt_value?: string; http_url?: string; http_body?: string; cname?: string; cname_target?: string }[];
    validation_errors?: { message?: string }[];
  };
  verification_errors?: string[];
  ownership_verification?: { type?: string; name?: string; value?: string };
  created_at?: string;
}

export class CfApiError extends Error {
  override name = "CfApiError";
  constructor(
    message: string,
    readonly status: number,
    readonly codes: number[],
  ) {
    super(message);
  }
}

function zoneUrl(cfg: SaasConfig, path: string): string {
  const base = cfg.apiBase ?? "https://api.cloudflare.com/client/v4";
  return `${base}/zones/${encodeURIComponent(cfg.zoneId)}/custom_hostnames${path}`;
}

async function cfCall<T>(cfg: SaasConfig, method: string, path: string, body: unknown, fetchImpl: typeof fetch): Promise<T> {
  if (!cfg.zoneId || !cfg.apiToken) throw new CfApiError("Cloudflare for SaaS is not configured (CF_ZONE_ID / CF_API_TOKEN)", 500, []);
  const res = await fetchImpl(zoneUrl(cfg, path), {
    method,
    headers: { authorization: `Bearer ${cfg.apiToken}`, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let json: { success?: boolean; errors?: { code?: number; message?: string }[]; result?: T } = {};
  try {
    json = (await res.json()) as typeof json;
  } catch {
    // non-JSON error page
  }
  if (!res.ok || json.success === false) {
    const msg = json.errors?.map((e) => e.message).filter(Boolean).join("; ") || `HTTP ${res.status}`;
    throw new CfApiError(msg, res.status, (json.errors ?? []).map((e) => e.code ?? 0));
  }
  return json.result as T;
}

/** Creates a custom hostname with HTTP domain-control validation (works once the CNAME is in place). */
export function createCustomHostname(cfg: SaasConfig, hostname: string, fetchImpl: typeof fetch = fetch): Promise<CfCustomHostname> {
  return cfCall<CfCustomHostname>(cfg, "POST", "", { hostname, ssl: { method: "http", type: "dv", settings: { min_tls_version: "1.2" } } }, fetchImpl);
}

export function getCustomHostname(cfg: SaasConfig, id: string, fetchImpl: typeof fetch = fetch): Promise<CfCustomHostname> {
  return cfCall<CfCustomHostname>(cfg, "GET", `/${encodeURIComponent(id)}`, undefined, fetchImpl);
}

export async function findCustomHostname(cfg: SaasConfig, hostname: string, fetchImpl: typeof fetch = fetch): Promise<CfCustomHostname | null> {
  const list = await cfCall<CfCustomHostname[]>(cfg, "GET", `?hostname=${encodeURIComponent(hostname)}`, undefined, fetchImpl);
  return Array.isArray(list) ? (list.find((h) => h.hostname === hostname) ?? null) : null;
}

export async function deleteCustomHostname(cfg: SaasConfig, id: string, fetchImpl: typeof fetch = fetch): Promise<void> {
  try {
    await cfCall<unknown>(cfg, "DELETE", `/${encodeURIComponent(id)}`, undefined, fetchImpl);
  } catch (e) {
    if (e instanceof CfApiError && e.status === 404) return;
    throw e;
  }
}

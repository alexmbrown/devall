import type { AccountDO } from "./account";
import type { InboxDO } from "./inbox";
import type { LoadDO } from "./load";
import type { MockDO } from "./mock";
import type { MonitorDO } from "./monitor";
import type { ShareDO } from "./share";
import type { TunnelDO } from "./tunnel";

/** Bindings and configuration (wrangler.toml `[vars]` + secrets). */
export interface Env {
  TUNNEL: DurableObjectNamespace<TunnelDO>;
  INBOX: DurableObjectNamespace<InboxDO>;
  ACCOUNT: DurableObjectNamespace<AccountDO>;
  /** One per published (hosted) mock name. */
  MOCK: DurableObjectNamespace<MockDO>;
  /** One per shared request/response snapshot. */
  SHARE: DurableObjectNamespace<ShareDO>;
  /** One per scheduled monitor. */
  MONITOR: DurableObjectNamespace<MonitorDO>;
  /** One per region of a cloud load run (`newUniqueId()` + `locationHint`). */
  LOAD: DurableObjectNamespace<LoadDO>;
  /** Reserved subdomains (`sub:<name>`, `subs:<uid>`), entitlements (`plan:<uid>`), blocked accounts (`blocked-user:<uid>`). */
  RELAY_KV: KVNamespace;

  /** Registrable domain tunnels live under, e.g. `devall.dev` (relay.<d>, hooks.<d>, <sub>.<d>). */
  PUBLIC_DOMAIN: string;
  /** `firebase` (hosted), `static` (self-host with RELAY_TOKENS) or `both`. */
  AUTH_MODE?: string;
  FIREBASE_PROJECT_ID?: string;
  /** Override for tests / mirrors; defaults to Google's securetoken JWKS. */
  FIREBASE_JWKS_URL?: string;
  /** Secret. Comma-separated static tokens (`token` or `name:token`) for self-hosting. */
  RELAY_TOKENS?: string;
  /** Plan given to static-token users (default `selfhost` = everything unlimited, no interstitial). */
  STATIC_PLAN?: string;
  /** Optional JSON map of static-token name → plan, e.g. `{"friend":"free"}` (community relays). */
  STATIC_PLANS?: string;
  /** Secret. Bearer token for /admin/* (takedowns). */
  ADMIN_TOKEN?: string;
  /** `auto` (free plan only) or `off`. */
  INTERSTITIAL?: string;
  ABUSE_EMAIL?: string;
  /** Public URL of this relay's source code (AGPL-3.0 §13), linked from every page. */
  SOURCE_URL?: string;
  /** Where "Become a Supporter" points (the hosted sign-in site). */
  UPGRADE_URL?: string;
  /** Optional JSON overrides per plan, e.g. `{"free":{"bandwidthBytesPerMonth":2147483648}}`. */
  LIMITS_JSON?: string;
  /**
   * Self-host custom domains: `{"api.example.com":"staging-box"}` or `api.example.com=staging-box,…`.
   * Each hostname also needs a Worker route (or Custom Domain) in the same Cloudflare account.
   */
  CUSTOM_HOSTS?: string;
  /** What customers CNAME their hostname to (Cloudflare for SaaS fallback origin), shown in the app. */
  CUSTOM_DOMAIN_TARGET?: string;
  /** Hosted mocks live at `<name>.<MOCK_DOMAIN>` (default `mock.<PUBLIC_DOMAIN>`); `mock.<PUBLIC_DOMAIN>/<name>/…` always works. */
  MOCK_DOMAIN?: string;
  /** Which hosted-mock URL the API hands out: `host` (`<name>.<MOCK_DOMAIN>`, default) or `path`. */
  MOCK_URL_STYLE?: string;
  /** Secret. Master key for monitor secrets (AES-GCM, one derived key per account). Without it, monitors can't store secrets. */
  MONITOR_SECRET_KEY?: string;
  /** `off` disables cloud load regions on this relay (also `POST /admin/load/disable`). */
  CLOUD_LOAD?: string;
  /** `off`: verify load-test targets by the HTTP file only (no DNS-over-HTTPS TXT lookup). */
  LOAD_VERIFY_DNS?: string;
  /** URL answering `colo=XXX` (a `/cdn-cgi/trace`) used to record where a region ran; `off` to skip. */
  LOAD_COLO_PROBE?: string;
  /** Optional Workers rate-limiting binding for the connect endpoints (`[[ratelimits]]`). */
  CONNECT_LIMITER?: { limit(opts: { key: string }): Promise<{ success: boolean }> };
}

export function domainOf(env: Env): string {
  return (env.PUBLIC_DOMAIN || "devall.dev").toLowerCase().replace(/^\.+|\.+$/g, "");
}

export function relayHost(env: Env): string {
  return `relay.${domainOf(env)}`;
}

export function hooksHost(env: Env): string {
  return `hooks.${domainOf(env)}`;
}

export function tunnelUrl(env: Env, sub: string): string {
  return `https://${sub}.${domainOf(env)}`;
}

export function inboxUrl(env: Env, inboxId: string): string {
  return `https://${hooksHost(env)}/${inboxId}/`;
}

export function authModes(env: Env): { firebase: boolean; static: boolean } {
  const m = (env.AUTH_MODE || "firebase").toLowerCase();
  return { firebase: m === "firebase" || m === "both", static: m === "static" || m === "both" };
}

export function shareHost(env: Env): string {
  return `share.${domainOf(env)}`;
}

export function shareUrl(env: Env, id: string): string {
  return `https://${shareHost(env)}/${id}`;
}

/** Parent domain of host-style hosted mocks (`<name>.<mockDomain>`). */
export function mockDomain(env: Env): string {
  const d = (env.MOCK_DOMAIN || `mock.${domainOf(env)}`).toLowerCase().replace(/^\.+|\.+$/g, "");
  return d;
}

/** Path-style hosted mock base (works with the zone's Universal SSL certificate). */
export function mockPathUrl(env: Env, name: string): string {
  return `https://mock.${domainOf(env)}/${name}`;
}

export function mockHostUrl(env: Env, name: string): string {
  return `https://${name}.${mockDomain(env)}`;
}

/** The URL the API hands out for a hosted mock. */
export function mockUrl(env: Env, name: string): string {
  return (env.MOCK_URL_STYLE ?? "host").toLowerCase() === "path" ? mockPathUrl(env, name) : mockHostUrl(env, name);
}

/** CNAME target customers point their hostname at. */
export function customDomainTarget(env: Env): string {
  return (env.CUSTOM_DOMAIN_TARGET || `custom.${domainOf(env)}`).toLowerCase();
}

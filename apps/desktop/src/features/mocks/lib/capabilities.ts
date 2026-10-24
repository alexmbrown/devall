/**
 * What the connected WireMock can do, derived from `GET /__admin/version`
 * (3.x and 4.x) with `/__admin/docs/swagger` as a fallback when the version
 * is unknown (2.x, or a proxy that hides it).
 */
export interface Capabilities {
  version?: string;
  major?: number;
  /** WireMock 4.x (Java 17, Jetty 12). */
  v4: boolean;
  /** `/__admin/message-mappings`, `/__admin/channels`, `/__admin/messages` (WebSocket / SSE). */
  messages: boolean;
  /** equalToNumber, greaterThanNumber, … */
  numberMatchers: boolean;
  /** `method: { oneOf: […] }` / `{ noneOf: […] }`. */
  methodSets: boolean;
  /** `response.openWebsocketChannel` / `openSseChannel` (4.0.0-beta.39+). */
  channelStubs: boolean;
  /** Jetty 12 never sends a custom reason phrase, so `statusMessage` is ignored on the wire. */
  statusMessage: boolean;
  /** 3.x returns and accepts `uuid` as an alias for `id`; 4.x does neither. */
  uuidAlias: boolean;
  /** Which bundled JSON schema validates stubs. */
  schema: 3 | 4;
}

export interface ParsedVersion {
  major: number;
  minor: number;
  patch: number;
  /** e.g. `beta.40`. */
  pre?: string;
  /** Numeric part of a `beta.N` pre-release. */
  beta?: number;
}

export function parseVersion(v?: string): ParsedVersion | undefined {
  const m = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?/.exec(v?.trim() ?? '');
  if (!m) return undefined;
  const beta = m[4] ? /^beta\.(\d+)$/.exec(m[4]) : null;
  return { major: +m[1], minor: +m[2], patch: +m[3], pre: m[4], beta: beta ? +beta[1] : undefined };
}

/**
 * @param version   from /__admin/version or /__admin/health
 * @param specPaths OpenAPI path keys, used only when the version is unknown
 */
export function capabilitiesFor(version?: string, specPaths?: string[]): Capabilities {
  const pv = parseVersion(version);
  const hasPath = (p: string) => !!specPaths?.some((x) => x.endsWith(p));
  const v4 = pv ? pv.major >= 4 : hasPath('/message-mappings');
  // Channel-opening stubs arrived in 4.0.0-beta.39; assume support on any later 4.x.
  const channelStubs = v4 && (!pv || pv.major > 4 || pv.minor > 0 || pv.patch > 0 || pv.beta === undefined || pv.beta >= 39);
  return {
    version,
    major: pv?.major ?? (v4 ? 4 : undefined),
    v4,
    messages: v4,
    numberMatchers: v4,
    methodSets: v4,
    channelStubs,
    statusMessage: !v4,
    uuidAlias: !v4,
    schema: v4 ? 4 : 3,
  };
}

export const DEFAULT_CAPABILITIES = capabilitiesFor(undefined);

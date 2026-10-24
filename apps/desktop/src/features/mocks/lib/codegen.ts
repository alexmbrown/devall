import type { ContentPattern, LoggedRequest, RequestPattern, StubMapping } from './types';
import { METHODS_V4, methodSetOf, opOf, urlKeyOf, urlOf } from './stub-utils';

const shq = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;

/** A concrete example request that should match the stub (best effort for regex/template URLs). */
export function exampleRequest(r: RequestPattern): { method: string; path: string; headers: Record<string, string>; body?: string } {
  const method = exampleMethod(r.method);
  let path = urlOf(r) || '/';
  const key = urlKeyOf(r);
  if (key === 'urlPathTemplate') {
    path = path.replace(/\{([^}]+)\}/g, (_, name) => {
      const p = r.pathParameters?.[name];
      return encodeURIComponent(p ? exampleValue(p, guessValue(name)) : guessValue(name));
    });
  } else if (key === 'urlPattern' || key === 'urlPathPattern') {
    path = deRegex(path);
  }
  if (key !== 'url' && key !== 'urlPattern' && r.queryParameters) {
    const qs = Object.entries(r.queryParameters)
      .filter(([, p]) => opOf(p) !== 'absent')
      .map(([k, p]) => `${encodeURIComponent(k)}=${encodeURIComponent(exampleValue(p, k))}`)
      .join('&');
    if (qs) path += (path.includes('?') ? '&' : '?') + qs;
  }
  const headers: Record<string, string> = {};
  for (const [k, p] of Object.entries(r.headers ?? {})) if (opOf(p) !== 'absent') headers[k] = exampleValue(p, k);
  if (r.cookies && Object.keys(r.cookies).length)
    headers.Cookie = Object.entries(r.cookies)
      .map(([k, p]) => `${k}=${exampleValue(p, k)}`)
      .join('; ');
  if (r.basicAuthCredentials) headers.Authorization = `Basic ${btoa(`${r.basicAuthCredentials.username}:${r.basicAuthCredentials.password}`)}`;
  let body: string | undefined;
  const bp = r.bodyPatterns?.[0];
  if (bp) {
    if (bp.equalToJson !== undefined) body = typeof bp.equalToJson === 'string' ? bp.equalToJson : JSON.stringify(bp.equalToJson, null, 2);
    else if (bp.equalToXml) body = bp.equalToXml;
    else if (typeof bp.equalTo === 'string') body = bp.equalTo;
    else if (bp.contains) body = bp.contains;
    else if (bp.matchesJsonPath) body = '{}';
    if (body && !Object.keys(headers).some((h) => h.toLowerCase() === 'content-type')) {
      headers['Content-Type'] = bp.equalToXml || bp.matchesXPath ? 'application/xml' : bp.equalToJson !== undefined || bp.matchesJsonPath ? 'application/json' : 'text/plain';
    }
  }
  if (r.formParameters && !body) {
    body = Object.entries(r.formParameters)
      .map(([k, p]) => `${encodeURIComponent(k)}=${encodeURIComponent(exampleValue(p, k))}`)
      .join('&');
    headers['Content-Type'] = 'application/x-www-form-urlencoded';
  }
  return { method, path, headers, body };
}

/** A method the pattern accepts: the first of `oneOf`, or the first common method not in `noneOf`. */
function exampleMethod(m: RequestPattern['method']): string {
  const set = methodSetOf(m);
  if (set?.kind === 'oneOf') return set.methods[0] ?? 'GET';
  if (set) return METHODS_V4.find((x) => x !== 'ANY' && !set.methods.some((y) => y.toUpperCase() === x)) ?? 'GET';
  return !m || m === 'ANY' ? 'GET' : String(m);
}

/** Plausible value for an unconstrained path variable: ids → 1, otherwise the name. */
function guessValue(name: string): string {
  return /id$/i.test(name) ? '1' : name;
}

function exampleValue(p: ContentPattern, fallback: string): string {
  if (p.equalTo !== undefined) return String(p.equalTo);
  if (typeof p.contains === 'string') return p.contains;
  if (typeof p.matches === 'string') return deRegex(p.matches) || fallback;
  if (p.hasExactly?.[0]) return exampleValue(p.hasExactly[0], fallback);
  if (p.includes?.[0]) return exampleValue(p.includes[0], fallback);
  if (p.and?.[0]) return exampleValue(p.and[0], fallback);
  if (p.or?.[0]) return exampleValue(p.or[0], fallback);
  if (p.equalToDateTime) return String(p.equalToDateTime);
  if (p.equalToNumber !== undefined) return String(p.equalToNumber);
  if (p.greaterThanEqualNumber !== undefined) return String(p.greaterThanEqualNumber);
  if (p.lessThanEqualNumber !== undefined) return String(p.lessThanEqualNumber);
  if (p.greaterThanNumber !== undefined) return String(Math.floor(Number(p.greaterThanNumber)) + 1);
  if (p.lessThanNumber !== undefined) return String(Math.ceil(Number(p.lessThanNumber)) - 1);
  return fallback;
}

/** Turn a simple regex into a plausible literal (".*" → "x", "[0-9]+" → "1"). */
export function deRegex(re: string): string {
  return re
    .replace(/^\^|\$$/g, '')
    .replace(/\\d\+|\[0-9\]\+|\\d\*|\[0-9\]\*/g, '1')
    .replace(/\\d|\[0-9\]/g, '1')
    .replace(/\[\^\/\]\+|\[\^\/\]\*/g, 'x')
    .replace(/\.\*|\.\+/g, 'x')
    .replace(/\[[^\]]*\]\+?/g, 'a')
    .replace(/\((?:\?:)?([^|)]*)(\|[^)]*)?\)/g, '$1')
    .replace(/[?*+]/g, '')
    .replace(/\\(.)/g, '$1');
}

export function curlForStub(baseUrl: string, s: StubMapping): string {
  const ex = exampleRequest(s.request);
  return curl(baseUrl + ex.path, ex.method, ex.headers, ex.body);
}

export function curlForLogged(r: LoggedRequest, baseUrl?: string): string {
  const url = r.absoluteUrl || `${baseUrl ?? ''}${r.url}`;
  const headers: Record<string, string> = {};
  for (const [k, v] of Object.entries(r.headers ?? {})) {
    if (/^(host|content-length|connection|accept-encoding)$/i.test(k)) continue;
    headers[k] = Array.isArray(v) ? v.join(', ') : v;
  }
  return curl(url, r.method, headers, r.body || undefined);
}

export function curl(url: string, method: string, headers: Record<string, string>, body?: string): string {
  const parts = [`curl -i -X ${method.toUpperCase()} ${shq(url)}`];
  for (const [k, v] of Object.entries(headers)) parts.push(`-H ${shq(`${k}: ${v}`)}`);
  if (body) parts.push(`--data-raw ${shq(body)}`);
  return parts.join(' \\\n  ');
}

/** Admin-API cURL that creates this stub on a server. */
export function curlCreateStub(adminBase: string, s: StubMapping): string {
  return `curl -X POST ${shq(`${adminBase}/mappings`)} \\\n  -H 'Content-Type: application/json' \\\n  --data-raw ${shq(JSON.stringify(s, null, 2))}`;
}

// ---------------------------------------------------------------- Java DSL

const j = (s: unknown) => JSON.stringify(String(s));
/** A Java numeric literal (`18`, `10.5`, `1.0E21`); the DSL takes `Number`, so int and double both box. */
const javaNumber = (n: unknown) => {
  const x = Number(n);
  if (!Number.isFinite(x)) return `Double.valueOf(${j(n)})`;
  return Number.isInteger(x) && Math.abs(x) <= 2147483647 ? String(x) : Number.isInteger(x) ? `${x}d` : String(x);
};
const textBlock = (s: string) => (s.includes('\n') || s.length > 80 ? `"""\n${s.replace(/\\/g, '\\\\').replace(/"""/g, '\\"""')}"""` : j(s));

function javaPattern(p: ContentPattern): string {
  const op = opOf(p);
  const ci = p.caseInsensitive;
  switch (op) {
    case 'equalTo':
      return ci ? `equalToIgnoreCase(${j(p.equalTo)})` : `equalTo(${j(p.equalTo)})`;
    case 'contains':
      return `containing(${j(p.contains)})`;
    case 'doesNotContain':
      return `notContaining(${j(p.doesNotContain)})`;
    case 'matches':
      return `matching(${j(p.matches)})`;
    case 'doesNotMatch':
      return `notMatching(${j(p.doesNotMatch)})`;
    case 'absent':
      return 'absent()';
    case 'equalToJson': {
      const v = typeof p.equalToJson === 'string' ? p.equalToJson : JSON.stringify(p.equalToJson, null, 2);
      return `equalToJson(${textBlock(v)}, ${!!p.ignoreArrayOrder}, ${!!p.ignoreExtraElements})`;
    }
    case 'matchesJsonPath': {
      const v = p.matchesJsonPath;
      if (typeof v === 'string') return `matchingJsonPath(${j(v)})`;
      if (v && typeof v === 'object') {
        const { expression, ...rest } = v as { expression: string };
        return `matchingJsonPath(${j(expression)}, ${javaPattern(rest as ContentPattern)})`;
      }
      return 'matchingJsonPath("$")';
    }
    case 'matchesJsonSchema':
      return `matchingJsonSchema(${textBlock(typeof p.matchesJsonSchema === 'string' ? p.matchesJsonSchema : JSON.stringify(p.matchesJsonSchema, null, 2))})`;
    case 'equalToXml':
      return `equalToXml(${textBlock(String(p.equalToXml))}${p.enablePlaceholders ? ', true' : ''})`;
    case 'matchesXPath': {
      const v = p.matchesXPath;
      if (typeof v === 'string') return `matchingXPath(${j(v)})`;
      const { expression, ...rest } = v as { expression: string };
      return `matchingXPath(${j(expression)}, ${javaPattern(rest as ContentPattern)})`;
    }
    case 'before':
      return `before(${j(p.before)})`;
    case 'after':
      return `after(${j(p.after)})`;
    case 'equalToDateTime':
      return `equalToDateTime(${j(p.equalToDateTime)})`;
    case 'binaryEqualTo':
      return `binaryEqualTo(java.util.Base64.getDecoder().decode(${j(p.binaryEqualTo)}))`;
    case 'equalToNumber':
    case 'greaterThanNumber':
    case 'greaterThanEqualNumber':
    case 'lessThanNumber':
    case 'lessThanEqualNumber':
      return `${op}(${javaNumber(p[op])})`;
    case 'and':
      return `and(${(p.and ?? []).map(javaPattern).join(', ')})`;
    case 'or':
      return `or(${(p.or ?? []).map(javaPattern).join(', ')})`;
    case 'not':
      return `not(${javaPattern(p.not as ContentPattern)})`;
    case 'hasExactly':
      return `havingExactly(${(p.hasExactly ?? []).map(javaPattern).join(', ')})`;
    case 'includes':
      return `including(${(p.includes ?? []).map(javaPattern).join(', ')})`;
    default:
      return `/* unsupported matcher ${op} */ equalTo("")`;
  }
}

const JAVA_METHOD_CONSTANTS = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'OPTIONS', 'HEAD', 'TRACE', 'ANY', 'QUERY'];
const javaMethod = (m: string) => {
  const u = m.toUpperCase();
  return JAVA_METHOD_CONSTANTS.includes(u) ? `RequestMethod.${u}` : `RequestMethod.fromString(${j(m)})`;
};

/** Generate WireMock Java DSL (com.github.tomakehurst.wiremock.client.WireMock.*). */
export function javaDsl(s: StubMapping): string {
  const r = s.request;
  const set = methodSetOf(r.method);
  const method = set ? '' : typeof r.method === 'string' && r.method ? r.method.toUpperCase() : 'ANY';
  const key = urlKeyOf(r);
  const urlFn: Record<string, string> = {
    url: 'urlEqualTo',
    urlPath: 'urlPathEqualTo',
    urlPattern: 'urlMatching',
    urlPathPattern: 'urlPathMatching',
    urlPathTemplate: 'urlPathTemplate',
  };
  const urlExpr = key ? `${urlFn[key]}(${j(r[key])})` : 'anyUrl()';
  const methodFn: Record<string, string> = { GET: 'get', POST: 'post', PUT: 'put', PATCH: 'patch', DELETE: 'delete', HEAD: 'head', OPTIONS: 'options', TRACE: 'trace', ANY: 'any', QUERY: 'query' };
  const lines: string[] = [];
  let imports = '// import static com.github.tomakehurst.wiremock.client.WireMock.*;';
  if (set) {
    // WireMock 4: RequestMethod.isOneOf / isNoneOf.
    imports += '\n// import com.github.tomakehurst.wiremock.http.RequestMethod;';
    lines.push(`request(RequestMethod.${set.kind === 'oneOf' ? 'isOneOf' : 'isNoneOf'}(${set.methods.map(javaMethod).join(', ')}), ${urlExpr})`);
  } else lines.push(methodFn[method] ? `${methodFn[method]}(${urlExpr})` : `request(${j(method)}, ${urlExpr})`);
  const ind = '    ';
  if (s.name) lines.push(`${ind}.withName(${j(s.name)})`);
  if (s.priority !== undefined) lines.push(`${ind}.atPriority(${s.priority})`);
  if (s.id) lines.push(`${ind}.withId(java.util.UUID.fromString(${j(s.id)}))`);
  for (const [k, p] of Object.entries(r.pathParameters ?? {})) lines.push(`${ind}.withPathParam(${j(k)}, ${javaPattern(p)})`);
  for (const [k, p] of Object.entries(r.queryParameters ?? {})) lines.push(`${ind}.withQueryParam(${j(k)}, ${javaPattern(p)})`);
  for (const [k, p] of Object.entries(r.headers ?? {})) lines.push(`${ind}.withHeader(${j(k)}, ${javaPattern(p)})`);
  for (const [k, p] of Object.entries(r.cookies ?? {})) lines.push(`${ind}.withCookie(${j(k)}, ${javaPattern(p)})`);
  for (const [k, p] of Object.entries(r.formParameters ?? {})) lines.push(`${ind}.withFormParam(${j(k)}, ${javaPattern(p)})`);
  if (r.host) lines.push(`${ind}.withHost(${javaPattern(r.host)})`);
  if (r.port) lines.push(`${ind}.withPort(${r.port})`);
  if (r.scheme) lines.push(`${ind}.withScheme(${j(r.scheme)})`);
  if (r.basicAuthCredentials) lines.push(`${ind}.withBasicAuth(${j(r.basicAuthCredentials.username)}, ${j(r.basicAuthCredentials.password)})`);
  for (const bp of r.bodyPatterns ?? []) lines.push(`${ind}.withRequestBody(${javaPattern(bp)})`);
  for (const mp of r.multipartPatterns ?? []) {
    const parts = [`aMultipart()`];
    if (mp.name) parts.push(`.withName(${j(mp.name)})`);
    for (const [k, p] of Object.entries(mp.headers ?? {})) parts.push(`.withHeader(${j(k)}, ${javaPattern(p)})`);
    for (const bp of mp.bodyPatterns ?? []) parts.push(`.withBody(${javaPattern(bp)})`);
    lines.push(`${ind}.withMultipartRequestBody(${parts.join('')})`);
  }
  if (s.scenarioName) {
    lines.push(`${ind}.inScenario(${j(s.scenarioName)})`);
    if (s.requiredScenarioState) lines.push(`${ind}.whenScenarioStateIs(${s.requiredScenarioState === 'Started' ? 'com.github.tomakehurst.wiremock.stubbing.Scenario.STARTED' : j(s.requiredScenarioState)})`);
    if (s.newScenarioState) lines.push(`${ind}.willSetStateTo(${j(s.newScenarioState)})`);
  }
  if (s.persistent) lines.push(`${ind}.persistent()`);
  if (s.metadata) lines.push(`${ind}.withMetadata(com.github.tomakehurst.wiremock.common.Metadata.metadata()/* ${JSON.stringify(s.metadata)} */)`);

  const resp = s.response ?? {};
  const rl: string[] = [];
  if (resp.fault) {
    rl.push(`aResponse().withFault(com.github.tomakehurst.wiremock.http.Fault.${resp.fault})`);
  } else if (resp.openWebsocketChannel || resp.openSseChannel) {
    // WireMock 4 (beta.39+): the request is upgraded / turned into an event stream.
    rl.push(`aResponse().${resp.openWebsocketChannel ? 'openWebsocketChannel' : 'openSseChannel'}()`);
    if (resp.status !== undefined && !resp.openWebsocketChannel) rl.push(`.withStatus(${resp.status})`);
  } else if (resp.proxyBaseUrl) {
    rl.push(`aResponse().proxiedFrom(${j(resp.proxyBaseUrl)})`);
    if (resp.proxyUrlPrefixToRemove) rl.push(`.withProxyUrlPrefixToRemove(${j(resp.proxyUrlPrefixToRemove)})`);
    for (const [k, v] of Object.entries(resp.additionalProxyRequestHeaders ?? {})) rl.push(`.withAdditionalRequestHeader(${j(k)}, ${j(v)})`);
    for (const k of resp.removeProxyRequestHeaders ?? []) rl.push(`.withRemoveRequestHeader(${j(k)})`);
  } else {
    rl.push('aResponse()');
    rl.push(`.withStatus(${resp.status ?? 200})`);
    if (resp.statusMessage) rl.push(`.withStatusMessage(${j(resp.statusMessage)})`);
  }
  for (const [k, v] of Object.entries(resp.headers ?? {})) rl.push(`.withHeader(${j(k)}, ${(Array.isArray(v) ? v : [v]).map(j).join(', ')})`);
  if (resp.jsonBody !== undefined) rl.push(`.withJsonBody(com.github.tomakehurst.wiremock.common.Json.node(${textBlock(JSON.stringify(resp.jsonBody, null, 2))}))`);
  else if (resp.body !== undefined) rl.push(`.withBody(${textBlock(resp.body)})`);
  else if (resp.base64Body) rl.push(`.withBase64Body(${j(resp.base64Body)})`);
  else if (resp.bodyFileName) rl.push(`.withBodyFile(${j(resp.bodyFileName)})`);
  if (resp.fixedDelayMilliseconds) rl.push(`.withFixedDelay(${resp.fixedDelayMilliseconds})`);
  const dd = resp.delayDistribution;
  if (dd?.type === 'uniform') rl.push(`.withUniformRandomDelay(${dd.lower}, ${dd.upper})`);
  if (dd?.type === 'lognormal') rl.push(`.withLogNormalRandomDelay(${dd.median}, ${dd.sigma})`);
  if (resp.chunkedDribbleDelay) rl.push(`.withChunkedDribbleDelay(${resp.chunkedDribbleDelay.numberOfChunks}, ${resp.chunkedDribbleDelay.totalDuration})`);
  if (resp.transformers?.length) rl.push(`.withTransformers(${resp.transformers.map(j).join(', ')})`);
  for (const [k, v] of Object.entries(resp.transformerParameters ?? {})) rl.push(`.withTransformerParameter(${j(k)}, ${typeof v === 'string' ? j(v) : String(v)})`);
  lines.push(`${ind}.willReturn(${rl.join(`\n${ind}${ind}`)})`);
  for (const l of s.serveEventListeners ?? []) {
    if (l.name === 'webhook') {
      const p = (l.parameters ?? {}) as { method?: string; url?: string; headers?: Record<string, string>; body?: string };
      const wl = [`webhook()`, `.withMethod(${j(p.method ?? 'POST')})`, `.withUrl(${j(p.url ?? '')})`];
      for (const [k, v] of Object.entries(p.headers ?? {})) wl.push(`.withHeader(${j(k)}, ${j(v)})`);
      if (p.body) wl.push(`.withBody(${textBlock(p.body)})`);
      lines.push(`${ind}.withServeEventListener("webhook", ${wl.join('')})`);
    } else lines.push(`${ind}.withServeEventListener(${j(l.name)}, com.github.tomakehurst.wiremock.extension.Parameters.from(java.util.Map.of(/* ${JSON.stringify(l.parameters ?? {})} */)))`);
  }
  return `${imports}\nstubFor(${lines.join('\n')});`;
}

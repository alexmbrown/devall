/** The supported script API: inline help sections and the member tree behind autocomplete. */

export interface HelpItem {
  code: string;
  doc: string;
}

export interface HelpSection {
  title: string;
  items: HelpItem[];
}

export const HELP: HelpSection[] = [
  {
    title: 'Variables',
    items: [
      { code: "pm.variables.get('name')", doc: 'Any layer: runtime → data row → environment → folders → project.' },
      { code: "pm.variables.set('name', value)", doc: 'Runtime variable (in memory, never saved) usable as {{name}} by later requests.' },
      { code: 'pm.environment.get / set / unset / has / toObject', doc: 'Active environment with runtime overrides. set never writes the environment file.' },
      { code: 'pm.collectionVariables.get / set / unset', doc: 'Folder and project variables with runtime overrides.' },
      { code: 'pm.globals.get / set / unset', doc: 'The runtime layer.' },
      { code: "pm.iterationData.get('column')", doc: 'Current data-file row in a collection run.' },
      { code: "pm.variables.replaceIn('{{baseUrl}}/x')", doc: 'Resolves {{templates}} and {{$dynamic}} values; {{secret.*}} stays unresolved.' },
      { code: "pm.secrets.get('NAME')", doc: 'Keychain secret (literal name only). Its value is masked in logs, errors and history.' },
    ],
  },
  {
    title: 'Request (pre-request)',
    items: [
      { code: "pm.request.method = 'POST'", doc: 'Change the method.' },
      { code: 'pm.request.url.toString() · pm.request.url = "…"', doc: 'Read or replace the URL ({{templates}} are not resolved yet).' },
      { code: "pm.request.url.query.upsert({ key, value }) · .remove('key')", doc: 'Edit query parameters.' },
      { code: "pm.request.headers.upsert({ key, value }) · .add · .remove · .get · .has", doc: 'Edit headers (names are case-insensitive).' },
      { code: "pm.request.body.raw = '…' · pm.request.body.update('…')", doc: 'Replace a raw (JSON/text/XML) body.' },
      { code: 'pm.execution.skipRequest()', doc: 'Don’t send this request.' },
    ],
  },
  {
    title: 'Response (post-response)',
    items: [
      { code: 'pm.response.code · status · responseTime · responseSize', doc: 'Status code, reason phrase, total time (ms), body size (bytes).' },
      { code: 'pm.response.json() · pm.response.text()', doc: 'Parsed JSON body (throws JSONError if invalid) or raw text.' },
      { code: "pm.response.headers.get('Content-Type')", doc: 'Header value (case-insensitive).' },
      { code: "pm.cookies.get('session')", doc: 'Cookie set by this response.' },
    ],
  },
  {
    title: 'Tests',
    items: [
      { code: "pm.test('name', () => { … })", doc: 'A named check; also supports a done callback or an async function.' },
      { code: 'pm.expect(x).to.equal(y) · eql · include · have.property · have.lengthOf', doc: 'chai-style assertions; .not negates, .deep compares structurally.' },
      { code: "be.a('string') · above · below · at.least · at.most · match(/re/) · oneOf([…])", doc: 'More chai assertions.' },
      { code: 'be.true · be.false · be.null · be.undefined · exist · be.empty · be.ok', doc: 'Property assertions.' },
      { code: "pm.response.to.have.status(200) · header('X') · jsonBody('a.b') · jsonSchema(s)", doc: 'Response assertions.' },
      { code: 'pm.response.to.be.ok · json · success · clientError · serverError · notFound', doc: 'Status-class assertions.' },
    ],
  },
  {
    title: 'Crypto & helpers',
    items: [
      { code: 'CryptoJS.HmacSHA256(msg, key).toString(CryptoJS.enc.Base64)', doc: 'Also SHA1/224/256/384/512 and MD5, plain or Hmac*. require(\'crypto-js\') works too.' },
      { code: 'CryptoJS.enc.Utf8 · Base64 · Hex · Latin1 .parse / .stringify', doc: 'Encoders.' },
      { code: "btoa · atob · Buffer.from(s).toString('base64')", doc: 'Base64 helpers.' },
      { code: 'crypto.randomUUID() · Date · JSON · _ (lodash subset)', doc: 'Standard helpers.' },
      { code: 'await · setTimeout · setInterval', doc: 'Top-level await works; timers run in virtual time after the script (no real waiting).' },
      { code: 'console.log · info · warn · error', doc: 'Output appears in the response panel’s Console tab.' },
    ],
  },
  {
    title: 'Bruno & legacy Postman',
    items: [
      { code: 'bru.getEnvVar · setEnvVar · getVar · setVar · interpolate', doc: 'Mapped onto the same variable model.' },
      { code: 'req.getUrl · setUrl · getHeader · setHeader · getBody · setBody · setMethod', doc: 'Pre-request request editing.' },
      { code: "res.getStatus() · res.getBody() · res.getHeader(n) · res('a.b')", doc: 'Post-response access; test() and expect() are globals.' },
      { code: "postman.setEnvironmentVariable · tests['name'] = bool · responseBody", doc: 'Old Postman sandbox globals.' },
    ],
  },
];

export const UNSUPPORTED = [
  'pm.sendRequest',
  'pm.execution.setNextRequest / postman.setNextRequest',
  'pm.visualizer',
  'CryptoJS.AES and other ciphers, SHA3, PBKDF2',
  'moment, cheerio, ajv, xml2Json',
  'editing form-data / urlencoded / GraphQL bodies',
  'files, network, process and OS environment',
];

export const LIMITS = '5 s and 64 MB per script · 1,000 console lines · a fresh sandbox for every script';

// ---------------------------------------------------------------- autocomplete

export type MemberKind = 'method' | 'property' | 'object';

export interface ApiMember {
  name: string;
  kind: MemberKind;
  doc: string;
  /** Snippet inserted for methods (Monaco snippet syntax); defaults to `name($0)`. */
  insert?: string;
}

const m = (name: string, doc: string, insert?: string): ApiMember => ({ name, kind: 'method', doc, insert });
const p = (name: string, doc: string): ApiMember => ({ name, kind: 'property', doc });
const o = (name: string, doc: string): ApiMember => ({ name, kind: 'object', doc });

const scopeMembers = (scope: string): ApiMember[] => [
  m('get', `Read a value (${scope})`, "get('${1:name}')"),
  m('set', `Set a value (${scope}; runtime only, never written to files)`, "set('${1:name}', ${2:value})"),
  m('unset', 'Remove the runtime value (a file value shows through again)', "unset('${1:name}')"),
  m('has', 'Whether the variable is defined', "has('${1:name}')"),
  m('toObject', 'All values as an object', 'toObject()'),
  m('clear', 'Remove the runtime overrides of this scope', 'clear()'),
  m('replaceIn', 'Resolve {{templates}} in a string', "replaceIn('${1:{{name\\}\\}}')"),
];

const listMembers = (what: string): ApiMember[] => [
  m('get', `${what} value by key`, "get('${1:key}')"),
  m('has', `Whether the ${what} exists`, "has('${1:key}')"),
  m('add', `Add a ${what}`, "add({ key: '${1:key}', value: '${2:value}' })"),
  m('upsert', `Add or replace a ${what}`, "upsert({ key: '${1:key}', value: '${2:value}' })"),
  m('remove', `Remove a ${what} by key`, "remove('${1:key}')"),
  m('all', 'Every entry as { key, value }', 'all()'),
  m('toObject', 'Entries as an object', 'toObject()'),
  m('each', 'Iterate entries', 'each((${1:item}) => {\n\t$0\n})'),
  m('count', 'Number of entries', 'count()'),
];

export const API_TREE: Record<string, ApiMember[]> = {
  '': [
    o('pm', 'Postman-compatible API'),
    o('console', 'Console output (shown in the response panel)'),
    o('CryptoJS', 'Hashes, HMACs and encoders (crypto-js compatible)'),
    o('crypto', 'randomUUID / getRandomValues'),
    m('btoa', 'Base64-encode a Latin-1 string'),
    m('atob', 'Decode base64'),
    m('require', "Built-in modules: 'crypto-js', 'chai', 'uuid', 'lodash', 'buffer', 'tv4'", "require('${1:crypto-js}')"),
    o('_', 'lodash subset'),
    o('bru', 'Bruno compatibility'),
    o('req', 'Bruno request (pre-request)'),
    o('res', 'Bruno response (post-response)'),
  ],
  pm: [
    m('test', 'Declare a named test', "test('${1:name}', () => {\n\t$0\n});"),
    m('expect', 'chai-style assertion', 'expect(${1:value}).to.${2:equal}(${3})'),
    o('variables', 'All variable layers'),
    o('environment', 'Active environment (runtime overrides)'),
    o('collectionVariables', 'Folder + project variables'),
    o('globals', 'Runtime variables'),
    o('iterationData', 'Current data-file row'),
    o('request', 'The request (editable in pre-request scripts)'),
    o('response', 'The response (post-response scripts)'),
    o('info', 'eventName, requestName, iteration, …'),
    o('cookies', 'Cookies set by the response'),
    o('execution', 'skipRequest()'),
    o('secrets', 'Keychain secrets by literal name'),
  ],
  'pm.variables': scopeMembers('all layers'),
  'pm.environment': [...scopeMembers('environment'), p('name', 'Active environment name')],
  'pm.collectionVariables': scopeMembers('folders and project'),
  'pm.globals': scopeMembers('runtime layer'),
  'pm.iterationData': [m('get', 'Value from the data row', "get('${1:column}')"), m('has', 'Whether the column exists', "has('${1:column}')"), m('toObject', 'The whole row', 'toObject()')],
  'pm.request': [
    p('method', 'HTTP method (assignable)'),
    o('url', 'URL object (assignable)'),
    o('headers', 'Header list'),
    o('body', 'Body (raw text)'),
    p('name', 'Request name'),
    p('id', 'Request id'),
    m('addHeader', 'Add a header', "addHeader({ key: '${1:key}', value: '${2:value}' })"),
    m('upsertHeader', 'Add or replace a header', "upsertHeader({ key: '${1:key}', value: '${2:value}' })"),
    m('removeHeader', 'Remove a header', "removeHeader('${1:key}')"),
  ],
  'pm.request.url': [
    m('toString', 'Full URL with query', 'toString()'),
    m('update', 'Replace the URL', "update('${1:url}')"),
    m('getHost', 'Host name', 'getHost()'),
    m('getPath', 'Path', 'getPath()'),
    m('getPathWithQuery', 'Path and query string', 'getPathWithQuery()'),
    m('getQueryString', 'Query string', 'getQueryString()'),
    o('query', 'Query parameter list'),
    m('addQueryParams', 'Add parameters', "addQueryParams('${1:a=1&b=2}')"),
    m('removeQueryParams', 'Remove parameters', "removeQueryParams(['${1:key}'])"),
    p('protocol', 'Scheme, e.g. https'),
    p('port', 'Port'),
    p('host', 'Host segments'),
    p('path', 'Path segments'),
    p('hash', 'Fragment'),
  ],
  'pm.request.url.query': listMembers('query parameter'),
  'pm.request.headers': listMembers('header'),
  'pm.request.body': [
    p('raw', 'Raw body text (assignable)'),
    p('mode', "'raw' or 'none'"),
    m('update', 'Replace the body', "update('${1:text}')"),
    m('toString', 'Body text', 'toString()'),
    m('isEmpty', 'Whether the body is empty', 'isEmpty()'),
  ],
  'pm.response': [
    p('code', 'Status code'),
    p('status', 'Reason phrase'),
    p('responseTime', 'Total time in ms'),
    p('responseSize', 'Body size in bytes'),
    o('headers', 'Header list'),
    m('json', 'Parsed JSON body', 'json()'),
    m('text', 'Body text', 'text()'),
    m('reason', 'Reason phrase', 'reason()'),
    m('size', '{ body, header, total }', 'size()'),
    o('to', 'Response assertions'),
  ],
  'pm.response.headers': listMembers('header'),
  'pm.response.to': [o('have', 'status / header / body / jsonBody / jsonSchema'), o('be', 'ok / json / success / error / …'), o('not', 'Negate')],
  'pm.response.to.have': [
    m('status', 'Status code or reason', 'status(${1:200})'),
    m('header', 'Header present (and value)', "header('${1:Content-Type}')"),
    m('body', 'Body text / RegExp / JSON', 'body(${1})'),
    m('jsonBody', 'JSON path exists (and equals)', "jsonBody('${1:path}')"),
    m('jsonSchema', 'Body matches a JSON schema', 'jsonSchema(${1:schema})'),
  ],
  'pm.response.to.be': ['ok', 'json', 'success', 'error', 'clientError', 'serverError', 'redirection', 'notFound', 'unauthorized', 'forbidden', 'badRequest', 'withBody'].map((n) =>
    p(n, `Assert the response is ${n}`),
  ),
  'pm.info': ['eventName', 'iteration', 'iterationCount', 'requestName', 'requestId'].map((n) => p(n, `pm.info.${n}`)),
  'pm.execution': [m('skipRequest', 'Don’t send this request (pre-request)', 'skipRequest()'), p('location', 'Path of the running request')],
  'pm.cookies': [m('get', 'Cookie value', "get('${1:name}')"), m('has', 'Whether the cookie was set', "has('${1:name}')"), m('toObject', 'All cookies', 'toObject()')],
  'pm.secrets': [m('get', 'Keychain secret by literal name', "get('${1:NAME}')"), m('has', 'Whether the secret was found', "has('${1:NAME}')")],
  console: ['log', 'info', 'warn', 'error', 'debug'].map((n) => m(n, `console.${n}`, `${n}(\${1})`)),
  CryptoJS: [
    ...['HmacSHA256', 'HmacSHA1', 'HmacSHA512', 'HmacSHA384', 'HmacSHA224', 'HmacMD5'].map((n) => m(n, `${n}(message, key) → WordArray`, `${n}(\${1:message}, \${2:key})`)),
    ...['SHA256', 'SHA1', 'SHA512', 'SHA384', 'SHA224', 'MD5'].map((n) => m(n, `${n}(message) → WordArray`, `${n}(\${1:message})`)),
    o('enc', 'Hex, Base64, Base64url, Utf8, Latin1'),
    o('lib', 'WordArray'),
    o('algo', 'Progressive hashing'),
  ],
  'CryptoJS.enc': ['Hex', 'Base64', 'Base64url', 'Utf8', 'Latin1'].map((n) => o(n, `${n} encoder: parse / stringify`)),
  crypto: [m('randomUUID', 'Random v4 UUID', 'randomUUID()'), m('getRandomValues', 'Fill a typed array with random bytes', 'getRandomValues(${1:new Uint8Array(16)})')],
  bru: [
    m('getEnvVar', 'Environment variable', "getEnvVar('${1:name}')"),
    m('setEnvVar', 'Set an environment variable (runtime)', "setEnvVar('${1:name}', ${2:value})"),
    m('getVar', 'Runtime variable', "getVar('${1:name}')"),
    m('setVar', 'Set a runtime variable', "setVar('${1:name}', ${2:value})"),
    m('interpolate', 'Resolve {{templates}}', "interpolate('${1}')"),
    m('getEnvName', 'Active environment name', 'getEnvName()'),
  ],
  req: ['getUrl', 'setUrl', 'getMethod', 'setMethod', 'getHeader', 'getHeaders', 'setHeader', 'setHeaders', 'deleteHeader', 'getBody', 'setBody', 'getName'].map((n) => m(n, `req.${n}`, `${n}(\${1})`)),
  res: ['getStatus', 'getStatusText', 'getHeader', 'getHeaders', 'getBody', 'getResponseTime', 'getSize'].map((n) => m(n, `res.${n}`, `${n}(\${1})`)),
};

import type { StubMapping } from './types';

export interface StubTemplate {
  id: string;
  group: 'Basics' | 'Matching' | 'Dynamic' | 'Resilience' | 'Proxy' | 'Integration';
  label: string;
  description: string;
  stubs: StubMapping[];
}

const json = { 'Content-Type': 'application/json' };

export const TEMPLATES: StubTemplate[] = [
  {
    id: 'json',
    group: 'Basics',
    label: 'JSON response',
    description: 'GET returning a static JSON body.',
    stubs: [{ name: 'Get item', request: { method: 'GET', urlPath: '/api/items/1' }, response: { status: 200, headers: json, jsonBody: { id: 1, name: 'Widget' } } }],
  },
  {
    id: 'crud',
    group: 'Basics',
    label: 'REST resource (CRUD)',
    description: 'List, get, create, update and delete stubs for /api/things, in one folder.',
    stubs: [
      { name: 'List things', request: { method: 'GET', urlPath: '/api/things' }, response: { status: 200, headers: json, jsonBody: [{ id: 1, name: 'One' }, { id: 2, name: 'Two' }] }, metadata: { 'wiremock-gui': { folder: '/things' } } },
      {
        name: 'Get thing',
        request: { method: 'GET', urlPathTemplate: '/api/things/{id}' },
        response: { status: 200, headers: json, body: '{ "id": "{{request.path.id}}", "name": "Thing {{request.path.id}}" }', transformers: ['response-template'] },
        metadata: { 'wiremock-gui': { folder: '/things' } },
      },
      {
        name: 'Create thing',
        request: { method: 'POST', urlPath: '/api/things', headers: { 'Content-Type': { contains: 'json' } } },
        response: { status: 201, headers: { ...json, Location: '/api/things/{{randomValue length=4 type=\'NUMERIC\'}}' }, body: '{{{request.body}}}', transformers: ['response-template'] },
        metadata: { 'wiremock-gui': { folder: '/things' } },
      },
      {
        name: 'Update thing',
        request: { method: 'PUT', urlPathTemplate: '/api/things/{id}' },
        response: { status: 200, headers: json, body: '{{{request.body}}}', transformers: ['response-template'] },
        metadata: { 'wiremock-gui': { folder: '/things' } },
      },
      { name: 'Delete thing', request: { method: 'DELETE', urlPathTemplate: '/api/things/{id}' }, response: { status: 204 }, metadata: { 'wiremock-gui': { folder: '/things' } } },
    ],
  },
  {
    id: 'not-found',
    group: 'Basics',
    label: 'Error (404 / 500)',
    description: 'Problem+JSON error body.',
    stubs: [
      {
        name: 'Not found',
        request: { method: 'GET', urlPathPattern: '/api/missing/.*' },
        response: { status: 404, headers: { 'Content-Type': 'application/problem+json' }, jsonBody: { type: 'about:blank', title: 'Not Found', status: 404 } },
      },
    ],
  },
  {
    id: 'file',
    group: 'Basics',
    label: 'Body from file',
    description: 'Serves a file from __files (upload it on the Files page).',
    stubs: [{ name: 'Large payload', request: { method: 'GET', urlPath: '/api/report' }, response: { status: 200, headers: json, bodyFileName: 'report.json' } }],
  },
  {
    id: 'cors',
    group: 'Basics',
    label: 'CORS preflight',
    description: 'Answers OPTIONS for any path with permissive CORS headers.',
    stubs: [
      {
        name: 'CORS preflight',
        priority: 1,
        request: { method: 'OPTIONS', urlPattern: '.*' },
        response: {
          status: 204,
          headers: {
            'Access-Control-Allow-Origin': '{{request.headers.Origin}}',
            'Access-Control-Allow-Methods': 'GET,POST,PUT,PATCH,DELETE,OPTIONS',
            'Access-Control-Allow-Headers': '{{request.headers.Access-Control-Request-Headers}}',
            'Access-Control-Allow-Credentials': 'true',
          },
          transformers: ['response-template'],
        },
      },
    ],
  },
  {
    id: 'json-body',
    group: 'Matching',
    label: 'Match JSON body',
    description: 'equalToJson with ${json-unit.any-string} placeholders and ignoreExtraElements.',
    stubs: [
      {
        name: 'Create order (match body)',
        request: {
          method: 'POST',
          urlPath: '/api/orders',
          bodyPatterns: [{ equalToJson: { customerId: '${json-unit.any-string}', items: '${json-unit.ignore}' }, ignoreExtraElements: true }],
        },
        response: { status: 201, headers: json, jsonBody: { orderId: 'ord_123', status: 'CREATED' } },
      },
    ],
  },
  {
    id: 'jsonpath',
    group: 'Matching',
    label: 'Match by JSONPath',
    description: 'Different responses depending on a field in the body.',
    stubs: [
      {
        name: 'Premium customer',
        priority: 1,
        request: { method: 'POST', urlPath: '/api/quote', bodyPatterns: [{ matchesJsonPath: { expression: '$.tier', equalTo: 'premium' } }] },
        response: { status: 200, headers: json, jsonBody: { discount: 20 } },
      },
      { name: 'Default customer', priority: 5, request: { method: 'POST', urlPath: '/api/quote' }, response: { status: 200, headers: json, jsonBody: { discount: 0 } } },
    ],
  },
  {
    id: 'graphql',
    group: 'Matching',
    label: 'GraphQL operation',
    description: 'Match a GraphQL POST by operationName.',
    stubs: [
      {
        name: 'GraphQL GetUser',
        request: { method: 'POST', urlPath: '/graphql', bodyPatterns: [{ matchesJsonPath: { expression: '$.operationName', equalTo: 'GetUser' } }] },
        response: { status: 200, headers: json, jsonBody: { data: { user: { id: '1', name: 'Ada' } } } },
      },
    ],
  },
  {
    id: 'soap',
    group: 'Matching',
    label: 'SOAP / XML (XPath)',
    description: 'Match a SOAP action by XPath.',
    stubs: [
      {
        name: 'SOAP GetPrice',
        request: {
          method: 'POST',
          urlPath: '/soap/prices',
          bodyPatterns: [{ matchesXPath: '//*[local-name()="GetPrice"]' }],
        },
        response: {
          status: 200,
          headers: { 'Content-Type': 'text/xml; charset=utf-8' },
          body: '<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><GetPriceResponse><Price>9.99</Price></GetPriceResponse></soap:Body></soap:Envelope>',
        },
      },
    ],
  },
  {
    id: 'query-header',
    group: 'Matching',
    label: 'Query + header + auth',
    description: 'Path, query parameter, bearer header and absent header.',
    stubs: [
      {
        name: 'Search (authorized)',
        request: {
          method: 'GET',
          urlPath: '/api/search',
          queryParameters: { q: { matches: '.+' }, page: { matches: '[0-9]+' } },
          headers: { Authorization: { matches: 'Bearer .+' }, 'X-Debug': { absent: true } },
        },
        response: { status: 200, headers: json, jsonBody: { results: [], page: 1 } },
      },
    ],
  },
  {
    id: 'template',
    group: 'Dynamic',
    label: 'Templated response',
    description: 'Echo path params, query, headers and generate random values with Handlebars.',
    stubs: [
      {
        name: 'Echo user',
        request: { method: 'GET', urlPathTemplate: '/api/users/{userId}' },
        response: {
          status: 200,
          headers: json,
          body: '{\n  "id": "{{request.path.userId}}",\n  "requestId": "{{randomValue type=\'UUID\'}}",\n  "lang": "{{request.query.lang}}",\n  "agent": "{{request.headers.User-Agent}}",\n  "generatedAt": "{{now format=\'yyyy-MM-dd\'T\'HH:mm:ssZ\'}}"\n}',
          transformers: ['response-template'],
        },
      },
    ],
  },
  {
    id: 'stateful',
    group: 'Dynamic',
    label: 'Stateful scenario',
    description: 'A to-do list that changes after a POST (scenario state machine).',
    stubs: [
      { name: 'Todos — empty', scenarioName: 'Todo list', requiredScenarioState: 'Started', request: { method: 'GET', urlPath: '/api/todos' }, response: { status: 200, headers: json, jsonBody: [] } },
      {
        name: 'Add todo',
        scenarioName: 'Todo list',
        requiredScenarioState: 'Started',
        newScenarioState: 'Has item',
        request: { method: 'POST', urlPath: '/api/todos' },
        response: { status: 201 },
      },
      {
        name: 'Todos — one item',
        scenarioName: 'Todo list',
        requiredScenarioState: 'Has item',
        request: { method: 'GET', urlPath: '/api/todos' },
        response: { status: 200, headers: json, jsonBody: [{ id: 1, title: 'Buy milk' }] },
      },
    ],
  },
  {
    id: 'delay',
    group: 'Resilience',
    label: 'Slow response',
    description: 'Log-normal random delay (realistic latency with a long tail).',
    stubs: [
      { name: 'Slow endpoint', request: { method: 'GET', urlPath: '/api/slow' }, response: { status: 200, delayDistribution: { type: 'lognormal', median: 800, sigma: 0.4 }, headers: json, jsonBody: { ok: true } } },
    ],
  },
  {
    id: 'fault',
    group: 'Resilience',
    label: 'Network fault',
    description: 'Connection reset by peer — test your client retry logic.',
    stubs: [{ name: 'Flaky connection', request: { method: 'GET', urlPath: '/api/flaky' }, response: { fault: 'CONNECTION_RESET_BY_PEER' } }],
  },
  {
    id: 'flaky',
    group: 'Resilience',
    label: 'Fail then succeed',
    description: 'Scenario returning 503 twice, then 200 — retry testing.',
    stubs: [
      { name: 'Retry #1 → 503', scenarioName: 'Retry', requiredScenarioState: 'Started', newScenarioState: 'failed-once', request: { method: 'GET', urlPath: '/api/retry' }, response: { status: 503 } },
      { name: 'Retry #2 → 503', scenarioName: 'Retry', requiredScenarioState: 'failed-once', newScenarioState: 'failed-twice', request: { method: 'GET', urlPath: '/api/retry' }, response: { status: 503 } },
      { name: 'Retry #3 → 200', scenarioName: 'Retry', requiredScenarioState: 'failed-twice', request: { method: 'GET', urlPath: '/api/retry' }, response: { status: 200, headers: json, jsonBody: { ok: true } } },
    ],
  },
  {
    id: 'dribble',
    group: 'Resilience',
    label: 'Chunked dribble',
    description: 'Send the body in 5 chunks over 2 seconds.',
    stubs: [{ name: 'Dribble', request: { method: 'GET', urlPath: '/api/stream' }, response: { status: 200, body: 'chunk chunk chunk chunk chunk', chunkedDribbleDelay: { numberOfChunks: 5, totalDuration: 2000 } } }],
  },
  {
    id: 'proxy-all',
    group: 'Proxy',
    label: 'Proxy everything else',
    description: 'Low-priority catch-all that forwards unmatched requests to a real backend.',
    stubs: [{ name: 'Proxy fallback', priority: 10, request: { method: 'ANY', urlPattern: '.*' }, response: { proxyBaseUrl: 'https://api.example.com' } }],
  },
  {
    id: 'proxy-path',
    group: 'Proxy',
    label: 'Proxy one path',
    description: 'Forward /payments/** to a sandbox, stripping the prefix and adding a header.',
    stubs: [
      {
        name: 'Proxy payments',
        request: { method: 'ANY', urlPathPattern: '/payments/.*' },
        response: { proxyBaseUrl: 'https://sandbox.payments.example.com', proxyUrlPrefixToRemove: '/payments', additionalProxyRequestHeaders: { 'X-Api-Key': 'sandbox-key' } },
      },
    ],
  },
  {
    id: 'webhook',
    group: 'Integration',
    label: 'Webhook callback',
    description: 'Respond 202 and call back another service 1 s later.',
    stubs: [
      {
        name: 'Payment with callback',
        request: { method: 'POST', urlPath: '/api/payments' },
        response: { status: 202, headers: json, jsonBody: { status: 'PENDING' } },
        serveEventListeners: [
          {
            name: 'webhook',
            parameters: {
              method: 'POST',
              url: 'http://localhost:9000/callbacks/payment',
              headers: { 'Content-Type': 'application/json' },
              body: '{ "status": "COMPLETED", "requestId": "{{originalRequest.headers.X-Request-Id}}" }',
              delay: { type: 'fixed', milliseconds: 1000 },
            },
          },
        ],
      },
    ],
  },
  {
    id: 'binary',
    group: 'Integration',
    label: 'Binary body',
    description: 'Returns a 1×1 PNG from base64.',
    stubs: [
      {
        name: 'Pixel',
        request: { method: 'GET', urlPath: '/pixel.png' },
        response: { status: 200, headers: { 'Content-Type': 'image/png' }, base64Body: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==' },
      },
    ],
  },
];

/** Handlebars helpers available in WireMock 3 response templates, for the insert menu. */
export const TEMPLATE_SNIPPETS: { group: string; label: string; snippet: string }[] = [
  { group: 'Request', label: 'Path segment [0]', snippet: '{{request.pathSegments.[0]}}' },
  { group: 'Request', label: 'Path param (template)', snippet: '{{request.path.id}}' },
  { group: 'Request', label: 'Query param', snippet: '{{request.query.name}}' },
  { group: 'Request', label: 'Header', snippet: '{{request.headers.X-Request-Id}}' },
  { group: 'Request', label: 'Cookie', snippet: '{{request.cookies.session}}' },
  { group: 'Request', label: 'Method', snippet: '{{request.method}}' },
  { group: 'Request', label: 'Full URL', snippet: '{{request.url}}' },
  { group: 'Request', label: 'Raw body (unescaped)', snippet: '{{{request.body}}}' },
  { group: 'Request', label: 'Form field', snippet: '{{formData request.body \'form\' urlDecode=true}}{{form.field}}' },
  { group: 'Body', label: 'JSONPath from body', snippet: "{{jsonPath request.body '$.id'}}" },
  { group: 'Body', label: 'XPath from body', snippet: "{{xPath request.body '/root/id/text()'}}" },
  { group: 'Body', label: 'Parse JSON', snippet: '{{#parseJson request.body \'data\'}}{{/parseJson}}{{data.name}}' },
  { group: 'Body', label: 'Loop JSON array', snippet: "{{#each (jsonPath request.body '$.items') as |item|}}{{item.id}}{{#unless @last}},{{/unless}}{{/each}}" },
  { group: 'Random', label: 'UUID', snippet: "{{randomValue type='UUID'}}" },
  { group: 'Random', label: 'Alphanumeric (10)', snippet: "{{randomValue length=10 type='ALPHANUMERIC'}}" },
  { group: 'Random', label: 'Integer 1–100', snippet: '{{randomInt lower=1 upper=100}}' },
  { group: 'Random', label: 'Decimal', snippet: '{{randomDecimal lower=0.5 upper=99.99}}' },
  { group: 'Random', label: 'Pick random', snippet: "{{pickRandom 'red' 'green' 'blue'}}" },
  { group: 'Date', label: 'Now (ISO)', snippet: '{{now}}' },
  { group: 'Date', label: 'Now +3 days, formatted', snippet: "{{now offset='3 days' format='yyyy-MM-dd'}}" },
  { group: 'Date', label: 'Epoch millis', snippet: "{{now format='epoch'}}" },
  { group: 'Date', label: 'Parse date', snippet: "{{date (parseDate request.headers.Date) offset='1 hours'}}" },
  { group: 'Logic', label: 'If / else', snippet: "{{#if (eq request.query.type 'premium')}}premium{{else}}standard{{/if}}" },
  { group: 'Logic', label: 'Math', snippet: "{{math 1 '+' 2}}" },
  { group: 'Logic', label: 'Size of array', snippet: "{{size (jsonPath request.body '$.items')}}" },
  { group: 'Logic', label: 'Regex extract', snippet: "{{regexExtract request.path '[A-Z]+' 'parts'}}{{parts.0}}" },
  { group: 'Strings', label: 'Upper case', snippet: '{{upper request.query.name}}' },
  { group: 'Strings', label: 'Base64 encode', snippet: '{{base64 request.body}}' },
  { group: 'Strings', label: 'URL encode', snippet: '{{urlEncode request.query.q}}' },
  { group: 'Strings', label: 'Trim', snippet: '{{trim request.body}}' },
  { group: 'Strings', label: 'Format JSON', snippet: '{{formatJson request.body}}' },
  { group: 'Data sources', label: 'Row by key (mocks/data)', snippet: "{{dataSource 'users' 'id' request.path.id}}" },
  { group: 'Data sources', label: 'Field of a row', snippet: "{{lookup (dataSource 'users' 'id' request.path.id) 'name'}}" },
  { group: 'Data sources', label: 'Matching rows (JSON array)', snippet: "{{dataRows 'users' 'role' request.query.role limit=20}}" },
  { group: 'Data sources', label: 'Loop rows', snippet: "{{#each (dataRows 'users')}}{{name}}{{#unless @last}}, {{/unless}}{{/each}}" },
  { group: 'Data sources', label: 'Count rows', snippet: "{{dataCount 'users'}}" },
  { group: 'Misc', label: 'System value (env)', snippet: "{{systemValue type='ENVIRONMENT' key='HOME'}}" },
  { group: 'Misc', label: 'Hostname', snippet: '{{hostname}}' },
  { group: 'Misc', label: 'JWT (extension)', snippet: '{{jwt maxAge=\'12 days\'}}' },
  { group: 'Misc', label: 'Faker (extension)', snippet: "{{random 'Name.firstName'}}" },
];

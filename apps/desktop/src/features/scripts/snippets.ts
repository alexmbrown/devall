import type { ScriptPhase } from '@/lib/api/scripts';

export interface Snippet {
  id: string;
  label: string;
  /** Shown under the label in the snippets menu. */
  hint: string;
  /** Phases the snippet makes sense in. */
  phases: ScriptPhase[];
  code: string;
}

const BOTH: ScriptPhase[] = ['preRequest', 'postResponse'];
const PRE: ScriptPhase[] = ['preRequest'];
const POST: ScriptPhase[] = ['postResponse'];

export const SNIPPETS: Snippet[] = [
  {
    id: 'set-variable',
    label: 'Set a variable',
    hint: 'Runtime only — usable as {{name}} by later requests',
    phases: BOTH,
    code: "pm.variables.set('name', 'value');",
  },
  {
    id: 'set-env',
    label: 'Set an environment variable',
    hint: 'A runtime override; the environment file is never written',
    phases: BOTH,
    code: "pm.environment.set('name', 'value');",
  },
  {
    id: 'get-variable',
    label: 'Read a variable',
    hint: 'Runtime → data row → environment → folders → project',
    phases: BOTH,
    code: "const value = pm.variables.get('name');",
  },
  {
    id: 'log',
    label: 'Log the request',
    hint: 'Shows in the response panel’s Console tab',
    phases: BOTH,
    code: "console.log(pm.request.method, pm.request.url.toString());",
  },
  {
    id: 'test-status',
    label: 'Test: status code is 200',
    hint: 'pm.response.to.have.status',
    phases: POST,
    code: "pm.test('Status code is 200', () => {\n  pm.response.to.have.status(200);\n});",
  },
  {
    id: 'test-json-field',
    label: 'Test: JSON field value',
    hint: 'pm.expect on pm.response.json()',
    phases: POST,
    code: "pm.test('Body has the expected id', () => {\n  const json = pm.response.json();\n  pm.expect(json).to.have.property('id');\n  pm.expect(json.id).to.eql(1);\n});",
  },
  {
    id: 'test-header',
    label: 'Test: Content-Type is JSON',
    hint: 'pm.response.to.have.header',
    phases: POST,
    code: "pm.test('Content-Type is JSON', () => {\n  pm.response.to.have.header('Content-Type');\n  pm.expect(pm.response.headers.get('Content-Type')).to.include('application/json');\n});",
  },
  {
    id: 'test-time',
    label: 'Test: response time below 500 ms',
    hint: 'pm.response.responseTime',
    phases: POST,
    code: "pm.test('Response time is below 500 ms', () => {\n  pm.expect(pm.response.responseTime).to.be.below(500);\n});",
  },
  {
    id: 'test-schema',
    label: 'Test: JSON schema',
    hint: 'Draft-07 subset (type, required, properties, items, enum, …)',
    phases: POST,
    code: "const schema = {\n  type: 'object',\n  required: ['id'],\n  properties: {\n    id: { type: 'integer' },\n    name: { type: 'string' },\n  },\n};\n\npm.test('Body matches the schema', () => {\n  pm.response.to.have.jsonSchema(schema);\n});",
  },
  {
    id: 'extract-token',
    label: 'Extract a token into a variable',
    hint: 'Later requests use {{token}}',
    phases: POST,
    code: "const json = pm.response.json();\nif (json.access_token) {\n  pm.environment.set('token', json.access_token);\n} else {\n  console.warn('No access_token in the response');\n}",
  },
  {
    id: 'hmac-signature',
    label: 'HMAC-SHA256 signature header',
    hint: 'Signs method, path, timestamp and body with a keychain secret',
    phases: PRE,
    code: "// Pre-request scripts see {{templates}} unresolved — resolve what you sign.\nconst secret = pm.secrets.get('API_SECRET'); // keychain secret (environment editor)\nconst timestamp = Math.floor(Date.now() / 1000).toString();\nconst path = pm.variables.replaceIn(pm.request.url.getPathWithQuery());\nconst body = pm.variables.replaceIn(pm.request.body.raw);\nconst payload = [pm.request.method, path, timestamp, body].join('\\n');\nconst signature = CryptoJS.HmacSHA256(payload, secret).toString(CryptoJS.enc.Hex);\npm.request.headers.upsert({ key: 'X-Timestamp', value: timestamp });\npm.request.headers.upsert({ key: 'X-Signature', value: signature });",
  },
  {
    id: 'timestamp-header',
    label: 'Timestamp header',
    hint: 'ISO-8601 time of sending',
    phases: PRE,
    code: "pm.request.headers.upsert({ key: 'X-Request-Time', value: new Date().toISOString() });",
  },
  {
    id: 'request-id',
    label: 'Request ID header (UUID)',
    hint: 'crypto.randomUUID()',
    phases: PRE,
    code: "pm.request.headers.upsert({ key: 'X-Request-Id', value: crypto.randomUUID() });",
  },
  {
    id: 'basic-auth',
    label: 'Basic auth header from variables',
    hint: 'btoa(user:password)',
    phases: PRE,
    code: "const user = pm.variables.get('username');\nconst password = pm.variables.get('password');\npm.request.headers.upsert({ key: 'Authorization', value: 'Basic ' + btoa(`${user}:${password}`) });",
  },
  {
    id: 'skip-request',
    label: 'Skip the request conditionally',
    hint: 'pm.execution.skipRequest()',
    phases: PRE,
    code: "if (!pm.variables.get('featureEnabled')) {\n  pm.execution.skipRequest();\n}",
  },
];

export function snippetsFor(phase: ScriptPhase): Snippet[] {
  return SNIPPETS.filter((s) => s.phases.includes(phase));
}

/** Appends `code` after the existing script, separated by a blank line. */
export function appendSnippet(script: string, code: string): string {
  const trimmed = script.replace(/\s+$/, '');
  return trimmed ? `${trimmed}\n\n${code}\n` : `${code}\n`;
}

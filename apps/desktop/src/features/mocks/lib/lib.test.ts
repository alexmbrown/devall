import { describe, expect, it } from 'vitest';
import { setDisabled, isDisabled, toggleProxy, withFolder, folderOf, matchesQuery, buildFolderTree, DISABLED_HEADER, cleanForSave } from './stub-utils';
import { exampleRequest, javaDsl, curlForStub, deRegex } from './codegen';
import { curlToStub, importText, openApiToStubs, buildZip, importZip } from './importers';
import { requestToStub } from './request-to-stub';
import { suggestFix } from './fix';
import { matchRequest } from './match';
import { harToStubs } from './har';
import type { StubMapping } from './types';

const stub = (o: Partial<StubMapping> = {}): StubMapping => ({ id: '11111111-1111-1111-1111-111111111111', request: { method: 'GET', urlPath: '/x' }, response: { status: 200 }, ...o });

describe('metadata helpers', () => {
  it('disables and re-enables without losing headers', () => {
    const s = stub({ request: { method: 'GET', urlPath: '/x', headers: { A: { equalTo: '1' } } } });
    const d = setDisabled(s, true);
    expect(isDisabled(d)).toBe(true);
    expect(d.request.headers?.[DISABLED_HEADER]).toEqual({ equalTo: 'true' });
    const e = setDisabled(d, false);
    expect(isDisabled(e)).toBe(false);
    expect(e.request.headers).toEqual({ A: { equalTo: '1' } });
    expect(e.metadata).toBeUndefined();
  });
  it('parks and restores proxy settings', () => {
    const s = stub({ response: { proxyBaseUrl: 'https://up', proxyUrlPrefixToRemove: '/p' } });
    const off = toggleProxy(s);
    expect(off.response.proxyBaseUrl).toBeUndefined();
    expect(off.response.status).toBe(200);
    const on = toggleProxy(off);
    expect(on.response.proxyBaseUrl).toBe('https://up');
    expect(on.response.proxyUrlPrefixToRemove).toBe('/p');
    expect(on.metadata).toBeUndefined();
  });
  it('stores folders holomekc-style and reads testsmith sections', () => {
    const s = withFolder(stub(), 'a/ b /c');
    expect(s.metadata).toEqual({ 'wiremock-gui': { folder: '/a/b/c' } });
    expect(folderOf(s)).toBe('a/b/c');
    expect(folderOf(stub({ metadata: { section: 'Todos' } }))).toBe('Todos');
    const tree = buildFolderTree([s, stub()]);
    expect(tree.count).toBe(2);
    expect(tree.children.get('a')?.children.get('b')?.children.get('c')?.stubs.length).toBe(1);
  });
  it('filters with field syntax', () => {
    const s = stub({ request: { method: 'POST', urlPath: '/orders' }, response: { proxyBaseUrl: 'https://x' } });
    expect(matchesQuery(s, 'method:post')).toBe(true);
    expect(matchesQuery(s, 'is:proxy orders')).toBe(true);
    expect(matchesQuery(s, 'method:get')).toBe(false);
  });
  it('cleanForSave strips read-only fields', () => {
    expect(cleanForSave({ ...stub(), insertionIndex: 3, response: { status: 200, fromConfiguredStub: true } })).not.toHaveProperty('insertionIndex');
  });
});

describe('codegen', () => {
  it('builds example requests for templates and regex', () => {
    expect(exampleRequest({ method: 'GET', urlPathTemplate: '/u/{userId}' }).path).toBe('/u/1');
    expect(exampleRequest({ urlPathPattern: '/api/[0-9]+' }).path).toBe('/api/1');
    expect(exampleRequest({ urlPath: '/s', queryParameters: { q: { equalTo: 'a b' } } }).path).toBe('/s?q=a%20b');
    expect(deRegex('^/v[0-9]+/.*$')).toBe('/v1/x');
  });
  it('emits cURL and Java DSL', () => {
    const s = stub({ request: { method: 'POST', urlPath: '/o', headers: { 'Content-Type': { equalTo: 'application/json' } }, bodyPatterns: [{ equalToJson: { a: 1 } }] }, response: { status: 201, jsonBody: { ok: true }, fixedDelayMilliseconds: 50 } });
    expect(curlForStub('http://h', s)).toContain("-X POST 'http://h/o'");
    const j = javaDsl(s);
    expect(j).toContain('post(urlPathEqualTo("/o"))');
    expect(j).toContain('.withHeader("Content-Type", equalTo("application/json"))');
    expect(j).toContain('.withStatus(201)');
    expect(j).toContain('.withFixedDelay(50)');
  });
});

describe('importers', () => {
  it('parses cURL commands', () => {
    const s = curlToStub(`curl -X POST 'https://api.example.com/orders?x=1' \\\n  -H 'Content-Type: application/json' \\\n  --data-raw '{"sku":"A1"}'`)!;
    expect(s.request.method).toBe('POST');
    expect(s.request.urlPath).toBe('/orders');
    expect(s.request.queryParameters).toEqual({ x: { equalTo: '1' } });
    expect(s.request.bodyPatterns?.[0].equalToJson).toEqual({ sku: 'A1' });
    expect(curlToStub('curl https://h/a')!.request.method).toBe('GET');
  });
  it('recognises stub JSON shapes', () => {
    expect(importText(JSON.stringify(stub())).stubs).toHaveLength(1);
    expect(importText(JSON.stringify({ mappings: [stub(), stub()] })).stubs).toHaveLength(2);
    expect(importText(JSON.stringify([stub()])).stubs).toHaveLength(1);
  });
  it('generates stubs from OpenAPI with examples and schemas', () => {
    const spec = {
      openapi: '3.0.0',
      info: { title: 'Pets' },
      servers: [{ url: 'https://api.x.com/v1' }],
      paths: {
        '/pets/{id}': { get: { tags: ['pets'], summary: 'Get pet', responses: { '200': { content: { 'application/json': { schema: { $ref: '#/components/schemas/Pet' } } } } } } },
        '/pets': { post: { responses: { '201': { content: { 'application/json': { example: { id: 7 } } } } } } },
      },
      components: { schemas: { Pet: { type: 'object', properties: { id: { type: 'integer' }, name: { type: 'string', example: 'Rex' } } } } },
    };
    const { stubs } = openApiToStubs(spec);
    expect(stubs).toHaveLength(2);
    expect(stubs[0].request).toMatchObject({ method: 'GET', urlPathTemplate: '/v1/pets/{id}' });
    expect(stubs[0].response.jsonBody).toEqual({ id: 1, name: 'Rex' });
    expect(stubs[1].response).toMatchObject({ status: 201, jsonBody: { id: 7 } });
    expect(importText('openapi: 3.0.0\ninfo: {title: Y}\npaths:\n  /a:\n    get:\n      responses:\n        "204": {description: ok}\n').stubs[0].response.status).toBe(204);
  });
  it('round-trips the root-dir ZIP', async () => {
    const z = buildZip([withFolder(stub({ name: 'One' }), 'f')], { 'a/b.json': new TextEncoder().encode('{}') });
    const r = await importZip(z, 'x.zip');
    expect(r.stubs).toHaveLength(1);
    expect(Object.keys(r.files)).toEqual(['a/b.json']);
  });
  it('turns HAR entries into stubs and skips assets', () => {
    const har = {
      log: {
        version: '1.2',
        creator: { name: 't', version: '1' },
        entries: [
          { startedDateTime: '', time: 1, request: { method: 'GET', url: 'https://x.com/api/a?b=1', headers: [] }, response: { status: 200, headers: [{ name: 'content-type', value: 'application/json' }], content: { mimeType: 'application/json', text: '{"ok":1}' } } },
          { startedDateTime: '', time: 1, request: { method: 'GET', url: 'https://x.com/app.js', headers: [] }, response: { status: 200, headers: [], content: { mimeType: 'application/javascript', text: '' } } },
        ],
      },
    };
    const s = harToStubs(har);
    expect(s).toHaveLength(1);
    expect(s[0].response.jsonBody).toEqual({ ok: 1 });
  });
});

describe('request → stub and fixes', () => {
  it('creates a stub that matches the request', () => {
    const r = { method: 'PUT', url: '/items/5?force=true', headers: { 'Content-Type': 'application/json' }, body: '{"a":1}' };
    const s = requestToStub(r, { status: 200, headers: { 'Content-Type': 'application/json' }, body: '{"ok":true}' });
    expect(matchRequest(s.request, r).verdict).toBe('match');
    expect(s.response.jsonBody).toEqual({ ok: true });
  });
  it('suggests a fix that makes a near miss match', () => {
    const s = stub({ request: { method: 'GET', urlPath: '/a', headers: { 'X-V': { equalTo: '1' } } } });
    const r = { method: 'GET', url: '/a', headers: { 'X-V': '2' } };
    const fix = suggestFix(s, r, matchRequest(s.request, r))!;
    expect(matchRequest(fix.stub.request, r).verdict).toBe('match');
  });
});

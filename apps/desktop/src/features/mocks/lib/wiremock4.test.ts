// WireMock 4 support (for remote 4.x servers): capabilities, identity, method sets, number matchers,
// Java codegen. Plus the admin client's transport behaviour. No server needed.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { capabilitiesFor, parseVersion } from './capabilities';
import { cleanForSave, displayName, matchesQuery, methodAccepts, methodLabel, normalizeIdentity } from './stub-utils';
import { matchRequest, matchValue, parseJavaDouble } from './match';
import { exampleRequest, javaDsl } from './codegen';
import { ApiError, WireMockClient, isDevallVersion } from './api';
import type { LoggedRequest, StubMapping } from './types';

const req = (o: Partial<LoggedRequest>): LoggedRequest => ({ method: 'GET', url: '/', headers: {}, ...o });
const stub = (o: Partial<StubMapping> = {}): StubMapping => ({ id: '11111111-1111-1111-1111-111111111111', request: { method: 'GET', urlPath: '/x' }, response: { status: 200 }, ...o });

describe('capabilities', () => {
  it('parses WireMock versions', () => {
    expect(parseVersion('3.13.2')).toMatchObject({ major: 3, minor: 13, patch: 2 });
    expect(parseVersion('4.0.0-beta.40')).toMatchObject({ major: 4, beta: 40, pre: 'beta.40' });
    expect(parseVersion('nonsense')).toBeUndefined();
  });
  it('turns on 4.x features by version', () => {
    const v3 = capabilitiesFor('3.13.2');
    expect(v3).toMatchObject({ v4: false, messages: false, numberMatchers: false, methodSets: false, statusMessage: true, uuidAlias: true, schema: 3 });
    const v4 = capabilitiesFor('4.0.0-beta.40');
    expect(v4).toMatchObject({ v4: true, messages: true, numberMatchers: true, methodSets: true, channelStubs: true, statusMessage: false, uuidAlias: false, schema: 4 });
    expect(capabilitiesFor('4.0.0-beta.38').channelStubs).toBe(false);
    expect(capabilitiesFor('4.0.0').channelStubs).toBe(true);
  });
  it('treats the built-in /dev/all engine as WireMock 3.x', () => {
    const caps = capabilitiesFor('3.13.0-devall');
    expect(caps).toMatchObject({ v4: false, schema: 3, numberMatchers: false, methodSets: false });
    expect(isDevallVersion('3.13.0-devall')).toBe(true);
    expect(isDevallVersion('3.13.2')).toBe(false);
  });
  it('falls back to the OpenAPI paths when the version is hidden', () => {
    expect(capabilitiesFor(undefined, ['/__admin/mappings', '/__admin/message-mappings']).v4).toBe(true);
    expect(capabilitiesFor(undefined, ['/__admin/mappings']).v4).toBe(false);
    expect(capabilitiesFor(undefined).schema).toBe(3);
  });
});

describe('identity (uuid is gone in WireMock 4)', () => {
  it('turns a lone uuid into id and drops the alias', () => {
    expect(normalizeIdentity({ uuid: 'u1', name: 'x' })).toEqual({ id: 'u1', name: 'x' });
    expect(normalizeIdentity({ id: 'i1', uuid: 'u1' })).toEqual({ id: 'i1' });
    const s = { id: 'i1' };
    expect(normalizeIdentity(s)).toBe(s);
  });
  it('cleanForSave never sends uuid', () => {
    const out = cleanForSave({ uuid: '22222222-2222-2222-2222-222222222222', request: { urlPath: '/a' }, response: {} });
    expect(out.id).toBe('22222222-2222-2222-2222-222222222222');
    expect(out).not.toHaveProperty('uuid');
  });
});

describe('WireMockClient', () => {
  afterEach(() => vi.unstubAllGlobals());
  const capture = () => {
    const calls: { url: string; init: RequestInit }[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string, init: RequestInit) => {
        calls.push({ url, init });
        return new Response('{}', { status: 200 });
      }),
    );
    return calls;
  };
  const bodyOf = async (init: RequestInit) => JSON.parse(await new Response(init.body as BodyInit).text());

  it('sends id, not uuid, when creating, updating and importing', async () => {
    const calls = capture();
    const c = new WireMockClient({ url: 'http://localhost:9' });
    const s = { uuid: 'u1', request: { urlPath: '/a' }, response: {} };
    await c.createMapping(s);
    await c.updateMapping('u1', s);
    await c.importMappings([s]);
    expect(await bodyOf(calls[0].init)).toEqual({ id: 'u1', request: { urlPath: '/a' }, response: {} });
    expect(await bodyOf(calls[1].init)).toEqual({ id: 'u1', request: { urlPath: '/a' }, response: {} });
    expect((await bodyOf(calls[2].init)).mappings[0]).toEqual({ id: 'u1', request: { urlPath: '/a' }, response: {} });
  });
  it('puts the admin API under the context path', async () => {
    const calls = capture();
    await new WireMockClient({ url: 'https://h.example/', contextPath: 'mocks/' }).mappings();
    await new WireMockClient({ url: 'h.example:8080', contextPath: '/mocks/__admin' }).health();
    expect(calls.map((c) => c.url)).toEqual(['https://h.example/mocks/__admin/mappings', 'http://h.example:8080/mocks/__admin/health']);
  });
  it('adds session credentials and extra headers', async () => {
    const calls = capture();
    await new WireMockClient({ url: 'http://h', auth: { type: 'basic', username: 'a', value: 'b' } }).health();
    await new WireMockClient({ url: 'http://h', auth: { type: 'bearer', value: 't' }, headers: [{ key: 'X-Team', value: 'pay' }] }).health();
    await new WireMockClient({ url: 'http://h', auth: { type: 'header', headerName: 'X-Api-Key', value: 'k' } }).health();
    expect((calls[0].init.headers as Record<string, string>).Authorization).toBe(`Basic ${btoa('a:b')}`);
    expect(calls[1].init.headers).toMatchObject({ Authorization: 'Bearer t', 'X-Team': 'pay' });
    expect(calls[2].init.headers).toMatchObject({ 'X-Api-Key': 'k' });
  });
  it('never sends keychain credentials from the webview', async () => {
    const calls = capture();
    const err = await new WireMockClient({ url: 'http://h', auth: { type: 'bearer', secret: 'MOCK_SERVER_TEAM_TOKEN' } })
      .health()
      .catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.kind).toBe('credentials');
    expect(calls).toHaveLength(0);
  });
  it('turns WireMock error bodies into readable messages', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ errors: [{ title: 'Invalid stub mapping', detail: 'bad matcher' }] }), { status: 422 })));
    const err = await new WireMockClient({ url: 'http://h' })
      .createMapping(stub())
      .catch((e) => e);
    expect(err).toMatchObject({ kind: 'http', status: 422, message: 'Invalid stub mapping: bad matcher' });
  });
  it('reports an unreachable server as a blocked / network failure', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new TypeError('Failed to fetch');
      }),
    );
    expect((await new WireMockClient({ url: 'http://localhost:9' }).health().catch((e) => e)).kind).toBe('blocked');
  });
});

describe('method sets ({oneOf} / {noneOf})', () => {
  it('labels and evaluates', () => {
    expect(methodLabel({ oneOf: ['PUT', 'PATCH'] })).toBe('PUT|PATCH');
    expect(methodLabel({ noneOf: ['GET'] })).toBe('!GET');
    expect(methodLabel(undefined)).toBe('ANY');
    expect(methodAccepts({ oneOf: ['PUT', 'PATCH'] }, 'patch')).toBe(true);
    expect(methodAccepts({ oneOf: ['PUT', 'PATCH'] }, 'GET')).toBe(false);
    expect(methodAccepts({ noneOf: ['GET', 'DELETE'] }, 'POST')).toBe(true);
    expect(methodAccepts({ noneOf: ['GET', 'DELETE'] }, 'DELETE')).toBe(false);
    expect(methodAccepts('ANY', 'QUERY')).toBe(true);
  });
  it('explains method mismatches', () => {
    const p = { method: { oneOf: ['PUT', 'PATCH'] }, urlPath: '/m' };
    expect(matchRequest(p, req({ method: 'PUT', url: '/m' })).verdict).toBe('match');
    const r = matchRequest(p, req({ method: 'GET', url: '/m' }));
    expect(r.verdict).toBe('nomatch');
    expect(r.fields[0]).toMatchObject({ field: 'Method', expected: 'one of PUT, PATCH', verdict: 'nomatch' });
    expect(matchRequest({ method: { noneOf: ['GET'] }, urlPath: '/m' }, req({ method: 'POST', url: '/m' })).verdict).toBe('match');
  });
  it('works in search, names and examples', () => {
    const s = stub({ name: undefined, request: { method: { oneOf: ['PUT', 'PATCH'] }, urlPath: '/m' } });
    expect(displayName(s)).toBe('PUT|PATCH /m');
    expect(matchesQuery(s, 'method:patch')).toBe(true);
    expect(matchesQuery(s, 'method:get')).toBe(false);
    expect(matchesQuery(stub({ request: { method: { noneOf: ['GET'] }, urlPath: '/n' } }), 'method:post')).toBe(true);
    expect(exampleRequest({ method: { oneOf: ['PATCH'] }, urlPath: '/m' }).method).toBe('PATCH');
    expect(exampleRequest({ method: { noneOf: ['GET', 'POST'] }, urlPath: '/m' }).method).toBe('PUT');
  });
});

describe('number matchers', () => {
  // Expected results come from WireMock 4.0.0-beta.40 (query param v, greaterThanNumber 5).
  it.each([
    ['6', 'match'],
    ['5', 'nomatch'],
    ['1e3', 'match'],
    ['+6', 'match'],
    [' 6', 'match'],
    ['6 ', 'match'],
    ['0x10', 'nomatch'],
    ['6abc', 'nomatch'],
    ['', 'nomatch'],
    ['Infinity', 'match'],
    ['NaN', 'nomatch'],
    ['6.0000000001', 'match'],
    ['-1', 'nomatch'],
    ['6,', 'nomatch'],
    ['1_000', 'nomatch'],
    ['6d', 'match'],
    ['6f', 'match'],
    ['1,000', 'nomatch'],
  ])('greaterThanNumber 5 vs %j → %s (as on the server)', (value, verdict) => {
    expect(matchValue({ greaterThanNumber: 5 }, value).verdict).toBe(verdict);
  });
  it('parses like Double.parseDouble', () => {
    expect(parseJavaDouble('18.0')).toBe(18);
    expect(parseJavaDouble('.5')).toBe(0.5);
    expect(parseJavaDouble('5.')).toBe(5);
    expect(parseJavaDouble('-Infinity')).toBe(-Infinity);
    expect(parseJavaDouble('abc')).toBeUndefined();
    expect(parseJavaDouble('0x1p3')).toBeNull();
  });
  it('implements each comparison', () => {
    expect(matchValue({ equalToNumber: 3 }, '3.0').verdict).toBe('match');
    expect(matchValue({ equalToNumber: 3 }, '3.1').verdict).toBe('nomatch');
    expect(matchValue({ greaterThanEqualNumber: 18 }, '18').verdict).toBe('match');
    expect(matchValue({ greaterThanEqualNumber: 18 }, '17').verdict).toBe('nomatch');
    expect(matchValue({ lessThanNumber: 10.5 }, '10.5').verdict).toBe('nomatch');
    expect(matchValue({ lessThanNumber: 10.5 }, '10').verdict).toBe('match');
    expect(matchValue({ lessThanEqualNumber: 3 }, '2.5').verdict).toBe('match');
    // The operand may be a numeric string; WireMock converts it.
    expect(matchValue({ greaterThanNumber: '5' as unknown as number }, '6').verdict).toBe('match');
    expect(matchValue({ lessThanNumber: 1 }, undefined).verdict).toBe('nomatch');
  });
  it('works in request matching and JSONPath sub-matchers', () => {
    const p = { urlPath: '/num', queryParameters: { age: { greaterThanEqualNumber: 18 } }, headers: { 'X-N': { lessThanEqualNumber: 3 } } };
    expect(matchRequest(p, req({ url: '/num?age=18', headers: { 'X-N': '2.5' } })).verdict).toBe('match');
    const miss = matchRequest(p, req({ url: '/num?age=17', headers: { 'X-N': '3' } }));
    expect(miss.fields.find((f) => f.field === 'Query age')).toMatchObject({ verdict: 'nomatch', expected: 'number ≥ 18' });
    const jp = { matchesJsonPath: { expression: '$.age', greaterThanEqualNumber: 18 } };
    expect(matchValue(jp, '{"age":18}').verdict).toBe('match');
    expect(matchValue(jp, '{"age":"18"}').verdict).toBe('match');
    expect(matchValue(jp, '{"age":17}').verdict).toBe('nomatch');
    expect(matchValue(jp, '{}').verdict).toBe('nomatch');
  });
  it('treats JSONPath array results like WireMock: any element may match', () => {
    // Verified against WireMock 3.13.2 and 4.0.0-beta.40.
    expect(matchValue({ matchesJsonPath: { expression: '$.age', greaterThanEqualNumber: 18 } }, '{"age":[20]}').verdict).toBe('match');
    const eq = { matchesJsonPath: { expression: '$.age', equalTo: '20' } };
    expect(matchValue(eq, '{"age":["20"]}').verdict).toBe('match');
    expect(matchValue(eq, '{"age":[1,20]}').verdict).toBe('match');
    expect(matchValue(eq, '{"age":{"a":"20"}}').verdict).toBe('nomatch');
  });
  it('generates example values that satisfy the matcher', () => {
    const ex = exampleRequest({ urlPath: '/n', queryParameters: { a: { greaterThanNumber: 5 }, b: { lessThanNumber: 1 }, c: { equalToNumber: 2.5 } } });
    expect(ex.path).toBe('/n?a=6&b=0&c=2.5');
  });
});

describe('Java DSL for WireMock 4', () => {
  it('emits number matchers and method sets', () => {
    const code = javaDsl(
      stub({
        request: {
          method: { oneOf: ['PUT', 'PATCH'] },
          urlPath: '/m',
          queryParameters: { age: { greaterThanEqualNumber: 18 }, score: { lessThanNumber: 10.5 } },
          bodyPatterns: [{ matchesJsonPath: { expression: '$.n', equalToNumber: 3 } }],
        },
      }),
    );
    expect(code).toContain('request(RequestMethod.isOneOf(RequestMethod.PUT, RequestMethod.PATCH), urlPathEqualTo("/m"))');
    expect(code).toContain('// import com.github.tomakehurst.wiremock.http.RequestMethod;');
    expect(code).toContain('.withQueryParam("age", greaterThanEqualNumber(18))');
    expect(code).toContain('.withQueryParam("score", lessThanNumber(10.5))');
    expect(code).toContain('matchingJsonPath("$.n", equalToNumber(3))');
    expect(javaDsl(stub({ request: { method: { noneOf: ['GET', 'X-CUSTOM'] }, urlPath: '/n' } }))).toContain('RequestMethod.isNoneOf(RequestMethod.GET, RequestMethod.fromString("X-CUSTOM"))');
    expect(javaDsl(stub({ request: { method: 'QUERY', urlPath: '/q' } }))).toContain('query(urlPathEqualTo("/q"))');
  });
  it('emits channel-opening responses', () => {
    expect(javaDsl(stub({ response: { openWebsocketChannel: true } }))).toContain('.willReturn(aResponse().openWebsocketChannel())');
    expect(javaDsl(stub({ response: { openSseChannel: true } }))).toContain('.willReturn(aResponse().openSseChannel())');
  });
});

describe('fix stub to match (WireMock 4 matchers)', () => {
  it('widens method sets and relaxes number bounds instead of replacing them', async () => {
    const { suggestFix } = await import('./fix');
    const s = stub({ request: { method: { oneOf: ['PUT', 'PATCH'] }, urlPath: '/i', queryParameters: { age: { greaterThanEqualNumber: 18 }, n: { lessThanNumber: 3 } } } });
    const r = req({ method: 'GET', url: '/i?age=17&n=3' });
    const fix = suggestFix(s, r, matchRequest(s.request, r))!;
    expect(fix.stub.request.method).toEqual({ oneOf: ['PUT', 'PATCH', 'GET'] });
    expect(fix.stub.request.queryParameters).toEqual({ age: { greaterThanEqualNumber: 17 }, n: { lessThanEqualNumber: 3 } });
    expect(matchRequest(fix.stub.request, r).verdict).toBe('match');
    const none = stub({ request: { method: { noneOf: ['GET'] }, urlPath: '/i' } });
    expect(suggestFix(none, r, matchRequest(none.request, r))!.stub.request.method).toBe('ANY');
  });
});

describe('importing message stubs', () => {
  it('reads Messages-page exports and snapshots', async () => {
    const { importText } = await import('./importers');
    const m = { id: 'm1', name: 'Echo', trigger: { type: 'message' }, actions: [] };
    const fromExport = importText(JSON.stringify({ messageMappings: [m] }));
    expect(fromExport.stubs).toEqual([]);
    expect(fromExport.messageStubs).toEqual([m]);
    const snap = importText(JSON.stringify({ kind: 'wiremock-ui/server-snapshot', mappings: [stub()], messageMappings: [m], files: {} }));
    expect(snap.kind).toBe('snapshot');
    expect(snap.stubs).toHaveLength(1);
    expect(snap.messageStubs).toEqual([m]);
  });
});

import { describe, expect, it } from 'vitest';
import { matchRequest, matchValue, javaRegex } from './match';
import { evaluate } from './jsonpath';
import type { LoggedRequest } from './types';

const req = (o: Partial<LoggedRequest>): LoggedRequest => ({ method: 'GET', url: '/', headers: {}, ...o });

describe('javaRegex', () => {
  it('anchors like Java matches()', () => {
    expect(javaRegex('/api/.*').test('/api/x')).toBe(true);
    expect(javaRegex('/api').test('/api/x')).toBe(false);
  });
  it('supports inline (?i)', () => {
    expect(javaRegex('(?i)abc').test('ABC')).toBe(true);
  });
});

describe('matchValue', () => {
  it('handles simple operators', () => {
    expect(matchValue({ equalTo: 'a' }, 'a').verdict).toBe('match');
    expect(matchValue({ equalTo: 'A', caseInsensitive: true }, 'a').verdict).toBe('match');
    expect(matchValue({ contains: 'ell' }, 'hello').verdict).toBe('match');
    expect(matchValue({ doesNotContain: 'x' }, 'hello').verdict).toBe('match');
    expect(matchValue({ matches: '[0-9]+' }, '123').verdict).toBe('match');
    expect(matchValue({ matches: '[0-9]+' }, '12a').verdict).toBe('nomatch');
    expect(matchValue({ absent: true }, undefined).verdict).toBe('match');
    expect(matchValue({ absent: true }, 'x').verdict).toBe('nomatch');
  });
  it('handles and / or / not', () => {
    expect(matchValue({ and: [{ contains: 'a' }, { contains: 'b' }] }, 'ab').verdict).toBe('match');
    expect(matchValue({ or: [{ equalTo: 'x' }, { equalTo: 'y' }] }, 'y').verdict).toBe('match');
    expect(matchValue({ not: { equalTo: 'x' } }, 'y').verdict).toBe('match');
  });
  it('equalToJson with options and placeholders', () => {
    expect(matchValue({ equalToJson: { a: 1, b: [1, 2] } }, '{"b":[1,2],"a":1}').verdict).toBe('match');
    expect(matchValue({ equalToJson: { a: 1 } }, '{"a":1,"b":2}').verdict).toBe('nomatch');
    expect(matchValue({ equalToJson: { a: 1 }, ignoreExtraElements: true }, '{"a":1,"b":2}').verdict).toBe('match');
    expect(matchValue({ equalToJson: [1, 2], ignoreArrayOrder: true }, '[2,1]').verdict).toBe('match');
    expect(matchValue({ equalToJson: { id: '${json-unit.any-string}' } }, '{"id":"x"}').verdict).toBe('match');
    expect(matchValue({ equalToJson: { id: '${json-unit.any-number}' } }, '{"id":"x"}').verdict).toBe('nomatch');
    expect(matchValue({ equalToJson: '{"a": 1}' }, '{"a":1}').verdict).toBe('match');
  });
  it('matchesJsonPath with and without sub-matcher', () => {
    const body = JSON.stringify({ tier: 'premium', items: [{ qty: 1 }, { qty: 3 }] });
    expect(matchValue({ matchesJsonPath: '$.tier' }, body).verdict).toBe('match');
    expect(matchValue({ matchesJsonPath: '$.missing' }, body).verdict).toBe('nomatch');
    expect(matchValue({ matchesJsonPath: { expression: '$.tier', equalTo: 'premium' } }, body).verdict).toBe('match');
    expect(matchValue({ matchesJsonPath: { expression: '$.tier', equalTo: 'basic' } }, body).verdict).toBe('nomatch');
    expect(matchValue({ matchesJsonPath: '$.items[?(@.qty > 2)]' }, body).verdict).toBe('match');
    expect(matchValue({ matchesJsonPath: '$.items[?(@.qty > 5)]' }, body).verdict).toBe('nomatch');
  });
  it('reports unknown for server-only matchers', () => {
    expect(matchValue({ matchesJsonSchema: {} }, '{}').verdict).toBe('unknown');
  });
  it('dates', () => {
    expect(matchValue({ before: 'now +1 days' }, new Date().toISOString()).verdict).toBe('match');
    expect(matchValue({ after: 'now +1 days' }, new Date().toISOString()).verdict).toBe('nomatch');
  });
});

describe('jsonpath', () => {
  const doc = { a: { b: [10, 20, 30] }, list: [{ n: 'x', v: 1 }, { n: 'y', v: 2 }] };
  it('evaluates common forms', () => {
    expect(evaluate('$.a.b[1]', doc)).toEqual([20]);
    expect(evaluate('$.a.b[-1]', doc)).toEqual([30]);
    expect(evaluate("$['a']['b'][0]", doc)).toEqual([10]);
    expect(evaluate('$.list[*].n', doc)).toEqual(['x', 'y']);
    expect(evaluate('$..n', doc)).toEqual(['x', 'y']);
    expect(evaluate("$.list[?(@.n == 'y')].v", doc)).toEqual([2]);
    expect(evaluate('$.a.b.length()', doc)).toEqual([3]);
    expect(evaluate('$.a.b[0:2]', doc)).toEqual([10, 20]);
  });
});

describe('matchRequest', () => {
  it('explains each field', () => {
    const r = matchRequest(
      { method: 'POST', urlPath: '/orders', queryParameters: { v: { equalTo: '2' } }, headers: { 'Content-Type': { contains: 'json' } }, bodyPatterns: [{ matchesJsonPath: '$.sku' }] },
      req({ method: 'POST', url: '/orders?v=1', headers: { 'content-type': 'application/json' }, body: '{"sku":"A"}' }),
    );
    expect(r.verdict).toBe('nomatch');
    const byField = Object.fromEntries(r.fields.map((f) => [f.field, f.verdict]));
    expect(byField).toMatchObject({ Method: 'match', 'Path equals': 'match', 'Query v': 'nomatch', 'Header Content-Type': 'match', Body: 'match' });
  });
  it('matches path templates and path params', () => {
    const p = { method: 'GET', urlPathTemplate: '/users/{id}', pathParameters: { id: { matches: '[0-9]+' } } };
    expect(matchRequest(p, req({ url: '/users/12' })).verdict).toBe('match');
    expect(matchRequest(p, req({ url: '/users/abc' })).verdict).toBe('nomatch');
    expect(matchRequest(p, req({ url: '/users/1/x' })).verdict).toBe('nomatch');
  });
  it('treats ANY as any method and url regex on full url', () => {
    expect(matchRequest({ method: 'ANY', urlPattern: '/a\\?x=.*' }, req({ method: 'DELETE', url: '/a?x=1' })).verdict).toBe('match');
  });
  it('multi-value query semantics', () => {
    const p = { urlPath: '/s', queryParameters: { t: { hasExactly: [{ equalTo: 'a' }, { equalTo: 'b' }] } } };
    expect(matchRequest(p, req({ url: '/s?t=a&t=b' })).verdict).toBe('match');
    expect(matchRequest(p, req({ url: '/s?t=a' })).verdict).toBe('nomatch');
  });
  it('cookies and basic auth', () => {
    const p = { urlPath: '/', cookies: { session: { equalTo: 'abc' } }, basicAuthCredentials: { username: 'u', password: 'p' } };
    expect(matchRequest(p, req({ headers: { Cookie: 'a=1; session=abc', Authorization: `Basic ${btoa('u:p')}` } })).verdict).toBe('match');
  });
});

describe('withScenario', () => {
  it('blames the scenario when every request field matched', async () => {
    const { withScenario } = await import('./match');
    const stub = { scenarioName: 'Order', requiredScenarioState: 'Placed', request: { method: 'GET', urlPath: '/o' } };
    const r = withScenario(matchRequest(stub.request, req({ url: '/o' })), stub, 'Started');
    expect(r.verdict).toBe('nomatch');
    expect(r.fields.at(-1)).toMatchObject({ field: 'Scenario state', verdict: 'nomatch' });
    expect(withScenario(matchRequest(stub.request, req({ url: '/x' })), stub, 'Started').fields.at(-1)?.verdict).toBe('unknown');
  });
  it('JSONPath with an equalToJson sub-matcher', () => {
    expect(matchValue({ matchesJsonPath: { expression: '$.items', equalToJson: '[]' } }, '{"items":[]}').verdict).toBe('match');
    expect(matchValue({ matchesJsonPath: { expression: '$.items', equalToJson: '[]' } }, '{"items":[{"id":1}]}').verdict).toBe('nomatch');
  });
});

// The in-house replacements for the original UI's dependencies (YAML, ZIP, query cache,
// virtual list maths) and the servers.yaml model.
import { describe, expect, it } from 'vitest';
import { parseYaml, stringifyYaml } from './yaml';
import { crc32, unzip, zipSync, strToU8, strFromU8 } from './zip';
import { QueryClient } from './query';
import { computeOffsets, visibleRange } from './virtual';
import { readServers, secretNameFor, secretProblems, serializeServers, uniqueId, type RemoteServer } from './servers';
import { reportFromServer, explainNearMiss } from './match';
import { suggestFix } from './fix';
import { FAKER_KEYS, fakerGroups, fakerHelper } from './faker';

describe('yaml', () => {
  it('reads block and flow collections, scalars and comments', () => {
    const doc = parseYaml(`# servers
version: 1
servers:
  - id: team
    name: "Team: shared"   # quoted because of the colon
    url: https://wm.example.com
    readOnly: true
    port: 8080
    ratio: 0.5
    nothing: ~
    headers:
      - { key: X-Team, value: payments }
    tags: [a, 'b c', 3]
  - id: second
    name: It's fine
`);
    expect(doc).toEqual({
      version: 1,
      servers: [
        { id: 'team', name: 'Team: shared', url: 'https://wm.example.com', readOnly: true, port: 8080, ratio: 0.5, nothing: null, headers: [{ key: 'X-Team', value: 'payments' }], tags: ['a', 'b c', 3] },
        { id: 'second', name: "It's fine" },
      ],
    });
  });
  it('reads block scalars and sequences at the key indentation', () => {
    expect(parseYaml('a: |\n  line 1\n  line 2\nb: >-\n  folded\n  text\nc:\n- x\n- y\n')).toEqual({ a: 'line 1\nline 2\n', b: 'folded text', c: ['x', 'y'] });
  });
  it('reads an OpenAPI-like document', () => {
    const doc = parseYaml('openapi: 3.0.0\ninfo: {title: Y}\npaths:\n  /a:\n    get:\n      responses:\n        "204": {description: ok}\n') as Record<string, unknown>;
    expect(doc).toEqual({ openapi: '3.0.0', info: { title: 'Y' }, paths: { '/a': { get: { responses: { '204': { description: 'ok' } } } } } });
  });
  it('rejects what it does not support', () => {
    expect(() => parseYaml('a: &x 1\nb: *x\n')).toThrow(/anchors/);
  });
  it('round-trips through the writer', () => {
    const v = { version: 1, list: [{ a: 'x: y', b: '', c: 'true', d: true, e: null, f: [1, 2], g: { h: 'multi\nline' } }, 'plain', []], empty: {} };
    const text = stringifyYaml(v);
    expect(parseYaml(text)).toEqual(v);
    expect(text).toContain('a: "x: y"');
    expect(text).toContain('c: "true"');
  });
});

describe('zip', () => {
  it('computes CRC-32', () => {
    expect(crc32(strToU8('123456789'))).toBe(0xcbf43926);
  });
  it('round-trips stored entries with UTF-8 names', async () => {
    const z = zipSync({ 'mappings/ä/one.json': strToU8('{"a":1}'), '__files/b.bin': new Uint8Array([0, 1, 2, 255]) });
    const out = await unzip(z);
    expect(Object.keys(out)).toEqual(['mappings/ä/one.json', '__files/b.bin']);
    expect(strFromU8(out['mappings/ä/one.json'])).toBe('{"a":1}');
    expect([...out['__files/b.bin']]).toEqual([0, 1, 2, 255]);
  });
  it('rejects non-zip data', async () => {
    await expect(unzip(strToU8('not a zip at all, definitely not'))).rejects.toThrow(/Not a ZIP/);
  });
});

describe('query cache', () => {
  it('dedupes fetches, caches data and invalidates by prefix', async () => {
    const qc = new QueryClient();
    let calls = 0;
    const fn = async () => ++calls;
    const [a, b] = await Promise.all([qc.fetchQuery({ queryKey: ['wm', 's', 'x'], queryFn: fn }), qc.fetchQuery({ queryKey: ['wm', 's', 'x'], queryFn: fn })]);
    expect([a, b, calls]).toEqual([1, 1, 1]);
    expect(await qc.fetchQuery({ queryKey: ['wm', 's', 'x'], queryFn: fn, staleTime: 60_000 })).toBe(1);
    qc.setQueryData<number>(['wm', 's', 'x'], (old) => (old ?? 0) + 10);
    expect(qc.getQueryData(['wm', 's', 'x'])).toBe(11);
    await qc.invalidateQueries({ queryKey: ['wm', 's'] });
    // Not observed: invalidation marks stale, the next fetch goes to the server.
    expect(await qc.fetchQuery({ queryKey: ['wm', 's', 'x'], queryFn: fn, staleTime: 60_000 })).toBe(2);
  });
});

describe('virtual list maths', () => {
  it('finds the visible rows', () => {
    const offsets = computeOffsets(100, (i) => (i % 10 === 0 ? 28 : 44));
    expect(offsets[1]).toBe(28);
    expect(offsets[100]).toBe(10 * 28 + 90 * 44);
    const [from, to] = visibleRange(offsets, 1000, 300, 0);
    expect(offsets[from]).toBeLessThanOrEqual(1000);
    expect(offsets[from + 1]).toBeGreaterThan(1000);
    expect(offsets[to - 1]).toBeLessThan(1300);
    expect(visibleRange(computeOffsets(0, () => 10), 0, 100, 5)).toEqual([0, 0]);
  });
});

describe('servers.yaml', () => {
  const s: RemoteServer = {
    id: 'team-shared',
    name: 'Team shared',
    url: 'https://wm.example.com',
    env: 'dev',
    protected: true,
    auth: { type: 'basic', username: 'admin' },
    headers: [{ key: 'X-Team', value: 'payments' }],
  };
  it('serialises without secrets and reads back', () => {
    const text = serializeServers([s]);
    expect(text).not.toMatch(/password|token:/i);
    const back = readServers(parseYaml(text));
    expect(back.warnings).toEqual([]);
    expect(back.servers).toEqual([s]);
  });
  it('drops secrets found in the file and warns', () => {
    const r = readServers({ servers: [{ name: 'x', url: 'http://h', auth: { type: 'bearer', token: 'abc' } }, { name: 'no url' }] });
    expect(r.servers).toHaveLength(1);
    expect(r.servers[0].auth).toEqual({ type: 'bearer' });
    expect(r.warnings.join('\n')).toMatch(/auth.token is ignored/);
    expect(r.warnings.join('\n')).toMatch(/missing url/);
  });
  it('blocks credential-looking extra headers', () => {
    expect(secretProblems([{ ...s, headers: [{ key: 'Authorization', value: 'Bearer x' }] }])).toHaveLength(1);
    expect(secretProblems([{ ...s, headers: [{ key: 'X-Api-Key', value: '' }] }])).toHaveLength(0);
  });
  it('names keychain secrets and ids predictably', () => {
    expect(secretNameFor('team-shared')).toBe('MOCK_SERVER_TEAM_SHARED_TOKEN');
    expect(uniqueId('Team Shared!', ['team-shared'])).toBe('team-shared-2');
  });
});

describe('engine near-miss explanations', () => {
  const fields = [
    { field: 'Method', expected: 'GET', actual: 'GET', matched: true, distance: 0 },
    { field: 'URL path', expected: '/a', actual: '/b', matched: false, distance: 0.5 },
    { field: 'Header X-K', expected: 'equals 1', actual: null, matched: false, distance: 1 },
  ];
  it('maps engine field names to the client vocabulary', () => {
    const r = reportFromServer(fields);
    expect(r.verdict).toBe('nomatch');
    expect(r.fields.map((f) => f.field)).toEqual(['Method', 'Path equals', 'Header X-K']);
    expect(r.fields[2].actual).toBe('(absent)');
  });
  it('prefers the engine report and still lets "fix stub" work', () => {
    const stub = { id: 's', request: { method: 'GET', urlPath: '/a', headers: { 'X-K': { equalTo: '1' } } }, response: { status: 200 } };
    const req = { method: 'GET', url: '/b', headers: {} };
    const report = explainNearMiss(stub, req, { distance: 0.4, fields });
    const fix = suggestFix(stub, req, report)!;
    expect(fix.stub.request.urlPath).toBe('/b');
    expect(fix.stub.request.headers).toBeUndefined();
    // Without engine fields the client-side matcher is used.
    expect(explainNearMiss(stub, req, { distance: 0.4 }).fields.find((f) => f.field === 'Path equals')?.verdict).toBe('nomatch');
  });
});

describe('faker keys', () => {
  it('match the engine list and group by category', () => {
    expect(FAKER_KEYS).toContain('Name.firstName');
    expect(FAKER_KEYS.length).toBeGreaterThan(80);
    expect(fakerGroups()[0]).toMatchObject({ group: 'Name' });
    expect(fakerHelper('Internet.emailAddress')).toBe("{{random 'Internet.emailAddress'}}");
  });
});

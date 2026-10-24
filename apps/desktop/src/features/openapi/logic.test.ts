import { describe, expect, it } from 'vitest';
import { filterFindings, kindLabel, specCandidates, toProjectPath } from './logic';

describe('openapi logic', () => {
  it('filters findings by severity and words', () => {
    const items = [
      { severity: 'error' as const, text: 'operationId list is duplicated' },
      { severity: 'warning' as const, text: 'GET /users has no summary' },
      { severity: 'info' as const, text: 'GET /users has no tags' },
    ];
    const t = (f: { text: string }) => f.text;
    expect(filterFindings(items, 'all', '', t)).toHaveLength(3);
    expect(filterFindings(items, 'warning', '', t)).toHaveLength(1);
    expect(filterFindings(items, 'all', 'users NO', t)).toHaveLength(2);
    expect(filterFindings(items, 'error', 'users', t)).toHaveLength(0);
  });

  it('picks spec-like changed paths', () => {
    expect(specCandidates(['openapi/api.yaml', 'http/a.req.yaml', 'mocks/mappings/x.json', 'docs/spec.json', 'notes/a.md', 'environments/dev.env.yaml', 'x.yml'])).toEqual([
      'openapi/api.yaml',
      'docs/spec.json',
      'x.yml',
    ]);
  });

  it('turns absolute paths inside the project into relative ones', () => {
    expect(toProjectPath('C:\\proj', 'C:\\proj\\specs\\a.yaml')).toBe('specs/a.yaml');
    expect(toProjectPath('c:/Proj/', 'C:/proj/specs/a.yaml')).toBe('specs/a.yaml');
    expect(toProjectPath('/home/me/p', '/home/me/p/openapi.yaml')).toBe('openapi.yaml');
    expect(toProjectPath('/home/me/p', '/home/me/other/openapi.yaml')).toBe('/home/me/other/openapi.yaml');
    expect(toProjectPath('/home/me/p', '/home/me/p2/a.yaml')).toBe('/home/me/p2/a.yaml');
  });

  it('labels kinds', () => {
    expect(kindLabel('invalidResponseBody')).toBe('Response body');
  });
});

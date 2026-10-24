import { describe, expect, it } from 'vitest';
import type { EnvironmentEntry } from '@/lib/types';
import { buildMatrix, filterRows, referenceNames, usageTarget } from './matrix';

const envs: EnvironmentEntry[] = [
  { id: 'dev', file: { name: 'Dev', variables: { baseUrl: 'http://localhost:3000', region: 'eu', debug: '' }, secrets: ['API_TOKEN'] } },
  { id: 'prod', file: { name: 'Prod', variables: { baseUrl: 'https://api.example.com', region: 'eu' }, secrets: ['API_TOKEN', 'DB_PASSWORD'], secretSources: { DB_PASSWORD: 'op://Eng/db/password' } } },
];

describe('buildMatrix', () => {
  const rows = buildMatrix(envs, { timeout: '30', debug: 'false' }, { dev: [{ name: 'API_TOKEN', source: 'keychain' }], prod: [{ name: 'API_TOKEN', source: null }, { name: 'DB_PASSWORD', source: '1Password' }] });
  const row = (k: string) => rows.find((r) => r.key === k)!;

  it('marks differing, equal, empty and inherited variables', () => {
    expect(row('baseUrl').differs).toBe(true);
    expect(row('region')).toMatchObject({ differs: false, missing: 0, empty: 0 });
    // dev sets it empty; prod inherits devall.yaml's value.
    expect(row('debug').cells.map((c) => c.state)).toEqual(['empty', 'inherited']);
    expect(row('debug').cells[1].value).toBe('false');
    expect(row('debug').empty).toBe(1);
  });

  it('reports secrets by status only', () => {
    expect(row('secret.API_TOKEN').cells.map((c) => c.state)).toEqual(['set', 'unset']);
    expect(row('secret.DB_PASSWORD').cells.map((c) => c.state)).toEqual(['absent', 'set']);
    expect(row('secret.DB_PASSWORD').cells[1]).toMatchObject({ source: '1Password', linked: 'op://Eng/db/password' });
    expect(rows.filter((r) => r.secret).every((r) => r.cells.every((c) => c.value === undefined))).toBe(true);
  });

  it('filters to rows needing attention', () => {
    expect(filterRows(rows, '', true).map((r) => r.key)).toEqual(['baseUrl', 'debug', 'secret.API_TOKEN', 'secret.DB_PASSWORD']);
    expect(filterRows(rows, 'example.com', false).map((r) => r.key)).toEqual(['baseUrl']);
  });
});

describe('referenceNames', () => {
  it('lists variables, project variables and secret references', () => {
    expect(referenceNames(envs, { timeout: '30' })).toEqual(['baseUrl', 'debug', 'region', 'secret.API_TOKEN', 'secret.DB_PASSWORD', 'timeout']);
  });
});

describe('usageTarget', () => {
  it('maps files to the view that edits them', () => {
    expect(usageTarget({ path: 'http/users/get.req.yaml', kind: 'request' })).toEqual({ section: 'http', param: 'users/get.req.yaml' });
    expect(usageTarget({ path: 'http/users/_folder.yaml', kind: 'folder' })).toEqual({ section: 'http', param: 'users/_folder.yaml' });
    expect(usageTarget({ path: 'mocks/mappings/a.json', kind: 'mock' })).toEqual({ section: 'mocks', param: 'stubs/file:mappings/a.json' });
    expect(usageTarget({ path: 'notes/setup.md', kind: 'note' })).toEqual({ section: 'notes', param: 'setup.md' });
    expect(usageTarget({ path: 'environments/prod.env.yaml', kind: 'environment' })).toEqual({ section: 'environments', param: 'prod' });
    expect(usageTarget({ path: 'devall.yaml', kind: 'project' })).toEqual({ section: 'environments', param: '__project' });
    expect(usageTarget({ path: 'README.md', kind: 'other' })).toBeNull();
  });
});

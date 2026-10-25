import { describe, expect, it } from 'vitest';
import type { Repo, RepoBranches } from '@/lib/api/git';
import { branchUnion, bulkSummary, gitTabLabel, groupRepos, isMultiRepo, suggestedGroup, switchPlan, upstreamLabel } from './model';

const repo = (name: string, over: Partial<Repo> = {}): Repo => ({
  name,
  path: `/src/${name}`,
  origin: 'manifest',
  state: 'found',
  relPath: name,
  remote: null,
  group: null,
  overridden: false,
  projectPrefix: null,
  ...over,
});

describe('git model', () => {
  it('tells single-repo projects from multi-repo ones', () => {
    expect(isMultiRepo([])).toBe(false);
    expect(isMultiRepo([repo('shop', { origin: 'containing' })])).toBe(false);
    expect(isMultiRepo([repo('api')])).toBe(true);
    expect(isMultiRepo([repo('shop', { origin: 'containing' }), repo('vendored', { origin: 'discovered' })])).toBe(true);
  });

  it('groups repositories, ungrouped first', () => {
    const g = groupRepos([repo('api', { group: 'Backend' }), repo('web', { group: 'Frontend' }), repo('tools'), repo('orders', { group: 'backend' })]);
    expect(g.map((x) => [x.group, x.repos.map((r) => r.name)])).toEqual([
      [null, ['tools']],
      ['Backend', ['api', 'orders']],
      ['Frontend', ['web']],
    ]);
  });

  it('labels the tab', () => {
    expect(gitTabLabel(false, 'shop', 'main')).toBe('main');
    expect(gitTabLabel(false, null, null)).toBeNull();
    expect(gitTabLabel(true, 'web', 'feature/x')).toBe('web (feature/x)');
    expect(gitTabLabel(true, null, null)).toBe('All repositories');
  });

  it('describes upstream state', () => {
    expect(upstreamLabel({ upstream: null, ahead: 0, behind: 0 })).toBe('no upstream');
    expect(upstreamLabel({ upstream: 'origin/main', ahead: 0, behind: 0 })).toBe('in sync');
    expect(upstreamLabel({ upstream: 'origin/main', ahead: 2, behind: 1 })).toBe('↑2 ↓1');
    expect(upstreamLabel(null)).toBeNull();
  });

  it('plans a branch switch across repos', () => {
    const b = (current: string, local: string[], remote: string[] = []): RepoBranches => ({ repo: 'x', branches: { current, local, remote }, error: null });
    expect(switchPlan(b('main', ['main', 'feature/a']), 'feature/a', false)).toBe('local');
    expect(switchPlan(b('feature/a', ['feature/a']), 'feature/a', false)).toBe('already');
    expect(switchPlan(b('main', ['main'], ['feature/a']), 'feature/a', false)).toBe('remote');
    expect(switchPlan(b('main', ['main']), 'feature/a', false)).toBe('skip');
    expect(switchPlan(b('main', ['main']), 'feature/a', true)).toBe('create');
    expect(switchPlan({ repo: 'x', branches: null, error: 'missing' }, 'a', true)).toBe('skip');
    expect(switchPlan(undefined, 'a', true)).toBe('unknown');
    expect(branchUnion([b('main', ['main', 'feature/a']), b('main', ['main'], ['feature/a', 'fix/b'])])).toEqual(['feature/a', 'main', 'fix/b']);
  });

  it('summarizes bulk results and suggests groups', () => {
    expect(bulkSummary([{ repo: 'a', ok: true, message: '' }, { repo: 'b', ok: false, message: 'x' }])).toBe('1 succeeded, 1 failed');
    expect(bulkSummary([{ repo: 'a', ok: true, message: '' }])).toBe('1 succeeded');
    expect(suggestedGroup('frontend/web')).toBe('Frontend');
    expect(suggestedGroup('web')).toBe('');
    expect(suggestedGroup('apps\\backend\\orders')).toBe('Backend');
  });
});

import { describe, expect, it } from 'vitest';
import { diffEnv, summarizeChanges } from './envDiff';

describe('diffEnv', () => {
  it('lists added, removed and changed variables', () => {
    const changes = diffEnv({ name: 'Dev', variables: { a: '1', b: '2', c: '3' } }, { name: 'Dev', variables: { a: '1', b: '20', d: '4' } });
    expect(changes).toEqual([
      { kind: 'changed', target: 'variable', name: 'b', from: '2', to: '20' },
      { kind: 'added', target: 'variable', name: 'd', value: '4' },
      { kind: 'removed', target: 'variable', name: 'c', value: '3' },
    ]);
    expect(summarizeChanges(changes)).toBe('1 added · 1 changed · 1 removed');
  });

  it('reports secrets by name, the protected flag and appearance', () => {
    const changes = diffEnv(
      { name: 'Prod', color: 'prod', secrets: ['OLD'], secretSources: { OLD: 'op://a' } },
      { name: 'Production', color: '#ff0000', label: 'EU', protected: true, secrets: ['OLD', 'API_KEY'], secretSources: { OLD: 'op://b' } },
    );
    expect(changes).toContainEqual({ kind: 'changed', target: 'name', from: 'Prod', to: 'Production' });
    expect(changes).toContainEqual({ kind: 'changed', target: 'color', from: 'prod', to: '#ff0000' });
    expect(changes).toContainEqual({ kind: 'changed', target: 'label', from: '', to: 'EU' });
    expect(changes).toContainEqual({ kind: 'changed', target: 'protected', from: false, to: true });
    expect(changes).toContainEqual({ kind: 'added', target: 'secret', name: 'API_KEY' });
    expect(changes).toContainEqual({ kind: 'changed', target: 'secretSource', name: 'OLD', from: 'op://a', to: 'op://b' });
    expect(changes.some((c) => c.target === 'variable')).toBe(false);
    expect(summarizeChanges(changes)).toContain('secret API_KEY added');
  });

  it('handles missing files and non-string values', () => {
    expect(diffEnv(null, { variables: { port: 8080 } })).toEqual([{ kind: 'added', target: 'variable', name: 'port', value: '8080' }]);
    expect(diffEnv({ variables: { x: 'y' }, secrets: ['S'] }, null)).toEqual([
      { kind: 'removed', target: 'variable', name: 'x', value: 'y' },
      { kind: 'removed', target: 'secret', name: 'S' },
    ]);
    expect(diffEnv({ name: 'A', variables: {} }, { name: 'A' })).toEqual([]);
    expect(summarizeChanges([])).toBe('No changes');
  });

  it('diffs manifest descriptions', () => {
    expect(diffEnv({ name: 'P', description: 'old' }, { name: 'P', description: 'new' })).toEqual([{ kind: 'changed', target: 'description', from: 'old', to: 'new' }]);
  });
});

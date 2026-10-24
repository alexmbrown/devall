import { describe, expect, it } from 'vitest';
import { parseIgnore, snapshotPath, suggestIgnorePaths, upsertSnapshotTest } from './snapshots';

describe('snapshots', () => {
  it('derives the default file like the Rust side', () => {
    expect(snapshotPath('users/list.req.yaml')).toBe('http/__snapshots__/users/list.snap.json');
    expect(snapshotPath('ping.req.yaml')).toBe('http/__snapshots__/ping.snap.json');
    expect(snapshotPath('a\\b.req.yaml')).toBe('http/__snapshots__/a/b.snap.json');
  });

  it('adds or updates the snapshot assertion', () => {
    const file = 'http/__snapshots__/x.snap.json';
    const one = upsertSnapshotTest([{ assert: 'status', op: 'eq', value: '200' }], file, ['$..id']);
    expect(one).toHaveLength(2);
    expect(one[1]).toEqual({ assert: 'snapshot', file, op: 'eq', ignore: ['$..id'] });
    const two = upsertSnapshotTest([{ ...one[1], enabled: false }], file, []);
    expect(two).toEqual([{ assert: 'snapshot', file, op: 'eq', enabled: false }]);
  });

  it('parses ignore lists', () => {
    expect(parseIgnore('$.a, $..b\n\n$.a')).toEqual(['$.a', '$..b']);
  });

  it('suggests volatile fields only', () => {
    const body = JSON.stringify({
      id: 1,
      name: 'Ada',
      createdAt: '2026-10-22T10:00:00Z',
      requestId: '01JBQ8Z6W4N7Q2F3K9M1T5R8XY',
      items: [{ uuid: '6f1c2a9e-7b1d-4c3e-9a2b-1c2d3e4f5a6b', qty: 2, 'last-seen': '2026-10-21 08:00' }],
      exp: 1767225600,
    });
    expect(suggestIgnorePaths(body)).toEqual(['$..createdAt', '$..requestId', '$..uuid', "$..['last-seen']"]);
    expect(suggestIgnorePaths('not json')).toEqual([]);
  });
});

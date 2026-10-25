import { describe, expect, it } from 'vitest';
import { chunkAt, hasMarkers, nearestChunk, parseConflicts, resolveAll, resolveChunk } from './conflictMarkers';

const merge = ['one', '<<<<<<< HEAD', 'ours 1', 'ours 2', '=======', 'theirs', '>>>>>>> feature', 'middle', '<<<<<<< HEAD', 'o', '=======', 't', '>>>>>>> feature', 'end', ''].join('\n');
const zdiff3 = ['a', '<<<<<<< HEAD', 'ours', '||||||| base', 'base line', '=======', 'theirs', '>>>>>>> topic', 'z'].join('\r\n');

describe('conflict markers', () => {
  it('finds chunks with and without a base', () => {
    const c = parseConflicts(merge);
    expect(c).toHaveLength(2);
    expect(c[0]).toMatchObject({ start: 1, end: 6, ours: ['ours 1', 'ours 2'], base: null, theirs: ['theirs'], oursLabel: 'HEAD', theirsLabel: 'feature' });
    const z = parseConflicts(zdiff3);
    expect(z[0].base).toEqual(['base line']);
    expect(z[0].theirs).toEqual(['theirs']);
    expect(hasMarkers('no conflicts\n')).toBe(false);
    // An unclosed chunk isn't a chunk.
    expect(parseConflicts('<<<<<<< a\nx\n=======\ny\n')).toHaveLength(0);
  });

  it('resolves one chunk at a time, keeping line endings', () => {
    expect(resolveChunk(merge, 0, 'theirs')).toBe(['one', 'theirs', 'middle', '<<<<<<< HEAD', 'o', '=======', 't', '>>>>>>> feature', 'end', ''].join('\n'));
    expect(resolveChunk(merge, 1, 'oursThenTheirs').split('\n').slice(-5)).toEqual(['middle', 'o', 't', 'end', '']);
    expect(resolveChunk(merge, 1, 'theirsThenOurs')).toContain('t\no\nend');
    expect(resolveChunk(zdiff3, 0, 'base')).toBe('a\r\nbase line\r\nz');
    expect(resolveChunk(zdiff3, 0, 'ours')).toBe('a\r\nours\r\nz');
    expect(resolveChunk(merge, 5, 'ours')).toBe(merge);
    expect(resolveAll(merge, 'ours')).toBe(['one', 'ours 1', 'ours 2', 'middle', 'o', 'end', ''].join('\n'));
  });

  it('navigates between chunks', () => {
    const c = parseConflicts(merge);
    expect(nearestChunk(c, 1, 1)).toBe(0);
    expect(nearestChunk(c, 3, 1)).toBe(1);
    expect(nearestChunk(c, 14, 1)).toBe(0); // wraps
    expect(nearestChunk(c, 14, -1)).toBe(1);
    expect(nearestChunk(c, 1, -1)).toBe(1); // wraps
    expect(chunkAt(c, 3)).toBe(0);
    expect(chunkAt(c, 8)).toBe(-1);
    expect(nearestChunk([], 1, 1)).toBe(-1);
  });
});

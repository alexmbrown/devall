import { describe, expect, it } from 'vitest';
import type { PatchView } from '@/lib/api/git';
import { pickLine, selectionCounts, toSelection } from './staging';

const view: PatchView = {
  path: 'f.txt',
  staged: false,
  binary: false,
  fingerprint: 'x',
  lineCount: 8,
  tooLarge: false,
  crlf: false,
  hunks: [
    {
      header: '@@ -1,3 +1,4 @@',
      lines: [
        { kind: ' ', text: 'one', old: 1, new: 1, noEol: false },
        { kind: '-', text: 'two', old: 2, new: null, noEol: false },
        { kind: '+', text: 'TWO', old: null, new: 2, noEol: false },
        { kind: '+', text: '2.5', old: null, new: 3, noEol: false },
        { kind: ' ', text: 'three', old: 3, new: 4, noEol: false },
      ],
    },
    { header: '@@ -10,2 +11,1 @@', lines: [{ kind: ' ', text: 'ten', old: 10, new: 11, noEol: false }, { kind: '-', text: 'eleven', old: 11, new: null, noEol: false }] },
  ],
};

describe('line staging selection', () => {
  it('toggles changed lines and ignores context', () => {
    let p = pickLine(view, {}, 0, 2, null);
    expect(p).toEqual({ 0: [2] });
    expect(pickLine(view, p, 0, 0, null)).toBe(p);
    p = pickLine(view, p, 0, 2, null);
    expect(p).toEqual({ 0: [] });
  });

  it('shift-selects a run within one hunk, skipping context', () => {
    const p = pickLine(view, {}, 0, 3, { hunk: 0, line: 1 });
    expect([...p[0]].sort()).toEqual([1, 2, 3]);
    // An anchor in another hunk just toggles.
    expect(pickLine(view, {}, 1, 1, { hunk: 0, line: 1 })).toEqual({ 1: [1] });
  });

  it('sends whole hunks when every changed line is picked, else sorted lines', () => {
    expect(toSelection(view, { 0: [3, 1], 1: [] })).toEqual([{ hunk: 0, lines: [1, 3] }]);
    expect(toSelection(view, { 1: [1], 0: [1, 2, 3] })).toEqual([
      { hunk: 0, lines: null },
      { hunk: 1, lines: null },
    ]);
    expect(toSelection(view, { 7: [1] })).toEqual([]);
  });

  it('counts what a selection adds and removes', () => {
    expect(selectionCounts(view, [{ hunk: 0, lines: null }])).toEqual({ added: 2, deleted: 1 });
    expect(selectionCounts(view, [{ hunk: 0, lines: [2] }, { hunk: 1, lines: null }])).toEqual({ added: 1, deleted: 1 });
  });
});

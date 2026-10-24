import { describe, expect, it } from 'vitest';
import { decodeEscapes, diffRanges, fromDisplay, toDisplay, visibleTokens } from './compare';

describe('visibleTokens', () => {
  it('marks whitespace and invisible characters', () => {
    const t = visibleTokens('a b\t ​﻿\r\n‮z');
    expect(t.map((x) => x.kind)).toEqual(['text', 'space', 'text', 'tab', 'nbsp', 'invisible', 'bom', 'cr', 'lf', 'bidi', 'text']);
    expect(t.find((x) => x.kind === 'invisible')?.text).toBe('⟨ZWSP⟩');
    expect(t.find((x) => x.kind === 'bidi')?.text).toBe('⟨RLO⟩');
    expect(t.find((x) => x.kind === 'lf')?.text).toBe('↵\n');
  });

  it('keeps look-alike letters as text', () => {
    expect(visibleTokens('pаy')).toEqual([{ kind: 'text', text: 'pаy' }]);
  });
});

describe('CR display mapping', () => {
  it('round-trips carriage returns through the textarea representation', () => {
    const raw = 'a\r\nb\rc\n';
    expect(toDisplay(raw)).toBe('a␍\nb␍c\n');
    expect(fromDisplay(toDisplay(raw))).toBe(raw);
  });
});

describe('decodeEscapes', () => {
  it('decodes common escapes', () => {
    expect(decodeEscapes('a\\tb\\r\\n\\u00e9\\u{1F44D}\\x41\\\\').value).toBe('a\tb\r\né\u{1F44D}A\\');
    expect(decodeEscapes('plain').value).toBe('plain');
    expect(decodeEscapes('keep \\q').value).toBe('keep \\q');
  });

  it('reports malformed escapes', () => {
    expect(decodeEscapes('\\u12').error).toBeTruthy();
    expect(decodeEscapes('\\xZZ').error).toBeTruthy();
    expect(decodeEscapes('\\u{110000}').error).toBeTruthy();
  });
});

describe('diffRanges', () => {
  it('maps segments to UTF-16 ranges per side', () => {
    const r = diffRanges([
      { tag: 'equal', value: 'p' },
      { tag: 'delete', value: 'а' },
      { tag: 'insert', value: 'a' },
      { tag: 'equal', value: 'y👍' },
      { tag: 'insert', value: '!' },
    ]);
    expect(r.a).toEqual([[1, 2]]);
    expect(r.b).toEqual([
      [1, 2],
      [5, 6],
    ]);
  });
});

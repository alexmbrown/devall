import { describe, expect, it } from 'vitest';
import { inCode, isLinkShortcut, replaceRange, wikiTrigger } from './insert';
import { markdownLink } from './syntax';

/** `|` marks the caret. */
function at(s: string): [string, number] {
  const i = s.indexOf('|');
  return [s.slice(0, i) + s.slice(i + 1), i];
}

describe('[[ trigger', () => {
  it('fires right after typing [[ (with or without auto-closed brackets)', () => {
    expect(wikiTrigger(...at('See [[|'))).toEqual({ from: 4, to: 6 });
    expect(wikiTrigger(...at('See [[|]] for more'))).toEqual({ from: 4, to: 8 });
    expect(wikiTrigger(...at('[[|'))).toEqual({ from: 0, to: 2 });
    expect(wikiTrigger(...at('a [[|] b'))).toEqual({ from: 2, to: 5 });
  });

  it('ignores single brackets, a third bracket, escapes and code', () => {
    expect(wikiTrigger(...at('See [|'))).toBeNull();
    expect(wikiTrigger(...at('See [[[|'))).toBeNull();
    expect(wikiTrigger(...at('See \\[[|'))).toBeNull();
    expect(wikiTrigger(...at('Use `a[[|'))).toBeNull();
    expect(wikiTrigger(...at('```\nx = a[[|\n'))).toBeNull();
    expect(wikiTrigger(...at('```\ncode\n```\nnow [[|'))).toEqual({ from: 17, to: 19 });
    expect(wikiTrigger(...at('[[x|'))).toBeNull();
  });

  it('knows when a position is inside code', () => {
    expect(inCode('a `b', 4)).toBe(true);
    expect(inCode('a `b` c', 7)).toBe(false);
    expect(inCode('~~~\nx', 5)).toBe(true);
  });

  it('replaces the typed brackets with the link and puts the caret after it', () => {
    const [text, caret] = at('Call [[|]] first');
    const t = wikiTrigger(text, caret)!;
    const md = markdownLink('Create order', 'devall:request/orders/create.req.yaml');
    const r = replaceRange(text, t.from, t.to, md);
    expect(r.text).toBe('Call [Create order](devall:request/orders/create.req.yaml) first');
    expect(r.text.slice(0, r.caret)).toBe(`Call ${md}`);
  });
});

describe('Link to… shortcut', () => {
  const k = (o: Partial<{ key: string; code: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean }>) => ({ key: 'k', code: 'KeyK', ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...o });
  it('is Ctrl+Alt+K on Windows/Linux and ⌘⌥K on macOS, never the palette key', () => {
    expect(isLinkShortcut(k({ ctrlKey: true, altKey: true }), false)).toBe(true);
    expect(isLinkShortcut(k({ ctrlKey: true }), false)).toBe(false);
    expect(isLinkShortcut(k({ ctrlKey: true, altKey: true, shiftKey: true }), false)).toBe(false);
    // AltGr+K typing a character on some layouts.
    expect(isLinkShortcut(k({ ctrlKey: true, altKey: true, key: 'ĸ' }), false)).toBe(false);
    expect(isLinkShortcut(k({ metaKey: true, altKey: true, key: '˚' }), true)).toBe(true);
    expect(isLinkShortcut(k({ metaKey: true }), true)).toBe(false);
    expect(isLinkShortcut(k({ ctrlKey: true, altKey: true }), true)).toBe(false);
  });
});

import { describe, expect, it } from 'vitest';
import { TermBuffer, ansiRuns, applySgr, color256, keyToData, runs, stripAnsi, styleOf } from './ansi';

const text = (b: TermBuffer) => b.text();

describe('TermBuffer', () => {
  it('handles newlines, carriage return overwrite and backspace', () => {
    const b = new TermBuffer();
    b.write('hello world\r\nabc');
    b.write('\rX');
    b.write('\b\bY');
    expect(text(b)).toBe('hello world\nYbc');
  });

  it('keeps escape sequences split across chunks', () => {
    const b = new TermBuffer();
    b.write('a\x1b[3');
    b.write('1mred\x1b[0m b');
    expect(text(b)).toBe('ared b');
    const r = runs(b.lines[0]);
    expect(r.map((x) => x.text)).toEqual(['a', 'red', ' b']);
    expect(styleOf(r[1].style).fg).toBe('var(--term-1)');
    expect(styleOf(r[2].style).fg).toBeUndefined();
  });

  it('supports readline-style editing (erase line, delete/insert chars, cursor moves)', () => {
    const b = new TermBuffer();
    b.write('$ ls -la');
    b.write('\x1b[3D\x1b[K'); // back 3, erase to end
    expect(text(b)).toBe('$ ls');
    b.write('\x1b[3D\x1b[P'); // back 3 (onto the "l"), delete it
    expect(text(b)).toBe('$ s');
    b.write('\x1b[@x'); // insert a blank, then overwrite it
    expect(text(b)).toBe('$ xs');
    b.write('\x1b[2K');
    expect(text(b)).toBe('');
  });

  it('clears the screen and tracks modes', () => {
    const b = new TermBuffer();
    b.write('one\r\ntwo\r\n');
    b.write('\x1b[H\x1b[2J\x1b[3J');
    expect(text(b)).toBe('');
    b.write('\x1b[?2004h\x1b[?1h');
    expect(b.bracketedPaste).toBe(true);
    expect(b.appCursorKeys).toBe(true);
    b.write('\x1b]0;title\x07ok');
    expect(text(b)).toBe('ok');
  });

  it('expands tabs and caps history', () => {
    const b = new TermBuffer(3);
    b.write('a\tb\n1\n2\n3\n4');
    expect(b.lines.length).toBe(3);
    expect(text(b)).toBe('2\n3\n4');
    const t = new TermBuffer();
    t.write('a\tb');
    expect(text(t)).toBe('a       b');
  });
});

describe('SGR and colors', () => {
  it('maps 16/256/truecolor and attributes', () => {
    expect(applySgr({}, [1, 92]).fg).toBe('var(--term-10)');
    expect(applySgr({}, [1, 92]).bold).toBe(true);
    expect(applySgr({}, [38, 5, 196]).fg).toBe('rgb(255, 0, 0)');
    expect(applySgr({}, [48, 2, 1, 2, 3]).bg).toBe('rgb(1, 2, 3)');
    expect(applySgr({ fg: 'x', bold: true }, [0])).toEqual({});
    expect(color256(244)).toBe('rgb(128, 128, 128)');
  });

  it('splits colored log lines and strips codes', () => {
    const line = '\x1b[32mINFO\x1b[0m started on :8080';
    expect(ansiRuns(line).map((r) => r.text)).toEqual(['INFO', ' started on :8080']);
    expect(ansiRuns('plain')).toEqual([{ text: 'plain', style: {} }]);
    expect(stripAnsi(line)).toBe('INFO started on :8080');
  });
});

describe('keyToData', () => {
  const k = (key: string, mods: Partial<{ ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean }> = {}) =>
    keyToData({ key, ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, ...mods }, false);
  it('maps control keys like xterm', () => {
    expect(k('c', { ctrlKey: true })).toBe('\x03');
    expect(k('d', { ctrlKey: true })).toBe('\x04');
    expect(k('Enter')).toBe('\r');
    expect(k('Backspace')).toBe('\x7f');
    expect(k('ArrowUp')).toBe('\x1b[A');
    expect(keyToData({ key: 'ArrowUp', ctrlKey: false, altKey: false, metaKey: false, shiftKey: false }, true)).toBe('\x1bOA');
    expect(k('x', { altKey: true })).toBe('\x1bx');
    expect(k('a')).toBe('a');
    expect(k('C', { ctrlKey: true, shiftKey: true })).toBeNull();
    expect(k('v', { metaKey: true })).toBeNull();
    expect(k('Shift')).toBeNull();
  });
});

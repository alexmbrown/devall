import { describe, expect, it } from 'vitest';
import { DEFAULT_ACCENT, accentTokens, readableText, validAccent } from './accent';

describe('accent colour', () => {
  it('derives tokens close to the built-in indigo palette', () => {
    const light = accentTokens(DEFAULT_ACCENT, 'light');
    expect(light['--accent']).toBe('#4f46e5');
    expect(light['--accent-text']).toBe('#ffffff');
    const dark = accentTokens(DEFAULT_ACCENT, 'dark');
    // tokens.css dark accent is #6d68f5; the derived one is within a few steps of it.
    const [r, g, b] = [1, 3, 5].map((i) => parseInt(dark['--accent'].slice(i, i + 2), 16));
    expect(Math.abs(r - 0x6d) + Math.abs(g - 0x68) + Math.abs(b - 0xf5)).toBeLessThan(30);
  });

  it('picks readable text for light and dark colours', () => {
    expect(readableText('#4f46e5')).toBe('#ffffff');
    expect(readableText('#facc15')).toBe('#111111');
    expect(readableText('#475569')).toBe('#ffffff');
  });

  it('accepts only hex colours', () => {
    expect(validAccent('#0D9488')).toBe('#0d9488');
    expect(validAccent('#abc')).toBe('#aabbcc');
    expect(validAccent('teal')).toBeNull();
    expect(validAccent(null)).toBeNull();
  });
});

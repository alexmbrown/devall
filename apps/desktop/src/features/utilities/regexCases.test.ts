import { describe, expect, it } from 'vitest';
import { aiSamples } from './regexCases';

describe('aiSamples', () => {
  it('uses test-text lines and cases', () => {
    const s = aiSamples('ORD-1234\r\n\n  ORD-0001  \nnope', ['ORD-9999', 'ORD-1234'], ['nope', 'ORD-12']);
    expect(s.positive).toEqual(['ORD-9999', 'ORD-1234', 'ORD-0001']);
    expect(s.negative).toEqual(['nope', 'ORD-12']);
  });

  it('caps the number and length of lines', () => {
    const text = Array.from({ length: 40 }, (_, i) => `line ${i}`).join('\n') + `\n${'x'.repeat(500)}`;
    const s = aiSamples(text, [], []);
    expect(s.positive).toHaveLength(20);
    expect(s.positive.every((l) => l.length <= 200)).toBe(true);
  });
});

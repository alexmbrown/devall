import { describe, expect, it } from 'vitest';
import { isLoopback } from '@/lib/api/ai';
import { linkCitations } from './citations';

const sources = [
  { id: 'S1', kind: 'todos', source: 'todos (personal)', title: 'My to-dos' },
  { id: 'S2', kind: 'note', source: 'notes/payments.md', title: 'Payments' },
];

describe('linkCitations', () => {
  it('links known source ids', () => {
    expect(linkCitations('Two items remain [S1].', sources)).toBe('Two items remain [1](cite/S1).');
    expect(linkCitations('See [S2, S1]', sources)).toBe('See [2](cite/S2)[1](cite/S1)');
  });
  it('keeps unknown markers and ordinary brackets', () => {
    expect(linkCitations('Nope [S9] and [link](http://x) and [x]', sources)).toBe('Nope [S9] and [link](http://x) and [x]');
  });
});

describe('isLoopback', () => {
  it('detects local endpoints', () => {
    expect(isLoopback('http://127.0.0.1:11434')).toBe(true);
    expect(isLoopback('http://localhost:1234/v1')).toBe(true);
    expect(isLoopback('http://[::1]:8080')).toBe(true);
    expect(isLoopback('https://api.openai.com/v1')).toBe(false);
    expect(isLoopback('not a url')).toBe(false);
  });
});

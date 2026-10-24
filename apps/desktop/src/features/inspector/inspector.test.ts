import { describe, expect, it } from 'vitest';
import type { BodyView, ExchangeSummary, PausedExchange } from '@/lib/api/proxy';
import { contentKind, matches } from './filter';
import { editFor, hexDump, parseMessage, pausedText } from './format';

const body = (text: string | null, base64: string | null = null): BodyView => ({
  size: (text ?? '').length,
  captured: (text ?? '').length,
  truncated: false,
  evicted: false,
  encoding: null,
  decoded: false,
  decodeError: null,
  contentType: 'application/json',
  text,
  base64,
});

const paused: PausedExchange = {
  id: 1,
  phase: 'request',
  method: 'POST',
  url: 'https://api.example.com/users',
  status: null,
  headers: [
    ['content-type', 'application/json'],
    ['x-a', '1'],
  ],
  body: body('{"a":1}'),
  deadline: 0,
};

describe('breakpoint text', () => {
  it('round-trips without changes', () => {
    expect(editFor(paused, pausedText(paused))).toBeUndefined();
  });

  it('detects method, url, header and body edits', () => {
    const text = pausedText(paused).replace('POST https://api.example.com/users', 'PUT https://localhost:3000/users').replace('x-a: 1', 'x-a: 2').replace('"a": 1', '"a": 2');
    expect(editFor(paused, text)).toEqual({
      method: 'PUT',
      url: 'https://localhost:3000/users',
      headers: [
        ['content-type', 'application/json'],
        ['x-a', '2'],
      ],
      body: '{\n  "a": 2\n}',
    });
  });

  it('parses responses and reports bad input', () => {
    const r = parseMessage('HTTP/1.1 418 I am a teapot\nx: y\n\nbody', 'response');
    expect(r).toEqual({ status: 418, headers: [['x', 'y']], body: 'body' });
    expect(typeof parseMessage('nonsense', 'response')).toBe('string');
    expect(typeof parseMessage('GET /\nbad header line\n\n', 'request')).toBe('string');
  });

  it('keeps binary bodies unless replaced', () => {
    const bin: PausedExchange = { ...paused, body: body(null, 'AQID') };
    expect(editFor(bin, pausedText(bin))).toBeUndefined();
  });
});

describe('filter', () => {
  const s = (over: Partial<ExchangeSummary>): ExchangeSummary => ({
    id: 1,
    startedAt: 0,
    method: 'GET',
    url: 'https://api.example.com/users?page=2',
    scheme: 'https',
    host: 'api.example.com',
    path: '/users?page=2',
    httpVersion: 'HTTP/2',
    status: 200,
    state: 'complete',
    contentType: 'application/json',
    requestSize: 0,
    responseSize: 10,
    durationMs: 5,
    rules: [],
    tags: [],
    paused: null,
    error: null,
    wsFrames: 0,
    clientAddr: null,
    mappedUrl: null,
    violations: 0,
    ...over,
  });

  it('mirrors the Rust filter', () => {
    expect(matches({ text: 'users -page=3' }, s({}))).toBe(true);
    expect(matches({ text: '-page=2' }, s({}))).toBe(false);
    expect(matches({ statusClasses: [4, 0] }, s({}))).toBe(false);
    expect(matches({ statusClasses: [0] }, s({ status: null, state: 'pending' }))).toBe(true);
    expect(matches({ hideStatic: true }, s({ contentType: null, path: '/app.js' }))).toBe(false);
    expect(matches({ host: 'API.' }, s({}))).toBe(true);
    expect(contentKind('font/woff2', '/x')).toBe('font');
  });

  it('hex dumps', () => {
    const line = hexDump('AQID');
    expect(line).toMatch(/^00000000 {2}01 02 03 +\.\.\.$/);
    expect(line.length).toBe(8 + 2 + 47 + 2 + 3);
  });
});

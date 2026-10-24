// "HTTP request → mock" and "stub → HTTP request" helpers (no Tauri needed).
import { describe, expect, it } from 'vitest';
import type { SendResult } from '@/lib/types';
import { stubFromSendResult, DEFAULT_FROM_RESPONSE } from './fromResponse';
import { requestForStub } from './stubs/openInRequests';
import { folderOf, tagsOf } from './lib/stub-utils';

const result = (o: Partial<SendResult> = {}): SendResult => ({
  response: {
    status: 201,
    statusText: 'Created',
    httpVersion: 'HTTP/1.1',
    headers: [
      ['Content-Type', 'application/json'],
      ['Date', 'x'],
      ['Set-Cookie', 'a=1'],
      ['set-cookie', 'b=2'],
    ],
    bodyTruncated: false,
    url: 'https://api.example.com/users?page=2',
    redirected: false,
    remoteAddr: null,
    timings: { ttfbMs: 1, downloadMs: 1, totalMs: 2 },
    bodySize: 20,
    bodyText: '{"id":7,"name":"Ada"}',
    bodyBase64: null,
    contentType: 'application/json',
  },
  error: null,
  cancelled: false,
  sent: { method: 'POST', url: 'https://api.example.com/users?page=2', headers: [['Content-Type', 'application/json']], body: '{"name":"Ada"}' },
  unresolved: [],
  secretErrors: [],
  tests: [],
  captures: [],
  historyId: null,
  at: 0,
  ...o,
});

describe('stub from an HTTP-client response', () => {
  it('matches method, path and query and returns the response', () => {
    const s = stubFromSendResult(result());
    expect(s.request).toEqual({ method: 'POST', urlPath: '/users', queryParameters: { page: { equalTo: '2' } } });
    expect(s.response.status).toBe(201);
    expect(s.response.jsonBody).toEqual({ id: 7, name: 'Ada' });
    expect(s.response.headers).toEqual({ 'Content-Type': 'application/json', 'Set-Cookie': 'a=1, b=2' });
    expect(s.persistent).toBe(true);
    expect(folderOf(s)).toBe('from-http');
    expect(tagsOf(s)).toEqual(['from-http']);
  });
  it('honours the options', () => {
    const s = stubFromSendResult(result(), { ...DEFAULT_FROM_RESPONSE, urlMode: 'url', matchBody: true, folder: '' });
    expect(s.request.url).toBe('/users?page=2');
    expect(s.request.bodyPatterns).toEqual([{ equalToJson: { name: 'Ada' }, ignoreExtraElements: true }]);
    expect(folderOf(s)).toBe('');
  });
  it('keeps binary bodies as base64', () => {
    const r = result();
    const s = stubFromSendResult({ ...r, response: { ...r.response!, bodyText: null, bodyBase64: 'AAEC', contentType: 'image/png', headers: [['Content-Type', 'image/png']] } });
    expect(s.response.base64Body).toBe('AAEC');
  });
  it('refuses results without a response', () => {
    expect(() => stubFromSendResult(result({ response: null }))).toThrow(/no response/);
    expect(() => stubFromSendResult(result({ sent: null }))).toThrow(/not sent/);
  });
});

describe('HTTP request for a stub', () => {
  it('calls the stub on the mock server', () => {
    const r = requestForStub(
      {
        name: 'Create order',
        request: { method: 'POST', urlPathTemplate: '/orders/{id}', headers: { 'X-Tenant': { equalTo: 'acme' } }, bodyPatterns: [{ equalToJson: { sku: 'A1' } }] },
        response: { status: 201 },
      },
      'http://127.0.0.1:52000/',
    );
    expect(r.method).toBe('POST');
    expect(r.url).toBe('http://127.0.0.1:52000/orders/1');
    expect(r.headers).toEqual([{ name: 'X-Tenant', value: 'acme' }]);
    expect(r.body).toEqual({ type: 'json', content: '{\n  "sku": "A1"\n}' });
    expect(r.name).toBe('Create order');
  });
  it('uses GET for ANY and form bodies for form matchers', () => {
    const r = requestForStub({ request: { method: 'ANY', urlPath: '/f', formParameters: { a: { equalTo: '1' } } }, response: {} }, 'http://h');
    expect(r.method).toBe('GET');
    expect(r.body).toEqual({ type: 'form', fields: [{ name: 'a', value: '1' }] });
  });
});

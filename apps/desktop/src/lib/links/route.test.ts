import { describe, expect, it } from 'vitest';
import { linkForRoute } from './route';

describe('linkForRoute', () => {
  it('names the item a page shows', () => {
    expect(linkForRoute('http', 'orders/create.req.yaml')).toBe('devall:request/orders/create.req.yaml');
    expect(linkForRoute('http', 'chat.ws.yaml')).toBe('devall:request/chat.ws.yaml');
    expect(linkForRoute('http', 'orders/_folder.yaml')).toBe('devall:folder/orders');
    expect(linkForRoute('http', '_folder.yaml')).toBe('devall:page/http');
    expect(linkForRoute('notes', 'design/auth.md')).toBe('devall:note/design/auth.md');
    expect(linkForRoute('environments', 'staging')).toBe('devall:env/staging');
    expect(linkForRoute('environments', '__project')).toBe('devall:variables');
    expect(linkForRoute('environments', '__compare')).toBe('devall:page/environments/__compare');
    expect(linkForRoute('load', 'checkout.load.yaml')).toBe('devall:load/checkout.load.yaml');
    expect(linkForRoute('fakedata', 'users.data.yaml')).toBe('devall:fakedata/users.data.yaml');
    expect(linkForRoute('data', 'prod/orders.conn.yaml')).toBe('devall:connection/prod/orders.conn.yaml');
    expect(linkForRoute('data', 'query:orders/top.sql|1712')).toBe('devall:query/orders/top.sql');
    expect(linkForRoute('webhooks', 'stripe')).toBe('devall:webhook/stripe');
    expect(linkForRoute('utilities', 'jwt.decode|eyJhbGci')).toBe('devall:utility/jwt.decode');
    expect(linkForRoute('git', 'api')).toBe('devall:repo/api');
    expect(linkForRoute('git', 'web:compare:origin/main...feature/x')).toBe('devall:compare/web/origin/main...feature/x');
    expect(linkForRoute('git', 'web:commit:abc1234')).toBe('devall:commit/web/abc1234');
  });

  it('falls back to the page and its param', () => {
    expect(linkForRoute('settings')).toBe('devall:page/settings');
    expect(linkForRoute('settings', 'sidebar')).toBe('devall:page/settings/sidebar');
    expect(linkForRoute('mocks', 'stubs/file:mappings/a.json|99')).toBe('devall:page/mocks/stubs/file:mappings/a.json');
    expect(linkForRoute('todos')).toBe('devall:page/todos');
  });
});

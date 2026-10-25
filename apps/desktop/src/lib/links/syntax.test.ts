import { describe, expect, it } from 'vitest';
import { canonicalLink, escapeMarkdownLabel, formatLink, isDevallLink, LINK_KINDS, markdownLink, pageLink, parseLink, parsePageId } from './syntax';

describe('link syntax', () => {
  it('formats readable links, encoding only what Markdown needs (same as devall_core::links)', () => {
    expect(formatLink('request', 'orders/create.req.yaml')).toBe('devall:request/orders/create.req.yaml');
    expect(formatLink('note', 'Design notes/Auth (v2).md')).toBe('devall:note/Design%20notes/Auth%20%28v2%29.md');
    expect(formatLink('utility', 'my-plugin:hash')).toBe('devall:utility/my-plugin:hash');
    expect(formatLink('note', 'café.md')).toBe('devall:note/caf%C3%A9.md');
    expect(formatLink('variables')).toBe('devall:variables');
    expect(formatLink('page', '/settings/sidebar/')).toBe('devall:page/settings/sidebar');
    expect(formatLink('query', 'a#b?c%d.sql')).toBe('devall:query/a%23b%3Fc%25d.sql');
  });

  it('round-trips every kind', () => {
    for (const kind of LINK_KINDS) {
      const id = kind === 'variables' ? '' : 'a b/c(1)ü.yaml';
      expect(parseLink(formatLink(kind, id))).toEqual({ kind, id });
    }
  });

  it('parses leniently but never climbs out of a folder', () => {
    expect(parseLink('  DEVALL://Note/design/auth.md#pkce ')).toEqual({ kind: 'note', id: 'design/auth.md', anchor: 'pkce' });
    expect(parseLink('devall://notes/design/auth.md')).toEqual({ kind: 'note', id: 'design/auth.md' });
    expect(parseLink('devall:request/a//b.req.yaml?x=1')).toEqual({ kind: 'request', id: 'a/b.req.yaml' });
    expect(parseLink('devall:variables/anything')).toEqual({ kind: 'variables', id: '' });
    expect(parseLink('devall:note/My Note.md')?.id).toBe('My Note.md');
    expect(parseLink('devall:request/../secrets.env')).toBeNull();
    expect(parseLink('devall:request/a/%2E%2E/b')).toBeNull();
    expect(parseLink('devall:note/a%2Fb.md')).toBeNull();
    expect(parseLink('devall:request')).toBeNull();
    expect(parseLink('devall:bogus/x')).toBeNull();
    expect(parseLink('http/x.req.yaml')).toBeNull();
    expect(parseLink(undefined)).toBeNull();
    expect(isDevallLink('Devall:todo/1')).toBe(true);
    expect(isDevallLink('https://devall.dev')).toBe(false);
  });

  it('canonicalizes spellings of the same link', () => {
    expect(canonicalLink('DEVALL:Request/orders/create.req.yaml#x')).toBe('devall:request/orders/create.req.yaml');
    expect(canonicalLink('devall:note/My Note.md')).toBe('devall:note/My%20Note.md');
    expect(canonicalLink('devall:note/My%20Note.md')).toBe('devall:note/My%20Note.md');
    expect(canonicalLink('notes/x.md')).toBeNull();
  });

  it('builds page links and Markdown links', () => {
    expect(pageLink('settings')).toBe('devall:page/settings');
    expect(pageLink('mocks', 'requests/abc')).toBe('devall:page/mocks/requests/abc');
    expect(parsePageId('mocks/requests/abc')).toEqual({ section: 'mocks', param: 'requests/abc' });
    expect(parsePageId('settings')).toEqual({ section: 'settings' });
    expect(markdownLink('Create order', 'devall:request/orders/create.req.yaml')).toBe('[Create order](devall:request/orders/create.req.yaml)');
    expect(markdownLink('[WIP] fix\nrefunds \\ now', 'devall:todo/1')).toBe('[\\[WIP\\] fix refunds \\\\ now](devall:todo/1)');
    expect(markdownLink('  ', 'devall:todo/1')).toBe('[devall:todo/1](devall:todo/1)');
    expect(escapeMarkdownLabel('a]b')).toBe('a\\]b');
  });
});

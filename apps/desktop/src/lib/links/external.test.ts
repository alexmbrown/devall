import { describe, expect, it } from 'vitest';
import { checkIncomingLink, isProjectScoped, MAX_INCOMING_LENGTH, resolveLinkProject, shareableLink, type ProjectCandidate } from './external';
import { parseLink } from './syntax';

describe('shareableLink', () => {
  it('names the project of project items only', () => {
    expect(shareableLink('devall:request/orders/create.req.yaml', '01JC8Z')).toBe('devall:request/orders/create.req.yaml?project=01JC8Z');
    expect(shareableLink('devall:note/My Note.md#top', '01JC8Z')).toBe('devall:note/My%20Note.md?project=01JC8Z#top');
    expect(shareableLink('devall:page/notes', 'P')).toBe('devall:page/notes?project=P');
    expect(shareableLink('devall:compare/web/main...feature/x', 'P')).toBe('devall:compare/web/main...feature/x?project=P');
    // Personal, device-wide and app pages stay as they are.
    expect(shareableLink('devall:todo/01ABC', 'P')).toBe('devall:todo/01ABC');
    expect(shareableLink('devall:utility/jwt.decode', 'P')).toBe('devall:utility/jwt.decode');
    expect(shareableLink('devall:page/settings/sidebar', 'P')).toBe('devall:page/settings/sidebar');
    // Already qualified (pasted from elsewhere), no project open, not a devall: link.
    expect(shareableLink('devall:note/a.md?project=OTHER', 'P')).toBe('devall:note/a.md?project=OTHER');
    expect(shareableLink('devall:note/a.md', null)).toBe('devall:note/a.md');
    expect(shareableLink('https://example.com', 'P')).toBe('https://example.com');
  });

  it('knows which kinds and pages need a project', () => {
    expect(isProjectScoped({ kind: 'variables', id: '' })).toBe(true);
    expect(isProjectScoped({ kind: 'page', id: 'git/api' })).toBe(true);
    expect(isProjectScoped({ kind: 'page', id: 'utilities' })).toBe(false);
    expect(isProjectScoped({ kind: 'page', id: 'nope' })).toBe(false);
    expect(isProjectScoped({ kind: 'todo', id: '1' })).toBe(false);
  });
});

describe('checkIncomingLink', () => {
  const ok = (s: string) => {
    const r = checkIncomingLink(s);
    if (!r.ok) throw new Error(`${s}: ${r.reason}`);
    return r.link;
  };
  const bad = (s: unknown) => {
    const r = checkIncomingLink(s);
    if (r.ok) throw new Error(`accepted ${String(s)}`);
    return r.reason;
  };

  it('accepts what the app copies and what links legitimately contain', () => {
    expect(ok('devall:page/settings')).toEqual({ kind: 'page', id: 'settings' });
    expect(ok('  devall:request/orders/create.req.yaml?project=01JC8Z  ')).toEqual({ kind: 'request', id: 'orders/create.req.yaml', project: '01JC8Z' });
    expect(ok('devall://notes/My%20Note.md').id).toBe('My Note.md');
    expect(ok('devall:compare/web/origin/main...feature/x').id).toBe('web/origin/main...feature/x');
    expect(ok('devall:utility/my-plugin:hash').id).toBe('my-plugin:hash');
    expect(ok('devall:variables').kind).toBe('variables');
  });

  it('refuses anything else (same rules as devall_core::links::parse_incoming)', () => {
    expect(bad(42)).toBe('not a link');
    expect(bad(`devall:note/${'a'.repeat(MAX_INCOMING_LENGTH)}.md`)).toContain('longer');
    expect(bad('https://example.com')).toContain('not a devall: link');
    expect(bad('javascript:alert(1)')).toContain('not a devall: link');
    expect(bad('devall:bogus/x')).toContain('unknown kind “bogus” (use one of request, note');
    expect(bad('devall:request')).toContain('no id');
    expect(bad('devall:request/../secrets.env')).toContain('climbs');
    expect(bad('devall:request/a/%2e%2E/b')).toContain('climbs');
    expect(bad('devall:note/a%5Cb.md')).toContain('never use');
    expect(bad('devall:note/a%2Fb.md')).toContain('never use');
    expect(bad('devall:note/a\\b.md')).toContain('backslashes');
    expect(bad('devall:note/a\nb.md')).toContain('control');
    expect(bad('devall:note/a%0Ab.md')).toContain('never use');
    expect(bad('devall:utility/base64.decode%7Csecret')).toContain('never use');
    expect(bad('devall:page/mocks/stubs/file:../.env')).toContain('climbs');
    expect(bad('devall:page/mocks/stubs/file:%2E%2E/.env')).toContain('climbs');
    expect(bad('devall:note/C:/Windows/win.ini')).toContain('absolute');
    expect(bad('devall:note/~/notes.md')).toContain('absolute');
    expect(bad('devall:note/a.md?project=')).toContain('project id');
    expect(bad('devall:note/a.md?project=a%20b')).toContain('project id');
    expect(bad('devall:page/terminal/rm')).toContain('no “terminal” page');
  });
});

describe('resolveLinkProject', () => {
  const recent: ProjectCandidate[] = [
    { path: 'C:/work/api', name: 'API', id: 'P-API', openedAt: 300 },
    { path: 'C:/old/api-clone', name: 'API (old clone)', id: 'P-API', openedAt: 100 },
    { path: 'C:/work/web', name: 'Web', id: 'P-WEB', openedAt: 200 },
    { path: 'C:/Users/me/AppData/Roaming/dev.devall.app/scratch', name: 'Scratch pad', id: 'P-SCRATCH', openedAt: 0, scratch: true },
  ];
  const link = (s: string) => parseLink(s)!;
  const open = { id: 'P-WEB' };

  it('follows unqualified links in the open project, or asks for one', () => {
    expect(resolveLinkProject(link('devall:note/a.md'), open, recent)).toEqual({ action: 'follow' });
    expect(resolveLinkProject(link('devall:note/a.md'), null, recent)).toEqual({ action: 'ask' });
  });

  it('follows links naming the open project here', () => {
    expect(resolveLinkProject(link('devall:note/a.md?project=P-WEB'), open, recent)).toEqual({ action: 'follow' });
  });

  it('switches to the most recently opened project with that id', () => {
    expect(resolveLinkProject(link('devall:request/x.req.yaml?project=P-API'), open, recent)).toEqual({ action: 'switch', to: recent[0] });
    expect(resolveLinkProject(link('devall:request/x.req.yaml?project=P-API'), open, [recent[1], recent[0]])).toEqual({ action: 'switch', to: recent[0] });
    expect(resolveLinkProject(link('devall:note/a.md?project=P-API'), null, recent)).toEqual({ action: 'switch', to: recent[0] });
    expect(resolveLinkProject(link('devall:note/a.md?project=P-SCRATCH'), open, recent)).toEqual({ action: 'switch', to: recent[3] });
  });

  it('reports a project that is not on this device, matching ids exactly', () => {
    expect(resolveLinkProject(link('devall:note/a.md?project=P-NONE'), open, recent)).toEqual({ action: 'missing', project: 'P-NONE' });
    expect(resolveLinkProject(link('devall:note/a.md?project=p-api'), open, recent)).toEqual({ action: 'missing', project: 'p-api' });
    expect(resolveLinkProject(link('devall:note/a.md?project=P-API'), open, [])).toEqual({ action: 'missing', project: 'P-API' });
  });

  it('follows links that need no project wherever the user is', () => {
    expect(resolveLinkProject(link('devall:page/settings'), null, recent)).toEqual({ action: 'follow' });
    expect(resolveLinkProject(link('devall:todo/01ABC?project=P-NONE'), open, recent)).toEqual({ action: 'follow' });
    expect(resolveLinkProject(link('devall:utility/jwt.decode'), null, [])).toEqual({ action: 'follow' });
  });
});

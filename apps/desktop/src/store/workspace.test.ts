import { describe, expect, it } from 'vitest';
import { docForParam, docParam, parseDocParam, type Tab } from './workspace';

describe('http documents', () => {
  it('maps view params to documents and back', () => {
    expect(parseDocParam('users/get.req.yaml')).toEqual({ kind: 'request', path: 'users/get.req.yaml' });
    expect(parseDocParam('chat.ws.yaml')).toEqual({ kind: 'proto', path: 'chat.ws.yaml', protocol: 'ws' });
    expect(parseDocParam('users/_folder.yaml')).toEqual({ kind: 'folder', path: 'users' });
    expect(parseDocParam('_folder.yaml')).toEqual({ kind: 'folder', path: '' });
    expect(docParam({ kind: 'folder', path: '' })).toBe('_folder.yaml');
    expect(docParam({ kind: 'folder', path: 'a/b' })).toBe('a/b/_folder.yaml');
    expect(docParam({ kind: 'request', path: 'x.req.yaml' })).toBe('x.req.yaml');
  });

  it('finds the loaded document of a param', () => {
    const tabs = [
      { id: '1', kind: 'folder', path: 'users', draft: {}, saved: '{}' },
      { id: '2', kind: 'request', path: 'users', draft: { id: '', name: 'odd', method: 'GET', url: '' }, saved: '' },
    ] as Tab[];
    expect(docForParam(tabs, 'users/_folder.yaml')?.id).toBe('1');
    expect(docForParam(tabs, 'users')?.id).toBe('2');
    expect(docForParam(tabs, undefined)).toBeUndefined();
  });
});

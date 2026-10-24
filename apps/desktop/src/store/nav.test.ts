import { beforeEach, describe, expect, it } from 'vitest';
import { allGroups } from '@/lib/workbench/model';
import { useNav } from './nav';
import { activeViewOf, recentDoc, registerCloseGuard, useWorkbench } from './workbench';

const wb = () => useWorkbench.getState();
const tabs = () => allGroups(wb().tree).map((g) => g.tabs.map((id) => `${wb().views[id].section}${wb().views[id].param ? `:${wb().views[id].param}` : ''}${wb().views[id].preview ? '*' : ''}`));

describe('nav facade', () => {
  beforeEach(() => wb().load(`test-${Math.random()}`));

  it('getState() reads and drives the active view', () => {
    expect(useNav.getState().section).toBe('http');
    useNav.getState().go('utilities', 'jwt');
    expect(useNav.getState()).toMatchObject({ section: 'utilities', param: 'jwt' });
    expect(tabs()).toEqual([['http', 'utilities:jwt*']]);
    // Going back to a section focuses its tab instead of opening another.
    useNav.getState().go('http');
    expect(useNav.getState().section).toBe('http');
    expect(tabs()).toEqual([['http', 'utilities:jwt*']]);
  });

  it('go from a view navigates that view, not the active one', () => {
    useNav.getState().go('environments', 'dev', { preview: false });
    useNav.getState().go('utilities', undefined, { toSide: true });
    const env = Object.values(wb().views).find((v) => v.section === 'environments')!;
    expect(activeViewOf(wb())?.section).toBe('utilities');
    wb().goFrom(env.id, 'environments', 'prod');
    expect(wb().views[env.id].param).toBe('prod');
    // The view's group becomes active.
    expect(activeViewOf(wb())?.id).toBe(env.id);
    expect(tabs()).toEqual([['http', 'environments:prod'], ['utilities']]);
  });

  it('documents open as tabs and remember the most recent one', () => {
    useNav.getState().go('http', 'a.req.yaml');
    useNav.getState().go('http', 'b.req.yaml', { preview: false });
    useNav.getState().go('notes', 'n.md');
    expect(tabs()).toEqual([['http:a.req.yaml', 'http:b.req.yaml', 'notes:n.md*']]);
    expect(recentDoc('http')).toBe('b.req.yaml');
    expect(recentDoc('http', (p) => p.startsWith('a'))).toBe('a.req.yaml');
  });

  it('close guards can keep a view open', async () => {
    let allow = false;
    registerCloseGuard('notes', async () => allow);
    useNav.getState().go('notes', 'n.md');
    const id = activeViewOf(wb())!.id;
    expect(await wb().close(id)).toBe(false);
    expect(wb().views[id]).toBeDefined();
    allow = true;
    expect(await wb().close(id)).toBe(true);
    expect(wb().views[id]).toBeUndefined();
    registerCloseGuard('notes', async () => true);
  });

  it('splits duplicate views but move single-instance sections', () => {
    useNav.getState().go('mocks', undefined, { preview: false });
    wb().split(activeViewOf(wb())!.id, 'right');
    expect(tabs()).toEqual([['http'], ['mocks']]);
    wb().focusGroupIndex(0);
    wb().split(null, 'down');
    expect(tabs()).toEqual([['http'], ['http'], ['mocks']]);
  });
});

import { describe, expect, it } from 'vitest';
import { filterLinkItems, fuzzyScore, parseQuery, type LinkItem } from './fuzzy';

const items: LinkItem[] = [
  { link: 'devall:request/orders/create.req.yaml', kind: 'request', title: 'Create order', detail: 'orders/create.req.yaml', keywords: 'POST' },
  { link: 'devall:request/orders/list.req.yaml', kind: 'request', title: 'List orders', detail: 'orders/list.req.yaml', keywords: 'GET' },
  { link: 'devall:request/users/me.req.yaml', kind: 'request', title: 'Me', detail: 'users/me.req.yaml', keywords: 'GET' },
  { link: 'devall:note/runbooks/orders.md', kind: 'note', title: 'Orders runbook', detail: 'runbooks/orders.md' },
  { link: 'devall:todo/01A', kind: 'todo', title: 'Fix order refunds', detail: 'Shop API' },
  { link: 'devall:env/staging', kind: 'env', title: 'Staging', detail: 'environments/staging.env.yaml' },
  { link: 'devall:variables', kind: 'variables', title: 'Project variables', detail: 'devall.yaml' },
  { link: 'devall:page/settings', kind: 'page', title: 'Settings', detail: 'settings' },
];

describe('fuzzyScore', () => {
  it('prefers prefixes, word starts and substrings over scattered letters', () => {
    expect(fuzzyScore('cre', 'Create order')).toBeGreaterThan(fuzzyScore('ord', 'Create order'));
    expect(fuzzyScore('ord', 'Create order')).toBeGreaterThan(fuzzyScore('rdr', 'Create order'));
    expect(fuzzyScore('cord', 'Create order')).toBeGreaterThan(0);
    expect(fuzzyScore('xyz', 'Create order')).toBe(-1);
    expect(fuzzyScore('', 'anything')).toBe(0);
  });
});

describe('parseQuery', () => {
  it('narrows to a kind with a leading alias', () => {
    expect(parseQuery('todo: refunds')).toEqual({ kinds: ['todo'], text: 'refunds' });
    expect(parseQuery('note:auth')).toEqual({ kinds: ['note'], text: 'auth' });
    expect(parseQuery('env:')).toEqual({ kinds: ['env'], text: '' });
    expect(parseQuery('vars')).toEqual({ text: 'vars' });
    expect(parseQuery('https://example.com/x')).toEqual({ text: 'https://example.com/x' });
    expect(parseQuery('req: me')).toEqual({ kinds: ['request'], text: 'me' });
    expect(parseQuery('nope: x')).toEqual({ text: 'nope: x' });
  });
});

describe('filterLinkItems', () => {
  it('groups by kind and needs every word to match', () => {
    const groups = filterLinkItems(items, 'order');
    expect(groups.map((g) => g.label)).toEqual(['Requests', 'Notes', 'To-dos']);
    expect(groups[0].items.map((i) => i.title)).toEqual(['List orders', 'Create order']);
    expect(filterLinkItems(items, 'order post').flatMap((g) => g.items.map((i) => i.title))).toEqual(['Create order']);
    expect(filterLinkItems(items, 'zzz')).toEqual([]);
  });

  it('ranks the best match first within its group', () => {
    expect(filterLinkItems(items, 'runbook')[0].label).toBe('Notes');
    expect(filterLinkItems(items, 'list ord')[0].items[0].title).toBe('List orders');
    expect(filterLinkItems(items, 'ord')[0].items.map((i) => i.title)).toEqual(['List orders', 'Create order']);
  });

  it('matches details and kind names, filters kinds, merges environments and project variables', () => {
    expect(filterLinkItems(items, 'users/me')[0].items[0].title).toBe('Me');
    expect(filterLinkItems(items, 'todo: order').map((g) => g.label)).toEqual(['To-dos']);
    expect(filterLinkItems(items, 'order', { kinds: ['note'] }).map((g) => g.label)).toEqual(['Notes']);
    const envs = filterLinkItems(items, '').find((g) => g.label === 'Environments');
    expect(envs?.items.map((i) => i.kind)).toEqual(['env', 'variables']);
  });

  it('shows a few of each kind without a query', () => {
    const groups = filterLinkItems(items, '', { perGroup: 2 });
    expect(groups.find((g) => g.label === 'Requests')?.items).toHaveLength(2);
    expect(groups.map((g) => g.label)).toEqual(['Requests', 'Notes', 'To-dos', 'Environments', 'Pages']);
  });
});

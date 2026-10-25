import { describe, expect, it } from 'vitest';
import { toDevallLink } from '@/features/notes/links';
import { buildBacklinkIndex, extractTargets } from './backlinks';
import { formatLink } from './syntax';

const linkOf = { note: (p: string) => formatLink('note', p), todo: (id: string) => formatLink('todo', id) };

describe('extractTargets', () => {
  it('finds project link targets like devall_core::links::extract_targets', () => {
    const md = [
      '# Runbook',
      'See [Create order](devall:request/orders/create.req.yaml "the request") and <devall:note/design/auth.md>.',
      'Old style: [GET users](http/users/list.req.yaml), [ops](../ops/run.md), [api](repo:api/src/main.rs).',
      'Bare devall:todo/01JC8Z6Q3W. Web: [docs](https://example.com) <https://x.dev> [top](#top) ![img](C:/x.png)',
      'Twice [again](devall:request/orders/create.req.yaml), bad [x](devall:nope/1), `devall:note/in-code.md`',
      '[ref]: devall:env/staging',
      '```md',
      '[in a fence](devall:note/skip.md)',
      '```',
      '[spaced](<devall:note/My Note.md>)',
    ].join('\n');
    expect(extractTargets(md)).toEqual([
      'devall:request/orders/create.req.yaml',
      'devall:note/design/auth.md',
      'http/users/list.req.yaml',
      '../ops/run.md',
      'repo:api/src/main.rs',
      'devall:todo/01JC8Z6Q3W',
      'devall:env/staging',
      'devall:note/My Note.md',
    ]);
  });
});

describe('toDevallLink', () => {
  it('maps devall: links and older project-relative links to one canonical form', () => {
    expect(toDevallLink('DEVALL:request/a.req.yaml')).toBe('devall:request/a.req.yaml');
    expect(toDevallLink('http/users/list.req.yaml')).toBe('devall:request/users/list.req.yaml');
    expect(toDevallLink('../ops/run.md', 'design/auth.md')).toBe('devall:note/ops/run.md');
    expect(toDevallLink('other.md', 'design/auth.md')).toBe('devall:note/design/other.md');
    expect(toDevallLink('queries/orders/top.sql')).toBe('devall:query/orders/top.sql');
    expect(toDevallLink('connections/prod.conn.yaml')).toBe('devall:connection/prod.conn.yaml');
    expect(toDevallLink('environments/staging.env.yaml')).toBe('devall:env/staging');
    expect(toDevallLink('repo:api')).toBe('devall:repo/api');
    expect(toDevallLink('repo:api/src/main.rs')).toBeNull();
    expect(toDevallLink('https://example.com')).toBeNull();
    expect(toDevallLink('C:\\Users\\me\\notes.txt')).toBeNull();
  });
});

describe('buildBacklinkIndex', () => {
  const notes = [
    { path: 'runbook.md', title: 'Runbook', targets: ['devall:request/orders/create.req.yaml', 'http/users/me.req.yaml', 'devall:note/runbook.md', 'devall:todo/T1'] },
    { path: 'design/auth.md', title: 'Auth', targets: ['../runbook.md', 'https://example.com'] },
  ];
  const tasks = [
    { id: 'T1', content: 'Fix order creation', links: ['devall:request/orders/create.req.yaml', 'http/users/me.req.yaml'] },
    { id: 'T2', content: 'Write docs', done: true, description: 'See [the runbook](devall:note/runbook.md)', comments: [{ text: 'and <devall:request/orders/create.req.yaml>' }] },
  ];
  const index = buildBacklinkIndex(notes, tasks, toDevallLink, linkOf);

  it('collects notes and to-dos linking to an item, in either spelling', () => {
    expect(index.get('devall:request/orders/create.req.yaml')?.map((b) => `${b.kind}:${b.id}`)).toEqual(['note:runbook.md', 'todo:T1', 'todo:T2']);
    expect(index.get('devall:request/users/me.req.yaml')?.map((b) => b.title)).toEqual(['Runbook', 'Fix order creation']);
    expect(index.get('devall:todo/T1')?.map((b) => b.id)).toEqual(['runbook.md']);
  });

  it('resolves relative note links, skips self-links and web links', () => {
    expect(index.get('devall:note/runbook.md')?.map((b) => `${b.kind}:${b.id}`)).toEqual(['note:design/auth.md', 'todo:T2']);
    expect(index.get('devall:note/runbook.md')?.find((b) => b.id === 'T2')?.done).toBe(true);
    expect([...index.keys()].some((k) => k.includes('example.com'))).toBe(false);
  });
});

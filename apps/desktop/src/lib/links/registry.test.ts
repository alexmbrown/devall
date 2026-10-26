import { describe, expect, it, vi } from 'vitest';
import { allLinkItems, configureLinks, describe as describeLink, linkTooltip, notifyLinks, openLink, registerLinkProvider, subscribeLinks, type GoFn, type LinkProvider } from './registry';
import { formatLink } from './syntax';

// A fake data source for two kinds: notes resolve from a list, to-dos are "on another machine".
const notes = new Map<string, string | undefined>([['runbook.md', 'Runbook']]);
let loading = false;
const opened: { kind: string; id: string; toSide: boolean }[] = [];
const provider: LinkProvider = {
  kinds: ['note', 'todo'],
  items: () => [...notes].map(([path, title]) => ({ link: formatLink('note', path), kind: 'note' as const, title: title ?? path })),
  resolve(kind, id) {
    if (kind === 'todo') return { unavailable: 'personal to-do — not on this machine' };
    if (loading) return undefined;
    const title = notes.get(id);
    return title ? { title, detail: `notes/${id}` } : null;
  },
  open(kind, id, ctx) {
    opened.push({ kind, id, toSide: ctx.toSide });
    ctx.go('notes', id);
  },
  icon: () => null,
};
registerLinkProvider(provider);

describe('describe', () => {
  it('resolves live titles and every state', () => {
    expect(describeLink('devall:note/runbook.md', 'Old title')).toMatchObject({ state: 'ok', title: 'Runbook', kind: 'note', kindLabel: 'Note', detail: 'notes/runbook.md' });
    notes.set('runbook.md', 'Runbook v2');
    expect(describeLink('devall:note/runbook.md').title).toBe('Runbook v2');
    expect(describeLink('devall:note/gone.md', 'Gone note')).toMatchObject({ state: 'missing', title: 'Gone note', note: 'Note not found' });
    expect(describeLink('devall:todo/01ABC', 'Fix it')).toMatchObject({ state: 'unavailable', title: 'Fix it', note: 'personal to-do — not on this machine' });
    expect(describeLink('devall:env/staging')).toMatchObject({ state: 'unavailable', title: 'staging' });
    expect(describeLink('devall:nope/x', 'Text')).toMatchObject({ state: 'invalid', kind: null, title: 'Text' });
    loading = true;
    expect(describeLink('devall:note/runbook.md', 'Runbook').state).toBe('loading');
    loading = false;
  });

  it('uses the last path segment when the Markdown text is just the link', () => {
    expect(describeLink('devall:note/x/gone.md', 'devall:note/x/gone.md').title).toBe('gone.md');
  });

  it('builds tooltips from kind, path and note', () => {
    expect(linkTooltip(describeLink('devall:todo/1'))).toBe('To-do · 1\npersonal to-do — not on this machine');
  });
});

describe('openLink', () => {
  it('opens through the provider, applying "to the side" to navigation', async () => {
    const go = vi.fn<GoFn>();
    expect(await openLink('devall:note/runbook.md', { go, toSide: true })).toBe(true);
    expect(opened.at(-1)).toEqual({ kind: 'note', id: 'runbook.md', toSide: true });
    expect(go).toHaveBeenCalledWith('notes', 'runbook.md', { toSide: true });
    await openLink('devall:note/runbook.md', { go });
    expect(go).toHaveBeenLastCalledWith('notes', 'runbook.md', undefined);
  });

  it('falls back to the configured navigator and reports links it cannot open', async () => {
    const go = vi.fn<GoFn>();
    const info = vi.fn();
    configureLinks({ go, info });
    await openLink('devall:note/runbook.md');
    expect(go).toHaveBeenCalledWith('notes', 'runbook.md', undefined);
    expect(await openLink('devall:env/staging')).toBe(false);
    expect(await openLink('https://example.com')).toBe(false);
    expect(info).toHaveBeenCalledTimes(2);
  });
});

describe('links naming a project', () => {
  it('resolve here for the open project and open other projects through the configured handler', async () => {
    const go = vi.fn<GoFn>();
    const openInProject = vi.fn(async () => {});
    configureLinks({ go, info: vi.fn(), projectId: () => 'P1', openInProject });
    expect(describeLink('devall:note/runbook.md?project=P1')).toMatchObject({ state: 'ok', title: 'Runbook v2' });
    expect(describeLink('devall:note/runbook.md?project=P2', 'Their runbook')).toMatchObject({ state: 'unavailable', title: 'Their runbook', note: 'in another project' });
    // To-dos aren't project items: the qualifier doesn't matter.
    expect(describeLink('devall:todo/1?project=P2').note).toBe('personal to-do — not on this machine');
    await openLink('devall:note/runbook.md?project=P1');
    expect(go).toHaveBeenCalledWith('notes', 'runbook.md', undefined);
    expect(await openLink('devall:note/runbook.md?project=P2')).toBe(true);
    expect(openInProject).toHaveBeenCalledWith({ kind: 'note', id: 'runbook.md', project: 'P2' });
    expect(go).toHaveBeenCalledTimes(1);
    configureLinks({ go, info: vi.fn() });
  });
});

describe('items and change notification', () => {
  it('lists provider items and batches notifications', async () => {
    expect(allLinkItems().map((i) => i.link)).toContain('devall:note/runbook.md');
    const l = vi.fn();
    const off = subscribeLinks(l);
    notifyLinks();
    notifyLinks();
    await Promise.resolve();
    expect(l).toHaveBeenCalledTimes(1);
    off();
  });
});

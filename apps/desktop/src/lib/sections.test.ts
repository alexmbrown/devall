import { describe, expect, it } from 'vitest';
import { SECTION_IDS, defaultSidebar, moveSidebarGroup, normalizeSidebar, placeSidebarItem, setSectionHidden } from './sections';

const all = (l: ReturnType<typeof defaultSidebar>) => l.groups.flatMap((g) => g.items);

describe('sidebar layout', () => {
  it('defaults to every section once, app sections last', () => {
    const d = defaultSidebar();
    expect(all(d).sort()).toEqual([...SECTION_IDS].sort());
    expect(d.groups.at(-1)?.items).toEqual(['projects', 'settings', 'plugins']);
    expect(d.hidden).toEqual([]);
  });

  it('normalizes persisted data: drops unknown and duplicate ids, adds new sections', () => {
    const n = normalizeSidebar({
      groups: [
        { id: 'main', label: '', items: ['notes', 'http', 'bogus', 'http'] },
        { id: 'main', label: 'dupe', items: ['git'] },
        { id: 'app', label: 'App', items: ['settings'] },
      ],
      hidden: ['git', 'git', 'nope'],
    });
    expect(n.groups.map((g) => g.id)).toEqual(['main', 'app']);
    expect(n.groups[0].items.slice(0, 2)).toEqual(['notes', 'http']);
    expect(all(n).sort()).toEqual([...SECTION_IDS].sort());
    // Missing app sections go back to their default group.
    expect(n.groups[1].items).toEqual(['settings', 'projects', 'plugins']);
    expect(n.hidden).toEqual(['git']);
    expect(normalizeSidebar(null)).toEqual(defaultSidebar());
    expect(normalizeSidebar({ groups: [] })).toEqual(defaultSidebar());
  });

  it('reorders items within and across groups', () => {
    let l = defaultSidebar();
    l = placeSidebarItem(l, 'notes', 'main', { before: 'http' });
    expect(l.groups[0].items[0]).toBe('notes');
    l = placeSidebarItem(l, 'notes', 'main', { after: 'environments' });
    expect(l.groups[0].items.slice(0, 3)).toEqual(['http', 'environments', 'notes']);
    l = placeSidebarItem(l, 'notes', 'app');
    expect(l.groups[1].items.at(-1)).toBe('notes');
    expect(l.groups[0].items).not.toContain('notes');
    expect(all(l).sort()).toEqual([...SECTION_IDS].sort());
    // Dropping onto itself changes nothing.
    expect(placeSidebarItem(l, 'notes', 'app', { before: 'notes' })).toBe(l);
  });

  it('reorders groups and hides sections', () => {
    let l = moveSidebarGroup(defaultSidebar(), 'app', 0);
    expect(l.groups.map((g) => g.id)).toEqual(['app', 'main']);
    l = setSectionHidden(l, 'git', true);
    l = setSectionHidden(l, 'git', true);
    expect(l.hidden).toEqual(['git']);
    l = setSectionHidden(l, 'git', false);
    expect(l.hidden).toEqual([]);
  });
});

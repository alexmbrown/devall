import { describe, expect, it } from 'vitest';
import {
  MAX_GROUPS,
  MAX_LABEL,
  activeGroup,
  activeView,
  allGroups,
  closeView,
  createLayout,
  cycleTab,
  docViews,
  focusGroupIndex,
  goFromView,
  groupOfView,
  keepDoc,
  labelView,
  moveView,
  moveViewToSplit,
  openSection,
  pinView,
  repathDocs,
  resizeSplit,
  restoreLayout,
  serializeLayout,
  splitGroup,
  viewsToClose,
  type Layout,
  type SplitNode,
} from './model';

function gen() {
  let n = 0;
  return () => `id${++n}`;
}

const tabsOf = (l: Layout, groupIndex = 0) => allGroups(l.tree)[groupIndex].tabs.map((id) => `${l.views[id].section}${l.views[id].param ? `:${l.views[id].param}` : ''}${l.views[id].preview ? '*' : ''}${l.views[id].pinned ? '^' : ''}`);

describe('workbench layout: opening sections', () => {
  it('starts with one permanent view', () => {
    const l = createLayout([{ section: 'http' }], gen());
    expect(tabsOf(l)).toEqual(['http']);
    expect(activeView(l)?.section).toBe('http');
  });

  it('opens a new section as a preview tab and replaces it with the next preview', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }], g);
    l = openSection(l, 'utilities', undefined, {}, g);
    expect(tabsOf(l)).toEqual(['http', 'utilities*']);
    l = openSection(l, 'fakedata', undefined, {}, g);
    expect(tabsOf(l)).toEqual(['http', 'fakedata*']);
    expect(activeView(l)?.section).toBe('fakedata');
  });

  it('focuses an existing tab of the section instead of opening another', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }, { section: 'utilities' }], g);
    l = openSection(l, 'http', undefined, {}, g);
    expect(tabsOf(l)).toEqual(['http', 'utilities']);
    expect(activeView(l)?.section).toBe('http');
    // A param navigates the existing (non-document) view.
    l = openSection(l, 'utilities', 'base64', {}, g);
    expect(tabsOf(l)).toEqual(['http', 'utilities:base64']);
  });

  it('newTab opens a permanent second tab; singletons never duplicate', () => {
    const g = gen();
    let l = createLayout([{ section: 'utilities' }, { section: 'mocks' }], g);
    l = openSection(l, 'utilities', undefined, { newTab: true }, g);
    // New tabs open right after the active one.
    expect(tabsOf(l)).toEqual(['utilities', 'utilities', 'mocks']);
    l = openSection(l, 'mocks', undefined, { newTab: true }, g);
    expect(tabsOf(l).filter((t) => t.startsWith('mocks'))).toHaveLength(1);
    expect(activeView(l)?.section).toBe('mocks');
  });

  it('preview false makes an existing preview permanent', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }], g);
    l = openSection(l, 'notes', 'a.md', {}, g);
    expect(tabsOf(l)).toEqual(['http', 'notes:a.md*']);
    l = openSection(l, 'notes', 'a.md', { preview: false }, g);
    expect(tabsOf(l)).toEqual(['http', 'notes:a.md']);
  });

  it('documents: one tab per path, an empty landing view takes the first document', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }], g);
    l = openSection(l, 'http', 'a.req.yaml', {}, g);
    expect(tabsOf(l)).toEqual(['http:a.req.yaml']);
    l = openSection(l, 'http', 'b.req.yaml', {}, g);
    expect(tabsOf(l)).toEqual(['http:a.req.yaml', 'http:b.req.yaml*']);
    l = openSection(l, 'http', 'c.req.yaml', {}, g);
    expect(tabsOf(l)).toEqual(['http:a.req.yaml', 'http:c.req.yaml*']);
    l = openSection(l, 'http', 'a.req.yaml', {}, g);
    expect(activeView(l)?.param).toBe('a.req.yaml');
    expect(tabsOf(l)).toHaveLength(2);
    l = keepDoc(l, 'http', 'c.req.yaml');
    expect(tabsOf(l)).toEqual(['http:a.req.yaml', 'http:c.req.yaml']);
  });

  it('reveals a document already open in another group', () => {
    const g = gen();
    let l = createLayout([{ section: 'http', param: 'a.req.yaml' }], g);
    l = openSection(l, 'http', 'b.req.yaml', { toSide: true }, g);
    expect(allGroups(l.tree)).toHaveLength(2);
    expect(activeGroup(l).id).toBe(allGroups(l.tree)[1].id);
    l = openSection(l, 'http', 'a.req.yaml', {}, g);
    expect(activeGroup(l).id).toBe(allGroups(l.tree)[0].id);
    expect(allGroups(l.tree).flatMap((x) => x.tabs)).toHaveLength(2);
  });

  it('opens to the side: the next group, else a new split on the right', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }], g);
    l = openSection(l, 'utilities', undefined, { toSide: true }, g);
    expect(allGroups(l.tree)).toHaveLength(2);
    expect((l.tree as SplitNode).dir).toBe('row');
    expect(tabsOf(l, 1)).toEqual(['utilities']);
    // Focus the first group and open another to the side: lands in the existing second group.
    l = focusGroupIndex(l, 0);
    l = openSection(l, 'environments', undefined, { toSide: true }, g);
    expect(allGroups(l.tree)).toHaveLength(2);
    expect(tabsOf(l, 1)).toEqual(['utilities', 'environments']);
  });

  it('moves a singleton to the side instead of duplicating it', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }, { section: 'mocks' }], g);
    l = openSection(l, 'mocks', undefined, { toSide: true }, g);
    expect(tabsOf(l, 0)).toEqual(['http']);
    expect(tabsOf(l, 1)).toEqual(['mocks']);
  });
});

describe('workbench layout: navigation from inside a view', () => {
  it('same section navigates the view in place', () => {
    const g = gen();
    let l = createLayout([{ section: 'utilities' }], g);
    const id = activeView(l)!.id;
    l = goFromView(l, id, 'utilities', 'jwt', {}, g);
    expect(tabsOf(l)).toEqual(['utilities:jwt']);
    expect(activeView(l)?.id).toBe(id);
  });

  it('another section opens/focuses a tab and leaves the current view alone', () => {
    const g = gen();
    let l = createLayout([{ section: 'notes', param: 'a.md' }], g);
    const id = activeView(l)!.id;
    l = goFromView(l, id, 'environments', 'dev', {}, g);
    expect(tabsOf(l)).toEqual(['notes:a.md', 'environments:dev*']);
    expect(l.views[id].param).toBe('a.md');
  });

  it('document sections open other documents as tabs, reusing preview views', () => {
    const g = gen();
    let l = createLayout([{ section: 'notes', param: 'a.md' }], g);
    const a = activeView(l)!.id;
    l = goFromView(l, a, 'notes', 'b.md', {}, g);
    expect(tabsOf(l)).toEqual(['notes:a.md', 'notes:b.md*']);
    const b = activeView(l)!.id;
    l = goFromView(l, b, 'notes', 'c.md', {}, g);
    expect(tabsOf(l)).toEqual(['notes:a.md', 'notes:c.md*']);
    expect(activeView(l)?.id).toBe(b);
    l = goFromView(l, b, 'notes', 'c.md', { preview: false }, g);
    expect(tabsOf(l)).toEqual(['notes:a.md', 'notes:c.md']);
    // Clearing the document (e.g. it was deleted) empties the view.
    l = goFromView(l, b, 'notes', undefined, {}, g);
    expect(tabsOf(l)).toEqual(['notes:a.md', 'notes']);
  });
});

describe('workbench layout: closing, pinning, moving', () => {
  it('closes to the most recently used tab and removes emptied groups', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }, { section: 'utilities' }, { section: 'environments' }], g);
    const [h, u, e] = allGroups(l.tree)[0].tabs;
    l = openSection(l, 'environments', undefined, {}, g);
    l = openSection(l, 'http', undefined, {}, g);
    l = closeView(l, h);
    expect(activeView(l)?.id).toBe(e);
    l = closeView(l, e);
    expect(activeView(l)?.id).toBe(u);
    l = openSection(l, 'fakedata', undefined, { toSide: true }, g);
    const side = activeView(l)!.id;
    l = closeView(l, side);
    expect(allGroups(l.tree)).toHaveLength(1);
    expect(l.tree.kind).toBe('group');
    // The last group stays, even when empty.
    l = closeView(l, u);
    expect(allGroups(l.tree)).toHaveLength(1);
    expect(activeView(l)).toBeUndefined();
  });

  it('pins tabs to the front and keeps them out of bulk closes', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }, { section: 'utilities' }, { section: 'environments' }, { section: 'fakedata' }], g);
    const [h, u, e, f] = allGroups(l.tree)[0].tabs;
    l = pinView(l, e, true);
    expect(tabsOf(l)).toEqual(['environments^', 'http', 'utilities', 'fakedata']);
    expect(viewsToClose(l, u, 'others')).toEqual([h, f]);
    expect(viewsToClose(l, h, 'right')).toEqual([u, f]);
    expect(viewsToClose(l, h, 'all')).toEqual([h, u, f]);
    l = pinView(l, e, false);
    expect(tabsOf(l)).toEqual(['environments', 'http', 'utilities', 'fakedata']);
  });

  it('reorders within a group and moves between groups', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }, { section: 'utilities' }, { section: 'environments' }], g);
    const [h, , e] = allGroups(l.tree)[0].tabs;
    l = moveView(l, e, allGroups(l.tree)[0].id, 0);
    expect(tabsOf(l)).toEqual(['environments', 'http', 'utilities']);
    l = moveView(l, e, allGroups(l.tree)[0].id, 2);
    expect(tabsOf(l)).toEqual(['http', 'environments', 'utilities']);
    const s = splitGroup(l, allGroups(l.tree)[0].id, 'down', g);
    l = moveView(s.layout, h, s.groupId!);
    expect((l.tree as SplitNode).dir).toBe('col');
    expect(tabsOf(l, 1)).toEqual(['http']);
    expect(groupOfView(l, h)?.id).toBe(s.groupId);
    // Moving the last tab out removes its group.
    l = moveView(l, h, allGroups(l.tree)[0].id, 0);
    expect(allGroups(l.tree)).toHaveLength(1);
    expect(tabsOf(l)).toEqual(['http', 'environments', 'utilities']);
  });

  it('drops a tab onto a group edge to split, up to the group limit', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }, { section: 'utilities' }, { section: 'environments' }, { section: 'fakedata' }, { section: 'load' }], g);
    const ids = allGroups(l.tree)[0].tabs;
    const first = allGroups(l.tree)[0].id;
    l = moveViewToSplit(l, ids[1], first, 'right', g);
    l = moveViewToSplit(l, ids[2], first, 'down', g);
    l = moveViewToSplit(l, ids[3], first, 'left', g);
    expect(allGroups(l.tree)).toHaveLength(MAX_GROUPS);
    l = moveViewToSplit(l, ids[4], first, 'up', g);
    expect(allGroups(l.tree)).toHaveLength(MAX_GROUPS);
    expect(allGroups(l.tree).flatMap((x) => x.tabs).sort()).toEqual([...ids].sort());
    // The only tab of a group dropped on its own edge changes nothing.
    const lone = allGroups(l.tree).find((x) => x.tabs.length === 1)!;
    expect(moveViewToSplit(l, lone.tabs[0], lone.id, 'right', g)).toBe(l);
  });

  it('flattens same-direction splits and resizes with a minimum', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }], g);
    const first = allGroups(l.tree)[0].id;
    l = splitGroup(l, first, 'right', g).layout;
    l = splitGroup(l, first, 'right', g).layout;
    const root = l.tree as SplitNode;
    expect(root.children).toHaveLength(3);
    expect(root.sizes.reduce((a, b) => a + b, 0)).toBeCloseTo(1);
    l = resizeSplit(l, root.id, [0.9, 0.05, 0.05]);
    const sized = (l.tree as SplitNode).sizes;
    expect(Math.min(...sized)).toBeGreaterThan(0.05);
    expect(sized.reduce((a, b) => a + b, 0)).toBeCloseTo(1);
  });

  it('cycles tabs with wrap-around', () => {
    const g = gen();
    let l = createLayout([{ section: 'http' }, { section: 'utilities' }, { section: 'environments' }], g);
    expect(activeView(l)?.section).toBe('http');
    l = cycleTab(l, -1);
    expect(activeView(l)?.section).toBe('environments');
    l = cycleTab(l, 1);
    expect(activeView(l)?.section).toBe('http');
  });

  it('repaths and finds document views after renames', () => {
    const g = gen();
    let l = createLayout([{ section: 'http', param: 'users/get.req.yaml' }, { section: 'http', param: 'users/_folder.yaml' }, { section: 'http', param: 'other.req.yaml' }], g);
    l = repathDocs(l, 'http', 'users', 'people');
    expect(tabsOf(l)).toEqual(['http:people/get.req.yaml', 'http:people/_folder.yaml', 'http:other.req.yaml']);
    expect(docViews(l, 'http', ['people'])).toHaveLength(2);
  });
});

describe('workbench layout: persistence', () => {
  it('round-trips through JSON', () => {
    const g = gen();
    let l = createLayout([{ section: 'http', param: 'a.req.yaml' }, { section: 'mocks' }], g);
    l = openSection(l, 'notes', 'n.md', { toSide: true }, g);
    l = openSection(l, 'utilities', undefined, {}, g);
    const back = restoreLayout(JSON.parse(JSON.stringify(serializeLayout(l))), g);
    expect(back).toEqual(l);
  });

  it('repairs broken data', () => {
    const g = gen();
    const raw = {
      v: 1,
      activeGroup: 'nope',
      tree: {
        kind: 'split',
        id: 's',
        dir: 'row',
        sizes: [-1, 'x'],
        children: [
          { kind: 'group', id: 'g1', tabs: ['a', 'b', 'missing', 'a', 'm1'], active: 'zzz', mru: ['b', 'x'] },
          { kind: 'group', id: 'g2', tabs: ['m2', 'c'], active: 'c', mru: [] },
          { kind: 'group', id: 'g3', tabs: [], active: null, mru: [] },
        ],
      },
      views: {
        a: { id: 'a', section: 'http', preview: true, pinned: false },
        b: { id: 'b', section: 'utilities', preview: true, pinned: false },
        c: { id: 'c', section: 'bogus', preview: false, pinned: false },
        m1: { id: 'm1', section: 'mocks', preview: false, pinned: true },
        m2: { id: 'm2', section: 'mocks', preview: false, pinned: false },
      },
    };
    const l = restoreLayout(raw, g)!;
    expect(l).not.toBeNull();
    expect(allGroups(l.tree)).toHaveLength(1);
    expect(tabsOf(l)).toEqual(['mocks^', 'http*', 'utilities']);
    expect(activeView(l)?.section).toBe('utilities');
    expect(restoreLayout({ tree: { kind: 'group', id: 'g', tabs: [] }, views: {} }, g)).toBeNull();
    expect(restoreLayout('garbage', g)).toBeNull();
  });

  it('keeps view labels, survives navigation and drops bad ones', () => {
    const g = gen();
    let l = createLayout([{ section: 'utilities', param: 'regex.escape' }], g);
    const id = activeView(l)!.id;
    l = labelView(l, id, 'Regex escape');
    expect(labelView(l, id, 'Regex escape')).toBe(l);
    expect(goFromView(l, id, 'utilities', 'jwt.decode', {}, g).views[id].label).toBe('Regex escape');
    const back = restoreLayout(JSON.parse(JSON.stringify(serializeLayout(l))), g)!;
    expect(back.views[id].label).toBe('Regex escape');
    expect(labelView(l, id, undefined).views[id]).not.toHaveProperty('label');
    const raw = serializeLayout(l);
    raw.views[id] = { ...raw.views[id], label: 42 as unknown as string };
    expect(restoreLayout(raw, g)!.views[id]).not.toHaveProperty('label');
    raw.views[id] = { ...raw.views[id], label: 'x'.repeat(500) };
    expect(restoreLayout(raw, g)!.views[id].label).toHaveLength(MAX_LABEL);
  });
});

describe('workbench layout: drop zones', () => {
  const rect = { left: 0, top: 0, width: 400, height: 200 };
  it('maps pointer positions to edges and the centre', async () => {
    const { dropZone } = await import('./model');
    expect(dropZone(200, 100, rect)).toBe('center');
    expect(dropZone(10, 100, rect)).toBe('left');
    expect(dropZone(395, 100, rect)).toBe('right');
    expect(dropZone(200, 5, rect)).toBe('up');
    expect(dropZone(200, 195, rect)).toBe('down');
    // Corner: the closer edge wins.
    expect(dropZone(4, 40, rect)).toBe('left');
  });

  it('opens a dropped section in a new split, moving singletons', async () => {
    const { openInSplit } = await import('./model');
    const g = gen();
    let l = createLayout([{ section: 'http' }, { section: 'mocks' }], g);
    const first = allGroups(l.tree)[0].id;
    l = openInSplit(l, { section: 'notes', param: 'a.md' }, first, 'right', g);
    expect(tabsOf(l, 1)).toEqual(['notes:a.md']);
    l = openInSplit(l, { section: 'mocks' }, first, 'down', g);
    expect(allGroups(l.tree).flatMap((x) => x.tabs).filter((id) => l.views[id].section === 'mocks')).toHaveLength(1);
    expect(allGroups(l.tree)).toHaveLength(3);
  });
});

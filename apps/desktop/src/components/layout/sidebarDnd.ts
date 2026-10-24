import { useState } from 'react';
import { moveSidebarGroup, placeSidebarItem, type Section } from '@/lib/sections';
import { usePrefs } from '@/store/prefs';
import { setOpenDragData } from '@/components/workbench/Workbench';

const ITEM_MIME = 'application/x-devall-nav-item';
const GROUP_MIME = 'application/x-devall-nav-group';

type Drop = { kind: 'item'; section: Section; side: 'before' | 'after' } | { kind: 'group'; groupId: string; side: 'before' | 'after' } | { kind: 'into'; groupId: string };

/**
 * Drag-and-drop reordering of sidebar items (within and across groups) and of the groups
 * themselves — shared by the sidebar and Settings → Sidebar. Item drags also carry the workbench
 * "open" payload, so dropping an item on an editor group opens it there.
 */
export function useSidebarDnd(opts: { openOnWorkbench?: boolean } = {}) {
  const [drop, setDrop] = useState<Drop | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const update = (fn: (l: ReturnType<typeof usePrefs.getState>['sidebar']) => ReturnType<typeof usePrefs.getState>['sidebar']) =>
    usePrefs.getState().set('sidebar', fn(usePrefs.getState().sidebar));
  const sideOf = (e: React.DragEvent, vertical = true): 'before' | 'after' => {
    const r = e.currentTarget.getBoundingClientRect();
    return vertical ? (e.clientY < r.top + r.height / 2 ? 'before' : 'after') : e.clientX < r.left + r.width / 2 ? 'before' : 'after';
  };
  const has = (e: React.DragEvent, mime: string) => e.dataTransfer.types.includes(mime);
  const end = () => {
    setDrop(null);
    setDragging(null);
  };

  const itemProps = (section: Section, groupId: string) => ({
    draggable: true,
    'data-drop': drop?.kind === 'item' && drop.section === section ? drop.side : undefined,
    'data-dragging': dragging === section ? true : undefined,
    onDragStart: (e: React.DragEvent) => {
      e.dataTransfer.setData(ITEM_MIME, section);
      if (opts.openOnWorkbench) setOpenDragData(e, section);
      e.dataTransfer.effectAllowed = 'copyMove';
      setDragging(section);
    },
    onDragEnd: end,
    onDragOver: (e: React.DragEvent) => {
      if (!has(e, ITEM_MIME)) return;
      e.preventDefault();
      e.stopPropagation();
      const side = sideOf(e);
      if (drop?.kind !== 'item' || drop.section !== section || drop.side !== side) setDrop({ kind: 'item', section, side });
    },
    onDrop: (e: React.DragEvent) => {
      const src = e.dataTransfer.getData(ITEM_MIME) as Section;
      if (!src) return;
      e.preventDefault();
      e.stopPropagation();
      const side = sideOf(e);
      update((l) => placeSidebarItem(l, src, groupId, side === 'before' ? { before: section } : { after: section }));
      end();
    },
  });

  /** A group's header: reorders groups, and accepts items (they go to the group's start). */
  const groupProps = (groupId: string, groupIndex: number, firstItem?: Section) => ({
    draggable: true,
    'data-drop': drop?.kind === 'group' && drop.groupId === groupId ? drop.side : drop?.kind === 'into' && drop.groupId === groupId ? 'into' : undefined,
    onDragStart: (e: React.DragEvent) => {
      e.dataTransfer.setData(GROUP_MIME, groupId);
      e.dataTransfer.effectAllowed = 'move';
      setDragging(groupId);
    },
    onDragEnd: end,
    onDragOver: (e: React.DragEvent) => {
      if (has(e, GROUP_MIME)) {
        e.preventDefault();
        const side = sideOf(e);
        if (drop?.kind !== 'group' || drop.groupId !== groupId || drop.side !== side) setDrop({ kind: 'group', groupId, side });
      } else if (has(e, ITEM_MIME)) {
        e.preventDefault();
        if (drop?.kind !== 'into' || drop.groupId !== groupId) setDrop({ kind: 'into', groupId });
      }
    },
    onDrop: (e: React.DragEvent) => {
      const g = e.dataTransfer.getData(GROUP_MIME);
      const item = e.dataTransfer.getData(ITEM_MIME) as Section;
      if (!g && !item) return;
      e.preventDefault();
      if (g) {
        const side = sideOf(e);
        update((l) => {
          const from = l.groups.findIndex((x) => x.id === g);
          let to = side === 'before' ? groupIndex : groupIndex + 1;
          if (from < to) to -= 1;
          return moveSidebarGroup(l, g, to);
        });
      } else update((l) => placeSidebarItem(l, item, groupId, firstItem ? { before: firstItem } : {}));
      end();
    },
  });

  /** An empty group's body (all its items hidden or moved away): items dropped here join it. */
  const emptyGroupProps = (groupId: string) => ({
    'data-drop': drop?.kind === 'into' && drop.groupId === groupId ? 'into' : undefined,
    onDragOver: (e: React.DragEvent) => {
      if (!has(e, ITEM_MIME)) return;
      e.preventDefault();
      if (drop?.kind !== 'into' || drop.groupId !== groupId) setDrop({ kind: 'into', groupId });
    },
    onDrop: (e: React.DragEvent) => {
      const item = e.dataTransfer.getData(ITEM_MIME) as Section;
      if (!item) return;
      e.preventDefault();
      update((l) => placeSidebarItem(l, item, groupId));
      end();
    },
  });

  return { itemProps, groupProps, emptyGroupProps, dragging: !!dragging, clear: end };
}

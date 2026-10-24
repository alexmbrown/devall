import { Fragment, useEffect, useRef, useState } from 'react';
import { create } from 'zustand';
import clsx from 'clsx';
import { ArrowRightToLine, Columns2, Ellipsis, LayoutGrid, Pin, PinOff, Rows2, X } from 'lucide-react';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { Empty, MOD, Spinner } from '@/components/ui/misc';
import { isSection, isSingleton, type Section } from '@/lib/sections';
import { allGroups, dropZone, viewsToClose, type DropZone, type GroupNode, type LayoutNode, type SplitNode, type View } from '@/lib/workbench/model';
import { useWorkbench } from '@/store/workbench';
import { ViewHost } from './ViewHost';
import { useViewInfo } from './viewInfo';
import './workbench.css';

/** Drag payload MIME types: a workbench tab, or something to open (sidebar item, request tree row). */
export const TAB_MIME = 'application/x-devall-tab';
export const OPEN_MIME = 'application/x-devall-open';

/** Sets the drag payload that opens `section`/`param` when dropped on the workbench. */
export function setOpenDragData(e: React.DragEvent, section: Section, param?: string) {
  e.dataTransfer.setData(OPEN_MIME, JSON.stringify({ section, param }));
  e.dataTransfer.effectAllowed = 'copyMove';
  markDrag({ kind: 'open' });
}

/** Records the drag after dragstart returns (changing the DOM inside dragstart can cancel the drag). */
function markDrag(p: { kind: 'tab' | 'open'; viewId?: string }) {
  setTimeout(() => useDrag.getState().set(p), 0);
}

/** What is being dragged right now (dataTransfer payloads can't be read during dragover). */
const useDrag = create<{ kind: 'tab' | 'open' | null; viewId?: string; set(p: { kind: 'tab' | 'open' | null; viewId?: string }): void }>((set) => ({
  kind: null,
  set: (p) => set({ kind: p.kind, viewId: p.viewId }),
}));

const isWorkbenchDrag = (e: React.DragEvent) => e.dataTransfer.types.includes(TAB_MIME) || e.dataTransfer.types.includes(OPEN_MIME);

function readOpen(e: React.DragEvent): { section: Section; param?: string } | null {
  try {
    const v = JSON.parse(e.dataTransfer.getData(OPEN_MIME) || 'null');
    return v && isSection(v.section) ? { section: v.section, param: typeof v.param === 'string' ? v.param : undefined } : null;
  } catch {
    return null;
  }
}

export function Workbench() {
  const tree = useWorkbench((s) => s.tree);
  // A drag that ends outside any drop target must not leave overlays behind.
  useEffect(() => {
    const end = () => useDrag.getState().set({ kind: null });
    window.addEventListener('dragend', end);
    window.addEventListener('drop', end);
    return () => {
      window.removeEventListener('dragend', end);
      window.removeEventListener('drop', end);
    };
  }, []);
  return (
    <div className="wb">
      <NodeView node={tree} />
    </div>
  );
}

function NodeView({ node }: { node: LayoutNode }) {
  return node.kind === 'group' ? <GroupView group={node} /> : <SplitView split={node} />;
}

function SplitView({ split }: { split: SplitNode }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <div ref={ref} className={clsx('wb-split', split.dir === 'row' ? 'wb-row' : 'wb-col')}>
      {split.children.map((c, i) => (
        <Fragment key={c.id}>
          {i > 0 && <SplitHandle split={split} index={i} container={ref} />}
          <div className="wb-cell" style={{ flex: `${split.sizes[i]} 1 0px` }}>
            <NodeView node={c} />
          </div>
        </Fragment>
      ))}
    </div>
  );
}

/** Drag handle between children `index - 1` and `index` of a split. */
function SplitHandle({ split, index, container }: { split: SplitNode; index: number; container: React.RefObject<HTMLDivElement | null> }) {
  const [drag, setDrag] = useState(false);
  const start = useRef({ pos: 0, sizes: split.sizes });
  const row = split.dir === 'row';
  useEffect(() => {
    if (!drag) return;
    const move = (e: PointerEvent) => {
      const el = container.current;
      if (!el) return;
      const total = row ? el.clientWidth : el.clientHeight;
      if (!total) return;
      const delta = ((row ? e.clientX : e.clientY) - start.current.pos) / total;
      const sizes = [...start.current.sizes];
      const pair = sizes[index - 1] + sizes[index];
      const a = Math.min(pair - 0.1, Math.max(0.1, sizes[index - 1] + delta));
      sizes[index - 1] = a;
      sizes[index] = pair - a;
      useWorkbench.getState().resize(split.id, sizes);
    };
    const up = () => setDrag(false);
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    document.body.style.cursor = row ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';
    return () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
  }, [drag, row, index, split.id, container]);
  return (
    <div
      className={clsx('wb-handle', drag && 'dragging')}
      role="separator"
      aria-orientation={row ? 'vertical' : 'horizontal'}
      onPointerDown={(e) => {
        e.preventDefault();
        start.current = { pos: row ? e.clientX : e.clientY, sizes: split.sizes };
        setDrag(true);
      }}
      onDoubleClick={() => {
        const sizes = [...split.sizes];
        const pair = sizes[index - 1] + sizes[index];
        sizes[index - 1] = sizes[index] = pair / 2;
        useWorkbench.getState().resize(split.id, sizes);
      }}
    />
  );
}

function GroupView({ group }: { group: GroupNode }) {
  const views = useWorkbench((s) => s.views);
  const isActive = useWorkbench((s) => s.activeGroup === group.id);
  const focusGroup = useWorkbench((s) => s.focusGroup);
  const active = group.active ? views[group.active] : undefined;
  return (
    <section
      className={clsx('wb-group', isActive && 'active')}
      onPointerDownCapture={() => !isActive && focusGroup(group.id)}
      onFocusCapture={() => !isActive && focusGroup(group.id)}
      aria-label="Editor group"
    >
      <TabStrip group={group} views={views} />
      <GroupBody group={group}>{active ? <ViewHost key={active.id} view={active} /> : <EmptyGroup group={group} />}</GroupBody>
    </section>
  );
}

/** The group's content area; while a tab is dragged over it, shows where it would land. */
function GroupBody({ group, children }: { group: GroupNode; children: React.ReactNode }) {
  const [zone, setZone] = useState<DropZone | null>(null);
  const dragging = useDrag((s) => s.kind);
  useEffect(() => {
    if (!dragging) setZone(null);
  }, [dragging]);
  const zoneOf = (e: React.DragEvent) => dropZone(e.clientX, e.clientY, e.currentTarget.getBoundingClientRect());
  return (
    <div
      className="wb-body"
      onDragOver={(e) => {
        if (!isWorkbenchDrag(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        const z = zoneOf(e);
        if (z !== zone) setZone(z);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setZone(null);
      }}
      onDrop={(e) => {
        if (!isWorkbenchDrag(e)) return;
        e.preventDefault();
        const z = zoneOf(e);
        setZone(null);
        const wb = useWorkbench.getState();
        const viewId = e.dataTransfer.getData(TAB_MIME);
        if (viewId) {
          if (z === 'center') wb.move(viewId, group.id);
          else wb.moveToSplit(viewId, group.id, z);
        } else {
          const spec = readOpen(e);
          if (!spec) return;
          if (z === 'center') wb.open(spec.section, spec.param, { group: group.id, preview: false });
          else wb.openInSplit(spec, group.id, z);
        }
        useDrag.getState().set({ kind: null });
      }}
    >
      {children}
      {/* Catches drags above the page (iframes, editors) and draws the target area. */}
      {dragging && <div className="wb-drop-catcher" />}
      {zone && <div className={clsx('wb-drop', `z-${zone}`)} />}
    </div>
  );
}

function EmptyGroup({ group }: { group: GroupNode }) {
  const multi = useWorkbench((s) => allGroups(s.tree).length > 1);
  return (
    <Empty
      icon={<LayoutGrid />}
      title="Nothing open here"
      action={
        multi ? (
          <button className="btn btn-sm" onClick={() => void useWorkbench.getState().closeGroup(group.id)}>
            <X /> Close group
          </button>
        ) : undefined
      }
    >
      Pick a module in the sidebar or drag a tab here. <span className="kbd">Ctrl</span>+<span className="kbd">Tab</span> switches tabs, <span className="kbd">{MOD}</span>+<span className="kbd">\</span> splits.
    </Empty>
  );
}

function TabStrip({ group, views }: { group: GroupNode; views: Record<string, View> }) {
  const [dropAt, setDropAt] = useState<number | null>(null);
  const multi = useWorkbench((s) => allGroups(s.tree).length > 1);
  const dragging = useDrag((s) => s.kind);
  useEffect(() => {
    if (!dragging) setDropAt(null);
  }, [dragging]);
  const stripRef = useRef<HTMLDivElement>(null);

  // Keep the active tab scrolled into view (scrolling the strip only: scrollIntoView would also
  // scroll the overflow-hidden layout containers around it).
  useEffect(() => {
    const strip = stripRef.current;
    const el = strip?.querySelector<HTMLElement>('.wb-tab.on');
    if (!strip || !el) return;
    const actions = strip.querySelector<HTMLElement>('.wb-tabs-actions')?.offsetWidth ?? 0;
    if (el.offsetLeft < strip.scrollLeft) strip.scrollLeft = el.offsetLeft;
    else if (el.offsetLeft + el.offsetWidth > strip.scrollLeft + strip.clientWidth - actions) strip.scrollLeft = el.offsetLeft + el.offsetWidth - strip.clientWidth + actions;
  }, [group.active]);

  const drop = (e: React.DragEvent, index: number) => {
    e.preventDefault();
    e.stopPropagation();
    setDropAt(null);
    const wb = useWorkbench.getState();
    const viewId = e.dataTransfer.getData(TAB_MIME);
    if (viewId) wb.move(viewId, group.id, index);
    else {
      const spec = readOpen(e);
      if (spec) wb.open(spec.section, spec.param, { group: group.id, preview: false });
    }
    useDrag.getState().set({ kind: null });
  };

  const groupMenu = (): MenuItemDef[] => [
    { label: 'Split right', icon: <Columns2 />, hint: `${MOD}+\\`, onSelect: () => useWorkbench.getState().split(group.active, 'right') },
    { label: 'Split down', icon: <Rows2 />, onSelect: () => useWorkbench.getState().split(group.active, 'down') },
    { separator: true },
    { label: 'Close all tabs', icon: <X />, disabled: !group.tabs.length, onSelect: () => group.active && void useWorkbench.getState().closeMany(viewsToClose(useWorkbench.getState(), group.active, 'all')) },
    ...(multi ? [{ label: 'Close group', icon: <X />, onSelect: () => void useWorkbench.getState().closeGroup(group.id) }] : []),
  ];

  return (
    <div
      ref={stripRef}
      className="wb-tabs"
      role="tablist"
      onDragOver={(e) => {
        if (!isWorkbenchDrag(e)) return;
        e.preventDefault();
        if (e.target === e.currentTarget) setDropAt(group.tabs.length);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropAt(null);
      }}
      onDrop={(e) => isWorkbenchDrag(e) && drop(e, dropAt ?? group.tabs.length)}
      // The strip has no visible scrollbar: the mouse wheel scrolls it sideways.
      onWheel={(e) => {
        if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) e.currentTarget.scrollLeft += e.deltaY;
      }}
      onDoubleClick={(e) => {
        // Double-click on the empty strip: a new tab of the active view's section (like a browser).
        if (e.target !== e.currentTarget) return;
        const v = group.active ? views[group.active] : undefined;
        if (v && !isSingleton(v.section)) useWorkbench.getState().open(v.section, undefined, { group: group.id, newTab: true });
      }}
    >
      {group.tabs.map((id, i) => {
        const v = views[id];
        if (!v) return null;
        return <TabItem key={id} view={v} index={i} on={group.active === id} dropBefore={dropAt === i} dropAfter={dropAt === group.tabs.length && i === group.tabs.length - 1} onDragPos={setDropAt} onDrop={drop} />;
      })}
      <div className="wb-tabs-actions">
        <button className="icon-btn sm" title={`Split right (${MOD}+\\)`} aria-label="Split right" onClick={() => useWorkbench.getState().split(group.active, 'right')}>
          <Columns2 />
        </button>
        <Menu
          align="right"
          items={groupMenu}
          trigger={(p) => (
            <button className="icon-btn sm" {...p} title="Group actions" aria-label="Group actions">
              <Ellipsis />
            </button>
          )}
        />
      </div>
    </div>
  );
}

function TabItem({
  view,
  index,
  on,
  dropBefore,
  dropAfter,
  onDragPos,
  onDrop,
}: {
  view: View;
  index: number;
  on: boolean;
  dropBefore: boolean;
  dropAfter: boolean;
  onDragPos(i: number | null): void;
  onDrop(e: React.DragEvent, index: number): void;
}) {
  const info = useViewInfo(view);
  const ctx = useContextMenu();
  const wb = useWorkbench.getState;
  const close = () => void wb().close(view.id);

  const menu = (): MenuItemDef[] => [
    ...(view.preview ? [{ label: 'Keep open', icon: <Pin />, onSelect: () => wb().keep(view.id) }] : []),
    view.pinned ? { label: 'Unpin', icon: <PinOff />, onSelect: () => wb().pin(view.id, false) } : { label: 'Pin', icon: <Pin />, onSelect: () => wb().pin(view.id, true) },
    { separator: true },
    { label: 'Open to the side', icon: <ArrowRightToLine />, onSelect: () => wb().toSide(view.id) },
    { label: 'Split right', icon: <Columns2 />, hint: `${MOD}+\\`, onSelect: () => wb().split(view.id, 'right') },
    { label: 'Split down', icon: <Rows2 />, onSelect: () => wb().split(view.id, 'down') },
    { separator: true },
    { label: 'Close', icon: <X />, hint: `${MOD}+W`, onSelect: close },
    { label: 'Close others', disabled: !viewsToClose(wb(), view.id, 'others').length, onSelect: () => void wb().closeMany(viewsToClose(wb(), view.id, 'others')) },
    { label: 'Close to the right', disabled: !viewsToClose(wb(), view.id, 'right').length, onSelect: () => void wb().closeMany(viewsToClose(wb(), view.id, 'right')) },
    { label: 'Close all', onSelect: () => void wb().closeMany(viewsToClose(wb(), view.id, 'all')) },
  ];

  return (
    <>
    <div
      className={clsx('etab wb-tab', on && 'on', view.preview && 'preview', view.pinned && 'pinned', info.dirty && 'dirty', dropBefore && 'drop-before', dropAfter && 'drop-after')}
      role="tab"
      aria-selected={on}
      title={info.tooltip}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData(TAB_MIME, view.id);
        e.dataTransfer.effectAllowed = 'move';
        markDrag({ kind: 'tab', viewId: view.id });
      }}
      onDragEnd={() => useDrag.getState().set({ kind: null })}
      onDragOver={(e) => {
        if (!isWorkbenchDrag(e)) return;
        e.preventDefault();
        e.stopPropagation();
        const r = e.currentTarget.getBoundingClientRect();
        onDragPos(e.clientX < r.left + r.width / 2 ? index : index + 1);
      }}
      onDrop={(e) => {
        if (!isWorkbenchDrag(e)) return;
        const r = e.currentTarget.getBoundingClientRect();
        onDrop(e, e.clientX < r.left + r.width / 2 ? index : index + 1);
      }}
      onClick={() => wb().focusView(view.id)}
      onDoubleClick={() => wb().keep(view.id)}
      onMouseDown={(e) => e.button === 1 && e.preventDefault()}
      onAuxClick={(e) => {
        if (e.button === 1 && !view.pinned) close();
      }}
      onContextMenu={(e) => ctx.open(e, menu())}
    >
      {info.icon}
      <span className="truncate grow wb-tab-title">
        {info.prefix && <span className="wb-tab-prefix">{info.prefix}: </span>}
        {info.title}
      </span>
      {info.busy && <Spinner className="wb-tab-spin" />}
      {view.pinned ? (
        <button
          className="close pin"
          onClick={(e) => {
            e.stopPropagation();
            wb().pin(view.id, false);
          }}
          title="Pinned — click to unpin"
          aria-label="Unpin tab"
        >
          {info.dirty ? <span className="dirty-dot" /> : <Pin />}
        </button>
      ) : (
        <button
          className="close"
          onClick={(e) => {
            e.stopPropagation();
            close();
          }}
          title={`Close (${MOD}+W)`}
          aria-label="Close tab"
        >
          <span className="dirty-dot" />
          <X />
        </button>
      )}
    </div>
    {ctx.node}
    </>
  );
}

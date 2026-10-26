import { useMemo, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { CalendarDays, CalendarRange, CheckCircle2, ChevronDown, Filter, FolderKanban, Hash, Inbox, ListChecks, Lock, Merge, MoreHorizontal, Palette, Pencil, Plus, Tag, Trash2 } from 'lucide-react';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { SearchInput } from '@/components/ui/misc';
import { SidePane } from '@/components/layout/SidePane';
import { confirmDialog } from '@/components/ui/dialogs';
import { moveTasks, updateTasks, useTodos } from '@/store/todos';
import { colorVar, labelColor, openCount, reorderIds, viewKey, type ViewRef } from './lib';
import { openTodoDialog } from './dialogs';
import { LabelColorPopover } from './pickers';
import { beginDrag, endDrag, useTodoUi, type DragItem } from './ui';
import { reorderLists, useDropZone } from './views';

/**
 * The left pane: views, lists, labels and saved filters with counts (the shared `SidePane`).
 */
export function TodoNav() {
  const snap = useTodos((s) => s.snap);
  const view = useTodos((s) => s.view);
  const badge = useTodos((s) => s.badge);
  const search = useTodoUi((s) => s.search);
  const setUi = useTodoUi((s) => s.set);
  const [closed, setClosed] = useState<Record<string, boolean>>({ other: true });
  const [colouring, setColouring] = useState<{ name: string; anchor: DOMRect } | null>(null);
  const ctx = useContextMenu();
  const toggle = (k: string) => setClosed((c) => ({ ...c, [k]: !c[k] }));
  const go = (v: ViewRef) => {
    setUi({ search: '', sel: [], cursor: null, composer: null });
    useTodos.getState().setView(v);
  };
  const tasks = snap?.tasks ?? [];
  const lists = useMemo(() => [...(snap?.lists ?? [])].sort((a, b) => a.order - b.order), [snap]);
  const project = lists.find((l) => l.id === snap?.projectList);
  const mine = lists.filter((l) => l.kind === 'user');
  const others = lists.filter((l) => l.kind === 'project' && l.id !== snap?.projectList);
  const overdue = tasks.some((t) => !t.done && (t.dueLocal ?? t.due?.date ?? '9999') < (snap?.today ?? ''));
  const current = view ? viewKey(view) : '';

  const listMenu = (id: string, name: string, kind: string): MenuItemDef[] => [
    { label: kind === 'user' ? 'Edit list…' : 'Colour…', icon: <Pencil />, onSelect: () => openTodoDialog({ kind: 'list', id }) },
    ...(kind !== 'inbox'
      ? [
          {
            label: 'Delete list…',
            icon: <Trash2 />,
            danger: true,
            onSelect: async () => {
              const n = openCount(tasks, (t) => t.listId === id);
              if (await confirmDialog({ title: `Delete “${name}”?`, message: `${n} open task${n === 1 ? '' : 's'} and its completed tasks will be deleted. This can't be undone.`, confirmText: 'Delete list', danger: true, typeToConfirm: n > 5 ? name : undefined })) {
                if (current === `list:${id}`) go({ kind: 'today' });
                void useTodos.getState().apply({ type: 'deleteList', id }, `Deleted ${name}`);
              }
            },
          },
        ]
      : []),
  ];

  return (
    <SidePane
      title="To-dos"
      label="To-do views"
      className="td-nav"
      actions={
        <button className="icon-btn" onClick={() => setUi({ quickAdd: true })} title="Add task (Q)" aria-label="Add task">
          <Plus />
        </button>
      }
      toolbar={
        <SearchInput
          value={search}
          onChange={(v) => setUi({ search: v })}
          placeholder="Search tasks  /"
          className="grow"
          inputRef={(el) => {
            if (el) el.id = 'td-search';
          }}
        />
      }
      footer={
        <span className="td-nav-foot">
          <Lock /> Personal — on this machine, never committed
        </span>
      }
    >
      <Item icon={<Inbox />} label="Inbox" on={current === 'inbox' && !search} count={openCount(tasks, (t) => t.listId === 'inbox')} onClick={() => go({ kind: 'inbox' })} onDropTasks={(ids) => void moveTasks(ids, { listId: 'inbox', sectionId: null, parentId: null, index: null }, 'Moved to Inbox')} />
      <Item icon={<CalendarDays />} label="Today" on={current === 'today' && !search} count={badge} hot={overdue} onClick={() => go({ kind: 'today' })} onDropTasks={(ids) => void updateTasks(ids, { dueDate: snap?.today ?? '' }, 'Moved to today')} />
      <Item icon={<CalendarRange />} label="Upcoming" on={current === 'upcoming' && !search} onClick={() => go({ kind: 'upcoming' })} />
      <Item icon={<ListChecks />} label="All tasks" on={current === 'all' && !search} count={openCount(tasks, () => true)} onClick={() => go({ kind: 'all' })} />
      <Item icon={<CheckCircle2 />} label="Completed" on={current === 'completed' && !search} onClick={() => go({ kind: 'completed' })} />

      {project && (
        <>
          <Group label="This project" />
          <ListItem l={project} icon={<FolderKanban />} on={current === `list:${project.id}` && !search} count={openCount(tasks, (t) => t.listId === project.id)} go={go} menu={listMenu(project.id, project.name, 'project')} ctx={ctx.open} />
        </>
      )}

      <Group label="My lists" closed={closed.mine} onToggle={() => toggle('mine')} onAdd={() => openTodoDialog({ kind: 'list' })} addTitle="New list" />
      {!closed.mine &&
        mine.map((l) => <ListItem key={l.id} l={l} icon={<Hash />} on={current === `list:${l.id}` && !search} count={openCount(tasks, (t) => t.listId === l.id)} go={go} menu={listMenu(l.id, l.name, 'user')} ctx={ctx.open} />)}
      {!closed.mine && !mine.length && (
        <button className="td-nav-item" onClick={() => openTodoDialog({ kind: 'list' })} style={{ width: 'calc(100% - 2 * var(--space-3))', background: 'none' }}>
          <Plus /> <span className="subtle">New list</span>
        </button>
      )}

      {others.length > 0 && (
        <>
          <Group label="Other projects" closed={closed.other} onToggle={() => toggle('other')} />
          {!closed.other &&
            others.map((l) => <ListItem key={l.id} l={l} icon={<FolderKanban />} on={current === `list:${l.id}` && !search} count={openCount(tasks, (t) => t.listId === l.id)} go={go} menu={listMenu(l.id, l.name, 'project')} ctx={ctx.open} />)}
        </>
      )}

      <Group label="Labels" closed={closed.labels} onToggle={() => toggle('labels')} onAdd={() => openTodoDialog({ kind: 'label' })} addTitle="New label" />
      {!closed.labels &&
        (snap?.labels ?? []).map((l) => {
          const color = labelColor(l.name, snap?.labels);
          const menu: MenuItemDef[] = [
            {
              label: 'Colour…',
              icon: <Palette />,
              onSelect: () => {
                const el = document.querySelector<HTMLElement>(`.td-nav [data-nav="${CSS.escape(`label:${l.name}`)}"]`);
                if (el) setColouring({ name: l.name, anchor: el.getBoundingClientRect() });
              },
            },
            { label: 'Rename…', icon: <Pencil />, onSelect: () => openTodoDialog({ kind: 'label', name: l.name }) },
            { label: 'Merge into…', icon: <Merge />, disabled: (snap?.labels.length ?? 0) < 2, onSelect: () => openTodoDialog({ kind: 'mergeLabel', name: l.name }) },
            { separator: true },
            {
              label: 'Delete label…',
              icon: <Trash2 />,
              danger: true,
              onSelect: async () => {
                if (await confirmDialog({ title: `Delete @${l.name}?`, message: 'It is removed from every task.', confirmText: 'Delete', danger: true })) {
                  if (current === `label:${l.name}`) go({ kind: 'today' });
                  void useTodos.getState().apply({ type: 'deleteLabel', name: l.name }, `Deleted @${l.name}`);
                }
              },
            },
          ];
          return (
            <Item
              key={l.name}
              navKey={`label:${l.name}`}
              icon={<Tag className="td-nav-label" style={{ color: colorVar(color), fill: `color-mix(in srgb, ${colorVar(color)} 30%, transparent)` }} />}
              label={l.name}
              on={current === `label:${l.name}` && !search}
              count={openCount(tasks, (t) => !!t.labels?.includes(l.name))}
              onClick={() => go({ kind: 'label', name: l.name })}
              menu={menu}
              onContext={(e) => ctx.open(e, menu)}
              onDropTasks={(ids) => void updateTasks(ids, { addLabels: [l.name] }, `Labelled @${l.name}`)}
            />
          );
        })}

      <Group label="Filters" closed={closed.filters} onToggle={() => toggle('filters')} onAdd={() => openTodoDialog({ kind: 'filter' })} addTitle="New filter" />
      {!closed.filters &&
        (snap?.filters ?? []).map((f) => {
          const menu: MenuItemDef[] = [
            { label: 'Edit filter…', icon: <Pencil />, onSelect: () => openTodoDialog({ kind: 'filter', id: f.id }) },
            {
              label: 'Delete filter',
              icon: <Trash2 />,
              danger: true,
              onSelect: () => {
                if (current === `filter:${f.id}`) go({ kind: 'today' });
                void useTodos.getState().apply({ type: 'deleteFilter', id: f.id }, `Deleted filter ${f.name}`);
              },
            },
          ];
          return (
            <Item
              key={f.id}
              icon={<Filter style={{ color: colorVar(f.color) }} />}
              label={f.name}
              title={f.error ? `${f.query} — ${f.error}` : f.query}
              on={current === `filter:${f.id}` && !search}
              count={f.error ? undefined : f.count}
              onClick={() => go({ kind: 'filter', id: f.id })}
              menu={menu}
              onContext={(e) => ctx.open(e, menu)}
              reorder={{
                item: { kind: 'filter', id: f.id },
                onItem: (moving, pos) => {
                  const ids = (snap?.filters ?? []).map((x) => x.id);
                  const next = reorderIds(ids, moving, f.id, pos);
                  if (next.join() !== ids.join()) void useTodos.getState().apply({ type: 'reorderFilters', ids: next });
                },
              }}
            />
          );
        })}
      {ctx.node}
      {colouring && <LabelColorPopover name={colouring.name} anchor={colouring.anchor} onClose={() => setColouring(null)} />}
    </SidePane>
  );
}

function Group({ label, closed, onToggle, onAdd, addTitle }: { label: string; closed?: boolean; onToggle?(): void; onAdd?(): void; addTitle?: string }) {
  return (
    <div className="td-nav-group">
      {onToggle ? (
        <button className="td-nav-toggle" onClick={onToggle} aria-expanded={!closed}>
          {label}
          <ChevronDown style={{ transform: closed ? 'rotate(-90deg)' : undefined }} />
        </button>
      ) : (
        label
      )}
      <span className="spacer" />
      {onAdd && (
        <button className="icon-btn sm" onClick={onAdd} title={addTitle} aria-label={addTitle}>
          <Plus />
        </button>
      )}
    </div>
  );
}

function ListItem({ l, icon, on, count, go, menu, ctx }: { l: { id: string; name: string; color?: string; kind: string }; icon: ReactNode; on: boolean; count: number; go(v: ViewRef): void; menu: MenuItemDef[]; ctx(e: React.MouseEvent, items: MenuItemDef[]): void }) {
  return (
    <Item
      icon={<span style={{ color: colorVar(l.color), display: 'inline-flex' }}>{icon}</span>}
      label={l.name}
      on={on}
      count={count}
      onClick={() => go({ kind: 'list', id: l.id })}
      menu={menu}
      onContext={(e) => ctx(e, menu)}
      onDropTasks={(ids) => void moveTasks(ids, { listId: l.id, sectionId: null, parentId: null, index: null }, `Moved to ${l.name}`)}
      reorder={{
        item: { kind: 'list', id: l.id },
        // Lists reorder within their group (my lists, other projects).
        onItem: (moving, pos) => {
          const lists = useTodos.getState().snap?.lists ?? [];
          if (lists.find((x) => x.id === moving)?.kind === l.kind) reorderLists(moving, l.id, pos);
        },
      }}
    />
  );
}

function Item({
  icon,
  label,
  on,
  count,
  hot,
  title,
  onClick,
  menu,
  onContext,
  onDropTasks,
  reorder,
  navKey,
}: {
  icon: ReactNode;
  label: string;
  on: boolean;
  count?: number;
  hot?: boolean;
  title?: string;
  onClick(): void;
  menu?: MenuItemDef[];
  onContext?(e: React.MouseEvent): void;
  onDropTasks?(ids: string[]): void;
  /** Drag to reorder among items of the same kind. */
  reorder?: { item: DragItem; onItem(moving: string, pos: 'before' | 'after'): void };
  /** `data-nav` (popovers anchor to the item). */
  navKey?: string;
}) {
  const zone = useDropZone(onDropTasks, reorder && { kind: reorder.item.kind, onItem: reorder.onItem });
  return (
    <div
      className={clsx('td-nav-item', on && 'on', zone.over && (zone.over === 'into' ? 'drop' : `drop-${zone.over}`))}
      role="button"
      tabIndex={0}
      data-nav={navKey}
      title={title ?? label}
      onClick={onClick}
      onKeyDown={(e) => e.key === 'Enter' && onClick()}
      onContextMenu={onContext}
      draggable={!!reorder}
      onDragStart={reorder ? (e) => beginDrag(e, { item: reorder.item }) : undefined}
      onDragEnd={reorder ? endDrag : undefined}
      {...zone.props}
    >
      {icon}
      <span className="truncate">{label}</span>
      {menu && (
        <Menu
          align="right"
          items={menu}
          trigger={(p) => (
            <button className="icon-btn sm td-nav-more" {...p} onClick={(e) => (e.stopPropagation(), p.onClick(e))} aria-label={`${label} actions`}>
              <MoreHorizontal />
            </button>
          )}
        />
      )}
      {count !== undefined && count > 0 && <span className={clsx('count', hot && 'hot')}>{count}</span>}
    </div>
  );
}

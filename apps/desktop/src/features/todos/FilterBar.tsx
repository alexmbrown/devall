import { useMemo } from 'react';
import clsx from 'clsx';
import { BookmarkPlus, CalendarDays, Check, CircleDot, Filter, Flag, FolderKanban, Hash, Inbox, Info, ListFilter, Paperclip, Tag, X } from 'lucide-react';
import { Segmented } from '@/components/ui/misc';
import type { Priority } from '@/lib/api/todos';
import { useTodos } from '@/store/todos';
import { openTodoDialog } from './dialogs';
import { DUE_FACETS, describeFilter, filterToQuery, HAS_FACETS, hiddenNotices, hiddenText, isFilterActive, viewLists, type Hidden, type TaskFilter } from './filterModel';
import { colorVar, labelColor, type ViewRef } from './lib';
import { LabelChip, PriorityIcon, StatusDot } from './pickers';
import { Popover, usePopover } from './Popover';
import { useBoardPrefs, useTaskFilter, useTodoUi, useViewFilter, useViewOptions } from './ui';

const toggle = <T,>(list: T[], v: T): T[] => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]);

/** One facet's button: icon, name, how many values are picked. */
function FacetButton({ icon, label, count, onClick }: { icon: React.ReactNode; label: string; count: number; onClick(e: React.MouseEvent): void }) {
  return (
    <button className={clsx('btn btn-sm btn-ghost td-facet', count > 0 && 'on')} onClick={onClick} title={label} aria-label={count ? `${label} (${count})` : label}>
      {icon}
      <span className="td-facet-label">{label}</span>
      {count > 0 && <span className="td-facet-count">{count}</span>}
    </button>
  );
}

/** A checkable row in a facet popover. */
function CheckRow({ on, onClick, children }: { on: boolean; onClick(): void; children: React.ReactNode }) {
  return (
    <button className={clsx('td-pop-item', on && 'on')} role="menuitemcheckbox" aria-checked={on} onClick={onClick}>
      {children}
      <span className="grow" />
      {on && <Check />}
    </button>
  );
}

/**
 * The view's filter bar: text (title, description, comments, labels) and facets — labels (any/all),
 * priority, due, status, list, "has…", a saved filter. Matching is instant; non-matching cards and
 * rows hide (board columns stay, so drag and drop still works). `F` focuses it, Esc clears it.
 * Remembered per view for the session.
 */
export function FilterBar({ view, board }: { view: ViewRef; board: boolean }) {
  const snap = useTodos((s) => s.snap)!;
  const f = useViewFilter(view);
  const stats = useTodoUi((s) => s.stats);
  const set = (p: Partial<TaskFilter>) => useTaskFilter.getState().set(view, p);
  const clear = () => useTaskFilter.getState().clear(view);
  const active = isFilterActive(f);
  const labels = usePopover();
  const prio = usePopover();
  const due = usePopover();
  const status = usePopover();
  const lists = usePopover();
  const has = usePopover();
  const saved = usePopover();
  // A single list's view doesn't need the list facet.
  const scope = viewLists(view);
  const manyLists = scope === 'all' || !scope;
  const words = useMemo(() => describeFilter(f, snap), [f, snap]);
  const query = active ? filterToQuery(f, snap) : null;
  return (
    <div className={clsx('td-filterbar', active && 'active')} role="search" aria-label="Filter">
      <div className="input-group td-filter-input">
        <Filter />
        <input
          id="td-filter"
          className="input sm"
          value={f.q}
          placeholder={board ? 'Filter cards…' : 'Filter tasks…'}
          title="Filter this view (F) — title, description, comments, labels; Esc clears"
          onChange={(e) => set({ q: e.target.value })}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === 'Escape') {
              if (active) clear();
              else (e.currentTarget as HTMLInputElement).blur();
            } else if (e.key === 'Enter' || e.key === 'ArrowDown') (e.currentTarget as HTMLInputElement).blur();
          }}
        />
        {f.q && (
          <button className="icon-btn sm input-suffix" onClick={() => set({ q: '' })} aria-label="Clear the text">
            <X />
          </button>
        )}
      </div>
      <FacetButton icon={<Tag />} label="Labels" count={f.labels.length} onClick={labels.toggle} />
      <FacetButton icon={<Flag />} label="Priority" count={f.priorities.length} onClick={prio.toggle} />
      <FacetButton icon={<CalendarDays />} label="Due" count={f.due.length} onClick={due.toggle} />
      <FacetButton icon={<CircleDot />} label="Status" count={f.statuses.length} onClick={status.toggle} />
      {manyLists && <FacetButton icon={<Hash />} label="List" count={f.lists.length} onClick={lists.toggle} />}
      <FacetButton icon={<Paperclip />} label="Has" count={f.has.length} onClick={has.toggle} />
      {snap.filters.length > 0 && <FacetButton icon={<ListFilter />} label="Saved" count={f.saved ? 1 : 0} onClick={saved.toggle} />}
      {active && (
        <span className="td-filter-summary">
          {stats && (
            <span className="td-filter-count" title={`Filtered: ${words.join(', ')}`}>
              {stats.shown} of {stats.total} {stats.unit}
            </span>
          )}
          {query && (
            <button className="icon-btn sm" onClick={() => openTodoDialog({ kind: 'filter', query })} title={`Save as a filter: ${query}`} aria-label="Save as a filter">
              <BookmarkPlus />
            </button>
          )}
          <button className="btn btn-sm btn-ghost td-filter-clear" onClick={clear} title="Clear the filter (Esc in the field)">
            <X /> Clear
          </button>
        </span>
      )}

      {labels.anchor && (
        <Popover anchor={labels.anchor} onClose={labels.close}>
          <div className="td-pop-title">
            <span className="grow">Labels</span>
            <Segmented
              value={f.labelMode}
              onChange={(labelMode) => set({ labelMode })}
              options={[
                { value: 'any', label: 'Any', title: 'Tasks with any of these labels' },
                { value: 'all', label: 'All', title: 'Tasks with all of these labels' },
              ]}
            />
          </div>
          <div className="td-pop-section">
            {snap.labels.map((l) => (
              <CheckRow key={l.name} on={f.labels.includes(l.name)} onClick={() => set({ labels: toggle(f.labels, l.name) })}>
                <LabelChip name={l.name} color={labelColor(l.name, snap.labels)} />
              </CheckRow>
            ))}
            {!snap.labels.length && <div className="subtle xs td-pop-empty">No labels yet.</div>}
          </div>
        </Popover>
      )}
      {prio.anchor && (
        <Popover anchor={prio.anchor} onClose={prio.close}>
          <div className="td-pop-section">
            {([1, 2, 3, 4] as Priority[]).map((p) => (
              <CheckRow key={p} on={f.priorities.includes(p)} onClick={() => set({ priorities: toggle(f.priorities, p) })}>
                <PriorityIcon p={p} /> Priority {p}
              </CheckRow>
            ))}
          </div>
        </Popover>
      )}
      {due.anchor && (
        <Popover anchor={due.anchor} onClose={due.close}>
          <div className="td-pop-section">
            {DUE_FACETS.map((d) => (
              <CheckRow key={d.value} on={f.due.includes(d.value)} onClick={() => set({ due: toggle(f.due, d.value) })}>
                {d.label}
              </CheckRow>
            ))}
          </div>
        </Popover>
      )}
      {status.anchor && (
        <Popover anchor={status.anchor} onClose={status.close}>
          <div className="td-pop-section">
            {snap.settings.columns.map((c) => (
              <CheckRow key={c.id} on={f.statuses.includes(c.id)} onClick={() => set({ statuses: toggle(f.statuses, c.id) })}>
                <StatusDot col={c} /> <span className="truncate">{c.name}</span>
              </CheckRow>
            ))}
          </div>
        </Popover>
      )}
      {lists.anchor && (
        <Popover anchor={lists.anchor} onClose={lists.close}>
          <div className="td-pop-section">
            {snap.lists.map((l) => (
              <CheckRow key={l.id} on={f.lists.includes(l.id)} onClick={() => set({ lists: toggle(f.lists, l.id) })}>
                <span style={{ color: colorVar(l.color), display: 'inline-flex' }}>{l.kind === 'inbox' ? <Inbox /> : l.kind === 'project' ? <FolderKanban /> : <Hash />}</span>
                <span className="truncate">{l.name}</span>
              </CheckRow>
            ))}
          </div>
        </Popover>
      )}
      {has.anchor && (
        <Popover anchor={has.anchor} onClose={has.close}>
          <div className="td-pop-section">
            {HAS_FACETS.map((h) => (
              <CheckRow key={h.value} on={f.has.includes(h.value)} onClick={() => set({ has: toggle(f.has, h.value) })}>
                {h.label}
              </CheckRow>
            ))}
          </div>
        </Popover>
      )}
      {saved.anchor && (
        <Popover anchor={saved.anchor} onClose={saved.close}>
          <div className="td-pop-title subtle xs">Only tasks a saved filter finds (open tasks)</div>
          <div className="td-pop-section">
            {snap.filters.map((sf) => (
              <CheckRow key={sf.id} on={f.saved === sf.id} onClick={() => set({ saved: f.saved === sf.id ? null : sf.id })}>
                <Filter style={{ color: colorVar(sf.color) }} />
                <span className="truncate" title={sf.query}>
                  {sf.name}
                </span>
              </CheckRow>
            ))}
          </div>
        </Popover>
      )}
    </div>
  );
}

/**
 * What the view leaves out, each with a one-click way to show it: "3 completed tasks hidden · Show",
 * "12 tasks completed more than a week ago not loaded · Load", folded sub-tasks, sub-tasks inside
 * cards, completions outside the done column's window.
 */
export function HiddenNotices({ view }: { view: ViewRef }) {
  const stats = useTodoUi((s) => s.stats);
  const items = hiddenNotices(stats?.hidden ?? []);
  if (!items.length) return null;
  return (
    <div className="td-notices" role="status">
      {items.map((h) => (
        <span key={h.kind} className="td-notice">
          <Info />
          <span className="truncate">{hiddenText(h)}</span>
          <button className="td-notice-act" onClick={() => reveal(h, view)}>
            {actionLabel(h)}
          </button>
        </span>
      ))}
    </div>
  );
}

function actionLabel(h: Hidden): string {
  switch (h.kind) {
    case 'done':
      return 'Show';
    case 'older':
      return 'Load';
    case 'folded':
      return 'Expand';
    case 'nested':
      return 'Show as cards';
    case 'window':
      return 'Show all';
  }
}

/** Shows what a notice says is hidden. */
function reveal(h: Hidden, view: ViewRef) {
  const todos = useTodos.getState();
  switch (h.kind) {
    case 'done':
      useTodoUi.getState().set({ showDone: true });
      // Completed tasks older than a week aren't in the snapshot: load them too.
      void todos.setAllDone(true);
      break;
    case 'older':
      void todos.setAllDone(true);
      break;
    case 'folded':
      if (h.parents.length) void todos.apply({ type: 'update', ids: h.parents, patch: { collapsed: false } });
      break;
    case 'nested':
      useViewOptions.getState().set(view, { subtasks: 'cards' });
      break;
    case 'window':
      useBoardPrefs.getState().set({ doneDays: 0 });
      void todos.setAllDone(true);
      break;
  }
}

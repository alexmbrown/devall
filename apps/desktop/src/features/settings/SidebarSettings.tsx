import clsx from 'clsx';
import { ChevronDown, ChevronUp, GripVertical, RotateCcw } from 'lucide-react';
import { Switch } from '@/components/ui/misc';
import { SectionIcon } from '@/components/layout/sectionIcons';
import { useSidebarDnd } from '@/components/layout/sidebarDnd';
import { defaultSidebar, moveSidebarGroup, sectionMeta, setSectionHidden } from '@/lib/sections';
import { usePrefs } from '@/store/prefs';

/** Settings → Sidebar: show/hide sections, drag to reorder items and groups, reset. */
export function SidebarSettings() {
  const sidebar = usePrefs((s) => s.sidebar);
  const set = (v: typeof sidebar) => usePrefs.getState().set('sidebar', v);
  const dnd = useSidebarDnd();
  const hidden = new Set(sidebar.hidden);

  return (
    <section className="section" id="settings-sidebar">
      <div className="section-head">
        <h3>Sidebar</h3>
        <span className="spacer" />
        <button className="btn btn-sm btn-ghost" onClick={() => set(defaultSidebar())} title="Default order, everything shown">
          <RotateCcw /> Reset to default
        </button>
      </div>
      <p className="subtle xs" style={{ margin: 0 }}>
        Drag to reorder (also works in the sidebar itself). Hidden modules stay one click away under <b>More…</b> at the bottom of the sidebar, in the command palette and in links.
      </p>
      <div className="sb-groups">
        {sidebar.groups.map((g, gi) => (
          <div key={g.id} className="sb-group">
            <div className="sb-group-head" {...dnd.groupProps(g.id, gi, g.items[0])}>
              <GripVertical size={13} className="sb-grip" />
              <span className="grow">{g.label || 'Modules'}</span>
              <button className="icon-btn sm" disabled={gi === 0} onClick={() => set(moveSidebarGroup(sidebar, g.id, gi - 1))} title="Move group up" aria-label="Move group up">
                <ChevronUp />
              </button>
              <button className="icon-btn sm" disabled={gi === sidebar.groups.length - 1} onClick={() => set(moveSidebarGroup(sidebar, g.id, gi + 1))} title="Move group down" aria-label="Move group down">
                <ChevronDown />
              </button>
            </div>
            {g.items.map((s) => (
              <div key={s} className={clsx('sb-row', hidden.has(s) && 'off')} {...dnd.itemProps(s, g.id)}>
                <GripVertical className="sb-grip" />
                <SectionIcon section={s} />
                <span className="grow sb-label small">{sectionMeta(s).label}</span>
                <Switch checked={!hidden.has(s)} onChange={(v) => set(setSectionHidden(sidebar, s, !v))} title={hidden.has(s) ? 'Hidden — shown under More…' : 'Shown in the sidebar'} />
              </div>
            ))}
            {!g.items.length && (
              <div className="sb-empty" {...dnd.emptyGroupProps(g.id)}>
                Drag modules here
              </div>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}

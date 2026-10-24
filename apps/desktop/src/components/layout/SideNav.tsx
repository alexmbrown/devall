import { useEffect } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { ArrowRightToLine, Ellipsis, EyeOff, Plus, Settings2, SquareArrowOutUpRight, Wrench, X } from 'lucide-react';
import clsx from 'clsx';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { MOD } from '@/components/ui/misc';
import { isSingleton, sectionMeta, setSectionHidden, type Section } from '@/lib/sections';
import { useTodos } from '@/store/todos';
import { usePrefs } from '@/store/prefs';
import { activeViewOf, useWorkbench } from '@/store/workbench';
import { useWorkspace } from '@/store/workspace';
import { SectionIcon } from './sectionIcons';
import { useSidebarDnd } from './sidebarDnd';

/** The To-dos badge (the to-dos store decides what it counts). The only place the sidebar reads that store. */
function useTodoBadge(): number | undefined {
  const project = useWorkspace((s) => s.project);
  const openTodos = useTodos((s) => s.badge);
  const todoRoot = useTodos((s) => s.root);
  useEffect(() => {
    // Keep the badge in sync with the open project's personal list.
    if (project && todoRoot !== project.root) useTodos.getState().load(project.root);
  }, [project, todoRoot]);
  return openTodos || undefined;
}

/** Opens a section from the sidebar: focus its tab or open a preview (Ctrl/Cmd or middle click: a new tab). */
export function openSection(section: Section, opts: { newTab?: boolean; toSide?: boolean } = {}) {
  useWorkbench.getState().open(section, undefined, opts);
}

/** Sidebar menu entries shared by the sidebar and the "More…" list. */
export function sectionMenu(section: Section, hidden: boolean): MenuItemDef[] {
  const views = Object.values(useWorkbench.getState().views).filter((v) => v.section === section);
  const singleton = isSingleton(section);
  return [
    { label: 'Open', icon: <SquareArrowOutUpRight />, onSelect: () => openSection(section) },
    ...(!singleton ? [{ label: 'Open in new tab', icon: <Plus />, hint: `${MOD}+click`, onSelect: () => openSection(section, { newTab: true }) }] : []),
    { label: 'Open to the side', icon: <ArrowRightToLine />, onSelect: () => openSection(section, { toSide: true }) },
    ...(views.length
      ? [{ label: views.length > 1 ? `Close ${views.length} tabs` : 'Close tab', icon: <X />, onSelect: () => void useWorkbench.getState().closeMany(views.map((v) => v.id)) }]
      : []),
    { separator: true },
    hidden
      ? { label: 'Show in sidebar', icon: <EyeOff />, onSelect: () => usePrefs.getState().set('sidebar', setSectionHidden(usePrefs.getState().sidebar, section, false)) }
      : { label: 'Hide from sidebar', icon: <EyeOff />, onSelect: () => usePrefs.getState().set('sidebar', setSectionHidden(usePrefs.getState().sidebar, section, true)) },
    { label: 'Customize sidebar…', icon: <Settings2 />, onSelect: () => useWorkbench.getState().open('settings', 'sidebar') },
  ];
}

function countRequests(n: ReturnType<typeof useWorkspace.getState>['tree']): number {
  return !n ? 0 : n.kind === 'request' ? 1 : n.children.reduce((a, c) => a + countRequests(c), 0);
}

export function SideNav() {
  const sidebar = usePrefs((s) => s.sidebar);
  const activeSection = useWorkbench((s) => activeViewOf(s)?.section);
  const openSections = useWorkbench(useShallow((s) => [...new Set(Object.values(s.views).map((v) => v.section))].sort()));
  const project = useWorkspace((s) => s.project);
  const requests = useWorkspace((s) => countRequests(s.tree));
  const todos = useTodoBadge();
  const ctx = useContextMenu();
  const dnd = useSidebarDnd({ openOnWorkbench: true });

  const counts: Partial<Record<Section, number | undefined>> = {
    http: requests || undefined,
    environments: project?.environments.length || undefined,
    todos,
  };
  const hidden = new Set(sidebar.hidden);
  const hiddenList = sidebar.groups.flatMap((g) => g.items).filter((s) => hidden.has(s));

  const item = (id: Section, groupId: string) => {
    const meta = sectionMeta(id);
    return (
      <button
        key={id}
        className={clsx('nav-item', activeSection === id && 'on', openSections.includes(id) && 'open')}
        onClick={(e) => openSection(id, { newTab: e.ctrlKey || e.metaKey })}
        onAuxClick={(e) => e.button === 1 && openSection(id, { newTab: true })}
        onMouseDown={(e) => e.button === 1 && e.preventDefault()}
        onContextMenu={(e) => ctx.open(e, sectionMenu(id, false))}
        title={`${meta.title ?? meta.label} — ${MOD}+click opens a new tab, right-click for more`}
        {...dnd.itemProps(id, groupId)}
      >
        <SectionIcon section={id} />
        <span>{meta.label}</span>
        {counts[id] !== undefined && <span className="count">{counts[id]}</span>}
      </button>
    );
  };

  return (
    <nav className={clsx('sidenav', dnd.dragging && 'dragging')} aria-label="Modules" onDragLeave={(e) => !e.currentTarget.contains(e.relatedTarget as Node | null) && dnd.clear()}>
      {sidebar.groups.map((g, gi) => {
        const visible = g.items.filter((s) => !hidden.has(s));
        if (!visible.length && !dnd.dragging) return null;
        return (
          <div key={g.id} className="nav-section">
            {g.label && (
              <div className="nav-group" {...dnd.groupProps(g.id, gi, visible[0])}>
                {g.label}
              </div>
            )}
            {visible.map((id) => item(id, g.id))}
            {!visible.length && (
              <div className="nav-drop-empty" {...dnd.emptyGroupProps(g.id)}>
                Drop here
              </div>
            )}
          </div>
        );
      })}
      {hiddenList.length > 0 && (
        <Menu
          items={() => [
            { heading: 'Hidden from the sidebar' },
            ...hiddenList.map((s) => ({ label: sectionMeta(s).label, icon: <SectionIcon section={s} />, onSelect: () => openSection(s) })),
            { separator: true },
            { label: 'Customize sidebar…', icon: <Settings2 />, onSelect: () => useWorkbench.getState().open('settings', 'sidebar') },
          ]}
          trigger={(p) => (
            <button className={clsx('nav-item nav-more', hiddenList.includes(activeSection as Section) && 'on')} {...p} title={`More: ${hiddenList.map((s) => sectionMeta(s).label).join(', ')}`}>
              <Ellipsis />
              <span>More…</span>
              <span className="count">{hiddenList.length}</span>
            </button>
          )}
        />
      )}
      <div className="spacer" />
      <div className="nav-foot">
        <Wrench size={12} /> Free &amp; local · no account
      </div>
      {ctx.node}
    </nav>
  );
}

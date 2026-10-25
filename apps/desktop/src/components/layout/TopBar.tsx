import clsx from 'clsx';
import { ArrowRightLeft, Check, ChevronDown, Command, Cookie, FolderKanban, FolderOpen, GitBranch, Layers, Monitor, Moon, NotebookPen, PanelLeft, Play, RefreshCw, Settings2, Sun, X } from 'lucide-react';
import { WINDOW_CHROME, useWindowState } from '@/lib/windowChrome';
import { WindowControls } from './WindowControls';
import { useMoveToProject } from '@/features/projects/MoveToProject';
import { openReposDialog } from '@/features/git/dialogs';
import { useRunner } from '@/features/http/RunnerModal';
import { useSessionModal } from '@/features/http/SessionModal';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { EnvChip } from '@/components/EnvChip';
import { Logo } from '@/components/Logo';
import { MOD } from '@/components/ui/misc';
import { usePalette } from '@/features/palette/CommandPalette';
import { usePrefs, type Theme } from '@/store/prefs';
import { useNav } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';

export function TopBar() {
  const project = useWorkspace((s) => s.project);
  const refresh = useWorkspace((s) => s.refresh);
  const setActiveEnv = useWorkspace((s) => s.setActiveEnv);
  const openScratch = useWorkspace((s) => s.openScratch);
  const closeProject = useWorkspace((s) => s.closeProject);
  const envId = usePrefs((s) => (project ? s.activeEnv[project.root] : undefined));
  const theme = usePrefs((s) => s.theme);
  const setPref = usePrefs((s) => s.set);
  const go = useNav((s) => s.go);
  const openPalette = usePalette((s) => s.setOpen);
  const env = project?.environments.find((e) => e.id === envId);
  const win = useWindowState();

  const projectItems = (): MenuItemDef[] => [
    { heading: project?.isScratch ? 'Scratch pad' : 'Project' },
    { label: <span className="mono xs truncate">{project?.root}</span>, disabled: true },
    { label: 'Reload from disk', icon: <RefreshCw />, onSelect: refresh },
    { label: 'Run all requests…', icon: <Play />, onSelect: () => useRunner.getState().open('') },
    { label: 'Cookies & captured variables…', icon: <Cookie />, onSelect: () => useSessionModal.getState().setOpen(true) },
    { label: project?.isScratch ? 'Move to project…' : 'Copy to another project…', icon: <ArrowRightLeft />, onSelect: () => useMoveToProject.getState().show() },
    ...(!project?.isScratch ? [{ label: 'Repositories…', icon: <GitBranch />, onSelect: openReposDialog }] : []),
    { separator: true },
    { label: 'Open or create project…', icon: <FolderOpen />, onSelect: () => go('projects') },
    ...(!project?.isScratch ? [{ label: 'Switch to scratch pad', icon: <NotebookPen />, onSelect: () => openScratch() }] : []),
    { label: 'Close project', icon: <X />, onSelect: closeProject },
  ];

  const envItems = (): MenuItemDef[] => [
    { heading: 'Environment' },
    { label: 'No environment', hint: !env ? <Check size={13} /> : undefined, onSelect: () => setActiveEnv(null) },
    ...(project?.environments ?? []).map((e) => ({
      label: (
        <span className="row-sm">
          <EnvChip env={e.file} />
          {e.file.name}
        </span>
      ),
      hint: e.id === envId ? <Check size={13} /> : e.file.protected ? 'protected' : undefined,
      onSelect: () => setActiveEnv(e.id),
    })),
    { separator: true },
    { label: 'Manage environments…', icon: <Settings2 />, onSelect: () => go('environments', envId) },
  ];

  const themeIcon = { system: <Monitor />, light: <Sun />, dark: <Moon /> }[theme];
  const nextTheme: Record<Theme, Theme> = { system: 'light', light: 'dark', dark: 'system' };

  return (
    // The top bar is the window's title bar (Windows, macOS): its empty parts drag the window and a
    // double-click maximizes; "deep" covers children but skips buttons and other controls.
    <header
      className={clsx('topbar', `chrome-${WINDOW_CHROME}`, win.fullscreen && 'is-fullscreen', !win.focused && 'is-blurred')}
      data-tauri-drag-region={WINDOW_CHROME === 'native' ? undefined : 'deep'}
    >
      <button className="icon-btn" onClick={() => setPref('sidebarCollapsed', !usePrefs.getState().sidebarCollapsed)} title="Toggle sidebar" aria-label="Toggle sidebar">
        <PanelLeft />
      </button>
      <div className="brand" title="/dev/all">
        <Logo height={22} />
      </div>
      {project && (
        <>
          <Menu
            items={projectItems}
            trigger={(p) => (
              <button className="switcher" {...p} title={project.root}>
                {project.isScratch ? <NotebookPen /> : <FolderKanban />}
                <span className="truncate strong">{project.manifest.name}</span>
                <ChevronDown />
              </button>
            )}
          />
          <Menu
            items={envItems}
            trigger={(p) => (
              <button className="switcher" {...p} title="Active environment">
                <Layers />
                {env ? (
                  <>
                    <EnvChip env={env.file} />
                    <span className="truncate">{env.file.name}</span>
                  </>
                ) : (
                  <span className="subtle">No environment</span>
                )}
                <ChevronDown />
              </button>
            )}
          />
          {env?.file.protected && <span className="badge warning">PROTECTED</span>}
        </>
      )}
      <div className="spacer" />
      <button className="btn btn-sm btn-subtle" onClick={() => openPalette(true)} title="Command palette">
        <Command /> Search &amp; actions <span className="kbd">{MOD}+K</span>
      </button>
      <button className="icon-btn" onClick={() => setPref('theme', nextTheme[theme])} title={`Theme: ${theme} (click to change)`} aria-label="Toggle theme">
        {themeIcon}
      </button>
      {WINDOW_CHROME === 'windows' && <WindowControls maximized={win.maximized} />}
    </header>
  );
}

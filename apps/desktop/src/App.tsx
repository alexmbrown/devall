import { useEffect } from 'react';
import clsx from 'clsx';
import { TopBar } from '@/components/layout/TopBar';
import { SideNav } from '@/components/layout/SideNav';
import { Workbench } from '@/components/workbench/Workbench';
import { Toaster } from '@/components/ui/toast';
import { DialogHost } from '@/components/ui/dialogs';
import { CommandPalette } from '@/features/palette/CommandPalette';
import { RunnerModal } from '@/features/http/RunnerModal';
import { SessionModal } from '@/features/http/SessionModal';
import { ImportExportHost } from '@/features/import';
import { OpenApiHost } from '@/features/openapi';
import { MoveToProjectDialog } from '@/features/projects/MoveToProject';
import { McpApprovalHost } from '@/features/ai/McpApprovalHost';
import { LocalHistoryHost } from '@/features/history/LocalHistoryDialog';
import { initLocalHistoryEvents } from '@/lib/api/history';
import { initAiDownloads } from '@/features/ai/downloads';
import { setLogViewerOpener } from '@/features/infra/logViewer';
import { openInLogViewer } from '@/features/logs';

// Container/pod logs from the Docker & K8s page open in the log viewer.
setLogViewerOpener(({ name, text }) => openInLogViewer({ type: 'text', name, text }));
import { useGlobalShortcuts } from '@/lib/shortcuts';
import { isTauri } from '@/lib/ipc';
import { usePrefs, resolveTheme } from '@/store/prefs';
import { useWorkspace } from '@/store/workspace';
import { useVarsVersion } from '@/store/variables';
import { usePluginCatalog } from '@/lib/api/plugins';

/** Keeps the plugin catalog (utilities, importers, codegen, auth providers) in sync with the open project. */
function usePluginCatalogSync() {
  const root = useWorkspace((s) => s.project?.root ?? null);
  useEffect(() => {
    if (isTauri) void usePluginCatalog.getState().refresh(root);
  }, [root]);
}

function useThemeSync() {
  const theme = usePrefs((s) => s.theme);
  const density = usePrefs((s) => s.density);
  useEffect(() => {
    const apply = () => (document.documentElement.dataset.theme = resolveTheme(theme));
    apply();
    if (theme !== 'system') return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', apply);
    return () => mq.removeEventListener('change', apply);
  }, [theme]);
  useEffect(() => {
    document.documentElement.dataset.density = density;
  }, [density]);
}

/** Pick up edits made outside the app (git pull, editor) when the window regains focus. */
function useRefreshOnFocus() {
  useEffect(() => {
    const onFocus = () => {
      useWorkspace.getState().refresh();
      useVarsVersion.getState().bump();
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, []);
}

export function App() {
  useThemeSync();
  useGlobalShortcuts();
  useRefreshOnFocus();
  usePluginCatalogSync();
  const collapsed = usePrefs((s) => s.sidebarCollapsed);

  useEffect(() => {
    if (!isTauri) return;
    useWorkspace.getState().init();
    // Model / runtime downloads report here whichever view started them.
    initAiDownloads();
    // Local file history changes (scan on open, watcher, restores) refresh open history views.
    initLocalHistoryEvents();
  }, []);

  if (!isTauri)
    return (
      <div className="empty">
        <h3>/dev/all runs as a desktop app</h3>
        <p>
          Start it with <code className="code-inline">npm run app</code> in <code className="code-inline">apps/desktop</code>.
        </p>
      </div>
    );

  return (
    <div className={clsx('app', collapsed && 'nav-collapsed')}>
      <TopBar />
      <SideNav />
      <main className="main">
        <Workbench />
      </main>
      <CommandPalette />
      <RunnerModal />
      <SessionModal />
      <ImportExportHost />
      <OpenApiHost />
      <MoveToProjectDialog />
      <McpApprovalHost />
      <LocalHistoryHost />
      <DialogHost />
      <Toaster />
    </div>
  );
}

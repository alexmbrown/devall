import { useEffect, useRef } from 'react';
import clsx from 'clsx';
import { Image as TauriImage } from '@tauri-apps/api/image';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { ACCENT_TOKEN_NAMES, DEFAULT_ACCENT, accentTokens, renderAppIcon, validAccent } from '@/lib/accent';
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
import { GitDialogsHost } from '@/features/git/dialogs';
import { NewProjectHost } from '@/features/projects/NewProjectDialog';
import { McpApprovalHost } from '@/features/ai/McpApprovalHost';
import { LocalHistoryHost } from '@/features/history/LocalHistoryDialog';
import { initLocalHistoryEvents } from '@/lib/api/history';
import { initAiDownloads } from '@/features/ai/downloads';
import { setLogViewerOpener } from '@/features/infra/logViewer';
import { openInLogViewer } from '@/features/logs';
import { LinkPickerHost } from '@/components/links/LinkPicker';
import { registerLinkProviders } from '@/lib/links/providers';
import { initIncomingLinks } from '@/lib/links/incoming';

// `devall:` links (notes, to-dos, agents) resolve and open through the features' link providers.
registerLinkProviders();

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

/**
 * The accent colour (Settings → Appearance): its tokens for the current theme on <html> (overriding
 * tokens.css), and the window/taskbar icon repainted in it. The bundled icon is the default indigo,
 * so the icon is only repainted once a custom colour was chosen (and back when it's reset).
 */
function useAccentSync() {
  const accent = usePrefs((s) => validAccent(s.accentColor));
  useEffect(() => {
    const root = document.documentElement;
    const apply = () => {
      for (const name of ACCENT_TOKEN_NAMES) root.style.removeProperty(name);
      if (!accent) return;
      for (const [name, value] of Object.entries(accentTokens(accent, root.dataset.theme === 'dark' ? 'dark' : 'light'))) root.style.setProperty(name, value);
    };
    apply();
    const observer = new MutationObserver(apply);
    observer.observe(root, { attributes: true, attributeFilter: ['data-theme'] });
    return () => observer.disconnect();
  }, [accent]);

  const painted = useRef(false);
  useEffect(() => {
    if (!isTauri || (!accent && !painted.current)) return;
    painted.current = true;
    // Debounced: the colour picker reports every step of a drag.
    const timer = setTimeout(async () => {
      try {
        const { rgba, size } = await renderAppIcon(accent ?? DEFAULT_ACCENT);
        await getCurrentWindow().setIcon(await TauriImage.new(rgba, size, size));
      } catch {
        /* no window icon on this platform (macOS uses the bundle's dock icon) */
      }
    }, 250);
    return () => clearTimeout(timer);
  }, [accent]);
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
  useAccentSync();
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
    // `devall:` links from outside the app (browser, chat, other editors) — after the workspace opens.
    initIncomingLinks();
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
      <GitDialogsHost />
      <NewProjectHost />
      <McpApprovalHost />
      <LocalHistoryHost />
      <LinkPickerHost />
      <DialogHost />
      <Toaster />
    </div>
  );
}

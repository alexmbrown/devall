import { lazy, memo, Suspense, type ComponentType, type LazyExoticComponent } from 'react';
import { ErrorBoundary } from '@/components/layout/ErrorBoundary';
import { Spinner } from '@/components/ui/misc';
import { sectionMeta, type Section } from '@/lib/sections';
import { ViewContext } from '@/store/nav';
import type { View } from '@/store/workbench';
import { useWorkspace } from '@/store/workspace';

const ProjectsPage = lazy(() => import('@/features/projects/ProjectsPage'));
const RoadmapPage = lazy(() => import('@/features/roadmap/RoadmapPage'));

/**
 * The page component of each section. A page renders once per view (tab) and reads its route with
 * `useNav` — see docs/DEVELOPING.md ("Pages, views and navigation").
 */
const PAGES: Record<Section, LazyExoticComponent<ComponentType>> = {
  http: lazy(() => import('@/features/http/HttpPage')),
  environments: lazy(() => import('@/features/environments/EnvironmentsPage')),
  utilities: lazy(() => import('@/features/utilities/UtilitiesPage')),
  projects: ProjectsPage,
  settings: lazy(() => import('@/features/settings/SettingsPage')),
  notes: lazy(() => import('@/features/notes/NotesPage')),
  todos: lazy(() => import('@/features/todos/TodosPage')),
  fakedata: lazy(() => import('@/features/data/DataPage')),
  git: lazy(() => import('@/features/git/GitPage')),
  load: lazy(() => import('@/features/load/LoadPage')),
  ai: lazy(() => import('@/features/ai/AiPage')),
  inspector: lazy(() => import('@/features/inspector/InspectorPage')),
  tunnels: lazy(() => import('@/features/tunnels/TunnelsPage')),
  logs: lazy(() => import('@/features/logs/LogsPage')),
  history: lazy(() => import('@/features/history/HistoryPage')),
  infra: lazy(() => import('@/features/infra/InfraPage')),
  data: lazy(() => import('@/features/db/DbPage')),
  webhooks: lazy(() => import('@/features/webhooks/WebhooksPage')),
  mocks: lazy(() => import('@/features/mocks/MocksPage')),
  plugins: lazy(() => import('@/features/plugins/PluginsPage')),
};

const loadingView = (
  <div className="empty">
    <Spinner />
  </div>
);

/** Renders one view: its page inside the view's navigation context, error boundary and suspense. */
export const ViewHost = memo(function ViewHost({ view }: { view: View }) {
  const project = useWorkspace((s) => s.project);
  const ready = useWorkspace((s) => s.ready);
  const Page = !ready ? null : sectionMeta(view.section).needsProject && !project ? ProjectsPage : (PAGES[view.section] ?? RoadmapPage);
  return (
    <ViewContext.Provider value={view.id}>
      <ErrorBoundary key={`${view.id}|${view.section}`}>
        <Suspense fallback={loadingView}>{Page ? <Page /> : loadingView}</Suspense>
      </ErrorBoundary>
    </ViewContext.Provider>
  );
});

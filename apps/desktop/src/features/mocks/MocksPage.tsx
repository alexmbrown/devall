import { lazy, Suspense, useEffect, useRef } from 'react';
import clsx from 'clsx';
import { LayoutDashboard, FileJson2, ScrollText, Crosshair, Disc, GitBranch, FolderOpen, ListChecks, GitCompare, Upload, SlidersHorizontal, Server, FileCheck2, Braces, Waypoints, Network, Radio } from 'lucide-react';
import { ErrorBoundary } from '@/components/layout/ErrorBoundary';
import { Empty, Spinner } from '@/components/ui/misc';
import { ResponsiveTabs, type TabDef } from '@/components/ui/ResponsiveTabs';
import { useWorkspace, useFsVersion } from '@/store/workspace';
import { useNav as useAppNav, useViewLabel } from '@/store/nav';
import { useNav, parseDeepLink, type Section } from './store/nav';
import { useMockTarget } from './store/target';
import { useMappings, useRecordingStatus, useUnmatchedNearMisses, useWm, invalidateTarget } from './lib/queries';
import { ServerBar } from './server/ServerBar';
import { ConnectionGate } from './server/ConnectionGate';
import { DashboardPage } from './dashboard/DashboardPage';
import './mocks.css';

const StubsPage = lazy(() => import('./stubs/StubsPage'));
const RequestsPage = lazy(() => import('./requests/RequestsPage'));
const NearMissesPage = lazy(() => import('./requests/NearMissesPage'));
const RecordingPage = lazy(() => import('./recording/RecordingPage'));
const ScenariosPage = lazy(() => import('./scenarios/ScenariosPage'));
const FilesPage = lazy(() => import('./files/FilesPage'));
const VerifyPage = lazy(() => import('./verify/VerifyPage'));
const ComparePage = lazy(() => import('./compare/ComparePage'));
const ImportPage = lazy(() => import('./import/ImportPage'));
const SettingsPage = lazy(() => import('./settings/SettingsPage'));
const ServersPage = lazy(() => import('./server/ServersPage'));
const DriftPage = lazy(() => import('./drift/DriftPage'));
const ProtocolsPage = lazy(() => import('./protocols/ProtocolsPage'));

/** Pages that work without a reachable server. */
const NO_SERVER: Section[] = ['servers', 'compare', 'drift', 'graphql', 'websocket', 'grpc', 'mqtt'];

/** Workbench tab label per sub-page ("Mocks: Stubs"; the overview shows "Mock server"). */
const VIEW_LABELS: Record<Section, string | null> = {
  dashboard: null,
  stubs: 'Stubs',
  graphql: 'GraphQL',
  websocket: 'WebSocket',
  grpc: 'gRPC',
  mqtt: 'MQTT',
  requests: 'Request journal',
  'near-misses': 'Near misses',
  recording: 'Recording',
  scenarios: 'Scenarios',
  files: 'Files',
  verify: 'Verify',
  compare: 'Compare',
  drift: 'Spec drift',
  import: 'Import / export',
  settings: 'Settings',
  servers: 'Servers',
};

/** Ignore file events this long after our own writes (the engine writes stub files itself). */
const OWN_WRITE_MS = 2500;

/**
 * Mock server module: the project's built-in WireMock-compatible engine (stubs in
 * `mocks/mappings/`) or a remote WireMock from `mocks/servers.yaml`, with the full admin UI
 * ported from wiremock-ui.
 */
export default function MocksPage() {
  const project = useWorkspace((s) => s.project);
  const root = project?.root ?? null;
  const init = useMockTarget((s) => s.init);
  const section = useNav((s) => s.section);
  const appParam = useAppNav((s) => s.param);

  useEffect(() => {
    void init(root);
  }, [init, root]);

  // Deep links: useNav.getState().go('mocks', 'requests') from the palette or other modules.
  useEffect(() => {
    const d = parseDeepLink(appParam);
    if (d) useNav.getState().go(d.section, d.param);
  }, [appParam]);

  useFileWatch();
  useRestartInvalidation();
  useStatusPolling(!!root);
  useViewLabel(VIEW_LABELS[section]);

  if (!project)
    return (
      <Empty icon={<Server />} title="Open a project">
        Mock stubs are files in the project's <code className="code-inline">mocks/</code> folder.
      </Empty>
    );

  const page = (() => {
    switch (section) {
      case 'dashboard':
        return <DashboardPage />;
      case 'stubs':
        return <StubsPage />;
      case 'graphql':
      case 'websocket':
      case 'grpc':
      case 'mqtt':
        return <ProtocolsPage protocol={section} />;
      case 'requests':
        return <RequestsPage />;
      case 'near-misses':
        return <NearMissesPage />;
      case 'recording':
        return <RecordingPage />;
      case 'scenarios':
        return <ScenariosPage />;
      case 'files':
        return <FilesPage />;
      case 'verify':
        return <VerifyPage />;
      case 'compare':
        return <ComparePage />;
      case 'drift':
        return <DriftPage />;
      case 'import':
        return <ImportPage />;
      case 'settings':
        return <SettingsPage />;
      case 'servers':
        return <ServersPage />;
    }
  })();

  return (
    <div className="page mocks-page">
      <ServerBar />
      <SubNav />
      <div className="mocks-body">
        <ErrorBoundary key={section}>
          <Suspense
            fallback={
              <div className="empty">
                <Spinner />
              </div>
            }
          >
            {NO_SERVER.includes(section) ? page : <ConnectionGate>{page}</ConnectionGate>}
          </Suspense>
        </ErrorBoundary>
      </div>
    </div>
  );
}

function SubNav() {
  const { section, go } = useNav();
  const { client } = useWm();
  const mappings = useMappings();
  const nm = useUnmatchedNearMisses();
  const rec = useRecordingStatus();
  const proto = useMockTarget((s) => s.status?.protocols ?? null);
  const unmatched = client ? new Set(nm.data?.nearMisses.map((n) => n.request.id ?? n.request.loggedDate)).size : 0;
  const recording = rec.data?.status === 'Recording';
  const items: { id: Section; label: string; icon: React.ReactNode; count?: number; alert?: boolean }[] = [
    { id: 'dashboard', label: 'Overview', icon: <LayoutDashboard size={14} /> },
    { id: 'stubs', label: 'Stubs', icon: <FileJson2 size={14} />, count: client ? mappings.data?.length : undefined },
    { id: 'graphql', label: 'GraphQL', icon: <Braces size={14} />, count: proto?.graphql.length || undefined },
    { id: 'websocket', label: 'WebSocket', icon: <Waypoints size={14} />, count: proto?.websocketConnections || undefined },
    { id: 'grpc', label: 'gRPC', icon: <Network size={14} /> },
    { id: 'mqtt', label: 'MQTT', icon: <Radio size={14} />, count: proto?.mqttClients || undefined },
    { id: 'requests', label: 'Requests', icon: <ScrollText size={14} /> },
    { id: 'near-misses', label: 'Near misses', icon: <Crosshair size={14} />, count: unmatched || undefined, alert: unmatched > 0 },
    { id: 'recording', label: 'Record', icon: <Disc size={14} />, alert: recording },
    { id: 'scenarios', label: 'Scenarios', icon: <GitBranch size={14} /> },
    { id: 'files', label: 'Files', icon: <FolderOpen size={14} /> },
    { id: 'verify', label: 'Verify', icon: <ListChecks size={14} /> },
    { id: 'compare', label: 'Compare', icon: <GitCompare size={14} /> },
    { id: 'drift', label: 'Drift', icon: <FileCheck2 size={14} /> },
    { id: 'import', label: 'Import / export', icon: <Upload size={14} /> },
    { id: 'settings', label: 'Settings', icon: <SlidersHorizontal size={14} /> },
    { id: 'servers', label: 'Servers', icon: <Server size={14} /> },
  ];
  // Same row as the request editor: never scrolls; it tightens, then shows icons + counts, then a More menu.
  const tabs: TabDef<Section>[] = items.map((it) => ({
    key: it.id,
    label: it.label,
    icon: it.icon,
    showIcon: true,
    title: it.label,
    badge:
      it.id === 'recording' && recording ? (
        <span className="mocks-count alert">REC</span>
      ) : it.count !== undefined ? (
        <span className={clsx('mocks-count', it.alert && 'alert')}>{it.count}</span>
      ) : undefined,
  }));
  return <ResponsiveTabs tabs={tabs} value={section} onChange={(k) => go(k)} className="mocks-subtabs" label="Mock server" prefer="labels" />;
}

/** Notices edits to `mocks/` made outside this UI (git pull, editors, the traffic inspector). */
function useFileWatch() {
  const v = useFsVersion((s) => s.v);
  useEffect(() => {
    if (!v) return;
    const paths = useFsVersion.getState().paths;
    const st = useMockTarget.getState();
    if (paths.includes('mocks/servers.yaml')) void st.reloadServers();
    const stubFiles = paths.some((p) => p === 'mocks/mappings' || p.startsWith('mocks/mappings/'));
    if (stubFiles && st.status?.running && Date.now() - st.lastLocalWrite > OWN_WRITE_MS) st.setDiskChanged(true);
  }, [v]);
}

/** Keeps the built-in engine's status (running, stub count, load errors) fresh while the page is open. */
function useStatusPolling(enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;
    const t = setInterval(() => {
      if (!document.hidden) void useMockTarget.getState().refreshStatus();
    }, 15_000);
    return () => clearInterval(t);
  }, [enabled]);
}

/** The built-in engine gets a new port on restart: drop what was cached for the previous run. */
function useRestartInvalidation() {
  const { target } = useWm();
  const url = useMockTarget((s) => s.status?.url ?? '');
  const last = useRef(url);
  useEffect(() => {
    if (url && url !== last.current && target.kind === 'builtin') void invalidateTarget(target);
    last.current = url;
  }, [url, target]);
}

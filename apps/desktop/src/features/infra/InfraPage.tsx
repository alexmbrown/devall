import { useEffect } from 'react';
import clsx from 'clsx';
import { Box, Cable, Cloud, Container } from 'lucide-react';
import { infraApi } from '@/lib/api/infra';
import { isTauri } from '@/lib/ipc';
import { DockerView } from './DockerView';
import { KubeView } from './KubeView';
import { ForwardsView } from './ForwardsView';
import { GcpView } from './GcpView';
import { ForwardDialog } from './ForwardDialog';
import { useInfraUi, type InfraTab } from './store';
import './infra.css';
import { useActiveViewRef, useViewLabel } from '@/store/nav';

const TABS: { id: InfraTab; label: string; icon: React.ReactNode }[] = [
  { id: 'docker', label: 'Docker', icon: <Container size={14} /> },
  { id: 'kube', label: 'Kubernetes', icon: <Box size={14} /> },
  { id: 'forwards', label: 'Port forwards', icon: <Cable size={14} /> },
  { id: 'gcp', label: 'GCP', icon: <Cloud size={14} /> },
];

/** Docker & Kubernetes viewers, the port-forward manager and GCP emulator views. */
export default function InfraPage() {
  const tab = useInfraUi((s) => s.tab);
  const set = useInfraUi((s) => s.set);
  useViewLabel(tab === 'gcp' ? 'Google Cloud' : TABS.find((t) => t.id === tab)?.label);

  useEffect(() => {
    // Streams opened by a previous mount (e.g. before a webview reload) have no reader any more.
    if (isTauri) void infraApi.streamStopAll().catch(() => {});
  }, []);

  return (
    <div className="page infra-page">
      <div className="subtabs" role="tablist" aria-label="Infrastructure">
        {TABS.map((t, i) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            className={clsx('subtab', tab === t.id && 'on')}
            onClick={() => set({ tab: t.id })}
            title={`${t.label} (Alt+${i + 1})`}
          >
            {t.icon}
            {t.label}
          </button>
        ))}
      </div>
      <TabKeys />
      <div className="infra-body">
        {tab === 'docker' && <DockerView />}
        {tab === 'kube' && <KubeView />}
        {tab === 'forwards' && <ForwardsView />}
        {tab === 'gcp' && <GcpView />}
      </div>
      <ForwardDialog />
    </div>
  );
}

/** Alt+1…4 switch tabs. */
function TabKeys() {
  const set = useInfraUi((s) => s.set);
  const activeView = useActiveViewRef();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!activeView.current) return;
      if (e.defaultPrevented || !e.altKey || e.ctrlKey || e.metaKey) return;
      const i = Number(e.key) - 1;
      if (i >= 0 && i < TABS.length) {
        e.preventDefault();
        set({ tab: TABS[i].id });
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [set]);
  return null;
}

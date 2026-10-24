import { useEffect } from 'react';
import { create } from 'zustand';
import clsx from 'clsx';
import { FileCheck2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { openapiApi, syncSummary, type DriftOptions } from '@/lib/api/openapi';
import { useFsVersion, useWorkspace } from '@/store/workspace';
import { DriftPanel, LinkPanel, LintPanel } from './panels';
import { specCandidates } from './logic';

export type OpenApiTab = 'lint' | 'drift' | 'link';

interface Target {
  tab: OpenApiTab;
  /** Request folder (relative to `http/`) for the Linked spec tab. */
  folder?: string;
  spec?: string;
  drift?: DriftOptions;
}

interface OpenApiState {
  target: Target | null;
  open(t: Target): void;
  setTab(tab: OpenApiTab): void;
  close(): void;
}

/** Opens the OpenAPI dialog from anywhere (request tree folder menu, inspector, palette). */
export const useOpenApi = create<OpenApiState>((set) => ({
  target: null,
  open: (target) => set({ target }),
  setTab: (tab) => set((s) => (s.target ? { target: { ...s.target, tab } } : s)),
  close: () => set({ target: null }),
}));

function OpenApiDialog({ target }: { target: Target }) {
  const { close, setTab } = useOpenApi();
  const tabs: { id: OpenApiTab; label: string }[] = [
    { id: 'lint', label: 'Lint' },
    { id: 'drift', label: 'Drift' },
    ...(target.folder ? [{ id: 'link' as const, label: 'Linked spec' }] : []),
  ];
  return (
    <Modal
      open
      onClose={close}
      size="xl"
      title={
        <span className="row-sm">
          <FileCheck2 size={16} /> OpenAPI{target.folder ? <span className="subtle xs mono">http/{target.folder}</span> : null}
        </span>
      }
    >
      <div className="col">
        <div className="subtabs" role="tablist">
          {tabs.map((t) => (
            <button key={t.id} role="tab" aria-selected={target.tab === t.id} className={clsx('subtab', target.tab === t.id && 'on')} onClick={() => setTab(t.id)}>
              {t.label}
            </button>
          ))}
        </div>
        {target.tab === 'lint' && <LintPanel initialSpec={target.spec} />}
        {target.tab === 'drift' && <DriftPanel initial={{ ...target.drift, spec: target.spec }} />}
        {target.tab === 'link' && target.folder && <LinkPanel folder={target.folder} />}
      </div>
    </Modal>
  );
}

/** Re-syncs linked folders (auto-sync on) when their spec file changes on disk. */
export function useOpenApiAutoSync() {
  const v = useFsVersion((s) => s.v);
  useEffect(() => {
    if (!v) return;
    const root = useWorkspace.getState().project?.root;
    const candidates = specCandidates(useFsVersion.getState().paths);
    if (!root || !candidates.length) return;
    openapiApi
      .autosync(root, candidates)
      .then(async (results) => {
        if (!results.length) return;
        for (const r of results) {
          if (r.report) toast.success(`Re-synced http/${r.folder} from ${r.report.source}`, syncSummary(r.report));
          else toast.error(`Could not re-sync http/${r.folder}`, r.error ?? undefined);
        }
        await useWorkspace.getState().refresh();
      })
      .catch((e) => toast.error('OpenAPI auto-sync failed', errorMessage(e)));
  }, [v]);
}

/** Mounted once in App. */
export function OpenApiHost() {
  const target = useOpenApi((s) => s.target);
  useOpenApiAutoSync();
  return target ? <OpenApiDialog target={target} /> : null;
}

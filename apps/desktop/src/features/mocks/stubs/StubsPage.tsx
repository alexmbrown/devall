import { useEffect } from 'react';
import clsx from 'clsx';
import { X, FilePlus2, FileJson2 } from 'lucide-react';
import { useWm, useMappings } from '@/features/mocks/lib/queries';
import { useServerTabs, useStubTabs, type StubTab } from '@/features/mocks/store/tabs';
import { MethodBadge, Empty } from '@/features/mocks/components/ui';
import { useContextMenu } from '@/components/ui/Menu';
import { confirmDialog } from '@/components/ui/dialogs';
import { StubList } from './StubList';
import { StubEditor } from './StubEditor';
import { openNewStub, openStub, BLANK_STUB } from './open';
import { findStubForFile, stubFileParam } from './openFile';
import { useNav } from '@/features/mocks/store/nav';
import { useWorkspace } from '@/store/workspace';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { SidePane } from '@/components/layout/SidePane';

export default function StubsPage() {
  const { sid } = useWm();
  const { tabs, active } = useServerTabs(sid);
  const ws = useStubTabs();
  const ctx = useContextMenu();
  const { data: mappings } = useMappings();
  const param = useNav((s) => s.param);
  const root = useWorkspace((s) => s.project?.root);

  // Deep link from a note: `stubs/file:mappings/<file>.json` opens the stub that file defines.
  useEffect(() => {
    const file = stubFileParam(param);
    if (!file || !mappings || !root) return;
    useNav.getState().go('stubs');
    findStubForFile(root, file, mappings)
      .then((stub) => (stub ? openStub(stub) : toast.info('Stub not loaded', `mocks/${file} isn't served by the active mock server — select the built-in engine for this project.`)))
      .catch((e) => toast.error(`Could not open mocks/${file}`, errorMessage(e)));
  }, [param, mappings, root]);

  const activeTab = tabs.find((t) => t.key === active);

  const close = async (t: StubTab) => {
    if (t.draft || t.draftText) {
      const ok = await confirmDialog({ title: `Discard changes to "${t.title}"?`, message: 'This tab has unsaved edits.', confirmText: 'Discard', danger: true });
      if (!ok) return;
    }
    ws.close(sid, t.key);
  };

  return (
    <div className="split">
      <SidePane title="Stubs">
        <StubList activeStubId={activeTab?.stubId} />
      </SidePane>
      <div className="pane fill">
        {tabs.length > 0 && (
          <div className="tabbar" role="tablist">
            {tabs.map((t, i) => (
              <div
                key={t.key}
                role="tab"
                aria-selected={t.key === active}
                className={clsx('etab', t.key === active && 'on', (t.draft || t.draftText) && 'dirty')}
                onClick={() => ws.activate(sid, t.key)}
                onDoubleClick={() => ws.update(sid, t.key, { pinned: true })}
                onAuxClick={(e) => e.button === 1 && close(t)}
                onContextMenu={(e) =>
                  ctx.open(e, [
                    { label: 'Close', onSelect: () => close(t) },
                    { label: 'Close others', onSelect: () => ws.closeOthers(sid, t.key) },
                    { label: 'Close all saved', onSelect: () => ws.closeAll(sid) },
                    { separator: true },
                    { label: 'Keep open (pin)', onSelect: () => ws.update(sid, t.key, { pinned: true }) },
                  ])
                }
                draggable
                onDragStart={(e) => e.dataTransfer.setData('wmui/tab', String(i))}
                onDragOver={(e) => e.preventDefault()}
                onDrop={(e) => {
                  const from = Number(e.dataTransfer.getData('wmui/tab'));
                  if (!Number.isNaN(from) && from !== i) ws.move(sid, from, i);
                }}
                title={t.title}
              >
                {t.isNew ? <FilePlus2 size={13} /> : <MethodBadge method={t.method} />}
                <span className="truncate grow" style={{ fontStyle: t.pinned || t.draft ? 'normal' : 'italic' }}>
                  {t.title || 'Untitled'}
                </span>
                <button
                  className="close"
                  onClick={(e) => {
                    e.stopPropagation();
                    close(t);
                  }}
                  aria-label="Close tab"
                >
                  <span className="dirty-dot" />
                  <X />
                </button>
              </div>
            ))}
          </div>
        )}
        {activeTab ? (
          <StubEditor key={activeTab.key} tab={activeTab} />
        ) : (
          <Empty
            icon={<FileJson2 />}
            title="Open a stub to edit it"
            action={
              <button className="btn btn-primary" onClick={() => openNewStub(structuredClone(BLANK_STUB))}>
                <FilePlus2 /> New stub
              </button>
            }
          >
            Click a stub to preview it, double-click to keep it open in a tab. Edits stay in the tab until you save — the server is always the source of truth.
          </Empty>
        )}
      </div>
      {ctx.node}
    </div>
  );
}

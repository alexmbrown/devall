import { create } from 'zustand';
import { History as HistoryIcon, Trash2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { historyApi, useLocalHistoryVersion } from '@/lib/api/history';
import { FileHistoryView } from './FileHistoryView';

interface Target {
  root: string;
  /** Project-relative (`http/a.req.yaml`, `notes/x.md`, `devall.yaml`). */
  path: string;
  /** Display name (defaults to the file name). */
  title?: string;
}

const useLocalHistoryDialog = create<{ target: Target | null; set(t: Target | null): void }>((set) => ({ target: null, set: (target) => set({ target }) }));

/** Opens the "Local history" dialog for a project file (context menus: "Local history…"). */
export function openLocalHistory(target: Target) {
  useLocalHistoryDialog.getState().set(target);
}

/** Mounted once in App. */
export function LocalHistoryHost() {
  const target = useLocalHistoryDialog((s) => s.target);
  const close = () => useLocalHistoryDialog.getState().set(null);
  if (!target) return null;
  const name = target.title ?? target.path.split('/').pop() ?? target.path;
  const clear = async () => {
    if (!(await confirmDialog({ title: `Clear local history of ${name}?`, message: 'Removes every saved version of this file from this computer. The file itself is not touched.', confirmText: 'Clear', danger: true })))
      return;
    try {
      await historyApi.clearFile(target.root, target.path);
      useLocalHistoryVersion.getState().bumpFiles();
    } catch (e) {
      toast.error('Could not clear', errorMessage(e));
    }
  };
  return (
    <Modal
      open
      onClose={close}
      size="xl"
      className="lh-modal"
      bodyClassName="lh-modal-body"
      title={
        <span className="row-sm">
          <HistoryIcon size={16} /> Local history — {name}
        </span>
      }
      headerExtra={
        <>
          <span className="xs subtle mono truncate lh-path" title={target.path}>
            {target.path}
          </span>
          <button className="btn btn-sm btn-danger-ghost" onClick={clear} title="Forget every version of this file">
            <Trash2 /> Clear
          </button>
        </>
      }
    >
      <FileHistoryView root={target.root} path={target.path} onRestored={close} />
    </Modal>
  );
}

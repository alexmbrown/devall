import { useEffect, useState } from 'react';
import { History, Trash2 } from 'lucide-react';
import { useContextMenu } from '@/components/ui/Menu';
import { Empty, RelTime, Segmented, Spinner } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { historyApi, useLocalHistoryVersion, type UtilityRun } from '@/lib/api/history';
import '@/features/history/history.css';

/**
 * Past runs (device-wide utility history) for the side pane: the current utility's by default,
 * or every utility's. Picking one restores its op, input and arguments.
 */
export function UtilityHistoryList({ opId, opName, query, onPick }: { opId: string; opName?: string; query: string; onPick(run: UtilityRun): void }) {
  const [all, setAll] = useState(false);
  const [runs, setRuns] = useState<UtilityRun[] | null>(null);
  const version = useLocalHistoryVersion((s) => s.utilities);
  const ctx = useContextMenu();

  useEffect(() => {
    let alive = true;
    const t = setTimeout(() => {
      historyApi
        .utilityRuns(all ? undefined : opId, query.trim(), 0, 200)
        .then((r) => alive && setRuns(r))
        .catch((e) => {
          if (alive) setRuns([]);
          toast.error('Utility history', errorMessage(e));
        });
    }, 120);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [opId, all, query, version]);

  const clear = async () => {
    const what = all ? 'every utility' : (opName ?? opId);
    if (!(await confirmDialog({ title: `Clear history of ${what}?`, message: 'Removes the recorded runs from this computer.', confirmText: 'Clear', danger: true }))) return;
    try {
      await historyApi.clearUtilityRuns(all ? undefined : opId);
      useLocalHistoryVersion.getState().bumpUtilities();
    } catch (e) {
      toast.error('Could not clear', errorMessage(e));
    }
  };
  const remove = async (r: UtilityRun) => {
    try {
      await historyApi.deleteUtilityRun(r.id);
      useLocalHistoryVersion.getState().bumpUtilities();
    } catch (e) {
      toast.error('Could not delete', errorMessage(e));
    }
  };

  return (
    <>
      <div className="util-hist-tools">
        <Segmented
          value={all ? 'all' : 'this'}
          onChange={(v) => setAll(v === 'all')}
          options={[
            { value: 'this', label: 'This utility' },
            { value: 'all', label: 'All utilities' },
          ]}
        />
        <span className="spacer" />
        <button className="icon-btn sm danger" onClick={clear} disabled={!runs?.length} title="Clear these runs" aria-label="Clear these runs">
          <Trash2 />
        </button>
      </div>
      <div className="list">
        {!runs ? (
          <div className="empty">
            <Spinner />
          </div>
        ) : !runs.length ? (
          <Empty icon={<History />} title={query ? 'Nothing matches' : 'No runs yet'}>
            Runs are kept on this computer once the input settles or you copy the output. Secret arguments are masked.
          </Empty>
        ) : (
          runs.map((r) => (
            <div
              key={r.id}
              className="list-item util-hist-item"
              onClick={() => onPick(r)}
              onContextMenu={(e) =>
                ctx.open(e, [
                  { label: 'Restore input and arguments', icon: <History />, onSelect: () => onPick(r) },
                  { separator: true },
                  { label: 'Delete', icon: <Trash2 />, danger: true, onSelect: () => void remove(r) },
                ])
              }
              role="button"
              tabIndex={0}
              onKeyDown={(e) => e.key === 'Enter' && onPick(r)}
              title={r.inputMasked ? 'The input was a secret and was not kept' : r.input.slice(0, 400)}
            >
              <div className="util-hist-text">
                {all && <span className="small strong truncate">{r.opName}</span>}
                <span className="mono xs truncate">{r.inputMasked ? '(secret input not kept)' : r.input.replace(/\s+/g, ' ').slice(0, 160) || '(no input)'}</span>
                <span className="xs subtle truncate">
                  <RelTime ts={r.at} />
                  {!r.ok && ' · error'}
                  {r.inputTruncated && ' · input truncated'}
                </span>
              </div>
            </div>
          ))
        )}
      </div>
      {ctx.node}
    </>
  );
}

import { useCallback, useEffect, useState } from 'react';
import { FolderOpen, Trash2 } from 'lucide-react';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { Field, Switch, formatBytes } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { historyApi, useLocalHistoryVersion, type ClearableKind, type HistorySettings, type HistoryUsage, type KindUsage } from '@/lib/api/history';
import { useHistoryVersion, useWorkspace } from '@/store/workspace';
import '@/features/history/history.css';

/** A number setting saved on blur / Enter. */
function NumberField({ label, hint, value, min, max, onSave }: { label: string; hint?: string; value: number; min: number; max: number; onSave(v: number): void }) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const n = Math.round(Number(text));
    if (!Number.isFinite(n) || n < min || n > max) {
      setText(String(value));
      toast.error(`${label}: enter a number from ${min} to ${max}`);
      return;
    }
    if (n !== value) onSave(n);
  };
  return (
    <Field label={label} hint={hint} className="hist-setting-num">
      <input className="input sm" type="number" min={min} max={max} value={text} onChange={(e) => setText(e.target.value)} onBlur={commit} onKeyDown={(e) => e.key === 'Enter' && commit()} />
    </Field>
  );
}

const ROWS: { key: keyof Omit<HistoryUsage, 'dataDir' | 'projectDir'>; label: string; clear?: ClearableKind; project: boolean; hint?: string }[] = [
  { key: 'responses', label: 'Responses', clear: 'responses', project: true },
  { key: 'files', label: 'File versions', clear: 'files', project: true },
  { key: 'utilities', label: 'Utility runs', clear: 'utilities', project: false, hint: 'all projects' },
  { key: 'events', label: 'Project activity', clear: 'events', project: true },
  { key: 'webhooks', label: 'Webhook events', project: true, hint: 'manage in Webhooks' },
  { key: 'loadRuns', label: 'Load test runs', project: true, hint: 'delete in Load testing' },
  { key: 'caches', label: 'Search index & saved captures', project: true, hint: 'rebuilt on demand' },
];

/**
 * Settings → History: what is recorded, for how long, what it uses on disk, and clearing it.
 * Stored in `<app data>/history/settings.json` so the CLI follows the same choices.
 */
export function HistorySettingsSection() {
  const root = useWorkspace((s) => s.project?.root ?? null);
  const [s, setS] = useState<HistorySettings | null>(null);
  const [usage, setUsage] = useState<HistoryUsage | null>(null);

  const loadUsage = useCallback(() => {
    historyApi
      .usage(root)
      .then(setUsage)
      .catch(() => setUsage(null));
  }, [root]);
  useEffect(() => {
    historyApi
      .settings()
      .then(setS)
      .catch((e) => toast.error('History settings', errorMessage(e)));
    loadUsage();
  }, [loadUsage]);

  const save = async (patch: Partial<HistorySettings>) => {
    if (!s) return;
    const next = { ...s, ...patch };
    setS(next);
    try {
      setS(await historyApi.setSettings(next));
    } catch (e) {
      toast.error('Could not save history settings', errorMessage(e));
    }
  };

  const clear = async (kind: ClearableKind, label: string, project: boolean) => {
    const where = project ? 'this project' : 'every project';
    if (!(await confirmDialog({ title: `Clear ${label.toLowerCase()}?`, message: `Removes the ${label.toLowerCase()} of ${where} from this computer. Project files are not touched.`, confirmText: 'Clear', danger: true })))
      return;
    try {
      const n = await historyApi.clear(kind, root);
      toast.success(`${label} cleared`, `${n.toLocaleString()} removed`);
      if (kind === 'files') useLocalHistoryVersion.getState().bumpFiles();
      if (kind === 'utilities') useLocalHistoryVersion.getState().bumpUtilities();
      if (kind === 'responses' || kind === 'events') useHistoryVersion.getState().bump();
      loadUsage();
    } catch (e) {
      toast.error('Could not clear', errorMessage(e));
    }
  };

  const reveal = () =>
    void historyApi
      .dataDir(root)
      .then((d) => revealItemInDir(d))
      .catch((e) => toast.error('Could not open the folder', errorMessage(e)));

  const cell = (u: KindUsage | undefined) => (u ? `${u.count != null ? `${u.count.toLocaleString()} · ` : ''}${formatBytes(u.bytes)}` : '—');

  return (
    <section className="col-sm">
      <h3 className="small strong">History</h3>
      <p className="xs subtle">
        Requests, file versions, utility runs and activity are kept on this computer, in the app data folder — never in the project, so nothing is committed. The <code className="code-inline">devall</code> CLI uses
        the same history and settings.
      </p>
      {s && (
        <>
          <Switch checked={s.responseBodies} onChange={(v) => void save({ responseBodies: v })} label="Keep response bodies (status, timing and headers are always kept)" />
          <Switch checked={s.utilities} onChange={(v) => void save({ utilities: v })} label="Record utility runs (secret arguments are masked)" />
          <Switch checked={s.fileVersions} onChange={(v) => void save({ fileVersions: v })} label="Keep versions of project files (local file history)" />
          <div className="row wrap hist-setting-row">
            <NumberField label="Responses per request" value={s.responsesPerRequest} min={1} max={100000} onSave={(v) => void save({ responsesPerRequest: v })} />
            <NumberField label="Utility runs kept" value={s.utilitiesMax} min={10} max={100000} onSave={(v) => void save({ utilitiesMax: v })} />
          </div>
          <div className="row wrap hist-setting-row">
            <NumberField label="Versions per file" value={s.fileMaxVersions} min={1} max={10000} onSave={(v) => void save({ fileMaxVersions: v })} />
            <NumberField label="Keep versions for (days)" hint="The latest version of each file is always kept." value={s.fileMaxDays} min={1} max={36500} onSave={(v) => void save({ fileMaxDays: v })} />
            <NumberField label="File history size cap (MB per project)" value={s.fileMaxMb} min={1} max={100000} onSave={(v) => void save({ fileMaxMb: v })} />
          </div>
          <p className="subtle xs">Older file versions are pruned when a project is opened.</p>
        </>
      )}
      <table className="table hist-usage">
        <thead>
          <tr>
            <th>Kind</th>
            <th>Stored{root ? '' : ' (open a project for per-project kinds)'}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {ROWS.map((r) => {
            const u = usage?.[r.key];
            const disabled = (r.project && !root) || !u || (u.count === 0 && u.bytes === 0);
            return (
              <tr key={r.key}>
                <td>
                  {r.label}
                  {r.hint && <span className="xs subtle"> · {r.hint}</span>}
                </td>
                <td className="mono xs">{r.project && !root ? '—' : cell(u)}</td>
                <td className="hist-usage-actions">
                  {r.clear && (
                    <button className="btn btn-sm btn-danger-ghost" disabled={disabled} onClick={() => void clear(r.clear!, r.label, r.project)}>
                      <Trash2 /> Clear…
                    </button>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <div className="row-sm">
        <button className="btn btn-sm" onClick={reveal} title={usage?.projectDir ?? usage?.dataDir}>
          <FolderOpen /> Show data folder
        </button>
        <span className="mono xs subtle break">{usage?.projectDir ?? usage?.dataDir}</span>
      </div>
    </section>
  );
}

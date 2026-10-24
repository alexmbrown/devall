import { useRef, useState } from 'react';
import { Ban, CheckCircle2, Download, FileUp, Play, XCircle } from 'lucide-react';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { Modal } from '@/components/ui/Modal';
import { Callout, Field, StatusCode, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import { uid } from '@/lib/id';
import type { RunItemResult, RunSummary } from '@/lib/types';
import { useVarsVersion } from '@/store/variables';
import { useHistoryVersion, useWorkspace } from '@/store/workspace';
import { create } from 'zustand';

export const useRunner = create<{ folder: string | null; open(folder: string): void; close(): void }>((set) => ({
  folder: null,
  open: (folder) => set({ folder }),
  close: () => set({ folder: null }),
}));

/** Runs every request in a folder (or the whole project) in order. */
export function RunnerModal() {
  const folder = useRunner((s) => s.folder);
  const close = useRunner((s) => s.close);
  const project = useWorkspace((s) => s.project);
  const env = useWorkspace((s) => s.activeEnv());
  const [iterations, setIterations] = useState(1);
  const [bail, setBail] = useState(false);
  const [delay, setDelay] = useState(0);
  const [failOnHttp, setFailOnHttp] = useState(false);
  const [keepVars, setKeepVars] = useState(true);
  const [updateSnapshots, setUpdateSnapshots] = useState(false);
  const [data, setData] = useState<{ name: string; text: string } | null>(null);
  const [items, setItems] = useState<RunItemResult[]>([]);
  const [total, setTotal] = useState(0);
  const [summary, setSummary] = useState<RunSummary | null>(null);
  const [running, setRunning] = useState(false);
  const runId = useRef('');

  if (folder === null || !project) return null;
  const label = folder || project.manifest.name;

  const start = async () => {
    runId.current = uid();
    setItems([]);
    setSummary(null);
    setRunning(true);
    try {
      const s = await api.runCollection(
        runId.current,
        project.root,
        { folder, env: env?.id, iterations, bail, delayMs: delay, failOnHttpError: failOnHttp, data: data?.text, keepVariables: keepVars, updateSnapshots },
        (e) => {
          if (e.type === 'started') setTotal(e.total);
          else setItems((x) => [...x, e.result]);
        },
      );
      setSummary(s);
      useHistoryVersion.getState().bump();
      if (keepVars) useVarsVersion.getState().bump();
    } catch (e) {
      toast.error('Run failed', errorMessage(e));
    } finally {
      setRunning(false);
    }
  };

  const pickData = async () => {
    const f = await openDialog({ multiple: false, filters: [{ name: 'Data', extensions: ['csv', 'json'] }] });
    if (typeof f !== 'string') return;
    try {
      const text = await readText(f);
      setData({ name: f.split(/[\\/]/).pop() ?? f, text });
    } catch (e) {
      toast.error('Could not read data file', errorMessage(e));
    }
  };

  const exportJunit = async () => {
    if (!summary) return;
    const xml = await api.runToJunit(summary, label);
    const dest = await saveDialog({ defaultPath: 'devall-junit.xml', filters: [{ name: 'JUnit XML', extensions: ['xml'] }] });
    if (!dest) return;
    await api.writeTextFile(dest, xml).then(() => toast.success('JUnit report saved', dest), (e) => toast.error('Save failed', errorMessage(e)));
  };

  const done = items.length;
  const failed = items.filter((i) => !i.passed).length;

  return (
    <Modal
      open
      onClose={() => !running && close()}
      title={`Run ${label}`}
      size="xl"
      footer={
        <>
          {summary && (
            <button className="btn" onClick={exportJunit}>
              <Download /> JUnit XML
            </button>
          )}
          <span className="spacer" />
          {running ? (
            <button className="btn" onClick={() => api.cancelRun(runId.current)}>
              <Ban /> Stop
            </button>
          ) : (
            <button className="btn btn-primary" onClick={start}>
              <Play /> {summary ? 'Run again' : 'Run'}
            </button>
          )}
        </>
      }
    >
      <div className="runner">
        <div className="col" style={{ width: 260, flexShrink: 0 }}>
          <Field label="Environment">
            <div className="small">{env?.file.name ?? 'None'}</div>
          </Field>
          <Field label="Iterations" hint={data ? 'Ignored: one iteration per data row.' : undefined}>
            <input className="input sm" type="number" min={1} max={10000} value={iterations} disabled={!!data} onChange={(e) => setIterations(Math.max(1, Number(e.target.value) || 1))} />
          </Field>
          <Field label="Delay between requests (ms)">
            <input className="input sm" type="number" min={0} value={delay} onChange={(e) => setDelay(Math.max(0, Number(e.target.value) || 0))} />
          </Field>
          <Field label="Data file (CSV or JSON)" hint="Each row's columns become {{variables}} for one iteration.">
            <div className="row-sm">
              <button className="btn btn-sm" onClick={pickData}>
                <FileUp /> {data ? data.name : 'Choose…'}
              </button>
              {data && (
                <button className="btn btn-sm btn-ghost" onClick={() => setData(null)}>
                  Clear
                </button>
              )}
            </div>
          </Field>
          <Switch checked={bail} onChange={setBail} label="Stop at first failure" />
          <Switch checked={failOnHttp} onChange={setFailOnHttp} label="Fail on HTTP ≥ 400 (requests without tests)" />
          <Switch checked={keepVars} onChange={setKeepVars} label="Keep captured variables after the run" />
          <Switch checked={updateSnapshots} onChange={setUpdateSnapshots} label="Update snapshots (record missing, accept changed response bodies)" />
          <Callout>
            Same engine as <code className="code-inline">devall run {folder ? `--filter ${folder}` : ''}</code> in CI.
          </Callout>
        </div>
        <div className="col min-h-0 grow">
          <div className="row">
            <span className="strong">
              {done}/{total || '—'}
            </span>
            {done > 0 && (
              <>
                <span className="badge success">{done - failed} passed</span>
                {failed > 0 && <span className="badge danger">{failed} failed</span>}
              </>
            )}
            {summary && <span className="subtle xs">{(summary.durationMs / 1000).toFixed(2)} s{summary.cancelled ? ' · stopped' : ''}</span>}
          </div>
          <div className="progress">
            <div style={{ width: total ? `${(done / total) * 100}%` : 0, background: failed ? 'var(--warning)' : undefined }} />
          </div>
          <div className="panel runner-results">
            {items.map((i, n) => (
              <details key={n} className="runner-item">
                <summary className="row">
                  {i.passed ? <CheckCircle2 size={14} className="text-success" /> : <XCircle size={14} className="text-danger" />}
                  {i.status ? <StatusCode status={i.status} /> : <span className="xs text-danger strong">ERR</span>}
                  <span className="strong small truncate">{i.name}</span>
                  {i.iteration > 0 && <span className="badge outline">#{i.iteration + 1}</span>}
                  <span className="mono xs subtle truncate grow">{i.url}</span>
                  <span className="xs subtle">{Math.round(i.durationMs)} ms</span>
                </summary>
                <div className="runner-detail">
                  {i.error && <div className="text-danger mono xs">{i.error}</div>}
                  {i.tests.map((t, k) => (
                    <div key={k} className="row-sm xs mono">
                      {t.passed ? <CheckCircle2 size={12} className="text-success" /> : <XCircle size={12} className="text-danger" />} {t.label}
                      {!t.passed && <span className="subtle">— {t.message ?? `actual: ${t.actual}`}</span>}
                    </div>
                  ))}
                  {i.captures.map((c) => (
                    <div key={c.name} className="xs mono subtle">
                      {`{{${c.name}}}`} = {c.error ?? c.value}
                    </div>
                  ))}
                </div>
              </details>
            ))}
          </div>
        </div>
      </div>
    </Modal>
  );
}

async function readText(path: string): Promise<string> {
  return api.readTextFile(path);
}

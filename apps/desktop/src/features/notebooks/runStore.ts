// Run state of notebook cells and Markdown code blocks for this app session. Lives outside the
// components so a running cell (say `npm run dev`) keeps streaming — and stays stoppable — while
// the user looks at another note. Streamed events are applied in batches (~30 fps).
import { create } from 'zustand';
import type { CellEvent, CellResult } from '@/lib/api/notebooks';
import type { UiOutput } from './nbformat';

export interface CellRun {
  runId: string;
  running: boolean;
  outputs: UiOutput[];
  executionCount: number | null;
  exitCode?: number | null;
  durationMs?: number;
  interrupted?: boolean;
  timedOut?: boolean;
  /** `Git Bash 5.2.37`, `Python 3.12.10`, … */
  interpreter?: string;
  startedAt: number;
}

interface RunsState {
  runs: Record<string, CellRun>;
  /** Execution counters per notebook (`root|path`). */
  counters: Record<string, number>;
}

export const useRuns = create<RunsState>(() => ({ runs: {}, counters: {} }));

export const runKey = (root: string, path: string, cellId: string) => `${root}|${path}|${cellId}`;

/** Stream text kept per run; older output is dropped beyond this. */
const MAX_STREAM = 1_500_000;

export function nextExecutionCount(root: string, path: string): number {
  const k = `${root}|${path}`;
  const n = (useRuns.getState().counters[k] ?? 0) + 1;
  useRuns.setState((s) => ({ counters: { ...s.counters, [k]: n } }));
  return n;
}

export function beginRun(key: string, runId: string, executionCount: number | null) {
  pending.delete(key);
  useRuns.setState((s) => ({ runs: { ...s.runs, [key]: { runId, running: true, outputs: [], executionCount, startedAt: Date.now() } } }));
}

function apply(run: CellRun, e: CellEvent): CellRun {
  const outputs = run.outputs;
  switch (e.type) {
    case 'started':
      return { ...run, interpreter: `${e.interpreter}${e.version ? ` ${e.version}` : ''}` };
    case 'stdout':
    case 'stderr': {
      const name = e.type;
      const last = outputs[outputs.length - 1];
      let next: UiOutput[];
      if (last && last.kind === 'stream' && last.name === name) next = [...outputs.slice(0, -1), { ...last, text: last.text + e.text }];
      else next = [...outputs, { kind: 'stream', name, text: e.text }];
      return { ...run, outputs: trim(next) };
    }
    case 'display':
      return { ...run, outputs: [...outputs, { kind: 'display', data: e.data, result: e.result, executionCount: e.result ? run.executionCount : undefined }] };
    case 'error':
      return { ...run, outputs: [...outputs, { kind: 'error', ename: e.ename, evalue: e.evalue, traceback: e.traceback }] };
    case 'notice':
      return { ...run, outputs: [...outputs, { kind: 'notice', text: e.text }] };
    case 'exit':
      return { ...run, running: false, exitCode: e.code, durationMs: e.durationMs, interrupted: e.interrupted, timedOut: e.timedOut };
  }
}

function trim(outputs: UiOutput[]): UiOutput[] {
  let total = 0;
  for (const o of outputs) if (o.kind === 'stream') total += o.text.length;
  if (total <= MAX_STREAM) return outputs;
  let excess = total - MAX_STREAM;
  const res: UiOutput[] = [];
  let noted = false;
  for (const o of outputs) {
    if (o.kind === 'stream' && excess > 0) {
      const cut = Math.min(excess, o.text.length);
      excess -= cut;
      const nl = o.text.indexOf('\n', cut);
      const text = o.text.slice(nl >= 0 && nl - cut < 200 ? nl + 1 : cut);
      if (!noted) {
        res.push({ kind: 'notice', text: 'Earlier output was trimmed.' });
        noted = true;
      }
      if (text) res.push({ ...o, text });
    } else if (!(o.kind === 'notice' && o.text === 'Earlier output was trimmed.')) res.push(o);
  }
  return res;
}

const pending = new Map<string, { runId: string; events: CellEvent[] }>();
let timer: ReturnType<typeof setTimeout> | undefined;

function flush() {
  timer = undefined;
  if (!pending.size) return;
  const batch = new Map(pending);
  pending.clear();
  useRuns.setState((s) => {
    const runs = { ...s.runs };
    for (const [key, { runId, events }] of batch) {
      let run = runs[key];
      if (!run || run.runId !== runId) continue;
      for (const e of events) run = apply(run, e);
      runs[key] = run;
    }
    return { runs };
  });
}

/** Queues a streamed event of run `runId` (ignored if the cell was re-run since). */
export function pushEvent(key: string, runId: string, e: CellEvent) {
  const p = pending.get(key);
  if (p && p.runId === runId) p.events.push(e);
  else pending.set(key, { runId, events: [e] });
  timer ??= setTimeout(flush, 33);
}

export function endRun(key: string, runId: string, result: CellResult) {
  flush();
  useRuns.setState((s) => {
    const run = s.runs[key];
    if (!run || run.runId !== runId) return s;
    return {
      runs: {
        ...s.runs,
        [key]: { ...run, running: false, exitCode: result.exitCode, durationMs: result.durationMs, interrupted: result.interrupted, timedOut: result.timedOut, interpreter: run.interpreter ?? result.interpreter ?? undefined },
      },
    };
  });
}

/** The run couldn't start (missing interpreter, untrusted, …): show the message as an error. */
export function failRun(key: string, runId: string, message: string) {
  flush();
  useRuns.setState((s) => {
    const run = s.runs[key];
    if (!run || run.runId !== runId) return s;
    return { runs: { ...s.runs, [key]: { ...run, running: false, exitCode: null, outputs: [...run.outputs, { kind: 'error', ename: 'Error', evalue: message, traceback: [] }] } } };
  });
}

/** Clears a cell's outputs (keeps an empty, finished entry so saved outputs are cleared too). */
export function clearRun(key: string) {
  pending.delete(key);
  useRuns.setState((s) => {
    const run = s.runs[key];
    if (run?.running) return { runs: { ...s.runs, [key]: { ...run, outputs: [] } } };
    return { runs: { ...s.runs, [key]: { runId: '', running: false, outputs: [], executionCount: null, startedAt: 0 } } };
  });
}

/** Running runs whose key starts with `prefix` (`root|path|`). */
export function runningUnder(prefix: string): CellRun[] {
  return Object.entries(useRuns.getState().runs)
    .filter(([k, r]) => k.startsWith(prefix) && r.running)
    .map(([, r]) => r);
}

import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import {
  ArrowDown,
  ArrowUp,
  Braces,
  Check,
  Code,
  Cpu,
  Ellipsis,
  Eraser,
  FastForward,
  FolderCog,
  GitBranch,
  KeyRound,
  Play,
  Plus,
  RotateCcw,
  Save,
  ShieldAlert,
  ShieldCheck,
  Square,
  Trash2,
  Type,
} from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Markdown } from '@/components/Markdown';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Callout, Spinner } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { notesApi } from '@/lib/api/notes';
import { LANGUAGE_LABELS, RUN_LANGUAGES, notebooksApi, type InterpreterStatus, type RunLanguage, type TrustStatus } from '@/lib/api/notebooks';
import { useNav } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { useProjectRepos } from '@/store/repos';
import { reposApi } from '@/lib/api/git';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { OutputList, RunBadge, useFollowOutput } from './CellOutputs';
import {
  cellLanguage,
  devallMeta,
  fromNbOutputs,
  kernelLanguage,
  monacoLanguage,
  newCell,
  parseNotebook,
  serializeNotebook,
  shortHash,
  withDevallMeta,
  withLanguage,
  type CellType,
  type EditCell,
  type NotebookDoc,
} from './nbformat';
import { beginRun, clearRun, endRun, failRun, nextExecutionCount, pushEvent, runKey, useRuns } from './runStore';
import './notebooks.css';

const SAVE_MS = 800;
type Json = Record<string, unknown>;

/** Opens `http/…` and `notes/…` links from rendered Markdown cells; reveals `repo:<name>/…` ones. */
function useLinkOpener() {
  const go = useNav((s) => s.go);
  return useCallback(
    (p: string) => {
      const root = useWorkspace.getState().project?.root;
      if (p.startsWith('repo:') && root) {
        void reposApi
          .resolve(root, p)
          .then((abs) => revealItemInDir(abs))
          .catch((e) => toast.error(`Could not show ${p}`, errorMessage(e)));
      } else if (p.startsWith('http/')) {
        void useWorkspace.getState().openRequest(p.slice(5));
      } else if (p.startsWith('notes/')) go('notes', p.slice(6));
      else if (p.endsWith('.md') || p.endsWith('.ipynb')) go('notes', p);
      else toast.info('Unknown link', p);
    },
    [go],
  );
}

interface CellActions {
  update(id: string, f: (c: EditCell) => EditCell): void;
  run(id: string, advance?: boolean): void;
  stop(id: string): void;
  clear(id: string): void;
  move(id: string, delta: number): void;
  insert(at: string, where: 'above' | 'below', type: CellType): void;
  remove(id: string): void;
  convert(id: string, type: CellType): void;
  toggleSecrets(id: string): void;
  select(id: string): void;
  setEditing(id: string, on: boolean): void;
  save(): void;
  openLink(path: string): void;
}

/**
 * Jupyter-style editor for a `.ipynb` note: Markdown and code cells (shell, PowerShell, cmd,
 * JavaScript, Python) that run on this machine, streaming output. Sources autosave through
 * `notesApi.save`; outputs are saved only when the notebook's "Save outputs" is on
 * (`metadata.devall.saveOutputs`). Unknown notebook/cell fields are preserved.
 */
export function NotebookEditor({ root, path, onSaved }: { root: string; path: string; onSaved?(): void }) {
  const [doc, setDoc] = useState<NotebookDoc | null>(null);
  const [cells, setCells] = useState<EditCell[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [trust, setTrust] = useState<TrustStatus | null>(null);
  const [pending, setPending] = useState<string[] | null>(null);
  const [bannerHidden, setBannerHidden] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditingState] = useState<Set<string>>(() => new Set());
  const [secretCells, setSecretCells] = useState<Set<string>>(() => new Set());
  const [saved, setSaved] = useState(true);
  const [runningAll, setRunningAll] = useState(false);
  const [interps, setInterps] = useState<InterpreterStatus[] | null>(null);
  const openLink = useLinkOpener();
  const allRepos = useProjectRepos();
  const cwdRepos = useMemo(() => allRepos.filter((r) => r.state === 'found' && r.origin !== 'containing'), [allRepos]);

  const docRef = useRef<NotebookDoc | null>(null);
  const cellsRef = useRef<EditCell[]>([]);
  const trustRef = useRef<TrustStatus | null>(null);
  const secretRef = useRef(secretCells);
  const lastSaved = useRef('');
  const dirty = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const runAllToken = useRef<{ cancelled: boolean } | null>(null);
  docRef.current = doc;
  cellsRef.current = cells;
  trustRef.current = trust;
  secretRef.current = secretCells;

  const prefix = `${root}|${path}|`;
  const anyRunning = useRuns((s) => Object.entries(s.runs).some(([k, r]) => k.startsWith(prefix) && r.running));

  const saveOutputs = useMemo(() => {
    const flag = ((doc?.top.metadata as Json | undefined)?.devall as Json | undefined)?.saveOutputs;
    if (typeof flag === 'boolean') return flag;
    // Not set: keep outputs if the file already had some (e.g. a Jupyter notebook), else don't.
    return cells.some((c) => Array.isArray(c.raw.outputs) && (c.raw.outputs as unknown[]).length > 0);
  }, [doc, cells]);
  const saveOutputsRef = useRef(saveOutputs);
  saveOutputsRef.current = saveOutputs;

  // ---------- load ----------
  useEffect(() => {
    let cancelled = false;
    notesApi
      .read(root, path)
      .then((text) => {
        if (cancelled) return;
        try {
          const parsed = parseNotebook(text);
          lastSaved.current = text;
          setDoc(parsed.doc);
          setCells(parsed.cells);
          setLoadError(null);
          notebooksApi
            .trustStatus(root, path, text)
            .then((st) => {
              if (cancelled) return;
              setTrust(st);
              setSecretCells(new Set(st.secretCells));
            })
            .catch((e) => {
              if (cancelled) return;
              setTrust({ trusted: false, hash: '', secretCells: [] });
              toast.error('Could not check notebook trust', errorMessage(e));
            });
        } catch (e) {
          setLoadError(errorMessage(e));
        }
      })
      .catch((e) => !cancelled && setLoadError(errorMessage(e)));
    notebooksApi
      .interpreters()
      .then((s) => !cancelled && setInterps(s))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [root, path]);

  // ---------- save ----------
  const serialize = useCallback(
    () =>
      serializeNotebook(docRef.current!, cellsRef.current, {
        saveOutputs: saveOutputsRef.current,
        outputs: (id) => {
          const r = useRuns.getState().runs[runKey(root, path, id)];
          return r ? { outputs: r.outputs, executionCount: r.executionCount } : undefined;
        },
      }),
    [root, path],
  );

  const flush = useCallback(async () => {
    clearTimeout(timer.current);
    if (!docRef.current || !dirty.current) return;
    dirty.current = false;
    const text = serialize();
    if (text === lastSaved.current) {
      setSaved(true);
      return;
    }
    try {
      await notesApi.save(root, path, text);
      lastSaved.current = text;
      setSaved(true);
      onSaved?.();
      // Your own edits stay trusted (the trust hash follows the saved code).
      if (trustRef.current?.trusted) notebooksApi.trust(root, path, text).then(setTrust).catch(() => {});
    } catch (e) {
      dirty.current = true;
      toast.error('Save failed', errorMessage(e));
    }
  }, [root, path, onSaved, serialize]);

  const markDirty = useCallback(() => {
    dirty.current = true;
    setSaved(false);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => void flush(), SAVE_MS);
  }, [flush]);

  const flushRef = useRef(flush);
  flushRef.current = flush;
  useEffect(() => () => void flushRef.current(), []);

  const setTopMeta = (patch: Json) => {
    setDoc((d) => {
      if (!d) return d;
      const md = { ...((d.top.metadata as Json) ?? {}) };
      const dv: Json = { ...((md.devall as Json) ?? {}), ...patch };
      for (const k of Object.keys(dv)) if (dv[k] === undefined) delete dv[k];
      if (Object.keys(dv).length) md.devall = dv;
      else delete md.devall;
      const next = { ...d, top: { ...d.top, metadata: md } };
      docRef.current = next;
      return next;
    });
    markDirty();
  };

  const mutateCells = useCallback(
    (f: (cs: EditCell[]) => EditCell[]) => {
      const next = f(cellsRef.current);
      cellsRef.current = next;
      setCells(next);
      markDirty();
    },
    [markDirty],
  );

  // ---------- running ----------
  const runOne = useCallback(
    async (id: string): Promise<boolean> => {
      const cell = cellsRef.current.find((c) => c.id === id);
      const top = docRef.current?.top;
      if (!cell || cell.type !== 'code' || !top) return true;
      const key = runKey(root, path, id);
      const { language, code } = cellLanguage(cell, top);
      const runId = crypto.randomUUID();
      if (!language) {
        beginRun(key, runId, null);
        failRun(key, runId, 'Pick a language for this cell (Shell, PowerShell, cmd, JavaScript or Python).');
        return false;
      }
      const count = nextExecutionCount(root, path);
      beginRun(key, runId, count);
      const secrets = secretRef.current.has(id);
      try {
        // The grant is pinned to the code; your own edits in this session keep it.
        if (secrets) await notebooksApi.allowSecrets(root, path, id, code, language, true);
        const cwd = ((top.metadata as Json | undefined)?.devall as Json | undefined)?.cwd;
        const r = await notebooksApi.runCell(
          root,
          path,
          id,
          code,
          language,
          useWorkspace.getState().activeEnv()?.id,
          { runId, templating: devallMeta(cell).templating === true, secrets, cwd: typeof cwd === 'string' ? cwd : undefined, executionCount: count },
          (e) => pushEvent(key, runId, e),
        );
        endRun(key, runId, r);
        if (saveOutputsRef.current) markDirty();
        return r.ok;
      } catch (e) {
        failRun(key, runId, errorMessage(e));
        return false;
      }
    },
    [root, path, markDirty],
  );

  const runMany = useCallback(
    async (ids: string[]) => {
      if (ids.length === 1) {
        await runOne(ids[0]);
        return;
      }
      const token = { cancelled: false };
      runAllToken.current = token;
      setRunningAll(true);
      for (const id of ids) {
        if (token.cancelled) break;
        // Like Jupyter, Run all stops at the first failing cell.
        if (!(await runOne(id))) break;
      }
      if (runAllToken.current === token) {
        runAllToken.current = null;
        setRunningAll(false);
      }
    },
    [runOne],
  );

  const requestRun = useCallback(
    (ids: string[]) => {
      if (!ids.length) return;
      if (!trustRef.current?.trusted) {
        setPending(ids);
        setBannerHidden(false);
        return;
      }
      void runMany(ids);
    },
    [runMany],
  );

  const trustNotebook = async () => {
    try {
      // Pending runs start from the effect above once the new status is in.
      setTrust(await notebooksApi.trust(root, path, serialize()));
    } catch (e) {
      toast.error('Could not trust the notebook', errorMessage(e));
    }
  };

  // A run requested while the trust check was still loading proceeds once it says "trusted".
  useEffect(() => {
    if (trust?.trusted && pending) {
      setPending(null);
      void runMany(pending);
    }
  }, [trust, pending, runMany]);

  const runAll = () => requestRun(cellsRef.current.filter((c) => c.type === 'code' && c.source.trim()).map((c) => c.id));

  const interruptAll = () => {
    if (runAllToken.current) runAllToken.current.cancelled = true;
    for (const [k, r] of Object.entries(useRuns.getState().runs)) if (k.startsWith(prefix) && r.running) void notebooksApi.interrupt(r.runId);
  };

  const restart = async () => {
    try {
      const n = await notebooksApi.restart(root, path);
      toast.success(n ? 'Kernels restarted' : 'No kernels were running', n ? 'JavaScript and Python variables were cleared.' : undefined);
    } catch (e) {
      toast.error('Restart failed', errorMessage(e));
    }
  };

  const clearAll = () => {
    for (const c of cellsRef.current) if (c.type === 'code') clearRun(runKey(root, path, c.id));
    mutateCells((cs) => cs.map((c) => (c.type === 'code' ? { ...c, raw: { ...c.raw, outputs: [], execution_count: null } } : c)));
  };

  // ---------- cell actions ----------
  const defaultLanguage = (): RunLanguage => (docRef.current && kernelLanguage(docRef.current.top)) || 'shell';
  const actions: CellActions = {
    update: (id, f) => mutateCells((cs) => cs.map((c) => (c.id === id ? f(c) : c))),
    run: (id, advance) => {
      const cs = cellsRef.current;
      const i = cs.findIndex((c) => c.id === id);
      if (cs[i]?.type === 'code') requestRun([id]);
      else setEditing(id, false);
      if (advance) {
        if (i === cs.length - 1) {
          const c = newCell('code', defaultLanguage());
          mutateCells((x) => [...x, c]);
          setSelected(c.id);
        } else if (i >= 0) setSelected(cs[i + 1].id);
      }
    },
    stop: (id) => {
      const r = useRuns.getState().runs[runKey(root, path, id)];
      if (r?.running) void notebooksApi.interrupt(r.runId);
    },
    clear: (id) => {
      clearRun(runKey(root, path, id));
      mutateCells((cs) => cs.map((c) => (c.id === id ? { ...c, raw: { ...c.raw, outputs: [], execution_count: null } } : c)));
    },
    move: (id, delta) =>
      mutateCells((cs) => {
        const i = cs.findIndex((c) => c.id === id);
        const j = i + delta;
        if (i < 0 || j < 0 || j >= cs.length) return cs;
        const next = [...cs];
        [next[i], next[j]] = [next[j], next[i]];
        return next;
      }),
    insert: (at, where, type) => {
      const c = newCell(type, type === 'code' ? defaultLanguage() : null);
      mutateCells((cs) => {
        const i = cs.findIndex((x) => x.id === at);
        const pos = i < 0 ? cs.length : where === 'above' ? i : i + 1;
        return [...cs.slice(0, pos), c, ...cs.slice(pos)];
      });
      setSelected(c.id);
      if (type === 'markdown') setEditing(c.id, true);
    },
    remove: async (id) => {
      const c = cellsRef.current.find((x) => x.id === id);
      if (c?.source.trim() && !(await confirmDialog({ title: 'Delete this cell?', message: 'Its content is removed from the notebook.', confirmText: 'Delete', danger: true }))) return;
      actions.stop(id);
      mutateCells((cs) => cs.filter((x) => x.id !== id));
    },
    convert: (id, type) =>
      mutateCells((cs) =>
        cs.map((c) => {
          if (c.id !== id || c.type === type) return c;
          const raw: Json = { ...c.raw, cell_type: type };
          if (type === 'code') {
            raw.outputs = [];
            raw.execution_count = null;
            return withLanguage({ ...c, type, raw }, defaultLanguage());
          }
          delete raw.outputs;
          delete raw.execution_count;
          return { ...c, type, raw };
        }),
      ),
    toggleSecrets: async (id) => {
      const c = cellsRef.current.find((x) => x.id === id);
      const top = docRef.current?.top;
      if (!c || !top) return;
      const { language, code } = cellLanguage(c, top);
      if (!language) return;
      const on = !secretRef.current.has(id);
      if (on) {
        const ok = await confirmDialog({
          title: 'Give this cell the environment’s secrets?',
          message:
            'When it runs, each secret of the active environment is available as DEVALL_SECRET_<NAME> (and {{secret.NAME}} when variable substitution is on). Secret values are masked in the output, but the code can still send them anywhere. This applies only on this machine and only to the cell’s current code.',
          confirmText: 'Allow secrets',
        });
        if (!ok) return;
      }
      try {
        await notebooksApi.allowSecrets(root, path, id, code, language, on);
        setSecretCells((s) => {
          const n = new Set(s);
          if (on) n.add(id);
          else n.delete(id);
          return n;
        });
      } catch (e) {
        toast.error('Could not change secrets', errorMessage(e));
      }
    },
    select: (id) => setSelected(id),
    setEditing: (id, on) => setEditing(id, on),
    save: () => {
      dirty.current = true;
      void flush();
    },
    openLink,
  };
  const actionsRef = useRef(actions);
  actionsRef.current = actions;
  // Stable facade so memoized cells don't re-render on every editor render.
  const stableActions = useMemo<CellActions>(
    () =>
      Object.fromEntries((Object.keys(actions) as (keyof CellActions)[]).map((k) => [k, (...a: unknown[]) => (actionsRef.current[k] as (...x: unknown[]) => unknown)(...a)])) as unknown as CellActions,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  function setEditing(id: string, on: boolean) {
    setEditingState((s) => {
      if (s.has(id) === on) return s;
      const n = new Set(s);
      if (on) n.add(id);
      else n.delete(id);
      return n;
    });
  }

  const addAtEnd = (type: CellType) => {
    const c = newCell(type, type === 'code' ? defaultLanguage() : null);
    mutateCells((cs) => [...cs, c]);
    setSelected(c.id);
    if (type === 'markdown') setEditing(c.id, true);
  };

  /** `metadata.devall.cwd`: project-relative, or `repo:<name>[/sub]` (one of the project's repositories). */
  const applyWorkingDir = (v: string) => {
    setTopMeta({ cwd: v.trim() ? v.trim().replace(/\\/g, '/') : undefined });
    void notebooksApi.restart(root, path).catch(() => {});
  };
  const setWorkingDir = async () => {
    const cur = ((docRef.current?.top.metadata as Json | undefined)?.devall as Json | undefined)?.cwd;
    const names = cwdRepos.map((r) => r.name);
    const v = await promptDialog({
      title: 'Working directory',
      label: `Folder the cells run in, relative to the project root${names.length ? `, or repo:<name>/<folder> (${names.join(', ')})` : ''} — empty = project root`,
      initial: typeof cur === 'string' ? cur : '',
      placeholder: names.length ? `e.g. repo:${names[0]} or apps/web` : 'e.g. apps/web',
      validate: (x) =>
        /(^|[\\/])\.\.([\\/]|$)/.test(x) || /^([a-zA-Z]:|[\\/])/.test(x.trim())
          ? 'Use a folder inside the project or one of its repositories'
          : x.trim().startsWith('repo:') && !names.includes(x.trim().slice(5).split(/[\\/]/)[0])
            ? `Unknown repository${names.length ? ` (${names.join(', ')})` : ''}`
            : undefined,
    });
    if (v !== null) applyWorkingDir(v);
  };

  const setInterpreterPath = async (s: InterpreterStatus) => {
    const v = await promptDialog({
      title: `${LANGUAGE_LABELS[s.language]} interpreter`,
      label: 'Path to the program (empty = detect automatically)',
      initial: s.customPath ?? '',
      placeholder: s.interpreter?.program ?? '',
    });
    if (v === null) return;
    try {
      const updated = await notebooksApi.setInterpreter(s.language, v.trim() || null);
      setInterps((all) => (all ?? []).map((x) => (x.language === s.language ? updated : x)));
      if (updated.error) toast.error(`${LANGUAGE_LABELS[s.language]}`, updated.error);
      else toast.success(`${LANGUAGE_LABELS[s.language]}: ${updated.interpreter?.name} ${updated.interpreter?.version ?? ''}`.trim());
      void notebooksApi.restart(root, path).catch(() => {});
    } catch (e) {
      toast.error('Could not set the interpreter', errorMessage(e));
    }
  };

  const menuItems = (): MenuItemDef[] => [
    { label: 'Add code cell', icon: <Code />, onSelect: () => addAtEnd('code') },
    { label: 'Add Markdown cell', icon: <Type />, onSelect: () => addAtEnd('markdown') },
    { separator: true },
    {
      label: 'Save outputs in the notebook',
      icon: saveOutputs ? <Check /> : <Save />,
      hint: saveOutputs ? 'on' : 'off',
      onSelect: () => setTopMeta({ saveOutputs: !saveOutputs }),
    },
    { label: 'Working directory…', icon: <FolderCog />, hint: (((doc?.top.metadata as Json | undefined)?.devall as Json | undefined)?.cwd as string) || 'project root', onSelect: setWorkingDir },
    // Multi-repo projects: run the cells in one of the repositories (wherever it is on this machine).
    ...(cwdRepos.length > 0 && cwdRepos.length <= 8
      ? cwdRepos.map((r) => {
          const cwd = `repo:${r.name}`;
          const on = (((doc?.top.metadata as Json | undefined)?.devall as Json | undefined)?.cwd as string | undefined) === cwd;
          return { label: `Run in ${r.name}`, icon: <GitBranch />, hint: on ? <Check size={13} /> : r.group ?? undefined, onSelect: () => applyWorkingDir(on ? '' : cwd) };
        })
      : []),
    { label: 'Shut down kernels', icon: <Square />, onSelect: () => void notebooksApi.shutdown(root, path).then((n) => toast.info(n ? 'Kernels shut down' : 'No kernels were running')) },
    { separator: true },
    { heading: 'Interpreters' },
    ...(interps ?? []).map((s) => ({
      label: `${LANGUAGE_LABELS[s.language]}: ${s.interpreter ? `${s.interpreter.name}${s.interpreter.version ? ` ${s.interpreter.version}` : ''}` : 'not found'}`,
      icon: <Cpu />,
      hint: s.customPath ? 'custom' : undefined,
      onSelect: () => setInterpreterPath(s),
    })),
    ...(interps ? [] : [{ label: 'Detecting…', disabled: true }]),
    {
      label: 'Re-detect interpreters',
      icon: <RotateCcw />,
      onSelect: () =>
        void notebooksApi
          .interpreters(true)
          .then(setInterps)
          .catch(() => {}),
    },
  ];

  if (loadError)
    return (
      <div className="page">
        <div className="page-body">
          <Callout kind="danger">
            Could not open <code className="code-inline">notes/{path}</code>: {loadError}
          </Callout>
        </div>
      </div>
    );
  if (!doc) return null;

  const trusted = !!trust?.trusted;
  const top = doc.top;
  const shellNote = interps?.find((s) => s.language === 'shell')?.interpreter?.note;

  return (
    <div className="page nb-page">
      <div className="page-header nb-header">
        <span className="mono subtle xs truncate" title={`notes/${path}`}>
          notes/{path}
        </span>
        <span className="subtle xs nowrap">{saved ? 'Saved' : 'Saving…'}</span>
        <span className="spacer" />
        {trust &&
          (trusted ? (
            <span className="badge success" title="You trusted this version of the notebook on this machine">
              <ShieldCheck /> Trusted
            </span>
          ) : (
            <button className="badge warning nb-badge-btn" title="Cells won't run until you trust this notebook" onClick={() => setBannerHidden(false)}>
              <ShieldAlert /> Not trusted
            </button>
          ))}
        <button className="btn btn-sm btn-primary" onClick={runAll} disabled={runningAll || !cells.some((c) => c.type === 'code')} title="Run every code cell in order (stops at the first failure)">
          {runningAll ? <Spinner /> : <FastForward />} Run all
        </button>
        <button className="btn btn-sm" onClick={interruptAll} disabled={!anyRunning && !runningAll} title="Interrupt running cells">
          <Square /> Interrupt
        </button>
        <button className="icon-btn" onClick={restart} title="Restart kernels (clears JavaScript and Python variables)">
          <RotateCcw />
        </button>
        <button className="icon-btn" onClick={clearAll} title="Clear all outputs">
          <Eraser />
        </button>
        <Menu
          align="right"
          items={menuItems}
          trigger={(p) => (
            <button className="icon-btn" {...p} aria-label="Notebook menu" title="More">
              <Ellipsis />
            </button>
          )}
        />
      </div>
      {!trusted && trust && !bannerHidden && (
        <div className="nb-trust">
          <ShieldAlert />
          <div className="grow">
            <strong>This notebook comes from the project repo.</strong> Running its cells executes code on your machine. Review the cells first.
            {pending && pending.length > 0 && <span className="subtle"> ({pending.length === 1 ? 'one cell' : `${pending.length} cells`} waiting to run)</span>}
          </div>
          <button className="btn btn-sm btn-primary" onClick={trustNotebook}>
            <ShieldCheck /> {pending ? 'Trust & run' : 'Trust notebook'}
          </button>
          <button
            className="btn btn-sm btn-ghost"
            onClick={() => {
              setPending(null);
              setBannerHidden(true);
            }}
          >
            Cancel
          </button>
        </div>
      )}
      <div className="page-body nb-body" onClick={() => setSelected(null)}>
        {shellNote && cells.some((c) => c.type === 'code' && cellLanguage(c, top).language === 'shell') && (
          <Callout kind="info" className="mb-4">
            {shellNote}
          </Callout>
        )}
        {cells.map((c, i) => (
          <CellView
            key={c.id}
            root={root}
            path={path}
            cell={c}
            top={top}
            index={i}
            last={i === cells.length - 1}
            selected={selected === c.id}
            editing={editing.has(c.id)}
            secrets={secretCells.has(c.id)}
            actions={stableActions}
          />
        ))}
        <div className="nb-add-end">
          <button className="btn btn-sm btn-ghost" onClick={() => addAtEnd('code')}>
            <Plus /> Code
          </button>
          <button className="btn btn-sm btn-ghost" onClick={() => addAtEnd('markdown')}>
            <Plus /> Markdown
          </button>
          <span className="subtle xs">
            <kbd className="kbd">Shift</kbd>+<kbd className="kbd">Enter</kbd> run &amp; advance · <kbd className="kbd">Ctrl</kbd>+<kbd className="kbd">Enter</kbd> run
          </span>
        </div>
      </div>
    </div>
  );
}

export default NotebookEditor;

const EXT: Record<string, string> = { shell: 'sh', powershell: 'ps1', cmd: 'cmd', javascript: 'js', python: 'py' };

interface CellViewProps {
  root: string;
  path: string;
  cell: EditCell;
  top: Json;
  index: number;
  last: boolean;
  selected: boolean;
  editing: boolean;
  secrets: boolean;
  actions: CellActions;
}

const CellView = memo(function CellView({ root, path, cell, top, index, last, selected, editing, secrets, actions }: CellViewProps) {
  const run = useRuns((s) => s.runs[runKey(root, path, cell.id)]);
  const loaded = useMemo(() => fromNbOutputs(cell.raw.outputs), [cell.raw.outputs]);
  const isCode = cell.type === 'code';
  const { language, magic } = isCode ? cellLanguage(cell, top) : { language: null, magic: false };
  const running = !!run?.running;
  const outputs = run ? run.outputs : loaded;
  const count = run ? (running ? '*' : run.executionCount ?? ' ') : ((cell.raw.execution_count as number | null) ?? ' ');
  const templating = devallMeta(cell).templating === true;
  const outRef = useFollowOutput(outputs);

  const onKeyDownCapture = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      e.stopPropagation();
      actions.run(cell.id, true);
    } else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      e.stopPropagation();
      actions.run(cell.id, false);
    } else if (e.key === 'Escape' && !isCode && editing) {
      e.preventDefault();
      e.stopPropagation();
      actions.setEditing(cell.id, false);
    }
  };

  const menu = (): MenuItemDef[] => [
    { label: 'Insert code cell above', icon: <ArrowUp />, onSelect: () => actions.insert(cell.id, 'above', 'code') },
    { label: 'Insert code cell below', icon: <ArrowDown />, onSelect: () => actions.insert(cell.id, 'below', 'code') },
    { label: 'Insert Markdown cell below', icon: <Type />, onSelect: () => actions.insert(cell.id, 'below', 'markdown') },
    { separator: true },
    ...(isCode
      ? [
          {
            label: templating ? 'Stop substituting {{variables}}' : 'Substitute {{variables}} before running',
            icon: <Braces />,
            onSelect: () => actions.update(cell.id, (c) => withDevallMeta(c, { templating: templating ? undefined : true })),
          },
          { label: secrets ? 'Stop giving this cell secrets' : 'Give this cell the environment’s secrets…', icon: <KeyRound />, disabled: !language, onSelect: () => actions.toggleSecrets(cell.id) },
          { label: 'Convert to Markdown', icon: <Type />, onSelect: () => actions.convert(cell.id, 'markdown') },
          { label: 'Clear output', icon: <Eraser />, onSelect: () => actions.clear(cell.id) },
        ]
      : [{ label: 'Convert to code', icon: <Code />, onSelect: () => actions.convert(cell.id, 'code') }]),
    { separator: true },
    { label: 'Delete cell', icon: <Trash2 />, danger: true, onSelect: () => actions.remove(cell.id) },
  ];

  const tools = (
    <div className="nb-cell-tools">
      <button className="icon-btn sm" title="Move up" disabled={index === 0} onClick={() => actions.move(cell.id, -1)}>
        <ArrowUp />
      </button>
      <button className="icon-btn sm" title="Move down" disabled={last} onClick={() => actions.move(cell.id, 1)}>
        <ArrowDown />
      </button>
      {isCode && outputs.length > 0 && !running && (
        <button className="icon-btn sm" title="Clear output" onClick={() => actions.clear(cell.id)}>
          <Eraser />
        </button>
      )}
      <button className="icon-btn sm danger" title="Delete cell" onClick={() => actions.remove(cell.id)}>
        <Trash2 />
      </button>
      <Menu
        align="right"
        items={menu}
        trigger={(p) => (
          <button className="icon-btn sm" {...p} title="Cell actions" aria-label="Cell actions">
            <Ellipsis />
          </button>
        )}
      />
    </div>
  );

  return (
    <div
      className={clsx('nb-cell', cell.type, selected && 'selected', running && 'running')}
      onClick={(e) => {
        e.stopPropagation();
        actions.select(cell.id);
      }}
      onKeyDownCapture={onKeyDownCapture}
    >
      <div className="nb-gutter">
        {isCode && (
          <>
            {running ? (
              <button className="icon-btn sm nb-run stop" title="Stop (interrupt)" onClick={() => actions.stop(cell.id)}>
                <Square />
              </button>
            ) : (
              <button className="icon-btn sm nb-run" title="Run cell (Ctrl+Enter)" onClick={() => actions.run(cell.id)}>
                <Play />
              </button>
            )}
            <span className="nb-count mono">[{count}]</span>
          </>
        )}
      </div>
      <div className="nb-main">
        {isCode ? (
          <>
            <div className="nb-cell-bar">
              <select
                className="select sm nb-lang-select"
                value={language ?? ''}
                onChange={(e) => actions.update(cell.id, (c) => withLanguage(c, e.target.value as RunLanguage))}
                title={magic ? 'Set by a %% magic on the first line; picking a language removes it' : 'Cell language (metadata.devall.language)'}
              >
                {!language && <option value="">Language…</option>}
                {RUN_LANGUAGES.map((l) => (
                  <option key={l} value={l}>
                    {LANGUAGE_LABELS[l]}
                  </option>
                ))}
              </select>
              {magic && (
                <span className="badge outline" title="Language set by a %% magic on the first line">
                  %%
                </span>
              )}
              {templating && (
                <span className="badge accent" title="{{variables}} are substituted before the cell runs">
                  <Braces /> vars
                </span>
              )}
              {secrets && (
                <span className="badge warning" title="This cell receives the active environment's secrets">
                  <KeyRound /> secrets
                </span>
              )}
              {run?.interpreter && <span className="subtle xs truncate">{run.interpreter}</span>}
              <span className="spacer" />
              {run && <RunBadge run={run} />}
              {tools}
            </div>
            <div className="nb-editor">
              <CodeEditor
                value={cell.source}
                onChange={(v) => actions.update(cell.id, (c) => ({ ...c, source: v }))}
                language={monacoLanguage(language)}
                path={`notebook/${shortHash(root)}/${path}/${cell.id}.${language ? EXT[language] : 'txt'}`}
                autoHeight={{ min: 38, max: 720 }}
                minimal
                onSave={actions.save}
              />
            </div>
            {(outputs.length > 0 || running) && (
              <div className="nb-cell-out" ref={outRef}>
                {running && outputs.length === 0 && (
                  <div className="nb-notice">
                    <Spinner /> Running…
                  </div>
                )}
                <OutputList outputs={outputs} />
              </div>
            )}
          </>
        ) : (
          <>
            <div className="nb-md-tools">{tools}</div>
            {editing ? (
              <div className="nb-editor">
                <CodeEditor
                  value={cell.source}
                  onChange={(v) => actions.update(cell.id, (c) => ({ ...c, source: v }))}
                  language={cell.type === 'markdown' ? 'markdown' : 'plaintext'}
                  path={`notebook/${shortHash(root)}/${path}/${cell.id}.md`}
                  autoHeight={{ min: 38, max: 720 }}
                  minimal
                  onSave={actions.save}
                />
                <div className="row-sm nb-md-done">
                  <button className="btn btn-sm" onClick={() => actions.setEditing(cell.id, false)}>
                    <Check /> Done
                  </button>
                  <span className="subtle xs">Shift+Enter or Esc</span>
                </div>
              </div>
            ) : (
              <div className="nb-md" onDoubleClick={() => actions.setEditing(cell.id, true)} title="Double-click to edit">
                {cell.source.trim() ? (
                  cell.type === 'markdown' ? (
                    <Markdown source={cell.source} onProjectLink={actions.openLink} />
                  ) : (
                    <pre className="nb-stream">{cell.source}</pre>
                  )
                ) : (
                  <span className="subtle small">Empty {cell.type === 'markdown' ? 'Markdown' : 'raw'} cell — double-click to edit</span>
                )}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
});

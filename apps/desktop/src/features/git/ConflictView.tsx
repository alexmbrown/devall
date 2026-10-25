import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { CheckCheck, ChevronDown, ChevronUp, Columns3, FileWarning, RotateCcw, Save, Trash2 } from 'lucide-react';
import type { monaco } from '@/components/editor/monaco';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Callout, Empty, Spinner, formatBytes } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type Conflict, type ConflictPick } from '@/lib/api/git';
import { diffLanguage } from './DiffPane';
import { chunkAt, nearestChunk, parseConflicts, resolveAll, resolveChunk, type ChunkChoice } from './conflictMarkers';

/**
 * The merge-conflict editor (free): ours | theirs — and the common base on demand — read from the
 * index stages, above an editable result that starts as the working-tree file with git's
 * markers. Each conflict chunk can take ours, theirs, both (either order) or the base; or edit
 * by hand. Next/previous conflict (also Alt+↓/↑), re-create the markers, take a whole side, and
 * "Mark resolved" writes the result and stages it. Binary and deleted-on-one-side conflicts pick
 * a side.
 */
export function ConflictView({ root, repo, path, version, onChanged }: { root: string; repo: string | null; path: string; version: unknown; onChanged(): void }) {
  const [c, setC] = useState<Conflict | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState('');
  const [dirty, setDirty] = useState(false);
  const [showBase, setShowBase] = useState(false);
  const [busy, setBusy] = useState(false);
  const [cursor, setCursor] = useState(1);
  const editor = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const token = useRef(0);

  const load = useCallback(async () => {
    const t = ++token.current;
    try {
      const x = await gitApi.conflictRead(root, repo, path);
      if (t !== token.current) return;
      setC(x);
      setError(null);
      setText(x.result.text ?? '');
      setDirty(false);
      setShowBase((b) => b || false);
    } catch (e) {
      if (t === token.current) setError(errorMessage(e));
    }
  }, [root, repo, path]);

  useEffect(() => {
    setC(null);
    void load();
  }, [load]);
  // Reload on outside changes unless the user is editing.
  useEffect(() => {
    if (!dirty) void load();
  }, [version]); // eslint-disable-line react-hooks/exhaustive-deps

  const chunks = useMemo(() => parseConflicts(text), [text]);
  const here = chunkAt(chunks, cursor);
  const active = here >= 0 ? here : chunks.length ? nearestChunk(chunks, cursor, 1) : -1;

  const reveal = (i: number) => {
    const ch = chunks[i];
    const ed = editor.current;
    if (!ch || !ed) return;
    ed.revealLineInCenter(ch.start + 1);
    ed.setPosition({ lineNumber: ch.start + 1, column: 1 });
    ed.focus();
    setCursor(ch.start + 1);
  };
  const nav = (d: 1 | -1) => {
    const i = nearestChunk(chunks, cursor, d);
    if (i >= 0) reveal(i);
  };
  const choose = (choice: ChunkChoice) => {
    if (active < 0) return;
    setText((t) => resolveChunk(t, active, choice));
    setDirty(true);
  };

  const onMount = (ed: monaco.editor.IStandaloneCodeEditor) => {
    editor.current = ed;
    ed.onDidChangeCursorPosition((e) => setCursor(e.position.lineNumber));
  };

  // Alt+↓ / Alt+↑ next / previous conflict.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.altKey || (e.key !== 'ArrowDown' && e.key !== 'ArrowUp')) return;
      if (!(e.target as HTMLElement | null)?.closest?.('.git-conflict')) return;
      e.preventDefault();
      nav(e.key === 'ArrowDown' ? 1 : -1);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  const act = async (label: string, fn: () => Promise<unknown>, success: string) => {
    setBusy(true);
    try {
      await fn();
      toast.success(success);
      setDirty(false);
    } catch (e) {
      toast.error(`${label} failed`, errorMessage(e));
    } finally {
      setBusy(false);
      onChanged();
      void load();
    }
  };

  const save = async (mark: boolean) => {
    if (mark && parseConflicts(text).length) {
      const ok = await confirmDialog({ title: 'Conflict markers are still in the file', message: `${chunks.length} conflict${chunks.length === 1 ? '' : 's'} not resolved yet. Mark ${path} resolved with the markers in it?`, confirmText: 'Mark resolved anyway', danger: true });
      if (!ok) return;
    }
    await act(mark ? 'Mark resolved' : 'Save', () => gitApi.conflictSave(root, repo, path, text, mark), mark ? `${path} resolved` : 'Saved');
  };
  const take = (side: ConflictPick) => act('Take side', () => gitApi.conflictTake(root, repo, path, side), `${path}: took ${side === 'ours' ? c?.labels.ours : c?.labels.theirs}`);
  const recreate = async () => {
    if (dirty && !(await confirmDialog({ title: 'Re-create the conflict markers?', message: 'Your edits in the result are replaced by the file with git’s markers again.', confirmText: 'Re-create', danger: true }))) return;
    await act('Re-create markers', () => gitApi.conflictRecreate(root, repo, path), 'Markers re-created');
  };

  if (error) return <Empty icon={<FileWarning />} title="Couldn't read the conflict">{error}</Empty>;
  if (!c) return <Empty icon={<Spinner />} title="Reading the conflict…" />;

  const lang = diffLanguage(path, c.ours.text ?? c.theirs.text ?? '');
  const header = (
    <div className="toolbar git-diff-toolbar">
      <span className="badge danger shrink-0">conflict</span>
      <span className="mono xs truncate grow" title={path}>
        {path}
      </span>
      <span className="xs subtle shrink-0">{c.description}</span>
      {c.resolved && <span className="badge success shrink-0">resolved</span>}
    </div>
  );

  // Binary files and files one side deleted: pick a side.
  const oneSided = !c.ours.exists || !c.theirs.exists;
  if (c.binary || oneSided || c.result.tooLarge) {
    const side = (k: ConflictPick) => {
      const v = c[k];
      const label = k === 'ours' ? c.labels.ours : c.labels.theirs;
      return (
        <div className="card col-sm git-side-card">
          <span className="small strong">{k === 'ours' ? 'Ours' : 'Theirs'}</span>
          <span className="xs subtle">{label}</span>
          <span className="xs">{v.exists ? `${v.binary ? 'binary, ' : ''}${formatBytes(v.size)}` : 'deleted on this side'}</span>
          <button className="btn btn-sm" disabled={busy || c.resolved} onClick={() => void take(k)}>
            {v.exists ? `Keep ${k === 'ours' ? 'ours' : 'theirs'}` : 'Accept the deletion'}
          </button>
        </div>
      );
    };
    return (
      <div className="col h-full min-h-0 git-conflict" style={{ gap: 0 }}>
        {header}
        <div className="p-5 col">
          <Callout kind="warning">{c.binary ? 'A binary file can’t be merged line by line: keep one side.' : c.result.tooLarge ? 'This file is too large to edit here: keep one side, or resolve it in your editor and mark it resolved.' : `${c.description}: keep the file as one side has it, or delete it.`}</Callout>
          <div className="git-side-cards">
            {side('ours')}
            {side('theirs')}
          </div>
          <div className="row-sm">
            <button className="btn btn-sm btn-danger-ghost" disabled={busy || c.resolved} onClick={() => void act('Delete', () => gitApi.conflictRemove(root, repo, path), `${path} resolved as deleted`)}>
              <Trash2 /> Resolve as deleted
            </button>
            {!c.binary && (
              <button className="btn btn-sm" disabled={busy || c.resolved} onClick={() => void act('Mark resolved', () => gitApi.conflictMark(root, repo, [path]), `${path} resolved`)}>
                <CheckCheck /> Mark resolved as it is
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }

  const pane = (title: string, sub: string, value: string | null, cls: string) => (
    <div className={clsx('git-conflict-pane', cls)}>
      <div className="git-conflict-pane-head">
        <span className="small strong">{title}</span>
        <span className="xs subtle truncate">{sub}</span>
      </div>
      <div className="grow min-h-0">
        <CodeEditor value={value ?? ''} readOnly language={lang} path={`conflict-${cls}/${path}`} />
      </div>
    </div>
  );

  return (
    <div className="col h-full min-h-0 git-conflict" style={{ gap: 0 }}>
      {header}
      <div className="toolbar git-conflict-bar">
        <span className="xs strong shrink-0">{chunks.length ? `Conflict ${active + 1} of ${chunks.length}` : 'No markers left'}</span>
        <button className="icon-btn sm" title="Previous conflict (Alt+↑)" disabled={!chunks.length} onClick={() => nav(-1)}>
          <ChevronUp />
        </button>
        <button className="icon-btn sm" title="Next conflict (Alt+↓)" disabled={!chunks.length} onClick={() => nav(1)}>
          <ChevronDown />
        </button>
        <span className="git-conflict-sep" />
        <button className="btn btn-sm" disabled={active < 0} onClick={() => choose('ours')} title={c.labels.ours}>
          Take ours
        </button>
        <button className="btn btn-sm" disabled={active < 0} onClick={() => choose('theirs')} title={c.labels.theirs}>
          Take theirs
        </button>
        <button className="btn btn-sm" disabled={active < 0} onClick={() => choose('oursThenTheirs')}>
          Both (ours first)
        </button>
        <button className="btn btn-sm" disabled={active < 0} onClick={() => choose('theirsThenOurs')}>
          Both (theirs first)
        </button>
        {chunks[active]?.base && (
          <button className="btn btn-sm btn-ghost" onClick={() => choose('base')}>
            Base
          </button>
        )}
        <span className="spacer" />
        <button className={clsx('icon-btn sm', showBase && 'on')} title="Show the common base" disabled={!c.base.exists} onClick={() => setShowBase((b) => !b)}>
          <Columns3 />
        </button>
        <button className="icon-btn sm" title="Re-create the conflict markers" disabled={busy || c.resolved} onClick={() => void recreate()}>
          <RotateCcw />
        </button>
      </div>
      <div className={clsx('git-conflict-inputs', showBase && 'three')}>
        {pane('Ours', c.labels.ours, c.ours.text, 'ours')}
        {showBase && pane('Base', c.labels.base, c.base.text, 'base')}
        {pane('Theirs', c.labels.theirs, c.theirs.text, 'theirs')}
      </div>
      <div className="git-conflict-result">
        <div className="git-conflict-pane-head">
          <span className="small strong">Result</span>
          <span className="xs subtle grow">{dirty ? 'edited — not saved yet' : 'the working-tree file'}</span>
          <button className="btn btn-sm btn-ghost" onClick={() => (setText(resolveAll(text, 'ours')), setDirty(true))} title="Resolve every remaining conflict with ours">
            All ours
          </button>
          <button className="btn btn-sm btn-ghost" onClick={() => (setText(resolveAll(text, 'theirs')), setDirty(true))} title="Resolve every remaining conflict with theirs">
            All theirs
          </button>
          <button className="btn btn-sm btn-ghost" disabled={busy || !c.ours.exists} onClick={() => (setText(c.ours.text ?? ''), setDirty(true))}>
            Whole file: ours
          </button>
          <button className="btn btn-sm btn-ghost" disabled={busy || !c.theirs.exists} onClick={() => (setText(c.theirs.text ?? ''), setDirty(true))}>
            Whole file: theirs
          </button>
          <button className="btn btn-sm" disabled={busy || !dirty} onClick={() => void save(false)}>
            <Save /> Save
          </button>
          <button className="btn btn-sm btn-primary" disabled={busy} onClick={() => void save(true)} title="Write the result and stage it (git add)">
            {busy ? <Spinner /> : <CheckCheck />} Mark resolved
          </button>
        </div>
        <div className="grow min-h-0">
          <CodeEditor
            value={text}
            language={lang}
            path={`conflict-result/${path}`}
            onChange={(v) => {
              setText(v);
              setDirty(true);
            }}
            onSave={() => void save(false)}
            onMount={onMount}
          />
        </div>
      </div>
    </div>
  );
}

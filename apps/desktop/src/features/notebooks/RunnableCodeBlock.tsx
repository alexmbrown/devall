import { useCallback, useState } from 'react';
import { Eraser, Play, ShieldAlert, Square } from 'lucide-react';
import type { CodeBlockInfo } from '@/components/Markdown';
import { CopyButton, Spinner } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { LANGUAGE_LABELS, normalizeLanguage, notebooksApi, type RunLanguage } from '@/lib/api/notebooks';
import { useWorkspace } from '@/store/workspace';
import { OutputList, RunBadge, useFollowOutput } from './CellOutputs';
import { shortHash } from './nbformat';
import { beginRun, clearRun, endRun, failRun, pushEvent, runKey, useRuns } from './runStore';
import './notebooks.css';

/**
 * A fenced code block in a Markdown note with ▶ Run and an inline output panel. Runs from the
 * project root with the active environment's variables (`DEVALL_<name>`); JavaScript and Python
 * blocks of one note share a kernel. The first run of a block asks for consent (trust is per
 * block and per code version, stored outside the project).
 */
export function RunnableCodeBlock({ root, notePath, block, controlsOnly }: { root: string; notePath: string; block: CodeBlockInfo; controlsOnly?: boolean }) {
  const language = normalizeLanguage(block.language) as RunLanguage;
  // Keyed by content: blocks move when the note is edited.
  const key = runKey(root, notePath, `block:${shortHash(`${language}\n${block.code}`)}`);
  const run = useRuns((s) => s.runs[key]);
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const running = !!run?.running;
  const outRef = useFollowOutput(run?.outputs);

  const go = async () => {
    setAsking(false);
    const runId = crypto.randomUUID();
    beginRun(key, runId, null);
    try {
      const env = useWorkspace.getState().activeEnv()?.id;
      const r = await notebooksApi.runSnippet(root, notePath, language, block.code, env, runId, (e) => pushEvent(key, runId, e));
      endRun(key, runId, r);
    } catch (e) {
      failRun(key, runId, errorMessage(e));
    }
  };

  const start = async () => {
    setBusy(true);
    try {
      const st = await notebooksApi.trustStatus(root, notePath, block.code, language);
      if (st.trusted) await go();
      else setAsking(true);
    } catch (e) {
      const runId = crypto.randomUUID();
      beginRun(key, runId, null);
      failRun(key, runId, errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const trustAndRun = async () => {
    try {
      await notebooksApi.trust(root, notePath, block.code, language);
      await go();
    } catch (e) {
      setAsking(false);
      const runId = crypto.randomUUID();
      beginRun(key, runId, null);
      failRun(key, runId, errorMessage(e));
    }
  };

  return (
    <div className={controlsOnly ? 'nb-block controls-only' : 'nb-block'} onClick={(e) => e.stopPropagation()}>
      <div className="nb-block-bar">
        {!controlsOnly && <span className="nb-lang">{LANGUAGE_LABELS[language]}</span>}
        {run?.interpreter && <span className="subtle xs truncate">{run.interpreter}</span>}
        <span className="spacer" />
        {run && <RunBadge run={run} />}
        {run && !running && run.outputs.length > 0 && (
          <button className="icon-btn sm" title="Clear output" onClick={() => clearRun(key)}>
            <Eraser />
          </button>
        )}
        {!controlsOnly && <CopyButton text={block.code} title="Copy code" />}
        {running ? (
          <button className="btn btn-sm" onClick={() => notebooksApi.interrupt(run!.runId)} title="Stop (interrupt)">
            <Square /> Stop
          </button>
        ) : (
          <button className="btn btn-sm btn-primary" onClick={start} disabled={busy} title="Run on your machine, from the project folder">
            {busy ? <Spinner /> : <Play />} Run
          </button>
        )}
      </div>
      {!controlsOnly && (
        <pre className="nb-block-code">
          <code>{block.code}</code>
        </pre>
      )}
      {asking && (
        <div className="nb-trust inline">
          <ShieldAlert />
          <div className="grow">
            <strong>Run code from this note?</strong> It comes from the project repo; running it executes it on your machine. Review it first.
          </div>
          <button className="btn btn-sm btn-primary" onClick={trustAndRun}>
            Trust &amp; run
          </button>
          <button className="btn btn-sm btn-ghost" onClick={() => setAsking(false)}>
            Cancel
          </button>
        </div>
      )}
      {run && (running || run.outputs.length > 0) && (
        <div className="nb-block-out" ref={outRef}>
          {running && run.outputs.length === 0 && (
            <div className="nb-notice">
              <Spinner /> Running…
            </div>
          )}
          <OutputList outputs={run.outputs} />
        </div>
      )}
    </div>
  );
}

/**
 * `renderCodeBlock` for the `Markdown` component: fenced blocks in a runnable language
 * (bash/sh/zsh/shell, powershell/pwsh/ps1, cmd/bat, js/javascript/node, python/py) get ▶ Run;
 * others keep the default rendering.
 *
 * ```tsx
 * const renderCodeBlock = useRunnableCodeBlocks(project.root, notePath);
 * <Markdown source={body} onProjectLink={open} renderCodeBlock={renderCodeBlock} />
 * ```
 */
export function useRunnableCodeBlocks(root: string, notePath: string) {
  return useCallback(
    (block: CodeBlockInfo) => (block.code.trim() && normalizeLanguage(block.language) ? <RunnableCodeBlock key={`${block.index}`} root={root} notePath={notePath} block={block} /> : null),
    [root, notePath],
  );
}

/**
 * `codeBlockExtras` for the Markdown editor: the ▶ Run bar and output under a runnable code block
 * (the editor shows the code itself).
 */
export function useRunnableCodeExtras(root: string, notePath: string) {
  return useCallback(
    (block: { language: string; code: string }) => (block.code.trim() && normalizeLanguage(block.language) ? <RunnableCodeBlock root={root} notePath={notePath} block={{ ...block, index: 0 }} controlsOnly /> : null),
    [root, notePath],
  );
}

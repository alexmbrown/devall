import { useEffect, useState } from 'react';
import { Pause, Play, SkipForward, XOctagon } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Callout, Kbd, MOD } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { proxyApi, type PausedExchange } from '@/lib/api/proxy';
import { editFor, pausedText } from './format';

/** Edit a request or response held at a breakpoint as raw HTTP text, then resume or abort. */
export function BreakpointEditor({ paused }: { paused: PausedExchange }) {
  const [text, setText] = useState(() => pausedText(paused));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    setText(pausedText(paused));
    setError(null);
  }, [paused]);

  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  const left = Math.max(0, Math.round((paused.deadline - now) / 1000));

  const resume = async (unchanged = false) => {
    let edit = undefined;
    if (!unchanged) {
      const e = editFor(paused, text);
      if (typeof e === 'string') {
        setError(e);
        return;
      }
      edit = e;
    }
    setBusy(true);
    try {
      await proxyApi.resume(paused.id, edit);
    } catch (e) {
      toast.error('Could not resume', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const abort = async () => {
    setBusy(true);
    try {
      await proxyApi.abort(paused.id);
    } catch (e) {
      toast.error('Could not abort', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="insp-bp"
      onKeyDownCapture={(e) => {
        if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
          e.preventDefault();
          e.stopPropagation();
          void resume(e.shiftKey);
        }
      }}
    >
      <Callout kind="warning" className="m-3">
        <div className="row-sm">
          <Pause size={14} />
          <strong>Paused at the {paused.phase} breakpoint.</strong>
          <span className="grow" />
          <span className="xs mono">resumes unchanged in {left}s</span>
        </div>
        <div className="xs mt-2">
          Edit the {paused.phase === 'request' ? 'method, URL' : 'status'}, headers or body below. A changed body is sent uncompressed with a fresh Content-Length.
        </div>
      </Callout>
      {error && (
        <Callout kind="danger" className="m-3">
          {error}
        </Callout>
      )}
      <div className="insp-bp-editor">
        <CodeEditor key={`${paused.id}-${paused.phase}`} value={text} onChange={(v) => setText(v)} language="plaintext" path={`breakpoint-${paused.id}-${paused.phase}`} />
      </div>
      <div className="insp-bp-actions">
        <button className="btn btn-sm btn-danger-ghost" onClick={abort} disabled={busy} title="Abort: close the connection without forwarding">
          <XOctagon /> Abort
        </button>
        <span className="spacer" />
        <button className="btn btn-sm" onClick={() => resume(true)} disabled={busy} title={`Resume unchanged (${MOD}+Shift+Enter)`}>
          <SkipForward /> Resume unchanged
        </button>
        <button className="btn btn-sm btn-primary" onClick={() => resume(false)} disabled={busy} title={`Resume with edits (${MOD}+Enter)`}>
          <Play /> Resume <Kbd>{MOD}+↵</Kbd>
        </button>
      </div>
    </div>
  );
}

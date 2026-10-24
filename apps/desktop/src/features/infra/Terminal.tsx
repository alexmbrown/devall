import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { Copy, Eraser, Plug, RotateCcw, ScrollText, Square } from 'lucide-react';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { uid } from '@/lib/id';
import { infraApi, type ExecEvent } from '@/lib/api/infra';
import { TermBuffer, cssFor, keyToData, runs, styleOf, type TermLine } from './ansi';
import { useOpenInLogViewer } from './logViewer';

const Line = memo(
  function Line({ line, cursor }: { line: TermLine; v: number; cursor: number | null }) {
    const parts = runs(line);
    if (cursor === null) {
      return (
        <div className="term-line">
          {parts.length === 0 ? ' ' : parts.map((p, i) => <span key={i} style={cssFor(styleOf(p.style))}>{p.text}</span>)}
        </div>
      );
    }
    // Split the run under the cursor so the cursor cell can be drawn.
    const out: React.ReactNode[] = [];
    let pos = 0;
    let drawn = false;
    parts.forEach((p, i) => {
      const end = pos + p.text.length;
      if (!drawn && cursor >= pos && cursor < end) {
        const k = cursor - pos;
        const st = cssFor(styleOf(p.style));
        if (k > 0) out.push(<span key={`${i}a`} style={st}>{p.text.slice(0, k)}</span>);
        out.push(<span key={`${i}c`} className="term-cursor" style={st}>{p.text[k]}</span>);
        if (k + 1 < p.text.length) out.push(<span key={`${i}b`} style={st}>{p.text.slice(k + 1)}</span>);
        drawn = true;
      } else out.push(<span key={i} style={cssFor(styleOf(p.style))}>{p.text}</span>);
      pos = end;
    });
    if (!drawn) {
      if (cursor > pos) out.push(<span key="pad">{' '.repeat(cursor - pos)}</span>);
      out.push(<span key="cur" className="term-cursor">{' '}</span>);
    }
    return <div className="term-line">{out}</div>;
  },
  (a, b) => a.line === b.line && a.v === b.v && a.cursor === b.cursor,
);

export type ExecState = { phase: 'connecting' } | { phase: 'running' } | { phase: 'exited'; code: number | null; error: string | null };

interface TerminalProps {
  /** Starts the remote process; `onEvent` receives its output. */
  start(execId: string, onEvent: (e: ExecEvent) => void): Promise<void>;
  /** Shown in the toolbar and used when opening the transcript in the log viewer. */
  title: string;
  className?: string;
}

/**
 * Interactive exec terminal: keystrokes go straight to the remote TTY (Ctrl+C included; Ctrl+C
 * copies instead while text is selected), output is rendered with ANSI colors. Good for shells;
 * full-screen TUIs need xterm.js.
 */
export function ExecTerminal({ start, title, className }: TerminalProps) {
  const bufRef = useRef(new TermBuffer(5000));
  const [, setTick] = useState(0);
  const [state, setState] = useState<ExecState>({ phase: 'connecting' });
  const [focused, setFocused] = useState(false);
  const [session, setSession] = useState(0);
  const execId = useRef<string>('');
  const hostRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLSpanElement>(null);
  const atBottom = useRef(true);
  const size = useRef({ cols: 80, rows: 24 });
  const openInViewer = useOpenInLogViewer();
  const startRef = useRef(start);
  startRef.current = start;

  // Batch renders to one per frame.
  const scheduled = useRef(false);
  const redraw = useCallback(() => {
    if (scheduled.current) return;
    scheduled.current = true;
    requestAnimationFrame(() => {
      scheduled.current = false;
      setTick((t) => t + 1);
    });
  }, []);

  useEffect(() => {
    const id = `exec-${uid()}`;
    execId.current = id;
    const buf = bufRef.current;
    setState({ phase: 'connecting' });
    let live = true;
    startRef
      .current(id, (e) => {
        if (!live) return;
        if (e.type === 'output') {
          buf.write(e.data);
          setState((s) => (s.phase === 'connecting' ? { phase: 'running' } : s));
        } else {
          buf.write(`\r\n\x1b[2m[process exited${e.code !== null ? ` with code ${e.code}` : ''}${e.error ? `: ${e.error}` : ''}]\x1b[0m\r\n`);
          setState({ phase: 'exited', code: e.code, error: e.error });
        }
        redraw();
      })
      .then(() => {
        if (!live) return;
        setState((s) => (s.phase === 'connecting' ? { phase: 'running' } : s));
        void infraApi.execResize(id, size.current.cols, size.current.rows);
      })
      .catch((err) => {
        if (!live) return;
        const msg = errorMessage(err);
        buf.write(`\x1b[31m${msg}\x1b[0m\r\n`);
        setState({ phase: 'exited', code: null, error: msg });
        redraw();
      });
    return () => {
      live = false;
      void infraApi.streamStop(id);
    };
  }, [session, redraw]);

  // Fit cols/rows to the element.
  useEffect(() => {
    const el = scrollRef.current;
    const m = measureRef.current;
    if (!el || !m) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const fit = () => {
      const charW = m.getBoundingClientRect().width / 10 || 7.2;
      const lineH = m.getBoundingClientRect().height || 17;
      const cols = Math.max(20, Math.floor((el.clientWidth - 16) / charW));
      const rows = Math.max(5, Math.floor((el.clientHeight - 8) / lineH));
      if (cols === size.current.cols && rows === size.current.rows) return;
      size.current = { cols, rows };
      bufRef.current.screenRows = rows;
      clearTimeout(timer);
      timer = setTimeout(() => execId.current && void infraApi.execResize(execId.current, cols, rows), 120);
    };
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    fit();
    return () => {
      ro.disconnect();
      clearTimeout(timer);
    };
  }, []);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && atBottom.current) el.scrollTop = el.scrollHeight;
  });

  const send = (data: string) => {
    if (state.phase === 'exited' || !execId.current) return;
    atBottom.current = true;
    void infraApi.execInput(execId.current, data);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const sel = window.getSelection()?.toString();
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'c' && sel) return; // copy selection
    if ((e.ctrlKey && e.shiftKey && e.key.toLowerCase() === 'v') || (e.metaKey && e.key.toLowerCase() === 'v')) return; // paste event
    const data = keyToData(e, bufRef.current.appCursorKeys);
    if (data === null) return;
    e.preventDefault();
    send(data);
  };

  const onPaste = (e: React.ClipboardEvent) => {
    e.preventDefault();
    const text = e.clipboardData.getData('text').replace(/\r?\n/g, '\r');
    if (!text) return;
    send(bufRef.current.bracketedPaste ? `\x1b[200~${text}\x1b[201~` : text);
  };

  const buf = bufRef.current;
  const showCursor = focused && buf.cursorVisible && state.phase !== 'exited';
  const statusText = state.phase === 'connecting' ? 'Connecting…' : state.phase === 'running' ? 'Connected' : state.error ? `Exited: ${state.error}` : `Exited${state.code !== null ? ` (${state.code})` : ''}`;

  return (
    <div className={clsx('term', className)} ref={hostRef}>
      <div className="toolbar term-bar">
        <span className={clsx('dot', state.phase === 'running' ? 'ok' : state.phase === 'connecting' ? 'warn pulse' : 'err')} />
        <span className="small truncate grow" title={title}>
          {title} <span className="subtle">· {statusText}</span>
        </span>
        <button className="icon-btn sm" title="Send Ctrl+C" aria-label="Send Ctrl+C" onClick={() => send('\x03')} disabled={state.phase !== 'running'}>
          <Square />
        </button>
        <button className="icon-btn sm" title="Clear" aria-label="Clear" onClick={() => (buf.clear(), redraw())}>
          <Eraser />
        </button>
        <button
          className="icon-btn sm"
          title="Copy transcript"
          aria-label="Copy transcript"
          onClick={() => navigator.clipboard.writeText(buf.text()).then(() => toast.success('Transcript copied'), () => toast.error('Clipboard unavailable'))}
        >
          <Copy />
        </button>
        <button className="icon-btn sm" title="Open transcript in log viewer" aria-label="Open transcript in log viewer" onClick={() => openInViewer({ name: title, text: buf.text() })}>
          <ScrollText />
        </button>
        {state.phase === 'exited' && (
          <button className="btn btn-sm" onClick={() => (buf.clear(), setSession((n) => n + 1))}>
            <RotateCcw /> Reconnect
          </button>
        )}
      </div>
      <div
        ref={scrollRef}
        className={clsx('term-screen', focused && 'focused')}
        tabIndex={0}
        role="textbox"
        aria-label={`Terminal: ${title}`}
        aria-multiline
        onKeyDown={onKeyDown}
        onPaste={onPaste}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        onScroll={(e) => {
          const el = e.currentTarget;
          atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
      >
        <span ref={measureRef} className="term-measure" aria-hidden>
          MMMMMMMMMM
        </span>
        <div key={buf.generation}>
          {buf.lines.map((l, i) => (
            <Line key={i} line={l} v={l.v} cursor={showCursor && i === buf.row ? buf.col : null} />
          ))}
        </div>
        {state.phase === 'connecting' && (
          <div className="subtle small term-hint">
            <Plug size={12} /> Starting a shell…
          </div>
        )}
      </div>
    </div>
  );
}

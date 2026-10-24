import { useCallback, useLayoutEffect, useMemo, useRef } from 'react';
import DOMPurify from 'dompurify';
import { Clock, Info, OctagonX } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Markdown } from '@/components/Markdown';
import type { MimeBundle } from '@/lib/api/notebooks';
import { Ansi } from './ansi';
import type { UiOutput } from './nbformat';
import type { CellRun } from './runStore';

const asText = (v: unknown): string => (Array.isArray(v) ? v.join('') : typeof v === 'string' ? v : v === undefined ? '' : JSON.stringify(v, null, 2));

function SafeHtml({ html, svg }: { html: string; svg?: boolean }) {
  const clean = useMemo(() => DOMPurify.sanitize(html, svg ? { USE_PROFILES: { svg: true, svgFilters: true } } : { FORBID_TAGS: ['style', 'form', 'input', 'button', 'iframe'] }), [html, svg]);
  return (
    <div
      className="nb-html"
      dangerouslySetInnerHTML={{ __html: clean }}
      onClick={(e) => {
        // Never let output navigate the app window; web links open in the browser.
        const a = (e.target as HTMLElement).closest('a');
        if (!a) return;
        e.preventDefault();
        e.stopPropagation();
        const href = a.getAttribute('href') ?? '';
        if (/^https?:\/\//i.test(href)) openUrl(href).catch(() => {});
      }}
    />
  );
}

/** Best renderable representation of a MIME bundle (HTML is sanitized; scripts never run). */
function Rich({ data }: { data: MimeBundle }) {
  if (data['text/html'] !== undefined) return <SafeHtml html={asText(data['text/html'])} />;
  for (const mime of ['image/png', 'image/jpeg', 'image/gif', 'image/webp']) {
    if (typeof data[mime] === 'string') return <img className="nb-image" alt="" src={`data:${mime};base64,${String(data[mime]).replace(/\s/g, '')}`} />;
  }
  if (data['image/svg+xml'] !== undefined) return <SafeHtml html={asText(data['image/svg+xml'])} svg />;
  if (data['text/markdown'] !== undefined) return <Markdown source={asText(data['text/markdown'])} className="nb-md-output" />;
  if (data['application/json'] !== undefined) return <pre className="nb-stream">{JSON.stringify(data['application/json'], null, 2)}</pre>;
  return (
    <pre className="nb-stream">
      <Ansi text={asText(data['text/plain'])} />
    </pre>
  );
}

/** Traceback lines without the `Name: message` line already shown as the heading. */
function Traceback({ ename, evalue, lines }: { ename: string; evalue: string; lines: string[] }) {
  const head = `${ename}: ${evalue}`.trim();
  const rest = lines.filter((l) => {
    const t = l.trim();
    return t !== head && t !== ename && t !== `Uncaught ${evalue}`;
  });
  if (!rest.length) return null;
  return (
    <>
      {'\n'}
      <Ansi text={rest.join('\n')} />
    </>
  );
}

export function OutputList({ outputs }: { outputs: UiOutput[] }) {
  if (!outputs.length) return null;
  return (
    <div className="nb-outputs">
      {outputs.map((o, i) => {
        switch (o.kind) {
          case 'stream':
            return (
              <pre key={i} className={`nb-stream ${o.name}`}>
                <Ansi text={o.text} />
              </pre>
            );
          case 'display':
            return (
              <div key={i} className={o.result ? 'nb-result' : 'nb-display'}>
                {o.result && o.executionCount ? <span className="nb-out-label">Out[{o.executionCount}]</span> : null}
                <Rich data={o.data} />
              </div>
            );
          case 'error':
            return (
              <pre key={i} className="nb-stream nb-error">
                <span className="ansi-bold">
                  {o.ename}
                  {o.evalue ? `: ${o.evalue}` : ''}
                </span>
                <Traceback ename={o.ename} evalue={o.evalue} lines={o.traceback} />
              </pre>
            );
          case 'notice':
            return (
              <div key={i} className="nb-notice">
                <Info size={12} /> {o.text}
              </div>
            );
        }
      })}
    </div>
  );
}

/**
 * Keeps a scrollable output panel pinned to its end while new output streams in, unless the user
 * scrolled up to read. Returns a callback ref for the panel.
 */
export function useFollowOutput(dep: unknown) {
  const el = useRef<HTMLDivElement | null>(null);
  const stick = useRef(true);
  const onScroll = useCallback(() => {
    const e = el.current;
    if (e) stick.current = e.scrollTop + e.clientHeight >= e.scrollHeight - 12;
  }, []);
  const ref = useCallback(
    (node: HTMLDivElement | null) => {
      el.current?.removeEventListener('scroll', onScroll);
      el.current = node;
      node?.addEventListener('scroll', onScroll, { passive: true });
    },
    [onScroll],
  );
  useLayoutEffect(() => {
    const e = el.current;
    if (e && stick.current) e.scrollTop = e.scrollHeight;
  }, [dep]);
  return ref;
}

export function formatDuration(ms?: number): string {
  if (ms === undefined) return '';
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const m = Math.floor(ms / 60_000);
  return `${m}m ${Math.round((ms % 60_000) / 1000)}s`;
}

/** Exit code + duration (or "interrupted"/"timed out") of a finished run. */
export function RunBadge({ run }: { run: CellRun }) {
  if (run.running || !run.runId) return null;
  const dur = formatDuration(run.durationMs);
  if (run.timedOut)
    return (
      <span className="badge warning" title={run.interpreter}>
        <Clock /> timed out{dur && ` · ${dur}`}
      </span>
    );
  if (run.interrupted)
    return (
      <span className="badge warning" title={run.interpreter}>
        <OctagonX /> interrupted{dur && ` · ${dur}`}
      </span>
    );
  if (run.exitCode === undefined) return null;
  const ok = run.exitCode === 0 && !run.outputs.some((o) => o.kind === 'error');
  return (
    <span className={`badge ${ok ? 'success' : 'danger'}`} title={run.interpreter ? `Ran with ${run.interpreter}` : undefined}>
      {run.exitCode === null ? 'failed' : `exit ${run.exitCode}`}
      {dur && ` · ${dur}`}
    </span>
  );
}

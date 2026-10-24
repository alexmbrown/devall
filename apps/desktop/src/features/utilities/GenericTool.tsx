import { useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { ArrowDownUp, Play, RefreshCw } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Callout, CopyButton, Segmented, Spinner } from '@/components/ui/misc';
import { defaultArgs, type UtilArgs } from '@/lib/api/utils';
import { RICH_VIEWS, richView } from './ResultViews';
import { ArgField, INVERSE, inputLanguage, outputLanguage, PaneHead, sizeLabel, ToolHeader, useOpRunner, useRecordRun, useSeed, useUtilities, type ToolProps } from './shared';
import { useActiveViewRef } from '@/store/nav';

/** The registry-driven view: arguments, input editor, live output. */
export function GenericTool({ op, input, setInput, seed }: ToolProps) {
  const { open } = useUtilities();
  const [args, setArgs] = useState<UtilArgs>(() => defaultArgs(op));
  const [view, setView] = useState<'rich' | 'raw'>('rich');
  useSeed(seed, (a) => setArgs((cur) => ({ ...cur, ...a })));

  const manual = op.network;
  const enabled = op.noInput || input.length > 0;
  const run = useOpRunner(op.id, input, args, { enabled, manual });
  const recordRun = useRecordRun(op, input, args, run, { enabled });
  const hasRich = RICH_VIEWS.has(op.id);
  const rich = hasRich && view === 'rich' && !run.error ? richView(op.id, run.output, input) : null;

  // Ctrl/Cmd+Enter runs network ops.
  const trigger = useRef(run.trigger);
  trigger.current = run.trigger;
  const activeView = useActiveViewRef();
  useEffect(() => {
    if (!manual) return;
    const onKey = (e: KeyboardEvent) => {
      if (activeView.current && (e.ctrlKey || e.metaKey) && e.key === 'Enter') trigger.current();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [manual, activeView]);

  return (
    <div className="page">
      <ToolHeader
        op={op}
        onExample={(inp, a) => {
          if (!op.noInput) setInput(inp);
          setArgs({ ...defaultArgs(op), ...a });
        }}
      />
      <div className="util-body">
        {op.args.length > 0 && (
          <div className="row wrap util-args">
            {op.args.map((a) => (
              <ArgField key={a.name} arg={a} value={args[a.name]} onChange={(v) => setArgs({ ...args, [a.name]: v })} />
            ))}
          </div>
        )}
        <div className={clsx('util-io', op.noInput && 'no-input')}>
          {!op.noInput && (
            <div className="util-pane">
              <PaneHead title="Input" meta={input ? sizeLabel(input) : undefined}>
                {INVERSE[op.id] && (
                  <button className="btn btn-sm btn-ghost" title="Swap input/output and use the inverse operation" onClick={() => open(INVERSE[op.id], { input: run.output })}>
                    <ArrowDownUp /> Swap
                  </button>
                )}
                {manual && (
                  <button className="btn btn-sm btn-primary" onClick={run.trigger} disabled={!input.trim() || run.running} title="Run (Ctrl+Enter)">
                    {run.running ? <Spinner /> : <Play />} Run
                  </button>
                )}
              </PaneHead>
              <div className="editor-shell">
                <CodeEditor value={input} onChange={setInput} language={inputLanguage(op.id)} minimal />
              </div>
            </div>
          )}
          <div className="util-pane">
            <PaneHead title="Output" meta={run.output && !run.error ? sizeLabel(run.output) : undefined}>
              {run.running && !manual && <Spinner />}
              {hasRich && (
                <Segmented
                  value={view}
                  onChange={setView}
                  options={[
                    { value: 'rich', label: 'View' },
                    { value: 'raw', label: op.output === 'json' ? 'JSON' : 'Raw' },
                  ]}
                />
              )}
              {op.noInput && (
                <button className="btn btn-sm" onClick={run.trigger}>
                  <RefreshCw /> Regenerate
                </button>
              )}
              <CopyButton text={run.output} label="Copy" onCopy={recordRun} />
            </PaneHead>
            {run.error ? (
              <Callout kind="danger">
                <span className="util-pre-wrap">{run.error}</span>
              </Callout>
            ) : rich ? (
              <div className="util-rich-shell">{rich}</div>
            ) : manual && !run.output && !run.running ? (
              <Callout>This operation makes a network request. Enter a host and press Run.</Callout>
            ) : (
              <div className="editor-shell">
                <CodeEditor value={run.output} language={outputLanguage(op, args, run.output)} readOnly minimal />
              </div>
            )}
          </div>
        </div>
        <p className="subtle xs">
          {op.network ? 'Runs from this machine when you click Run; nothing is sent anywhere else.' : 'Everything runs locally in the app — inputs, tokens and keys are never sent anywhere.'} Also available as{' '}
          <code className="code-inline">devall util {op.id}</code>.
        </p>
      </div>
    </div>
  );
}

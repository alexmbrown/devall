import { useState } from 'react';
import clsx from 'clsx';
import { AlertTriangle, Bug, ChevronRight, Info, SkipForward, Terminal, XCircle } from 'lucide-react';
import { CopyButton, Empty, Segmented } from '@/components/ui/misc';
import { errorLocation, PHASE_LABEL, type ScriptError, type ScriptErrorKind, type ScriptLogLevel, type ScriptReport } from '@/lib/api/scripts';
import './scripts.css';

type Filter = 'all' | 'problems';

const KIND_LABEL: Record<ScriptErrorKind, string> = {
  syntax: 'Syntax error',
  runtime: 'Script error',
  timeout: 'Script timed out',
  memory: 'Script ran out of memory',
  internal: 'Script engine error',
};

const LEVEL_ICON: Record<ScriptLogLevel, typeof Info> = { log: ChevronRight, info: Info, warn: AlertTriangle, error: XCircle, debug: Bug };

/** Badge count for a "Console" tab: errors + warnings, else the number of lines. */
export function scriptLogCount(report: ScriptReport | null | undefined): number {
  if (!report) return 0;
  const problems = report.errors.length + report.logs.filter((l) => l.level === 'warn' || l.level === 'error').length;
  return problems || report.logs.length;
}

/** Whether a send produced anything worth a "Console" tab. */
export function hasScriptOutput(report: ScriptReport | null | undefined): boolean {
  return !!report && (report.logs.length > 0 || report.errors.length > 0 || report.skipped);
}

/** Console output and script errors of one send (response panel "Console" tab). */
export function ScriptLogs({ report, className }: { report: ScriptReport | null | undefined; className?: string }) {
  const [filter, setFilter] = useState<Filter>('all');
  const logs = (report?.logs ?? []).filter((l) => filter === 'all' || l.level === 'warn' || l.level === 'error');
  const errors = report?.errors ?? [];
  const asText = () =>
    [
      ...errors.map((e) => `[${KIND_LABEL[e.kind]}] ${errorLocation(e)}: ${e.message}`),
      ...(report?.logs ?? []).map((l) => `[${l.level}] ${PHASE_LABEL[l.phase].toLowerCase()} · ${l.source}: ${l.message}`),
    ].join('\n');

  if (!report || (!report.logs.length && !errors.length && !report.skipped)) {
    return (
      <div className={clsx('script-logs', className)}>
        <Empty icon={<Terminal />} title="No console output">
          <code>console.log(…)</code> in pre-request and post-response scripts shows up here, along with script errors.
        </Empty>
      </div>
    );
  }

  return (
    <div className={clsx('script-logs', className)}>
      <div className="toolbar">
        <Segmented<Filter>
          value={filter}
          onChange={setFilter}
          options={[
            { value: 'all', label: `All (${report.logs.length})` },
            { value: 'problems', label: 'Warnings & errors' },
          ]}
        />
        <span className="spacer" />
        {report.scriptsRun > 0 && (
          <span className="subtle xs">
            {report.scriptsRun} {report.scriptsRun === 1 ? 'script' : 'scripts'} ran
          </span>
        )}
        <CopyButton text={asText} label="Copy" title="Copy the console output" />
      </div>
      <div className="script-logs-body">
        {report.skipped && (
          <div className="script-log-banner">
            <SkipForward />
            <span>A pre-request script skipped this request (pm.execution.skipRequest()).</span>
          </div>
        )}
        {errors.map((e, i) => (
          <ScriptErrorRow key={`e${i}`} error={e} />
        ))}
        {logs.map((l, i) => {
          const Icon = LEVEL_ICON[l.level];
          return (
            <div key={i} className={clsx('script-log-row', `level-${l.level}`)}>
              <Icon className="script-log-icon" />
              <pre className="script-log-msg">{l.message}</pre>
              <span className="script-log-src" title={`${PHASE_LABEL[l.phase]} script · ${l.source}`}>
                {l.phase === 'preRequest' ? 'pre' : 'post'} · {l.source}
              </span>
            </div>
          );
        })}
        {!logs.length && !errors.length && !report.skipped && <p className="subtle xs script-logs-none">Nothing matches this filter.</p>}
      </div>
    </div>
  );
}

function ScriptErrorRow({ error }: { error: ScriptError }) {
  return (
    <div className="script-log-row level-error script-error-row">
      <XCircle className="script-log-icon" />
      <div className="script-log-msg">
        <strong>{KIND_LABEL[error.kind]}</strong> <span className="subtle">{errorLocation(error)}</span>
        <pre>{error.message}</pre>
      </div>
    </div>
  );
}

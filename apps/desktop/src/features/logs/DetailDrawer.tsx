import { useEffect, useMemo, useState } from 'react';
import { ExternalLink, Link2, Minus, Plus, X } from 'lucide-react';
import { DecodedValues, useDetections } from '@/components/DecodedValues';
import { CopyButton, Segmented, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { eventsApi, fieldClause, type DevEvent, type LogEntry } from '@/lib/api/logs';
import { useWorkspace } from '@/store/workspace';
import type { DataApi } from './source';
import { LEVEL_LABEL, fmtFull, fmtTs, levelVar, pretty, valueText } from './format';

interface Props {
  api: DataApi;
  entry: LogEntry;
  onClose(): void;
  onAddClause(clause: string): void;
  /** Show the project events sharing this correlation id. */
  onShowCorrelated?(id: string): void;
}

function flatten(fields: Record<string, unknown>, prefix = '', out: [string, unknown][] = [], depth = 0): [string, unknown][] {
  for (const [k, v] of Object.entries(fields)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object' && !Array.isArray(v) && depth < 3) flatten(v as Record<string, unknown>, key, out, depth + 1);
    else out.push([key, v]);
  }
  return out;
}

/** Opens the HTTP request an event refers to. */
export function openRequest(path: string) {
  void useWorkspace.getState().openRequest(path);
}

/** Line detail: metadata, fields (click +/− to filter in/out), raw or pretty text, related events. */
export function DetailDrawer({ api, entry, onClose, onAddClause, onShowCorrelated }: Props) {
  const [full, setFull] = useState<LogEntry | null>(null);
  const [mode, setMode] = useState<'pretty' | 'raw'>('pretty');
  const [related, setRelated] = useState<DevEvent[] | null>(null);
  const root = useWorkspace((s) => s.project?.root);
  const e = full ?? entry;

  useEffect(() => {
    let alive = true;
    setFull(null);
    api
      .detail(entry)
      .then((x) => alive && setFull(x))
      .catch((err) => toast.error('Could not load entry', errorMessage(err)));
    return () => {
      alive = false;
    };
  }, [api, entry]);

  useEffect(() => {
    setRelated(null);
    if (!root || !entry.correlationId) return;
    let alive = true;
    eventsApi
      .correlated(root, entry.correlationId)
      .then((r) => alive && setRelated(r))
      .catch(() => alive && setRelated([]));
    return () => {
      alive = false;
    };
  }, [root, entry.correlationId]);

  const fields = useMemo(() => flatten(e.fields), [e.fields]);
  const requestPath = typeof e.fields.requestPath === 'string' ? e.fields.requestPath : undefined;
  const text = mode === 'raw' ? e.raw : pretty(e);
  // JWTs / epoch timestamps in the line (tokens in access logs, exp claims…), decoded inline.
  const detections = useDetections(e.raw);

  return (
    <aside className="logs-drawer" aria-label="Entry details">
      <div className="toolbar">
        <span className="lvl-badge" style={{ ['--c' as string]: levelVar(e.level) }}>
          {LEVEL_LABEL[e.level]}
        </span>
        <span className="strong small grow truncate" title={fmtFull(e.ts)}>
          {e.ts != null ? fmtFull(e.ts) : 'No timestamp'}
        </span>
        <CopyButton text={() => e.raw} title="Copy raw" />
        <button className="icon-btn" onClick={onClose} title="Close (Esc)" aria-label="Close details">
          <X />
        </button>
      </div>
      <div className="logs-drawer-body">
        <dl className="kv small">
          {api.isEvents ? (
            <>
              <dt>Kind</dt>
              <dd className="mono">{String(e.fields.kind ?? '')}</dd>
              <dt>Module</dt>
              <dd>{e.source}</dd>
              {e.fields.durationMs != null && (
                <>
                  <dt>Duration</dt>
                  <dd>{Number(e.fields.durationMs).toFixed(1)} ms</dd>
                </>
              )}
            </>
          ) : (
            <>
              <dt>Line</dt>
              <dd>
                {e.lineNo}
                {e.lineCount > 1 ? `–${e.lineNo + e.lineCount - 1}` : ''}
              </dd>
              {e.source && (
                <>
                  <dt>Source</dt>
                  <dd>{e.source}</dd>
                </>
              )}
              <dt>Format</dt>
              <dd>{e.format}</dd>
            </>
          )}
          {e.correlationId && (
            <>
              <dt>Correlation</dt>
              <dd className="mono row-sm">
                <span className="truncate">{e.correlationId}</span>
                <button className="icon-btn sm" title="Filter by this id" onClick={() => onAddClause(fieldClause('trace', e.correlationId))}>
                  <Plus />
                </button>
                {onShowCorrelated && !api.isEvents && (
                  <button className="icon-btn sm" title="Show project events with this id" onClick={() => onShowCorrelated(e.correlationId!)}>
                    <Link2 />
                  </button>
                )}
              </dd>
            </>
          )}
        </dl>

        {requestPath && (
          <button className="btn btn-sm mt-2" onClick={() => openRequest(requestPath)}>
            <ExternalLink /> Open request {requestPath}
          </button>
        )}

        {related && related.length > 1 && (
          <div className="logs-related">
            <div className="section-title small">Same correlation id ({related.length})</div>
            {related.map((r) => (
              <div key={r.id} className="logs-related-row">
                <span className="dot" style={{ background: levelVar(r.level) }} />
                <span className="ts">{fmtTs(r.at)}</span>
                <span className="mono subtle">{r.kind}</span>
                <span className="truncate grow" title={r.title}>
                  {r.title}
                </span>
                {r.requestPath && (
                  <button className="icon-btn sm" title={`Open ${r.requestPath}`} onClick={() => openRequest(r.requestPath!)}>
                    <ExternalLink />
                  </button>
                )}
              </div>
            ))}
          </div>
        )}

        {fields.length > 0 && (
          <table className="table logs-fields">
            <tbody>
              {fields.map(([k, v]) => (
                <tr key={k}>
                  <td className="mono k" title={k}>
                    {k}
                  </td>
                  <td className="mono v">{valueText(v)}</td>
                  <td className="act">
                    <button className="icon-btn sm" title="Filter for this value" onClick={() => onAddClause(fieldClause(k, v))}>
                      <Plus />
                    </button>
                    <button className="icon-btn sm" title="Filter out this value" onClick={() => onAddClause(`-${fieldClause(k, v)}`)}>
                      <Minus />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}

        <div className="row-sm mt-4">
          <Segmented
            value={mode}
            onChange={setMode}
            options={[
              { value: 'pretty', label: 'Pretty' },
              { value: 'raw', label: 'Raw' },
            ]}
          />
          <span className="grow" />
          {!full && <Spinner />}
          <CopyButton text={text} title={`Copy ${mode}`} />
        </div>
        <pre className="logs-text">{text}</pre>
        {detections.length > 0 && (
          <>
            <div className="section-title small mt-4">Decoded ({detections.length})</div>
            <DecodedValues items={detections} className="col mt-2" />
          </>
        )}
      </div>
    </aside>
  );
}

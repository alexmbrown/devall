import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Bot, Check, CheckCheck, ShieldAlert, X } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, MethodBadge } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { aiApi, type ApprovalDecision, type ApprovalRequest } from '@/lib/api/ai';
import { errorMessage, isTauri } from '@/lib/ipc';
import { secondsLeft } from './approvals';
import './ai.css';

/**
 * Global prompt for MCP calls that need the user's approval (writes in "Ask each time" mode,
 * anything in a protected environment). Mounted once in `App.tsx`, so it works on every page.
 * The backend denies on its own when nobody answers in time.
 */
export function McpApprovalHost() {
  const [queue, setQueue] = useState<ApprovalRequest[]>([]);
  const [now, setNow] = useState(() => Date.now());
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!isTauri) return;
    let alive = true;
    const unlisten: (() => void)[] = [];
    void aiApi
      .mcpApprovalsPending()
      .then((list) => alive && setQueue(list))
      .catch(() => undefined);
    void listen<ApprovalRequest>('mcp-approval', (e) => setQueue((q) => (q.some((x) => x.id === e.payload.id) ? q : [...q, e.payload]))).then((u) => (alive ? unlisten.push(u) : u()));
    void listen<{ id: string; decision: ApprovalDecision }>('mcp-approval-closed', (e) => setQueue((q) => q.filter((x) => x.id !== e.payload.id))).then((u) => (alive ? unlisten.push(u) : u()));
    return () => {
      alive = false;
      unlisten.forEach((u) => u());
    };
  }, []);

  const current = queue[0];
  useEffect(() => {
    if (!current) return;
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, [current]);

  const answer = useCallback(
    async (decision: ApprovalDecision) => {
      if (!current) return;
      setBusy(true);
      try {
        const ok = await aiApi.mcpApprovalAnswer(current.id, decision);
        if (!ok) toast.info('Already settled', 'The request timed out or was answered elsewhere.');
      } catch (e) {
        toast.error('Could not answer', errorMessage(e));
      } finally {
        setQueue((q) => q.filter((x) => x.id !== current.id));
        setBusy(false);
      }
    },
    [current],
  );

  if (!current) return null;
  const left = secondsLeft(current, now);
  return (
    <Modal
      open
      onClose={() => void answer('deny')}
      title={
        <span className="row-sm">
          <Bot size={16} /> An agent wants to send a request
        </span>
      }
      footer={
        <>
          <span className="subtle xs grow">
            {queue.length > 1 ? `${queue.length - 1} more waiting · ` : ''}Denied automatically in {left} s
          </span>
          <button className="btn btn-danger-ghost" onClick={() => void answer('deny')} disabled={busy}>
            <X /> Deny
          </button>
          <button className="btn" onClick={() => void answer('allowSession')} disabled={busy} title="Don't ask again for this request, method and environment until the MCP endpoint stops">
            <CheckCheck /> Allow for this session
          </button>
          <button className="btn btn-primary" onClick={() => void answer('allowOnce')} disabled={busy}>
            <Check /> Allow once
          </button>
        </>
      }
    >
      <div className="col">
        {current.protected ? (
          <Callout kind="danger">
            <span className="row-sm">
              <ShieldAlert size={14} /> <strong>{current.environment}</strong> is a protected environment.
            </span>
          </Callout>
        ) : (
          <Callout kind="warning">{current.reason.charAt(0).toUpperCase() + current.reason.slice(1)}.</Callout>
        )}
        <div className="row-sm">
          <MethodBadge method={current.method} />
          <span className="mono small break">{current.url}</span>
        </div>
        <dl className="kv">
          <dt>Tool</dt>
          <dd className="mono xs">{current.tool}</dd>
          <dt>Saved request</dt>
          <dd>
            {current.requestName} <span className="subtle mono xs">http/{current.requestPath}</span>
          </dd>
          <dt>Environment</dt>
          <dd>{current.environment ?? 'none (project variables only)'}</dd>
        </dl>
        {current.headers.length > 0 && (
          <details>
            <summary className="small subtle">Headers ({current.headers.length})</summary>
            <pre className="ai-pre">{current.headers.map(([k, v]) => `${k}: ${v}`).join('\n')}</pre>
          </details>
        )}
        {current.bodyPreview && (
          <div className="col-sm">
            <span className="small subtle">Body{current.bodyTruncated ? ' (truncated)' : ''}</span>
            <pre className="ai-pre" style={{ maxHeight: 220, overflow: 'auto' }}>
              {current.bodyPreview}
            </pre>
          </div>
        )}
        <span className="xs subtle">Secret values are masked. Nothing is sent until you allow it.</span>
      </div>
    </Modal>
  );
}

import { useEffect, useMemo, useRef, useState } from 'react';
import { CheckSquare, Eye, FileText, Layers, MessageSquare, Send, Square, Trash2 } from 'lucide-react';
import { Markdown } from '@/components/Markdown';
import { Callout, Empty, RelTime, Switch } from '@/components/ui/misc';
import { aiApi, type AskAnswer, type Preview, type SourceRef } from '@/lib/api/ai';
import { historyApi, type AskRecord } from '@/lib/api/history';
import { errorMessage } from '@/lib/ipc';
import { useProjectLinkOpener } from '@/features/notes/NotesPage';
import { useNav } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { MetaLine, PreviewView } from './AiAssistButton';
import { linkCitations } from './citations';
import { newCallId, useAi } from './store';

interface Turn {
  id: string;
  question: string;
  /** Streamed text so far. */
  text: string;
  answer?: AskAnswer;
  error?: string;
  callId?: string;
  /** Loaded from this project's saved answers (last 50, on this computer): when it was asked. */
  restoredAt?: number;
}

/** A saved question + answer shown again when the tab opens. */
function restoredTurn(r: AskRecord): Turn {
  const meta = { provider: '', model: r.model ?? '', remote: false, redactions: 0, repaired: false, schemaEnforced: false };
  return { id: r.id, question: r.question, text: r.answer, answer: { answer: r.answer, citations: r.citations, sources: r.citations, meta }, restoredAt: r.at };
}

const STARTERS = ["What's left to do?", 'Summarize our notes', 'Which requests need auth?', 'What environments do we have?'];

function SourceIcon({ kind }: { kind: string }) {
  if (kind === 'todos') return <CheckSquare />;
  if (kind === 'requests') return <Send />;
  if (kind === 'environments') return <Layers />;
  return <FileText />;
}

export function AskTab() {
  const project = useWorkspace((s) => s.project);
  const { view, selected } = useAi();
  const go = useNav((s) => s.go);
  const openLink = useProjectLinkOpener();
  const [turns, setTurns] = useState<Turn[]>([]);
  const [q, setQ] = useState('');
  const [showPreview, setShowPreview] = useState(false);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const threadRef = useRef<HTMLDivElement>(null);
  const busy = turns.some((t) => t.callId && !t.answer && !t.error);
  const root = project?.root;

  // The project's recent questions and answers (kept on this computer) come back with the tab.
  useEffect(() => {
    if (!root) return;
    let alive = true;
    historyApi
      .askHistory(root)
      .then((list) => alive && setTurns((cur) => (cur.length ? cur : list.map(restoredTurn))))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [root]);

  useEffect(() => {
    threadRef.current?.scrollTo({ top: threadRef.current.scrollHeight });
  }, [turns]);

  // Live "what will be sent" for the question being typed.
  useEffect(() => {
    if (!showPreview || !project) return;
    const question = q.trim() || 'What is left to do?';
    const t = setTimeout(() => {
      aiApi
        .preview(project.root, 'ask', { question }, selected)
        .then((p) => {
          setPreview(p);
          setPreviewError(null);
        })
        .catch((e) => setPreviewError(errorMessage(e)));
    }, 350);
    return () => clearTimeout(t);
  }, [showPreview, q, project, selected]);

  const openSource = (s: SourceRef) => {
    if (s.kind === 'todos') go('todos');
    else if (s.kind === 'environments') go('environments');
    else if (s.kind === 'requests') go('http');
    else openLink(s.source);
  };

  const sourcesById = useMemo(() => new Map(turns.flatMap((t) => (t.answer?.sources ?? []).map((s) => [`${t.id}:${s.id}`, s] as const))), [turns]);

  if (!project)
    return (
      <Empty icon={<MessageSquare />} title="Open a project to ask about it">
        The assistant answers from the project's notes, your personal to-dos and the saved requests — on your machine.
      </Empty>
    );

  const noProvider = view && view.config.providers.length === 0 && !view.managed;

  const ask = async (question: string) => {
    const text = question.trim();
    if (!text || busy) return;
    const id = newCallId();
    setQ('');
    setTurns((ts) => [...ts, { id, question: text, text: '', callId: id }]);
    const patch = (fn: (t: Turn) => Turn) => setTurns((ts) => ts.map((t) => (t.id === id ? fn(t) : t)));
    try {
      const answer = await aiApi.ask(project.root, text, (tok) => patch((t) => ({ ...t, text: t.text + tok })), { provider: selected, callId: id });
      patch((t) => ({ ...t, answer, text: answer.answer }));
      void historyApi
        .addAsk(project.root, { id, at: Date.now(), question: text, answer: answer.answer, citations: answer.citations, model: `${answer.meta.provider} · ${answer.meta.model}` })
        .catch(() => {});
    } catch (e) {
      patch((t) => ({ ...t, error: errorMessage(e) }));
    }
  };

  return (
    <div className="ai-chat">
      <div className="ai-thread" ref={threadRef}>
        <div className="ai-thread-inner">
          {noProvider && (
            <Callout kind="warning">
              No model is set up yet. Connect Ollama or LM Studio, or download a model for the built-in llama.cpp runtime.{' '}
              <button className="btn btn-sm" onClick={() => go('ai', 'models')}>
                Set up a model
              </button>
            </Callout>
          )}
          {turns.length === 0 && (
            <Empty icon={<MessageSquare />} title={`Ask about ${project.manifest.name}`}>
              Answers cite the notes, to-dos and requests they use. Secret values never leave your machine.
            </Empty>
          )}
          {turns.length === 0 && (
            <div className="ai-starters">
              {STARTERS.map((s) => (
                <button key={s} className="btn btn-sm" onClick={() => ask(s)} disabled={!!noProvider}>
                  {s}
                </button>
              ))}
            </div>
          )}
          {turns.map((t) => (
            <div key={t.id} className="col">
              <div className="ai-msg-user">{t.question}</div>
              <div className="ai-msg-ai">
                {t.error ? (
                  <Callout kind="danger">{t.error}</Callout>
                ) : (
                  <div className={t.answer ? undefined : 'ai-caret'}>
                    <Markdown
                      source={linkCitations(t.text || (t.answer ? '' : '…'), t.answer?.sources ?? [])}
                      onProjectLink={(p) => {
                        if (p.startsWith('cite/')) {
                          const s = sourcesById.get(`${t.id}:${p.slice(5)}`);
                          if (s) openSource(s);
                        } else openLink(p);
                      }}
                    />
                  </div>
                )}
                {t.answer && t.answer.citations.length > 0 && (
                  <div className="ai-cites">
                    {t.answer.citations.map((s) => (
                      <button key={s.id} className="ai-cite" onClick={() => openSource(s)} title={s.source}>
                        <span className="ai-cite-id">{s.id.slice(1)}</span>
                        <SourceIcon kind={s.kind} />
                        <span className="truncate">{s.title}</span>
                      </button>
                    ))}
                  </div>
                )}
                {t.answer && !t.restoredAt && <MetaLine meta={t.answer.meta} />}
                {t.restoredAt && (
                  <div className="ai-meta">
                    <span className="truncate">
                      <RelTime ts={t.restoredAt} />
                      {t.answer?.meta.model ? ` · ${t.answer.meta.model}` : ''}
                    </span>
                  </div>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>
      <div className="ai-composer">
        <div className="ai-composer-inner">
          {showPreview && (previewError ? <Callout kind="warning">{previewError}</Callout> : preview && <PreviewView preview={preview} />)}
          <textarea
            className="textarea"
            placeholder="Ask about this project — e.g. “what’s left on payments?” (Enter to send, Shift+Enter for a new line)"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                ask(q);
              }
            }}
            disabled={!!noProvider}
          />
          <div className="row">
            <Switch checked={showPreview} onChange={setShowPreview} label={<span className="small"><Eye size={12} /> Show what will be sent</span>} />
            <span className="spacer" />
            {turns.length > 0 && !busy && (
              <button
                className="btn btn-sm btn-ghost"
                onClick={() => {
                  setTurns([]);
                  void historyApi.clearAsks(project.root).catch(() => {});
                }}
                title="Clear the conversation (and the saved answers on this computer)"
              >
                <Trash2 /> Clear
              </button>
            )}
            {busy ? (
              <button
                className="btn btn-sm"
                onClick={() => {
                  const cur = turns.find((t) => t.callId && !t.answer && !t.error);
                  if (cur?.callId) aiApi.cancel(cur.callId);
                }}
              >
                <Square /> Stop
              </button>
            ) : (
              <button className="btn btn-sm btn-primary" onClick={() => ask(q)} disabled={!q.trim() || !!noProvider}>
                <Send /> Ask
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

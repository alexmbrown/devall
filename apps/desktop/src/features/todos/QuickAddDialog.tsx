import { useRef, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { create } from 'zustand';
import { Link2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { todosApi, type NewTask } from '@/lib/api/todos';
import { addTask, useTodos } from '@/store/todos';
import { QuickAddInput, type QuickAddInputHandle } from './QuickAddInput';
import { dayLabel, linkLabel, todayYmd } from './lib';

export interface QuickAddOptions {
  /** Project whose list is in scope (default: the open project). */
  root?: string | null;
  /** Defaults the text can override: list, section, due, labels, links… */
  defaults?: Omit<NewTask, 'text'>;
  /** Initial text (quick-add syntax). */
  prefill?: string;
  title?: string;
}

const useQuickAdd = create<{ opts: QuickAddOptions | null }>(() => ({ opts: null }));

declare global {
  interface Window {
    __devallQuickAddRoot?: Root;
  }
}

/**
 * Opens the quick-add dialog from anywhere (to-do page `Q`, "Add to-do about this…" with
 * `defaults.links`). It mounts its own small React root on first use, so no app shell change is
 * needed.
 */
export function openQuickAdd(opts: QuickAddOptions = {}) {
  if (!window.__devallQuickAddRoot) {
    const el = document.createElement('div');
    el.id = 'devall-quick-add';
    document.body.appendChild(el);
    window.__devallQuickAddRoot = createRoot(el);
  }
  // Re-render with this module's host (keeps working across hot reloads).
  window.__devallQuickAddRoot.render(<QuickAddHost />);
  useQuickAdd.setState({ opts });
}

function QuickAddHost() {
  const opts = useQuickAdd((s) => s.opts);
  if (!opts) return null;
  return <QuickAddModal opts={opts} onClose={() => useQuickAdd.setState({ opts: null })} />;
}

function QuickAddModal({ opts, onClose }: { opts: QuickAddOptions; onClose(): void }) {
  const snap = useTodos((s) => s.snap);
  const storeRoot = useTodos((s) => s.root);
  const root = opts.root === undefined ? storeRoot : opts.root;
  const [text, setText] = useState(opts.prefill ?? '');
  const [ignore, setIgnore] = useState<string[]>([]);
  const [desc, setDesc] = useState('');
  const [busy, setBusy] = useState(false);
  const input = useRef<QuickAddInputHandle>(null);
  const defaults = opts.defaults ?? {};
  const listId = defaults.listId ?? (root === storeRoot ? (snap?.projectList ?? undefined) : undefined);
  const list = snap?.lists.find((l) => l.id === (listId ?? 'inbox'));
  const today = snap?.today ?? todayYmd();

  const submit = async () => {
    if (!text.trim() || busy) return;
    setBusy(true);
    const task: NewTask = { ...defaults, listId, text, ignore, description: desc.trim() || undefined };
    try {
      if (root === storeRoot) {
        await addTask(task);
      } else {
        const r = await todosApi.apply(root, { type: 'add', task });
        const t = r.snapshot.tasks.find((x) => x.id === r.applied.created);
        toast.success('Added to your to-dos', t?.content);
      }
      setText('');
      setIgnore([]);
      setDesc('');
      input.current?.focus();
    } catch (e) {
      toast.error('Could not add the task', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      title={opts.title ?? 'Quick add'}
      size="lg"
      footer={
        <>
          <span className="subtle xs grow">
            Enter adds and keeps this open · Esc closes · <code className="code-inline">tomorrow 5pm #List /Section @label p1 {'{fri}'}</code>
          </span>
          <button className="btn" onClick={onClose}>
            Done
          </button>
          <button className="btn btn-primary" disabled={!text.trim() || busy} onClick={() => void submit()}>
            Add task
          </button>
        </>
      }
    >
      <div className="col">
        <QuickAddInput ref={input} value={text} onChange={setText} ignore={ignore} onIgnore={setIgnore} listId={listId} autoFocus big placeholder="e.g. Ship refunds tomorrow 5pm @api p1" onSubmit={() => void submit()} onCancel={onClose} />
        <textarea
          className="textarea"
          rows={2}
          placeholder="Description (Markdown)"
          value={desc}
          onChange={(e) => setDesc(e.target.value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) void submit();
          }}
        />
        <div className="row-sm wrap subtle xs">
          <span>
            → {list?.name ?? 'Inbox'}
            {defaults.due ? ` · ${dayLabel(defaults.due.date, today)}` : ''}
            {defaults.labels?.length ? ` · ${defaults.labels.map((l) => `@${l}`).join(' ')}` : ''}
          </span>
          {defaults.links?.map((l) => (
            <span key={l} className="tag" title={l}>
              <Link2 /> {linkLabel(l)}
            </span>
          ))}
        </div>
      </div>
    </Modal>
  );
}

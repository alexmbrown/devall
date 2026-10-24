import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { create } from 'zustand';
import { Camera, GitCompare, History as HistoryIcon, Share2, Trash2 } from 'lucide-react';
import { CodeDiff, CodeEditor, languageFor } from '@/components/editor/CodeEditor';
import { Modal } from '@/components/ui/Modal';
import { useContextMenu } from '@/components/ui/Menu';
import { Callout, Empty, Field, RelTime, Segmented, StatusCode, formatBytes } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { HistoryDetail, HistorySummary } from '@/lib/types';
import { useHistoryVersion, useWorkspace } from '@/store/workspace';
import { parseIgnore, snapshotPath, suggestIgnorePaths, upsertSnapshotTest } from './snapshots';
import { ShareDialog } from '@/features/tunnels/ShareDialog';

function pretty(d: HistoryDetail | null): string {
  if (!d?.bodyText) return d?.bodyBase64 ? `<binary ${d.bodyBase64.length} base64 chars>` : '';
  try {
    return JSON.stringify(JSON.parse(d.bodyText), null, 2);
  } catch {
    return d.bodyText;
  }
}

function entryLabel(h: HistorySummary) {
  return `${new Date(h.at).toLocaleString()} · ${h.status ?? 'ERR'} · ${Math.round(h.durationMs)} ms`;
}

/**
 * One-shot request to show a past response (History page → "Open request"): the request's
 * response panel switches to its History tab and selects `id`, then clears it.
 */
export const useHistoryFocus = create<{ path: string | null; id: string | null; focus(path: string, id: string): void; clear(): void }>((set) => ({
  path: null,
  id: null,
  focus: (path, id) => set({ path, id }),
  clear: () => set({ path: null, id: null }),
}));

/**
 * Past responses of one request (newest first): view one, compare any two (the previous one by
 * default), or promote one to a snapshot test.
 */
export function HistoryView({ root, path }: { root: string; path: string }) {
  const version = useHistoryVersion((s) => s.v);
  const [items, setItems] = useState<HistorySummary[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  const [detail, setDetail] = useState<HistoryDetail | null>(null);
  /** Entry compared against; `null` = the selected entry's previous response. */
  const [baseId, setBaseId] = useState<string | null>(null);
  const [other, setOther] = useState<HistoryDetail | null>(null);
  const [mode, setMode] = useState<'view' | 'compare'>('view');
  const [promote, setPromote] = useState(false);
  const [share, setShare] = useState(false);
  const ctx = useContextMenu();

  useEffect(() => {
    api.historyList(root, path, 200).then(setItems).catch(() => setItems([]));
  }, [root, path, version]);
  useEffect(() => {
    if (!sel && items[0]) setSel(items[0].id);
  }, [items, sel]);
  const focus = useHistoryFocus();
  useEffect(() => {
    if (focus.path !== path || !focus.id) return;
    setSel(focus.id);
    setMode('view');
    useHistoryFocus.getState().clear();
  }, [focus.path, focus.id, path]);
  useEffect(() => {
    if (!sel) return setDetail(null);
    api
      .historyGet(root, sel)
      .then(setDetail)
      .catch((e) => toast.error('History', errorMessage(e)));
  }, [root, sel]);
  const compareId = baseId && baseId !== sel ? baseId : (detail?.previousId ?? null);
  useEffect(() => {
    if (!compareId) return setOther(null);
    api
      .historyGet(root, compareId)
      .then(setOther)
      .catch(() => setOther(null));
  }, [root, compareId]);

  const lang = useMemo(() => languageFor(detail?.entry.contentType ?? undefined, detail?.bodyText ?? undefined), [detail]);

  if (!items.length)
    return (
      <Empty icon={<HistoryIcon />} title="No history yet">
        Every send is kept on this computer, in the app data folder — never in the project, no limits, no cloud. See all of them under History.
      </Empty>
    );

  const compareWith = (id: string) => {
    setBaseId(id);
    setMode('compare');
  };

  return (
    <div className="history-split">
      <div className="history-list">
        <div className="toolbar">
          <span className="xs subtle grow">{items.length} responses</span>
          <button
            className="icon-btn sm danger"
            title="Clear this request's history"
            onClick={async () => {
              if (!(await confirmDialog({ title: 'Clear history for this request?', confirmText: 'Clear', danger: true }))) return;
              await api.historyClear(root, path);
              setItems([]);
              setSel(null);
              setBaseId(null);
            }}
          >
            <Trash2 />
          </button>
        </div>
        <div className="list">
          {items.map((h) => (
            <div
              key={h.id}
              className={clsx('list-item', sel === h.id && 'on', mode === 'compare' && compareId === h.id && 'history-base')}
              onClick={(e) => (e.altKey && sel && h.id !== sel ? compareWith(h.id) : setSel(h.id))}
              onContextMenu={(e) =>
                ctx.open(e, [
                  { label: 'Open', onSelect: () => setSel(h.id) },
                  { label: 'Compare with the selected response', icon: <GitCompare />, disabled: !sel || h.id === sel, onSelect: () => compareWith(h.id) },
                ])
              }
              title={mode === 'compare' ? 'Click to view · Alt+click to compare against this one' : undefined}
            >
              {h.status ? <StatusCode status={h.status} /> : <span className="text-danger xs strong">ERR</span>}
              <span className="xs subtle grow">
                <RelTime ts={h.at} />
              </span>
              {mode === 'compare' && compareId === h.id && <span className="badge outline">base</span>}
              {h.testsFailed > 0 && <span className="badge danger">{h.testsFailed}✗</span>}
              <span className="xs subtle">{Math.round(h.durationMs)} ms</span>
            </div>
          ))}
        </div>
        {ctx.node}
      </div>
      <div className="history-detail">
        {detail && (
          <>
            <div className="toolbar">
              <span className="mono xs truncate grow" title={detail.entry.url}>
                {detail.entry.method} {detail.entry.url}
              </span>
              <span className="xs subtle">{formatBytes(detail.entry.size)}</span>
              {detail.entry.environment && <span className="badge outline">{detail.entry.environment}</span>}
              {!detail.entry.error && detail.bodyText != null && (
                <button className="btn btn-sm btn-ghost" onClick={() => setPromote(true)} title="Record this body as the expected response and add a snapshot assertion">
                  <Camera /> Promote to snapshot test
                </button>
              )}
              <button className="btn btn-sm btn-ghost" onClick={() => setShare(true)} title="Create a read-only, expiring link to this request and response (secrets masked)">
                <Share2 /> Share…
              </button>
              <Segmented
                value={mode}
                onChange={setMode}
                options={[
                  { value: 'view', label: 'Response' },
                  {
                    value: 'compare',
                    label: (
                      <>
                        <GitCompare size={12} /> Compare
                      </>
                    ),
                  },
                ]}
              />
            </div>
            {mode === 'compare' && (
              <div className="toolbar">
                <span className="xs subtle">Base</span>
                <select className="select sm grow" value={compareId ?? ''} onChange={(e) => setBaseId(e.target.value || null)} aria-label="Compare against">
                  {detail.previousId && <option value="">Previous response</option>}
                  {!detail.previousId && !compareId && <option value="">Pick a response…</option>}
                  {items
                    .filter((h) => h.id !== sel)
                    .map((h) => (
                      <option key={h.id} value={h.id}>
                        {entryLabel(h)}
                      </option>
                    ))}
                </select>
                <span className="xs subtle">→ selected ({detail.entry.status ?? 'ERR'})</span>
              </div>
            )}
            <div className="grow min-h-0">
              {detail.entry.error ? (
                <pre className="code m-3">{detail.entry.error}</pre>
              ) : mode === 'compare' ? (
                other ? (
                  <CodeDiff original={pretty(other)} modified={pretty(detail)} language={lang} />
                ) : (
                  <Empty title="Nothing to compare with">Pick another response above, or Alt+click one in the list.</Empty>
                )
              ) : (
                <CodeEditor value={pretty(detail)} language={lang} readOnly />
              )}
            </div>
          </>
        )}
      </div>
      {promote && detail && <PromoteDialog root={root} path={path} detail={detail} onClose={() => setPromote(false)} />}
      {share && detail && <ShareDialog root={root} historyId={detail.entry.id} defaultTitle={detail.entry.requestName || `${detail.entry.method} ${detail.entry.url}`} onClose={() => setShare(false)} />}
    </div>
  );
}

/** Writes the snapshot file and adds (or updates) the request's snapshot assertion. */
function PromoteDialog({ root, path, detail, onClose }: { root: string; path: string; detail: HistoryDetail; onClose(): void }) {
  const [file, setFile] = useState(() => snapshotPath(path));
  const [ignore, setIgnore] = useState(() => suggestIgnorePaths(detail.bodyText).join('\n'));
  const [busy, setBusy] = useState(false);
  const tab = useWorkspace((s) => s.tabs.find((t) => t.kind === 'request' && t.path === path));

  const save = async () => {
    setBusy(true);
    try {
      const list = parseIgnore(ignore);
      const written = await api.snapshotSave(root, path, detail.bodyText ?? '', detail.entry.contentType, list, file);
      const ws = useWorkspace.getState();
      if (tab?.kind === 'request') ws.updateRequest(tab.id, (d) => ({ ...d, tests: upsertSnapshotTest(d.tests, written, list) }));
      else {
        const req = await api.readRequest(root, path);
        await api.saveRequest(root, path, { ...req, tests: upsertSnapshotTest(req.tests, written, list) });
      }
      toast.success('Snapshot test added', `${written} — commit it with the request. \`devall run --update-snapshots\` accepts future changes.`);
      onClose();
    } catch (e) {
      toast.error('Could not save the snapshot', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      title="Promote to snapshot test"
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy || !file.trim()} onClick={save}>
            <Camera /> Save snapshot
          </button>
        </>
      }
    >
      <div className="col-lg">
        <p className="muted small">
          This response ({detail.entry.status ?? '—'}, {formatBytes(detail.entry.size)}) becomes the expected body. Every send, run and <code className="code-inline">devall run</code> then compares
          against it — JSON semantically, so key order doesn’t matter.
        </p>
        <Field label="Snapshot file" hint="Relative to the project root; commit it next to the request.">
          <input className="input mono" value={file} onChange={(e) => setFile(e.target.value)} />
        </Field>
        <Field label="Ignore (JSONPath, one per line)" hint="Masked on both sides before comparing — timestamps, generated ids. Suggested from this body.">
          <textarea className="textarea mono" rows={4} value={ignore} placeholder={'$..createdAt\n$.meta.requestId'} onChange={(e) => setIgnore(e.target.value)} />
        </Field>
        {!detail.entry.contentType?.includes('json') && <Callout kind="info">Not a JSON response: it is compared as text, line by line.</Callout>}
      </div>
    </Modal>
  );
}

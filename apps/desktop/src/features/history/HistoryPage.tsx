import { Fragment, useEffect, useMemo, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { Activity, Binary, Database, ExternalLink, FileClock, FolderOpen, History as HistoryIcon, MessageSquare, ScrollText, Send, Settings2, Trash2 } from 'lucide-react';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { CodeEditor, languageFor } from '@/components/editor/CodeEditor';
import { ResponsiveTabs, type TabDef } from '@/components/ui/ResponsiveTabs';
import { Empty, MethodBadge, RelTime, SearchInput, Segmented, Spinner, StatusCode, formatBytes } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { HistoryDetail, HistorySummary } from '@/lib/types';
import { eventsApi, type DevEvent } from '@/lib/api/logs';
import { dbApi } from '@/lib/api/db';
import { historyApi, restorableArgs, useLocalHistoryVersion, versionLabel, type AskRecord, type FileSummary, type FileVersion, type UtilityRun } from '@/lib/api/history';
import { Markdown } from '@/components/Markdown';
import { useHistoryFocus } from '@/features/http/HistoryView';
import { openInUtilities } from '@/features/utilities/open';
import { clearHistory, connKey, newSqlTab, readHistory, useDb, type HistoryItem } from '@/features/db/store';
import { EVENTS_KEY, useLogs } from '@/features/logs';
import { useNav, useViewLabel } from '@/store/nav';
import { useHistoryVersion, useWorkspace } from '@/store/workspace';
import { FileHistoryView, SourceIcon } from './FileHistoryView';
import './history.css';

type Tab = 'activity' | 'requests' | 'files' | 'utilities' | 'queries' | 'ask';

const TABS: (TabDef<Tab> & { viewLabel: string; needsProject: boolean })[] = [
  { key: 'activity', label: 'Activity', icon: <Activity />, viewLabel: 'Project activity', needsProject: true },
  { key: 'requests', label: 'Requests', icon: <Send />, viewLabel: 'Request history', needsProject: true },
  { key: 'files', label: 'Files', icon: <FileClock />, viewLabel: 'File history', needsProject: true },
  { key: 'utilities', label: 'Utilities', icon: <Binary />, viewLabel: 'Utility history', needsProject: false },
  { key: 'queries', label: 'Queries', icon: <Database />, viewLabel: 'Query history', needsProject: true },
  { key: 'ask', label: 'AI answers', icon: <MessageSquare />, viewLabel: 'AI answer history', needsProject: true },
];

const PAGE = 200;

const dayLabel = (t: number) => {
  const d = new Date(t);
  const today = new Date();
  const yesterday = new Date(Date.now() - 86_400_000);
  if (d.toDateString() === today.toDateString()) return 'Today';
  if (d.toDateString() === yesterday.toDateString()) return 'Yesterday';
  return d.toLocaleDateString(undefined, { weekday: 'short', year: 'numeric', month: 'short', day: 'numeric' });
};

const timeLabel = (t: number) => new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

/** Rows with a sticky day header before each new day. */
function ByDay<T>({ items, at, id, render }: { items: T[]; at(item: T): number; id(item: T, index: number): string; render(item: T): ReactNode }) {
  let last = '';
  return (
    <>
      {items.map((item, i) => {
        const day = dayLabel(at(item));
        const header = day !== last ? <div className="hist-day">{day}</div> : null;
        last = day;
        return (
          <Fragment key={id(item, i)}>
            {header}
            {render(item)}
          </Fragment>
        );
      })}
    </>
  );
}

function NeedsProject() {
  return (
    <Empty icon={<HistoryIcon />} title="Open a project">
      Request, file, activity and query history belong to a project. Utility history works without one.
    </Empty>
  );
}

/**
 * Everything kept on this device for the open project (and the device-wide utility runs): what
 * happened, every response, every version of the project's files, utility runs and database
 * queries. Nothing here is stored in the repository.
 */
export default function HistoryPage() {
  const param = useNav((s) => s.param);
  const go = useNav((s) => s.go);
  const root = useWorkspace((s) => s.project?.root ?? null);
  const tab: Tab = TABS.some((t) => t.key === param) ? (param as Tab) : root ? 'activity' : 'utilities';
  const meta = TABS.find((t) => t.key === tab)!;
  useViewLabel(meta.viewLabel);

  return (
    <div className="page">
      <div className="page-header">
        <h1>History</h1>
        <span className="muted small truncate">Kept on this computer — never in the project</span>
        <span className="spacer" />
        <button className="icon-btn" title="History settings: what is kept and for how long" aria-label="History settings" onClick={() => go('settings', 'history')}>
          <Settings2 />
        </button>
        <button
          className="icon-btn"
          title={root ? "Show this project's data folder" : 'Show the app data folder'}
          aria-label="Show data folder"
          onClick={() => void historyApi.dataDir(root).then((d) => revealItemInDir(d)).catch((e) => toast.error('Could not open the folder', errorMessage(e)))}
        >
          <FolderOpen />
        </button>
      </div>
      <ResponsiveTabs tabs={TABS} value={tab} onChange={(k) => go('history', k)} className="subtabs" label="History" />
      <div className="hist-body">
        {meta.needsProject && !root ? (
          <NeedsProject />
        ) : tab === 'activity' ? (
          <ActivityTab root={root!} />
        ) : tab === 'requests' ? (
          <RequestsTab root={root!} />
        ) : tab === 'files' ? (
          <FilesTab root={root!} />
        ) : tab === 'utilities' ? (
          <UtilitiesTab />
        ) : tab === 'queries' ? (
          <QueriesTab root={root!} />
        ) : (
          <AskTab root={root!} />
        )}
      </div>
    </div>
  );
}

/** Shows a past response in its request's editor (History tab of the response panel). */
function openResponse(path: string, id: string) {
  useHistoryFocus.getState().focus(path, id);
  void useWorkspace.getState().openRequest(path, { preview: false });
}

// ---------- activity ----------

function ActivityTab({ root }: { root: string }) {
  const [q, setQ] = useState('');
  const [events, setEvents] = useState<DevEvent[] | null>(null);
  const [limit, setLimit] = useState(PAGE);
  const version = useHistoryVersion((s) => s.v);
  const go = useNav((s) => s.go);

  useEffect(() => {
    let alive = true;
    const t = setTimeout(() => {
      eventsApi
        .list(root, { newestFirst: true, limit, text: q.trim() || undefined })
        .then((e) => alive && setEvents(e))
        .catch((e) => {
          if (alive) setEvents([]);
          toast.error('Project activity', errorMessage(e));
        });
    }, 150);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [root, q, limit, version]);

  const open = (e: DevEvent) => {
    const data = (e.data ?? {}) as { historyId?: string | null };
    if (e.requestPath && data.historyId) openResponse(e.requestPath, data.historyId);
    else if (e.requestPath) void useWorkspace.getState().openRequest(e.requestPath, { preview: false });
  };

  return (
    <>
      <div className="hist-tools">
        <SearchInput value={q} onChange={setQ} placeholder="Search activity…" />
        <span className="spacer" />
        <button
          className="btn btn-sm btn-ghost"
          onClick={() => {
            useLogs.getState().setActive(EVENTS_KEY);
            go('logs');
          }}
          title="Query, chart and correlate events in the log viewer"
        >
          <ScrollText /> Open in Logs
        </button>
      </div>
      <div className="hist-list">
        {!events ? (
          <div className="empty">
            <Spinner />
          </div>
        ) : !events.length ? (
          <Empty icon={<Activity />} title={q ? 'Nothing matches' : 'No activity yet'}>
            Sends, mock hits, webhooks, load runs, notebook runs and more are recorded here as you work.
          </Empty>
        ) : (
          <>
            <ByDay
              items={events}
              at={(e) => e.at}
              id={(e) => e.id}
              render={(e) => (
                <div className="hist-row" onClick={() => open(e)} title={e.requestPath ? 'Open the request' : undefined}>
                  <span className="hist-when">{timeLabel(e.at)}</span>
                  <div className="hist-main">
                    <span className={clsx('small truncate', e.level === 'error' && 'text-danger', e.level === 'warn' && 'text-warning')}>{e.title}</span>
                    <span className="hist-meta">
                      <span className="mono">{e.kind}</span>
                      {e.requestPath && <span className="mono truncate">{e.requestPath}</span>}
                    </span>
                  </div>
                  <span className="hist-meta">{e.durationMs != null && `${Math.round(e.durationMs)} ms`}</span>
                </div>
              )}
            />
            {events.length >= limit && (
              <div className="hist-more">
                <button className="btn btn-sm" onClick={() => setLimit((l) => l + PAGE)}>
                  Load more
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </>
  );
}

// ---------- requests ----------

function pretty(d: HistoryDetail | null): string {
  if (!d?.bodyText) return d?.bodyBase64 ? `<binary ${d.bodyBase64.length} base64 chars>` : '';
  try {
    return JSON.stringify(JSON.parse(d.bodyText), null, 2);
  } catch {
    return d.bodyText;
  }
}

function RequestsTab({ root }: { root: string }) {
  const [q, setQ] = useState('');
  const [items, setItems] = useState<HistorySummary[] | null>(null);
  const [limit, setLimit] = useState(PAGE);
  const [sel, setSel] = useState<string | null>(null);
  const [detail, setDetail] = useState<HistoryDetail | null>(null);
  const version = useHistoryVersion((s) => s.v);

  useEffect(() => {
    let alive = true;
    const t = setTimeout(() => {
      api
        .historySearch(root, q.trim(), 0, limit)
        .then((r) => alive && setItems(r))
        .catch((e) => {
          if (alive) setItems([]);
          toast.error('Request history', errorMessage(e));
        });
    }, 150);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [root, q, limit, version]);
  useEffect(() => {
    if (!sel) return setDetail(null);
    let alive = true;
    api
      .historyGet(root, sel)
      .then((d) => alive && setDetail(d))
      .catch(() => alive && setDetail(null));
    return () => {
      alive = false;
    };
  }, [root, sel]);

  const lang = useMemo(() => languageFor(detail?.entry.contentType ?? undefined, detail?.bodyText ?? undefined), [detail]);

  return (
    <>
      <div className="hist-tools">
        <SearchInput value={q} onChange={setQ} placeholder="Search by URL, name, method or status…" />
      </div>
      <div className="hist-split">
        <div className="hist-list">
          {!items ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : !items.length ? (
            <Empty icon={<Send />} title={q ? 'Nothing matches' : 'No requests sent yet'}>
              Every send is kept here with its response.
            </Empty>
          ) : (
            <>
              <ByDay
                items={items}
                at={(h) => h.at}
                id={(h) => h.id}
                render={(h) => (
                  <div className={clsx('hist-row', sel === h.id && 'on')} onClick={() => setSel(h.id)} onDoubleClick={() => h.requestPath && openResponse(h.requestPath, h.id)}>
                    <span className="hist-when">{timeLabel(h.at)}</span>
                    <div className="hist-main">
                      <span className="row-sm small truncate">
                        <MethodBadge method={h.method} short />
                        <span className="truncate">{h.requestName || h.url}</span>
                      </span>
                      <span className="hist-meta">
                        <span className="mono truncate">{h.url}</span>
                      </span>
                    </div>
                    <span className="hist-meta">
                      {h.status ? <StatusCode status={h.status} /> : <span className="text-danger strong">ERR</span>}
                      {h.testsFailed > 0 && <span className="badge danger">{h.testsFailed}✗</span>}
                    </span>
                  </div>
                )}
              />
              {items.length >= limit && (
                <div className="hist-more">
                  <button className="btn btn-sm" onClick={() => setLimit((l) => l + PAGE)}>
                    Load more
                  </button>
                </div>
              )}
            </>
          )}
        </div>
        <div className="hist-detail">
          {!detail ? (
            <Empty icon={<Send />} title="Pick a request">
              Double-click opens the request with that response.
            </Empty>
          ) : (
            <>
              <div className="toolbar">
                <span className="mono xs truncate grow" title={detail.entry.url}>
                  {detail.entry.method} {detail.entry.url}
                </span>
                {detail.entry.environment && <span className="badge outline">{detail.entry.environment}</span>}
                <span className="xs subtle">
                  {Math.round(detail.entry.durationMs)} ms · {formatBytes(detail.entry.size)}
                </span>
                {detail.entry.requestPath && (
                  <button className="btn btn-sm btn-primary" onClick={() => openResponse(detail.entry.requestPath, detail.entry.id)}>
                    <ExternalLink /> Open request
                  </button>
                )}
              </div>
              <div className="grow min-h-0">
                {detail.entry.error ? (
                  <pre className="code m-3">{detail.entry.error}</pre>
                ) : detail.entry.bodyTruncated && !detail.bodyText && !detail.bodyBase64 ? (
                  <Empty title="Body not kept">Response bodies are off in Settings → History, or the body was too large.</Empty>
                ) : (
                  <CodeEditor value={pretty(detail)} language={lang} readOnly />
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </>
  );
}

// ---------- files ----------

function FilesTab({ root }: { root: string }) {
  const [q, setQ] = useState('');
  const [view, setView] = useState<'files' | 'timeline'>('files');
  const [files, setFiles] = useState<FileSummary[] | null>(null);
  const [recent, setRecent] = useState<FileVersion[] | null>(null);
  const [limit, setLimit] = useState(PAGE);
  const [sel, setSel] = useState<string | null>(null);
  const version = useLocalHistoryVersion((s) => s.files);

  useEffect(() => {
    let alive = true;
    const t = setTimeout(() => {
      const load = view === 'files' ? historyApi.files(root, q.trim()).then((f) => alive && setFiles(f)) : historyApi.recentVersions(root, q.trim(), 0, limit).then((r) => alive && setRecent(r));
      load.catch((e) => toast.error('File history', errorMessage(e)));
    }, 150);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [root, q, view, limit, version]);

  useEffect(() => {
    if (!sel && files?.[0]) setSel(files[0].path);
  }, [files, sel]);

  const list = view === 'files' ? files : recent;
  return (
    <>
      <div className="hist-tools">
        <SearchInput value={q} onChange={setQ} placeholder="Filter by path…" />
        <Segmented
          value={view}
          onChange={setView}
          options={[
            { value: 'files', label: 'By file' },
            { value: 'timeline', label: 'Timeline' },
          ]}
        />
        <span className="spacer" />
        <span className="xs subtle">Requests, environments, notes, mocks, load tests, schemas… — versions stay on this computer.</span>
      </div>
      <div className="hist-split">
        <div className="hist-list">
          {!list ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : !list.length ? (
            <Empty icon={<FileClock />} title={q ? 'Nothing matches' : 'No file versions yet'}>
              Versions are recorded when the project opens and whenever a project file changes.
            </Empty>
          ) : view === 'files' ? (
            files!.map((f) => (
              <div key={f.path} className={clsx('hist-row', sel === f.path && 'on')} onClick={() => setSel(f.path)}>
                <span className="hist-when">
                  <RelTime ts={f.lastAt} />
                </span>
                <div className="hist-main">
                  <span className={clsx('small mono truncate', f.deleted && 'subtle')} title={f.path}>
                    {f.path}
                  </span>
                  <span className="hist-meta">
                    {f.versions} version{f.versions === 1 ? '' : 's'} · {versionLabel({ source: f.lastSource, deleted: f.deleted })}
                  </span>
                </div>
                <span />
              </div>
            ))
          ) : (
            <>
              <ByDay
                items={recent!}
                at={(v) => v.at}
                id={(v) => String(v.id)}
                render={(v) => (
                  <div
                    className={clsx('hist-row', sel === v.path && 'on')}
                    onClick={() => {
                      setSel(v.path);
                    }}
                  >
                    <span className="hist-when">{timeLabel(v.at)}</span>
                    <div className="hist-main">
                      <span className="small mono truncate" title={v.path}>
                        {v.path}
                      </span>
                      <span className="hist-meta">
                        <SourceIcon v={v} /> {versionLabel(v)}
                        {!v.deleted && ` · ${formatBytes(v.size)}`}
                      </span>
                    </div>
                    <span />
                  </div>
                )}
              />
              {recent!.length >= limit && (
                <div className="hist-more">
                  <button className="btn btn-sm" onClick={() => setLimit((l) => l + PAGE)}>
                    Load more
                  </button>
                </div>
              )}
            </>
          )}
        </div>
        <div className="hist-detail">{sel ? <FileHistoryView key={sel} root={root} path={sel} /> : <Empty icon={<FileClock />} title="Pick a file" />}</div>
      </div>
    </>
  );
}

// ---------- utilities ----------

function restoreUtilityRun(r: UtilityRun) {
  openInUtilities(r.op, r.inputMasked ? '' : r.input, restorableArgs(r.args));
}

function UtilitiesTab() {
  const [q, setQ] = useState('');
  const [runs, setRuns] = useState<UtilityRun[] | null>(null);
  const [limit, setLimit] = useState(PAGE);
  const version = useLocalHistoryVersion((s) => s.utilities);

  useEffect(() => {
    let alive = true;
    const t = setTimeout(() => {
      historyApi
        .utilityRuns(undefined, q.trim(), 0, limit)
        .then((r) => alive && setRuns(r))
        .catch((e) => {
          if (alive) setRuns([]);
          toast.error('Utility history', errorMessage(e));
        });
    }, 150);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [q, limit, version]);

  const clear = async () => {
    if (!(await confirmDialog({ title: 'Clear utility history?', message: 'Removes every recorded utility run from this computer.', confirmText: 'Clear', danger: true }))) return;
    try {
      await historyApi.clearUtilityRuns();
      useLocalHistoryVersion.getState().bumpUtilities();
    } catch (e) {
      toast.error('Could not clear', errorMessage(e));
    }
  };

  return (
    <>
      <div className="hist-tools">
        <SearchInput value={q} onChange={setQ} placeholder="Search utility runs…" />
        <span className="spacer" />
        <button className="btn btn-sm btn-danger-ghost" onClick={clear} disabled={!runs?.length}>
          <Trash2 /> Clear
        </button>
      </div>
      <div className="hist-list">
        {!runs ? (
          <div className="empty">
            <Spinner />
          </div>
        ) : !runs.length ? (
          <Empty icon={<Binary />} title={q ? 'Nothing matches' : 'No utility runs yet'}>
            Runs are recorded once their input settles (or when you copy the output) — secrets in arguments are masked.
          </Empty>
        ) : (
          <>
            <ByDay
              items={runs}
              at={(r) => r.at}
              id={(r) => r.id}
              render={(r) => (
                <div className="hist-row" onClick={() => restoreUtilityRun(r)} title="Open in Utilities with this input and these arguments">
                  <span className="hist-when">{timeLabel(r.at)}</span>
                  <div className="hist-main">
                    <span className="small strong truncate">{r.opName}</span>
                    <span className="hist-meta">
                      <span className="mono truncate">{r.inputMasked ? '(secret input not kept)' : r.input.replace(/\s+/g, ' ').slice(0, 200) || '(no input)'}</span>
                    </span>
                  </div>
                  <span className="hist-meta">{!r.ok && <span className="badge danger">error</span>}</span>
                </div>
              )}
            />
            {runs.length >= limit && (
              <div className="hist-more">
                <button className="btn btn-sm" onClick={() => setLimit((l) => l + PAGE)}>
                  Load more
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </>
  );
}

// ---------- queries ----------

function QueriesTab({ root }: { root: string }) {
  const [q, setQ] = useState('');
  const [items, setItems] = useState<HistoryItem[]>(() => readHistory(root));
  const go = useNav((s) => s.go);
  useEffect(() => setItems(readHistory(root)), [root]);
  const shown = useMemo(() => {
    const s = q.trim().toLowerCase();
    return s ? items.filter((h) => h.sql.toLowerCase().includes(s) || h.connection.toLowerCase().includes(s)) : items;
  }, [items, q]);

  const open = async (h: HistoryItem) => {
    try {
      const conn = (await dbApi.list(root)).find((c) => c.slug === h.connection || c.path === h.connection);
      if (!conn) {
        toast.error('Connection not found', `“${h.connection}” is no longer in connections/.`);
        return;
      }
      useDb.getState().addTab(connKey(root, conn.path), newSqlTab({ title: 'From history', sql: h.sql }));
      go('data', conn.path);
    } catch (e) {
      toast.error('Could not open the query', errorMessage(e));
    }
  };

  const clear = async () => {
    if (!(await confirmDialog({ title: 'Clear query history?', message: "Removes this project's query history from this computer.", confirmText: 'Clear', danger: true }))) return;
    clearHistory(root);
    setItems([]);
  };

  return (
    <>
      <div className="hist-tools">
        <SearchInput value={q} onChange={setQ} placeholder="Search queries…" />
        <span className="spacer" />
        <button className="btn btn-sm btn-danger-ghost" onClick={clear} disabled={!items.length}>
          <Trash2 /> Clear
        </button>
      </div>
      <div className="hist-list">
        {!shown.length ? (
          <Empty icon={<Database />} title={q ? 'Nothing matches' : 'No queries yet'}>
            SQL you run in Databases is kept here (on this computer, never in the project).
          </Empty>
        ) : (
          <ByDay
            items={shown}
            at={(h) => h.at}
            id={(h, i) => `${h.at}-${i}`}
            render={(h) => (
              <div className="hist-row" onClick={() => void open(h)} title="Open in Databases (new query tab)">
                <span className="hist-when">{timeLabel(h.at)}</span>
                <div className="hist-main">
                  <span className="small mono truncate">{h.sql.replace(/\s+/g, ' ')}</span>
                  <span className="hist-meta">
                    <span className="mono">{h.connection}</span>
                    <span>{h.ms < 1000 ? `${Math.round(h.ms)} ms` : `${(h.ms / 1000).toFixed(1)} s`}</span>
                    {h.rows !== undefined && <span>{h.rows.toLocaleString()} rows</span>}
                  </span>
                </div>
                <span className="hist-meta">{!h.ok && <span className="badge danger">failed</span>}</span>
              </div>
            )}
          />
        )}
      </div>
    </>
  );
}

// ---------- AI answers ----------

function AskTab({ root }: { root: string }) {
  const [q, setQ] = useState('');
  const [items, setItems] = useState<AskRecord[] | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const go = useNav((s) => s.go);

  useEffect(() => {
    let alive = true;
    historyApi
      .askHistory(root)
      .then((list) => alive && setItems([...list].reverse()))
      .catch(() => alive && setItems([]));
    return () => {
      alive = false;
    };
  }, [root]);
  const shown = useMemo(() => {
    const s = q.trim().toLowerCase();
    return (items ?? []).filter((a) => !s || a.question.toLowerCase().includes(s) || a.answer.toLowerCase().includes(s));
  }, [items, q]);
  const current = shown.find((a) => a.id === sel) ?? shown[0];

  const clear = async () => {
    if (!(await confirmDialog({ title: 'Clear saved AI answers?', message: "Removes this project's saved questions and answers from this computer.", confirmText: 'Clear', danger: true }))) return;
    await historyApi.clearAsks(root).catch((e) => toast.error('Could not clear', errorMessage(e)));
    setItems([]);
  };

  return (
    <>
      <div className="hist-tools">
        <SearchInput value={q} onChange={setQ} placeholder="Search questions and answers…" />
        <span className="spacer" />
        <button className="btn btn-sm btn-ghost" onClick={() => go('ai', 'ask')}>
          <MessageSquare /> Ask
        </button>
        <button className="btn btn-sm btn-danger-ghost" onClick={clear} disabled={!items?.length}>
          <Trash2 /> Clear
        </button>
      </div>
      <div className="hist-split">
        <div className="hist-list">
          {!items ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : !shown.length ? (
            <Empty icon={<MessageSquare />} title={q ? 'Nothing matches' : 'No saved answers yet'}>
              The last 50 questions asked in Local AI → Ask are kept here, on this computer.
            </Empty>
          ) : (
            <ByDay
              items={shown}
              at={(a) => a.at}
              id={(a) => a.id}
              render={(a) => (
                <div className={clsx('hist-row', current?.id === a.id && 'on')} onClick={() => setSel(a.id)}>
                  <span className="hist-when">{timeLabel(a.at)}</span>
                  <div className="hist-main">
                    <span className="small truncate">{a.question}</span>
                    <span className="hist-meta">
                      <span className="truncate">{a.answer.replace(/\s+/g, ' ').slice(0, 160)}</span>
                    </span>
                  </div>
                  <span />
                </div>
              )}
            />
          )}
        </div>
        <div className="hist-detail">
          {current ? (
            <div className="p-5 col" style={{ overflow: 'auto' }}>
              <div className="strong">{current.question}</div>
              <Markdown source={current.answer} />
              {current.citations.length > 0 && (
                <div className="xs subtle">Sources: {current.citations.map((c) => c.title).join(' · ')}</div>
              )}
              {current.model && <div className="xs subtle">{current.model}</div>}
            </div>
          ) : (
            <Empty icon={<MessageSquare />} title="Pick a question" />
          )}
        </div>
      </div>
    </>
  );
}

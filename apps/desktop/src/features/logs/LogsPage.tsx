import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { Activity, ClipboardPaste, Combine, FileText, FolderOpen, GitBranch, History, Radio, ScrollText, Trash2, X, ZoomOut } from 'lucide-react';
import { Callout, Empty, Kbd, Spinner, Switch, formatBytes } from '@/components/ui/misc';
import { Modal } from '@/components/ui/Modal';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { LEVELS, eventsApi, logsApi, type Filter, type Histogram, type Level, type LogEntry, type SourceInfo } from '@/lib/api/logs';
import { useWorkspace } from '@/store/workspace';
import { useProjectRepos } from '@/store/repos';
import { isMultiRepo } from '@/features/git/model';
import { DetailDrawer } from './DetailDrawer';
import { LogTable, type LogTableHandle } from './LogTable';
import { PatternsView } from './PatternsView';
import { QueryBar } from './QueryBar';
import { SqlView } from './SqlView';
import { Timeline } from './Timeline';
import { dataApi } from './source';
import { EVENTS_KEY, useLogs, type Tab, type ViewState } from './store';
import { LEVEL_LABEL, PRESETS, TOGGLE_LEVELS, addClause, filterKey, fmtCount, fmtDuration, fmtSpan, levelVar } from './format';
import './logs.css';
import { useActiveViewRef, useViewLabel } from '@/store/nav';
import { SidePane } from '@/components/layout/SidePane';


function isTyping(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null;
  return !!t && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName) || !!t.closest('.monaco-editor'));
}

export default function LogsPage() {
  const root = useWorkspace((s) => s.project?.root);
  const sources = useLogs((s) => s.sources);
  const active = useLogs((s) => s.active);
  const pending = useLogs((s) => s.pending);
  useViewLabel(active === EVENTS_KEY ? 'Project events' : (sources.find((s) => s.handle === active)?.name ?? null));
  const [dropOver, setDropOver] = useState(false);
  const [paste, setPaste] = useState(false);

  // Requests from other modules (openInLogViewer).
  useEffect(() => {
    const p = useLogs.getState().takePending();
    if (!p) return;
    const st = useLogs.getState();
    const withQuery = (key: string | undefined) => {
      if (!key) return;
      st.setActive(key);
      if (p.query != null) st.updateView(key, (v) => ({ filter: { ...v.filter, query: p.query! } }));
    };
    const s = p.source;
    switch (s.type) {
      case 'file':
        void st.openFile(s.path).then(withQuery);
        break;
      case 'files':
        void st.openFiles(s.paths).then(withQuery);
        break;
      case 'text':
        void st.openText(s.name, s.text).then(withQuery);
        break;
      case 'handle':
        void st.adopt(s.handle).then(withQuery);
        break;
      case 'events': {
        const q = s.correlationId ? `trace=${JSON.stringify(s.correlationId)}` : (s.query ?? p.query);
        st.setActive(EVENTS_KEY);
        if (q != null) st.updateView(EVENTS_KEY, (v) => ({ filter: { ...v.filter, query: q } }));
        break;
      }
    }
  }, [pending]);

  // Pick something sensible when the active source disappears.
  useEffect(() => {
    if (active === EVENTS_KEY ? !root : !sources.some((s) => s.handle === active)) {
      useLogs.getState().setActive(root ? EVENTS_KEY : (sources.at(-1)?.handle ?? EVENTS_KEY));
    }
  }, [active, root, sources]);

  // Drop files onto the window to open them (several = merged by time).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let alive = true;
    getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        if (p.type === 'enter' || p.type === 'over') setDropOver(true);
        else if (p.type === 'leave') setDropOver(false);
        else if (p.type === 'drop') {
          setDropOver(false);
          if (p.paths.length) void useLogs.getState().openFiles(p.paths);
        }
      })
      .then((u) => (alive ? (unlisten = u) : u()))
      .catch(() => {});
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  // `dir`: start the file dialog in one of the project's repositories (multi-repo projects).
  const openFile = async (dir?: string) => {
    const picked = await openDialog({ multiple: true, title: 'Open log files (several are merged by time)', defaultPath: dir });
    const paths = Array.isArray(picked) ? picked : typeof picked === 'string' ? [picked] : [];
    if (paths.length) await useLogs.getState().openFiles(paths);
  };

  const api = useMemo(() => dataApi(active, root), [active, root]);
  const info = sources.find((s) => s.handle === active);

  return (
    <div className={clsx('split logs-page', dropOver && 'drop-over')}>
      <SourcesPane root={root} onOpen={openFile} onPaste={() => setPaste(true)} />
      <div className="pane fill">
        {api && (active === EVENTS_KEY || info) ? (
          <SourceView key={active} api={api} info={info} root={root} />
        ) : (
          <Empty
            icon={<ScrollText />}
            title="Open a log file"
            action={
              <button className="btn btn-primary btn-sm" onClick={() => openFile()}>
                <FolderOpen /> Open file…
              </button>
            }
          >
            Multi-GB files, .gz, JSON/logfmt/syslog/access logs — or drop files here. Open a project to see its event timeline.
          </Empty>
        )}
      </div>
      {dropOver && <div className="logs-drop">Drop to open (several files are merged by time)</div>}
      <PasteModal open={paste} onClose={() => setPaste(false)} />
    </div>
  );
}

// ---------- sidebar ----------

function phaseText(p: { phase: string; bytesDone: number; bytesTotal: number }) {
  const pct = p.bytesTotal ? Math.floor((p.bytesDone / p.bytesTotal) * 100) : 0;
  return p.phase === 'decompressing' ? `Decompressing ${pct}%` : p.phase === 'transcoding' ? `Converting ${pct}%` : `Indexing ${pct}%`;
}

function SourcesPane({ root, onOpen, onPaste }: { root?: string; onOpen(dir?: string): void; onPaste(): void }) {
  const allRepos = useProjectRepos();
  const repos = isMultiRepo(allRepos) ? allRepos.filter((r) => r.state === 'found') : [];
  const sources = useLogs((s) => s.sources);
  const active = useLogs((s) => s.active);
  const recent = useLogs((s) => s.recent);
  const progress = useLogs((s) => s.progress);
  const st = useLogs.getState();

  const item = (key: string, body: React.ReactNode, extra?: React.ReactNode, title?: string) => (
    <div
      key={key}
      role="option"
      aria-selected={active === key}
      tabIndex={0}
      title={title}
      className={clsx('list-item', active === key && 'on')}
      onClick={() => st.setActive(key)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          st.setActive(key);
        } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          e.preventDefault();
          const sib = (e.key === 'ArrowDown' ? e.currentTarget.nextElementSibling : e.currentTarget.previousElementSibling) as HTMLElement | null;
          sib?.focus();
        }
      }}
    >
      {body}
      {extra}
    </div>
  );

  return (
    <SidePane
      title="Logs"
      actions={
        <>
          <button className="icon-btn" onClick={onPaste} title="Paste text as a log" aria-label="Paste text as a log">
            <ClipboardPaste />
          </button>
          <button className="icon-btn" onClick={() => onOpen()} title="Open file(s)… (several are merged by time)" aria-label="Open log files">
            <FolderOpen />
          </button>
        </>
      }
      footer={<div className="logs-drop-hint xs subtle">Drop files anywhere to open · several are merged by timestamp</div>}
    >
      <div className="list logs-sources" role="listbox" aria-label="Log sources">
        {root &&
          item(
            EVENTS_KEY,
            <>
              <Activity size={14} className="text-accent" />
              <span className="grow truncate">Project events</span>
            </>,
            <span className="meta">live</span>,
            'Everything modules did in this project: sends, mock hits, webhooks, load runs…',
          )}
        {sources.length > 0 && <div className="list-group-header">Open</div>}
        {sources.map((s) => {
          const prog = Object.values(progress[s.handle] ?? {});
          const busy = prog.find((p) => p.phase !== 'ready' && p.phase !== 'failed');
          const Icon = s.kind.type === 'merge' ? Combine : s.kind.type === 'stream' ? Radio : FileText;
          return item(
            s.handle,
            <>
              <Icon size={14} className="subtle" />
              <span className="grow truncate">{s.name}</span>
            </>,
            <>
              <span className="meta">{busy ? phaseText(busy) : fmtCount(s.entries)}</span>
              <button
                className="icon-btn sm close"
                aria-label={`Close ${s.name}`}
                onClick={(e) => {
                  e.stopPropagation();
                  void st.close(s.handle);
                }}
              >
                <X />
              </button>
              {busy && (
                <span className="logs-src-progress">
                  <span style={{ width: `${busy.bytesTotal ? (busy.bytesDone / busy.bytesTotal) * 100 : 0}%` }} />
                </span>
              )}
            </>,
            s.kind.type === 'file' ? s.kind.path : s.kind.type === 'merge' ? s.kind.paths.join('\n') : s.name,
          );
        })}
        {recent.length > 0 && (
          <div className="list-group-header">
            <History size={12} /> Recent
          </div>
        )}
        {recent.map((r) => (
          <div
            key={r.path}
            className="list-item logs-recent"
            tabIndex={0}
            title={r.path}
            onClick={() => void st.openFile(r.path)}
            onKeyDown={(e) => e.key === 'Enter' && void st.openFile(r.path)}
          >
            <FileText size={14} className="subtle" />
            <span className="grow truncate">{r.name}</span>
            <button
              className="icon-btn sm close"
              aria-label={`Forget ${r.name}`}
              onClick={(e) => {
                e.stopPropagation();
                st.forgetRecent(r.path);
              }}
            >
              <X />
            </button>
          </div>
        ))}
        {repos.length > 0 && (
          <div className="list-group-header" title="Open log files from one of the project's repositories">
            <GitBranch size={12} /> Repositories
          </div>
        )}
        {repos.map((r) => (
          <div key={r.name} className="list-item logs-recent" tabIndex={0} title={`Open log files in ${r.path}`} onClick={() => onOpen(r.path)} onKeyDown={(e) => e.key === 'Enter' && onOpen(r.path)}>
            <FolderOpen size={14} className="subtle" />
            <span className="grow truncate">{r.name}</span>
            {r.group && <span className="meta">{r.group}</span>}
          </div>
        ))}
      </div>
    </SidePane>
  );
}

function PasteModal({ open, onClose }: { open: boolean; onClose(): void }) {
  const [name, setName] = useState('Pasted log');
  const [text, setText] = useState('');
  const submit = async () => {
    if (!text.trim()) return;
    const h = await useLogs.getState().openText(name.trim() || 'Pasted log', text);
    if (h) {
      setText('');
      onClose();
    }
  };
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Paste a log"
      size="lg"
      footer={
        <>
          <button className="btn btn-sm" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-sm btn-primary" onClick={submit} disabled={!text.trim()}>
            Open
          </button>
        </>
      }
    >
      <div className="field">
        <label>Name</label>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="field mt-4">
        <label>Text</label>
        <textarea className="textarea mono" rows={14} value={text} onChange={(e) => setText(e.target.value)} placeholder="Paste log lines (JSON, logfmt, syslog, access logs, stack traces…)" autoFocus />
      </div>
    </Modal>
  );
}

// ---------- one source ----------

function levelCounts(h: Histogram | null): Record<Level, number> {
  const out = Object.fromEntries(LEVELS.map((l) => [l, 0])) as Record<Level, number>;
  for (const b of h?.buckets ?? []) LEVELS.forEach((l, i) => (out[l] += b.counts[i] ?? 0));
  return out;
}

function SourceView({ api, info, root }: { api: NonNullable<ReturnType<typeof dataApi>>; info?: SourceInfo; root?: string }) {
  const key = api.key;
  const view: ViewState = useLogs((s) => s.views[key]) ?? useLogs.getState().view(key);
  const version = useLogs((s) => s.versions[key] ?? 0);
  const update = useCallback((patch: Partial<ViewState> | ((v: ViewState) => Partial<ViewState>)) => useLogs.getState().updateView(key, patch), [key]);
  const filter = view.filter;
  const fkey = filterKey(filter);
  const [hist, setHist] = useState<Histogram | null>(null);
  const [histLoading, setHistLoading] = useState(false);
  const [totals, setTotals] = useState({ total: 0, viewTotal: 0, indexing: false, error: undefined as string | undefined });
  const [selected, setSelected] = useState<number | null>(null);
  const [detail, setDetail] = useState<LogEntry | null>(null);
  const table = useRef<LogTableHandle>(null);
  const queryInput = useRef<HTMLInputElement>(null);
  const followable = api.isEvents || !!info?.followable;
  const merged = info?.kind.type === 'merge';

  const setFilter = useCallback((f: Partial<Filter>) => update((v) => ({ filter: { ...v.filter, ...f } })), [update]);
  const addFilter = useCallback(
    (clause: string) => {
      update((v) => ({ filter: { ...v.filter, query: addClause(v.filter.query, clause) }, tab: 'logs' }));
    },
    [update],
  );

  // Histogram for the current filter.
  useEffect(() => {
    let alive = true;
    setHistLoading(true);
    const t = setTimeout(() => {
      api
        .histogram(filter, 120)
        .then((h) => alive && setHist(h))
        .catch(() => alive && setHist(null))
        .finally(() => alive && setHistLoading(false));
    }, 60);
    return () => {
      alive = false;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, fkey, version]);

  // Live data: event stream (always) or file tail (when following).
  useEffect(() => {
    let id: string | undefined;
    let stopped = false;
    let last = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const bump = () => {
      const wait = 400 - (Date.now() - last);
      if (wait <= 0) {
        last = Date.now();
        useLogs.getState().bump(key);
      } else if (!timer) {
        timer = setTimeout(() => {
          timer = undefined;
          last = Date.now();
          useLogs.getState().bump(key);
        }, wait);
      }
    };
    const start = api.isEvents
      ? root
        ? eventsApi.tail(root, (m) => (m.type === 'events' ? bump() : useLogs.getState().bump(key)))
        : null
      : view.follow && info?.followable
        ? logsApi.tail(key, (m) => {
            if (m.type === 'appended') bump();
            else if (m.type === 'reset') {
              toast.info(m.reason === 'rotated' ? 'Log file rotated' : 'Log file truncated', 'Re-reading from the start.');
              useLogs.getState().bump(key);
              void useLogs.getState().refreshInfo(key);
            } else if (m.type === 'error') toast.error('Following stopped', m.message);
          })
        : null;
    start
      ?.then((tid) => {
        if (stopped) void logsApi.untail(tid);
        else id = tid;
      })
      .catch((e) => toast.error('Could not follow', errorMessage(e)));
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
      if (id) void logsApi.untail(id);
    };
  }, [api, key, root, view.follow, info?.followable]);

  // Page keys: / query, f follow, Alt+1/2/3 tabs, Esc closes details.
  const activeView = useActiveViewRef();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!activeView.current) return;
      if (e.altKey && ['1', '2', '3'].includes(e.key)) {
        e.preventDefault();
        update({ tab: (['logs', 'patterns', 'sql'] as Tab[])[Number(e.key) - 1] });
        return;
      }
      if (isTyping(e) || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === '/') {
        e.preventDefault();
        queryInput.current?.focus();
        queryInput.current?.select();
      } else if (e.key === 'f' && followable) {
        e.preventDefault();
        update((v) => ({ follow: !v.follow }));
      } else if (e.key === 'Escape' && detail) {
        setDetail(null);
        table.current?.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [update, followable, detail]);

  useEffect(() => setSelected(null), [fkey]);

  const counts = useMemo(() => levelCounts(hist), [hist]);
  const anchor = () => {
    if (api.isEvents) return Date.now();
    const last = Math.max(...(info?.files.map((f) => f.lastTs ?? 0) ?? [0]));
    return last > 0 ? last + 1 : Date.now();
  };
  const setPreset = (id: string) => {
    const p = PRESETS.find((x) => x.id === id);
    if (!p) return;
    if (p.ms == null) update((v) => ({ preset: id, filter: { ...v.filter, from: null, to: null } }));
    else {
      const a = anchor();
      update((v) => ({ preset: id, filter: { ...v.filter, from: a - p.ms!, to: null } }));
    }
  };
  const toggleLevel = (l: Level, solo: boolean) => {
    const on = filter.levels.length ? filter.levels : TOGGLE_LEVELS;
    let next: Level[];
    if (solo) next = on.length === 1 && on[0] === l ? [] : [l];
    else next = on.includes(l) ? on.filter((x) => x !== l) : [...on, l];
    if (next.length === TOGGLE_LEVELS.length) next = [];
    setFilter({ levels: next });
  };
  const locate = async (ts: number) => {
    try {
      const row = await api.locate(filter, ts);
      if (row != null) {
        update({ tab: 'logs', follow: false });
        setSelected(row);
        requestAnimationFrame(() => table.current?.scrollToRow(row));
      }
    } catch (e) {
      toast.error('Could not jump', errorMessage(e));
    }
  };
  const clearEvents = async () => {
    if (!root || !(await confirmDialog({ title: 'Clear project events?', message: 'Removes every recorded event of this project from this computer.', confirmText: 'Clear', danger: true }))) return;
    await eventsApi.clear(root).catch((e) => toast.error('Could not clear', errorMessage(e)));
    useLogs.getState().bump(key);
  };

  const file = info?.files[0];
  const indexing = info?.indexing || totals.indexing;
  const withDate = !!hist && hist.end - hist.start > 86_400_000;

  return (
    <div className="logs-view">
      <div className="toolbar logs-toolbar">
        <QueryBar applied={filter.query} onApply={(q) => setFilter({ query: q })} inputRef={queryInput} />
        <div className="logs-levels" role="group" aria-label="Levels">
          {TOGGLE_LEVELS.map((l) => {
            const on = !filter.levels.length || filter.levels.includes(l);
            return (
              <button
                key={l}
                className={clsx('lvl-toggle', on && 'on')}
                style={{ ['--c' as string]: levelVar(l) }}
                aria-pressed={on}
                title={`${LEVEL_LABEL[l]}: click to toggle, Alt/${'⌘'}+click to show only this`}
                onClick={(e) => toggleLevel(l, e.altKey || e.metaKey || e.ctrlKey)}
              >
                <span className="sw" />
                {LEVEL_LABEL[l]}
                {on && counts[l] > 0 && <span className="n">{fmtCount(counts[l])}</span>}
              </button>
            );
          })}
        </div>
        <select className="select sm" value={view.preset} onChange={(e) => setPreset(e.target.value)} aria-label="Time range" title={api.isEvents ? undefined : 'Relative to the newest entry'}>
          {PRESETS.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
          {view.preset === 'custom' && <option value="custom">Custom range</option>}
        </select>
        {followable && <Switch checked={view.follow} onChange={(v) => update({ follow: v })} label="Follow" title="Follow new entries (f)" />}
        {api.isEvents && (
          <button className="icon-btn" onClick={clearEvents} title="Clear project events" aria-label="Clear project events">
            <Trash2 />
          </button>
        )}
      </div>
      <div className="logs-timeline-row">
        <Timeline
          hist={hist}
          loading={histLoading}
          zoomed={view.preset === 'custom'}
          onZoom={(from, to) => update((v) => ({ preset: 'custom', filter: { ...v.filter, from, to } }))}
          onReset={() => setPreset('all')}
          onLocate={locate}
        />
        {view.preset === 'custom' && filter.from != null && filter.to != null && (
          <div className="logs-zoom small">
            <span className="subtle">{fmtSpan(filter.from, filter.to)}</span>
            <button className="btn btn-sm btn-ghost" onClick={() => setPreset('all')} title="Back to all time (Esc on the chart)">
              <ZoomOut /> Reset
            </button>
          </div>
        )}
      </div>
      <div className="subtabs">
        {(['logs', 'patterns', 'sql'] as Tab[]).map((t, i) => (
          <button key={t} className={clsx('subtab', view.tab === t && 'on')} onClick={() => update({ tab: t })} title={`Alt+${i + 1}`}>
            {t === 'logs' ? 'Logs' : t === 'patterns' ? 'Patterns' : 'SQL'}
            {t === 'logs' && <span className="badge">{fmtCount(totals.total)}</span>}
          </button>
        ))}
      </div>
      <div className="logs-body">
        {view.tab === 'logs' && (
          <div className="logs-main">
            {totals.error && <Callout kind="danger" className="m-3">{totals.error}</Callout>}
            <LogTable
              ref={table}
              api={api}
              filter={filter}
              version={version}
              follow={view.follow && followable}
              sourceColumn={api.isEvents || merged}
              withDate={withDate}
              selected={selected}
              onSelect={(row, e) => {
                setSelected(row);
                if (detail && e) setDetail(e);
              }}
              onOpen={(e) => setDetail(e)}
              onAddClause={addFilter}
              onTotals={(total, viewTotal, idx, error) => setTotals({ total, viewTotal, indexing: idx, error })}
              onLeaveEnd={() => update({ follow: false })}
            />
            {detail && (
              <DetailDrawer
                api={api}
                entry={detail}
                onClose={() => setDetail(null)}
                onAddClause={addFilter}
                onShowCorrelated={
                  root
                    ? (id) => {
                        useLogs.getState().updateView(EVENTS_KEY, (v) => ({ filter: { ...v.filter, query: `trace=${JSON.stringify(id)}` }, tab: 'logs' }));
                        useLogs.getState().setActive(EVENTS_KEY);
                      }
                    : undefined
                }
              />
            )}
          </div>
        )}
        {view.tab === 'patterns' && <PatternsView api={api} filter={filter} version={version} onPick={addFilter} />}
        {view.tab === 'sql' && <SqlView api={api} sql={view.sql} onSql={(sql) => update({ sql })} />}
      </div>
      <div className="statusbar logs-status">
        <span>
          {fmtCount(totals.total)} {totals.total === 1 ? 'entry' : 'entries'}
          {totals.viewTotal !== totals.total && ` of ${fmtCount(totals.viewTotal)}`}
        </span>
        {indexing && (
          <span className="row-sm">
            <Spinner /> indexing…
          </span>
        )}
        {file && !api.isEvents && (
          <>
            <span>{merged ? `${info!.files.length} files` : file.formatLabel}</span>
            {!merged && <span>{formatBytes(file.size)}{file.compressed ? ' (gz)' : ''}</span>}
            {!merged && file.lines > 0 && <span>{fmtCount(file.lines)} lines</span>}
            {file.firstTs != null && file.lastTs != null && <span>{fmtDuration(file.lastTs - file.firstTs)} span</span>}
          </>
        )}
        {hist && hist.withoutTs > 0 && <span className="subtle">{fmtCount(hist.withoutTs)} without timestamp</span>}
        <span className="grow" />
        <span className="subtle">
          <Kbd>/</Kbd> search · <Kbd>j</Kbd>/<Kbd>k</Kbd> move · <Kbd>Enter</Kbd> details{followable ? ' · ' : ''}
          {followable && <Kbd>f</Kbd>}
          {followable && ' follow'}
        </span>
      </div>
    </div>
  );
}

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import {
  Activity,
  Ban,
  Download,
  FileCheck2,
  FileCode2,
  FolderInput,
  FolderOpen,
  Gauge,
  Globe,
  Pause,
  Play,
  Plug,
  Radar,
  RotateCw,
  Save,
  Server,
  Settings2,
  Square,
  Terminal,
  Trash2,
  Upload,
  Wand2,
} from 'lucide-react';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Checkbox, Empty, Kbd, MOD, Resizer, SearchInput, Spinner, copyText, formatBytes } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import { proxyApi, type BrowserInfo, type ContentKind, type ExchangeId, type Rule } from '@/lib/api/proxy';
import { useNav, useActiveViewRef } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { useOpenApi } from '@/features/openapi';
import { BreakpointEditor } from './BreakpointEditor';
import { ConnectDialog, TrustBadge, launchBrowser } from './ConnectDialog';
import { ExchangeDetailView } from './ExchangeDetail';
import { ExchangeList, ROW_H } from './ExchangeList';
import { OpenApiDialog, StubsDialog } from './ExportDialogs';
import { KIND_LABEL, isEmptyFilter } from './filter';
import { hostOf } from './format';
import { RulesPanel, newRule } from './RulesPanel';
import { SettingsDialog } from './SettingsDialog';
import { useInspector, visibleIds } from './store';
import './inspector.css';

const LIST_KEY = 'devall.inspector.listWidth';
const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS', 'HEAD'];
const KINDS: ContentKind[] = ['json', 'html', 'script', 'css', 'image', 'font', 'media', 'xml', 'text', 'webSocket', 'other'];

type Dialog = null | 'connect' | 'rules' | 'settings' | 'stubs' | 'openapi';

function useListWidth(): [number, (w: number) => void] {
  const [w, setW] = useState(() => {
    try {
      return Number(localStorage.getItem(LIST_KEY)) || 620;
    } catch {
      return 620;
    }
  });
  const set = useCallback((n: number) => {
    setW(n);
    try {
      localStorage.setItem(LIST_KEY, String(n));
    } catch {
      /* storage unavailable */
    }
  }, []);
  return [w, set];
}

export default function InspectorPage() {
  const ready = useInspector((s) => s.ready);
  const status = useInspector((s) => s.status);
  const order = useInspector((s) => s.order);
  const items = useInspector((s) => s.items);
  const version = useInspector((s) => s.version);
  const filter = useInspector((s) => s.filter);
  const setFilter = useInspector((s) => s.setFilter);
  const selected = useInspector((s) => s.selected);
  const focus = useInspector((s) => s.focus);
  const paused = useInspector((s) => s.paused);
  const rules = useInspector((s) => s.rules);
  const project = useWorkspace((s) => s.project);
  const go = useNav((s) => s.go);
  const root = project?.root;

  const [dialog, setDialog] = useState<Dialog>(null);
  const [dialogIds, setDialogIds] = useState<ExchangeId[]>([]);
  const [ruleSeed, setRuleSeed] = useState<Rule | null>(null);
  const [port, setPort] = useState('');
  const [busy, setBusy] = useState(false);
  const [trusted, setTrusted] = useState<boolean | null | undefined>(undefined);
  const [browsers, setBrowsers] = useState<BrowserInfo[]>([]);
  const [listWidth, setListWidth] = useListWidth();
  const listRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    void useInspector.getState().init();
    proxyApi
      .caInfo(true)
      .then((v) => {
        setTrusted(v.trust.installed);
        setBrowsers(v.browsers);
      })
      .catch(() => setTrusted(null));
  }, []);

  useEffect(() => {
    if (status && !port) setPort(String(status.port ?? ''));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status]);

  const ids = useMemo(() => visibleIds({ order, items, filter }), [order, items, filter, version]); // eslint-disable-line react-hooks/exhaustive-deps
  const pausedList = Object.values(paused);
  const targets = selected.length ? selected : focus != null ? [focus] : [];
  const focusPaused = focus != null ? paused[focus] : undefined;

  // ---------- actions ----------

  const start = async () => {
    setBusy(true);
    try {
      const p = Number(port);
      const st = await proxyApi.start(Number.isInteger(p) && p > 0 && p < 65536 ? p : undefined);
      useInspector.setState({ status: st });
      setPort(String(st.port ?? ''));
    } catch (e) {
      toast.error('Could not start the proxy', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      useInspector.setState({ status: await proxyApi.stop() });
    } finally {
      setBusy(false);
    }
  };

  const clear = async () => {
    if (order.length > 50 && !(await confirmDialog({ title: 'Clear the capture?', message: `${order.length} exchanges will be removed from the list.`, confirmText: 'Clear', danger: true }))) return;
    await proxyApi.clear();
  };

  const remove = async (list: ExchangeId[]) => {
    if (!list.length) return;
    await proxyApi.remove(list);
  };

  const needRoot = (): string | null => {
    if (!root) {
      toast.info('Open a project first', 'Exports are written into the open project.');
      return null;
    }
    return root;
  };

  const saveRequests = async () => {
    const r = needRoot();
    if (!r || !targets.length) return;
    const folder = await promptDialog({ title: `Save ${targets.length} request${targets.length === 1 ? '' : 's'}`, label: 'Folder under http/', initial: 'captured', validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!folder) return;
    try {
      const paths = await proxyApi.toRequests(r, targets, folder);
      await useWorkspace.getState().refresh();
      toast.success(`${paths.length} request${paths.length === 1 ? '' : 's'} saved`, `http/${folder}/ — credentials were replaced by {{secret.…}} references`, {
        label: 'Open',
        run: () => go('http'),
      });
    } catch (e) {
      toast.error('Could not save requests', errorMessage(e));
    }
  };

  /** Redacts Authorization / Cookie / Set-Cookie / API-key values unless `includeSensitive`. */
  const exportHar = async (includeSensitive = false) => {
    const list = targets.length ? targets : ids;
    if (!list.length) return;
    if (
      includeSensitive &&
      !(await confirmDialog({
        title: 'Export with credentials?',
        message: 'Authorization, Cookie, Set-Cookie and API-key header values will be written as captured. Do not share this file.',
        confirmText: 'Export with credentials',
        danger: true,
      }))
    )
      return;
    const path = await saveDialog({ defaultPath: 'capture.har', filters: [{ name: 'HAR', extensions: ['har', 'json'] }] });
    if (!path) return;
    try {
      await proxyApi.exportHar(list, includeSensitive, path);
      toast.success(`HAR with ${list.length} entr${list.length === 1 ? 'y' : 'ies'} saved`, path);
    } catch (e) {
      toast.error('Export failed', errorMessage(e));
    }
  };

  const toLoad = async () => {
    const r = needRoot();
    if (!r || !targets.length) return;
    const name = await promptDialog({ title: 'Create a load test', label: 'Name', initial: `${hostOf(items.get(targets[0])?.url ?? '')} flow`, validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!name) return;
    try {
      const res = await proxyApi.toLoad(r, targets, name);
      await useWorkspace.getState().refresh();
      toast.success('Load test created', `load/${res.loadFile} replays ${res.requests.length} saved request(s)`, { label: 'Open', run: () => go('load', res.loadFile) });
    } catch (e) {
      toast.error('Could not create the load test', errorMessage(e));
    }
  };

  const replay = async () => {
    const list = targets.slice(0, 20);
    let last: ExchangeId | null = null;
    for (const id of list) {
      try {
        last = await proxyApi.replay(id);
      } catch (e) {
        toast.error('Replay failed', errorMessage(e));
        break;
      }
    }
    if (last != null) useInspector.getState().select(last);
  };

  const curl = async () => {
    if (targets.length !== 1) return;
    try {
      await copyText(await proxyApi.copyCurl(targets[0]), 'cURL copied (credentials redacted)');
    } catch (e) {
      toast.error('Copy failed', errorMessage(e));
    }
  };

  const ruleFrom = () => {
    const x = targets.length ? items.get(targets[0]) : undefined;
    let path: string | undefined;
    try {
      path = x ? new URL(x.url).pathname : undefined;
    } catch {
      path = undefined;
    }
    setRuleSeed(x ? newRule({ name: `${x.method} ${hostOf(x.url)}${path ?? ''}`, match: { methods: [x.method], host: hostOf(x.url).split(':')[0], path } }, 'breakpoint') : null);
    setDialog('rules');
  };

  const openDialogFor = (d: Dialog) => {
    if ((d === 'stubs' || d === 'openapi') && !needRoot()) return;
    setDialogIds(d === 'openapi' && !targets.length ? ids : targets);
    setDialog(d);
  };

  const saveSession = async () => {
    const r = needRoot();
    if (!r) return;
    const name = await promptDialog({ title: 'Save capture', label: 'Name (kept on this computer, never in the project)', initial: 'capture' });
    if (name == null) return;
    try {
      const file = await proxyApi.saveSession(r, targets.length > 1 ? targets : [], name);
      toast.success('Capture saved', file);
    } catch (e) {
      toast.error('Could not save the capture', errorMessage(e));
    }
  };

  const importHar = async () => {
    const path = await openDialog({ multiple: false, filters: [{ name: 'HAR', extensions: ['har', 'json'] }] });
    if (typeof path !== 'string') return;
    try {
      const n = await proxyApi.importHar(await api.readTextFile(path));
      toast.success(`Imported ${n.length} exchange${n.length === 1 ? '' : 's'}`, path);
    } catch (e) {
      toast.error('Import failed', errorMessage(e));
    }
  };

  const [sessions, setSessions] = useState<{ file: string; size: number }[]>([]);
  const loadSessions = async () => {
    if (!root) return setSessions([]);
    try {
      setSessions(await proxyApi.sessions(root));
    } catch {
      setSessions([]);
    }
  };

  // ---------- keyboard ----------

  const onListKey = (e: React.KeyboardEvent) => {
    const s = useInspector.getState();
    const idx = s.focus != null ? ids.indexOf(s.focus) : -1;
    const page = Math.max(1, Math.floor((listRef.current?.clientHeight ?? 400) / ROW_H) - 1);
    const moveTo = (n: number) => {
      const id = ids[Math.max(0, Math.min(ids.length - 1, n))];
      if (id != null) s.select(id, e.shiftKey ? 'range' : 'replace');
    };
    const mod = e.ctrlKey || e.metaKey;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        moveTo(idx < 0 ? 0 : idx + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        moveTo(idx < 0 ? ids.length - 1 : idx - 1);
        break;
      case 'PageDown':
        e.preventDefault();
        moveTo(idx + page);
        break;
      case 'PageUp':
        e.preventDefault();
        moveTo(idx - page);
        break;
      case 'Home':
        e.preventDefault();
        moveTo(0);
        break;
      case 'End':
        e.preventDefault();
        moveTo(ids.length - 1);
        break;
      case 'Escape':
        if (s.selected.length > 1 && s.focus != null) s.select(s.focus);
        break;
      case 'Delete':
      case 'Backspace':
        e.preventDefault();
        void remove(targets);
        break;
      default:
        if (mod && e.key.toLowerCase() === 'a') {
          e.preventDefault();
          s.selectMany(ids);
        } else if (mod && e.shiftKey && e.key.toLowerCase() === 'c') {
          e.preventDefault();
          void curl();
        } else if (!mod && e.key.toLowerCase() === 'r') {
          e.preventDefault();
          void replay();
        }
    }
  };

  const activeView = useActiveViewRef();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!activeView.current) return;
      const t = e.target as HTMLElement | null;
      const typing = !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
      if ((e.key === '/' && !typing) || ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f' && !t?.closest('.monaco-editor'))) {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // ---------- render ----------

  if (!ready)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );

  const running = !!status?.running;
  const statusClasses = filter.statusClasses ?? [];
  const toggleStatus = (c: number) => setFilter({ statusClasses: statusClasses.includes(c) ? statusClasses.filter((x) => x !== c) : [...statusClasses, c] });
  const browserItems: MenuItemDef[] = browsers.length
    ? browsers.map((b) => ({ label: b.name, icon: <Globe />, hint: 'isolated profile', onSelect: () => launchBrowser(b.kind, b.name) }))
    : [{ label: 'No Chromium-based browser found', disabled: true }];

  return (
    <div className="page insp-page">
      <div className="page-header insp-header">
        <Radar size={16} className="subtle" />
        <h2 className="insp-title">Traffic inspector</h2>
        {running ? (
          <>
            <span className="dot ok" aria-hidden />
            <span className="small">
              Listening on <code className="code-inline">{status?.address}</code>
            </span>
            <button className="btn btn-sm" onClick={stop} disabled={busy}>
              <Square /> Stop
            </button>
          </>
        ) : (
          <>
            <span className="dot" aria-hidden />
            <span className="small muted">Stopped</span>
            <input className="input sm insp-port" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ''))} onKeyDown={(e) => e.key === 'Enter' && start()} aria-label="Port" placeholder="8899" />
            <button className="btn btn-sm btn-primary" onClick={start} disabled={busy}>
              <Play /> Start
            </button>
          </>
        )}
        <span className="spacer" />
        <button className="btn btn-sm btn-ghost" onClick={() => setDialog('connect')} title="Trust the CA, launch a browser, configure terminals">
          {trusted === undefined ? <Spinner /> : <TrustBadge installed={trusted} />}
        </button>
        <Menu
          trigger={(p) => (
            <button className="btn btn-sm" {...p} title="Start a browser that routes through the proxy (no system changes)">
              <Globe /> Launch browser
            </button>
          )}
          items={browserItems}
          align="right"
        />
        <button className="btn btn-sm" onClick={() => setDialog('connect')} title="Connect apps & trust the CA">
          <Plug /> Connect…
        </button>
        <button className="btn btn-sm" onClick={() => setDialog('rules')} title="Breakpoints, Map Local/Remote, mocks, rewrites, throttling">
          <Wand2 /> Rules{rules.filter((r) => r.enabled !== false).length ? ` (${rules.filter((r) => r.enabled !== false).length})` : ''}
        </button>
        <button className="icon-btn" onClick={() => setDialog('settings')} title="Proxy settings" aria-label="Proxy settings">
          <Settings2 />
        </button>
      </div>

      <div className="toolbar insp-filters">
        <SearchInput value={filter.text ?? ''} onChange={(v) => setFilter({ text: v })} placeholder="Filter URL, method, status (-term excludes)…  /" inputRef={searchRef} className="insp-search" />
        <select className="select sm" value={filter.methods?.[0] ?? ''} onChange={(e) => setFilter({ methods: e.target.value ? [e.target.value] : [] })} aria-label="Method">
          <option value="">All methods</option>
          {METHODS.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
        <div className="insp-chips-row" role="group" aria-label="Status">
          {[2, 3, 4, 5, 0].map((c) => (
            <button key={c} className={clsx('insp-chip', statusClasses.includes(c) && 'on', `s${c}`)} onClick={() => toggleStatus(c)} title={c ? `${c}xx responses` : 'No response (pending, failed, tunnels)'}>
              {c ? `${c}xx` : 'ERR'}
            </button>
          ))}
        </div>
        <input className="input sm insp-host-filter" value={filter.host ?? ''} onChange={(e) => setFilter({ host: e.target.value })} placeholder="Host" aria-label="Host contains" />
        <select className="select sm" value={filter.contentKinds?.[0] ?? ''} onChange={(e) => setFilter({ contentKinds: e.target.value ? [e.target.value as ContentKind] : [] })} aria-label="Content type">
          <option value="">All types</option>
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {KIND_LABEL[k]}
            </option>
          ))}
        </select>
        <Checkbox checked={!!filter.hideStatic} onChange={(v) => setFilter({ hideStatic: v })} label="Hide static" title="Hide scripts, styles, images, fonts and media" />
        <span className="spacer" />
        <span className="subtle xs nowrap">{isEmptyFilter(filter) ? `${order.length}` : `${ids.length} / ${order.length}`}</span>
        <Menu
          trigger={(p) => (
            <button className="icon-btn sm" {...p} title="Captures" aria-label="Captures" onMouseDown={() => void loadSessions()}>
              <FolderOpen />
            </button>
          )}
          align="right"
          items={() => [
            { label: 'Save capture to project…', icon: <Save />, disabled: !root || !order.length, onSelect: saveSession },
            { label: 'Import HAR file…', icon: <Upload />, onSelect: importHar },
            {
              label: 'Check traffic against OpenAPI spec…',
              icon: <FileCheck2 />,
              disabled: !order.length,
              onSelect: () => needRoot() && useOpenApi.getState().open({ tab: 'drift', drift: { mocks: false, inspector: true, inspectorIds: [] } }),
            },
            { label: 'Export HAR…', hint: 'credentials redacted', icon: <Download />, disabled: !order.length, onSelect: () => void exportHar(false) },
            { label: 'Export HAR with credentials…', icon: <Download />, disabled: !order.length, onSelect: () => void exportHar(true) },
            { separator: true },
            { heading: root ? 'Saved captures (on this computer)' : 'Open a project to load saved captures' },
            ...sessions.map((s) => ({
              label: s.file,
              icon: <FolderInput />,
              hint: formatBytes(s.size),
              onSelect: () =>
                root &&
                proxyApi
                  .loadSession(root, s.file)
                  .then((n) => toast.success(`Loaded ${n.length} exchanges`, s.file))
                  .catch((e) => toast.error('Could not load', errorMessage(e))),
            })),
          ]}
        />
        <button className="icon-btn sm danger" onClick={clear} title="Clear the capture" aria-label="Clear the capture" disabled={!order.length}>
          <Trash2 />
        </button>
      </div>

      {pausedList.length > 0 && (
        <div className="insp-paused-bar" role="status">
          <Pause size={14} />
          <span className="small">
            {pausedList.length} {pausedList.length === 1 ? 'exchange is' : 'exchanges are'} paused at a breakpoint
          </span>
          {pausedList.slice(0, 4).map((p) => (
            <button key={p.id} className="btn btn-sm btn-ghost" onClick={() => useInspector.getState().select(p.id)}>
              {p.phase === 'request' ? '→' : '←'} {p.method} {hostOf(p.url)}
            </button>
          ))}
          <span className="spacer" />
          <button
            className="btn btn-sm"
            onClick={() => {
              for (const p of pausedList) void proxyApi.resume(p.id).catch(() => undefined);
            }}
          >
            <Play /> Resume all
          </button>
        </div>
      )}

      {targets.length > 0 && (
        <div className="toolbar insp-actions" role="toolbar" aria-label="Selection actions">
          <span className="small strong nowrap">{targets.length} selected</span>
          <button className="btn btn-sm btn-ghost" onClick={saveRequests} title="Save as requests in http/ (credentials become secret references)">
            <Save /> Save as requests
          </button>
          <button className="btn btn-sm btn-ghost" onClick={() => openDialogFor('stubs')} title="WireMock stubs in mocks/mappings/">
            <Server /> Create stubs
          </button>
          <button className="btn btn-sm btn-ghost" onClick={() => openDialogFor('openapi')} title="Infer an OpenAPI 3.1 draft">
            <FileCode2 /> OpenAPI
          </button>
          <button
            className="btn btn-sm btn-ghost"
            onClick={() => needRoot() && useOpenApi.getState().open({ tab: 'drift', drift: { mocks: false, inspector: true, inspectorIds: targets } })}
            title="Check these exchanges against the project's OpenAPI spec (undocumented endpoints, statuses, schema violations)"
          >
            <FileCheck2 /> Check spec
          </button>
          <button className="btn btn-sm btn-ghost" onClick={toLoad} title="Load test replaying these requests">
            <Gauge /> Load test
          </button>
          <button className="btn btn-sm btn-ghost" onClick={() => exportHar(false)} title="Export HAR 1.2 (credentials redacted)">
            <Download /> HAR
          </button>
          <span className="spacer" />
          <button className="icon-btn sm" onClick={curl} disabled={targets.length !== 1} title={`Copy as cURL (${MOD}+Shift+C)`} aria-label="Copy as cURL">
            <Terminal />
          </button>
          <button className="icon-btn sm" onClick={replay} title="Replay (R)" aria-label="Replay">
            <RotateCw />
          </button>
          <button className="icon-btn sm" onClick={ruleFrom} disabled={targets.length !== 1} title="New rule matching this request" aria-label="New rule from this request">
            <Wand2 />
          </button>
          <button className="icon-btn sm danger" onClick={() => remove(targets)} title="Remove from the list (Delete)" aria-label="Remove">
            <Ban />
          </button>
        </div>
      )}

      <div className="split insp-split">
        <div className="pane side insp-list-pane" style={{ width: listWidth }}>
          {order.length === 0 ? (
            <Empty
              icon={<Activity />}
              title={running ? 'Waiting for traffic' : 'Start the proxy to capture traffic'}
              action={
                <div className="row wrap" style={{ justifyContent: 'center' }}>
                  {!running && (
                    <button className="btn btn-sm btn-primary" onClick={start}>
                      <Play /> Start on {port || 8899}
                    </button>
                  )}
                  {browsers[0] && (
                    <button className="btn btn-sm" onClick={() => launchBrowser(browsers[0].kind, browsers[0].name)}>
                      <Globe /> Launch {browsers[0].name}
                    </button>
                  )}
                  <button className="btn btn-sm" onClick={() => setDialog('connect')}>
                    <Plug /> Connect an app…
                  </button>
                </div>
              }
            >
              Point any app at <code className="code-inline">127.0.0.1:{status?.port ?? (port || 8899)}</code> as its HTTP(S) proxy — or launch a browser through it. Everything stays on this machine.
            </Empty>
          ) : ids.length === 0 ? (
            <Empty title="Nothing matches the filter" action={<button className="btn btn-sm" onClick={() => setFilter({ text: '', methods: [], statusClasses: [], host: '', contentKinds: [], hideStatic: false })}>Clear filters</button>} />
          ) : (
            <ExchangeList ids={ids} onKeyDown={onListKey} listRef={listRef} />
          )}
        </div>
        <Resizer width={listWidth} min={360} max={1400} onChange={setListWidth} />
        <div className="pane fill insp-detail-pane">
          {focus == null ? (
            <Empty icon={<Radar />} title="Select an exchange">
              <span className="col-sm">
                <span>
                  <Kbd>↑</Kbd> <Kbd>↓</Kbd> move · <Kbd>Shift</Kbd>+click / <Kbd>{MOD}</Kbd>+click select several · <Kbd>{MOD}</Kbd>+<Kbd>A</Kbd> all
                </span>
                <span>
                  <Kbd>R</Kbd> replay · <Kbd>Del</Kbd> remove · <Kbd>/</Kbd> filter
                </span>
              </span>
            </Empty>
          ) : focusPaused ? (
            <BreakpointEditor key={`${focusPaused.id}-${focusPaused.phase}`} paused={focusPaused} />
          ) : (
            <ExchangeDetailView key={focus} id={focus} onReplayed={(nid) => useInspector.getState().select(nid)} />
          )}
        </div>
      </div>

      <ConnectDialog open={dialog === 'connect'} onClose={() => setDialog(null)} onTrustChanged={setTrusted} />
      <RulesPanel
        open={dialog === 'rules'}
        seed={ruleSeed}
        onClose={() => {
          setDialog(null);
          setRuleSeed(null);
        }}
      />
      <SettingsDialog open={dialog === 'settings'} onClose={() => setDialog(null)} />
      {root && <StubsDialog open={dialog === 'stubs'} onClose={() => setDialog(null)} root={root} ids={dialogIds} />}
      {root && <OpenApiDialog open={dialog === 'openapi'} onClose={() => setDialog(null)} root={root} ids={dialogIds} defaultName={(hostOf(items.get(dialogIds[0])?.url ?? 'api').split(':')[0] || 'api').replace(/[^a-z0-9.-]/gi, '-')} />}
    </div>
  );
}

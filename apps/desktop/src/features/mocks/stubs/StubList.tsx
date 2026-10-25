import { useEffect, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@/features/mocks/lib/virtual';
import clsx from 'clsx';
import {
  Plus,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  ListTree,
  List,
  RefreshCw,
  Trash2,
  Ban,
  CheckCircle2,
  FolderInput,
  Tag,
  Copy,
  Shuffle,
  Zap,
  Timer,
  Braces,
  Webhook,
  GitBranch,
  FileText,
  HardDrive,
  Download,
  ArrowRightLeft,
  MoreHorizontal,
  SquareCheckBig,
  LayoutTemplate,
  Code2,
  Send,
} from 'lucide-react';
import type { StubMapping } from '@/features/mocks/lib/types';
import { useMappings, useWm } from '@/features/mocks/lib/queries';
import { usePrefs } from '@/features/mocks/store/prefs';
import { useTargets } from '@/features/mocks/store/target';
import { openInRequests } from './openInRequests';
import {
  buildFolderTree,
  displayName,
  folderOf,
  isDisabled,
  matchesQuery,
  sortStubs,
  stubId,
  traitsOf,
  urlOf,
  tagsOf,
  type FolderNode,
  hasParkedProxy,
} from '@/features/mocks/lib/stub-utils';
import { MethodBadge, StatusCode, SearchInput, Checkbox, Spinner, Empty, copyText } from '@/features/mocks/components/ui';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { copyLinkItems } from '@/components/links/LinkChip';
import { formatLink } from '@/lib/links/syntax';
import { promptDialog } from '@/components/ui/dialogs';
import { useStubActions } from './actions';
import { openNewStub, openStub, BLANK_STUB } from './open';
import { TEMPLATES } from '@/features/mocks/lib/templates';
import { curlForStub } from '@/features/mocks/lib/codegen';
import { downloadJson } from '@/features/mocks/lib/download';

type Row = { kind: 'folder'; node: FolderNode; depth: number } | { kind: 'stub'; stub: StubMapping; depth: number };

const SORTS = [
  { v: 'priority', l: 'Match order' },
  { v: 'url', l: 'URL' },
  { v: 'name', l: 'Name' },
  { v: 'method', l: 'Method' },
] as const;

export function StubList({ activeStubId }: { activeStubId?: string }) {
  const mappings = useMappings();
  const { server, sid, client } = useWm();
  const showFolders = usePrefs((s) => s.showFolders);
  const setPref = usePrefs((s) => s.set);
  const [q, setQ] = useState('');
  const [sort, setSort] = useState<(typeof SORTS)[number]['v']>('priority');
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set(JSON.parse(sessionStorage.getItem(`wmui.collapsed.${sid}`) ?? '[]')));
  // Selection mode: checkboxes and bulk actions only appear after the user asks for them.
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const lastClicked = useRef<string | null>(null);
  const actions = useStubActions();
  const ctx = useContextMenu();
  const targets = useTargets();

  const all = mappings.data ?? [];
  const filtered = useMemo(() => sortStubs(all.filter((s) => matchesQuery(s, q.trim())), sort), [all, q, sort]);

  const rows: Row[] = useMemo(() => {
    if (!showFolders || q) return filtered.map((stub) => ({ kind: 'stub', stub, depth: 0 }));
    const tree = buildFolderTree(filtered);
    const out: Row[] = [];
    const walk = (node: FolderNode, depth: number) => {
      const kids = [...node.children.values()].sort((a, b) => a.name.localeCompare(b.name));
      for (const k of kids) {
        out.push({ kind: 'folder', node: k, depth });
        if (!collapsed.has(k.path)) walk(k, depth + 1);
      }
      for (const s of node.stubs) out.push({ kind: 'stub', stub: s, depth });
    };
    walk(tree, 0);
    return out;
  }, [filtered, showFolders, q, collapsed]);

  const parentRef = useRef<HTMLDivElement>(null);
  const virt = useVirtualizer({ count: rows.length, getScrollElement: () => parentRef.current, estimateSize: (i) => (rows[i].kind === 'folder' ? 28 : 44), overscan: 12 });

  const toggleFolder = (path: string) => {
    const next = new Set(collapsed);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    setCollapsed(next);
    sessionStorage.setItem(`wmui.collapsed.${sid}`, JSON.stringify([...next]));
  };

  const selectedStubs = all.filter((s) => selected.has(stubId(s)));
  const stubRows = rows.filter((r): r is Extract<Row, { kind: 'stub' }> => r.kind === 'stub');
  const exitSelection = () => {
    setSelecting(false);
    setSelected(new Set());
    lastClicked.current = null;
  };
  useEffect(() => {
    if (!selecting) return;
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (e.key === 'Escape' && !t?.closest('input, textarea, select, .monaco-editor, .modal, .menu')) exitSelection();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [selecting]);
  const toggleSelect = (id: string, shift: boolean) => {
    setSelecting(true);
    const next = new Set(selected);
    if (shift && lastClicked.current) {
      const ids = stubRows.map((r) => stubId(r.stub));
      const a = ids.indexOf(lastClicked.current);
      const b = ids.indexOf(id);
      if (a >= 0 && b >= 0) for (const x of ids.slice(Math.min(a, b), Math.max(a, b) + 1)) next.add(x);
    } else if (next.has(id)) next.delete(id);
    else next.add(id);
    lastClicked.current = id;
    setSelected(next);
  };

  const folders = useMemo(() => [...new Set(all.map(folderOf).filter(Boolean))].sort(), [all]);
  const otherServers = targets.filter((t) => t.id !== server?.id);

  const askFolder = async () => {
    const f = await promptDialog({ title: 'Move to folder', label: `Folder path (use / for nesting). Existing: ${folders.slice(0, 6).join(', ') || 'none'}`, placeholder: 'orders/create' });
    return f;
  };

  const stubMenu = (s: StubMapping): MenuItemDef[] => {
    const tr = traitsOf(s);
    const targets = selected.has(stubId(s)) && selected.size > 1 ? selectedStubs : [s];
    const many = targets.length > 1;
    return [
      ...(many ? [{ heading: `${targets.length} selected stubs` }] : []),
      ...(!many ? [{ label: 'Open', icon: <FileText />, onSelect: () => openStub(s) }] : []),
      ...(!many ? [{ label: 'Duplicate', icon: <Copy />, onSelect: () => actions.duplicate(s) }] : []),
      { separator: true },
      isDisabled(s) ? { label: 'Enable', icon: <CheckCircle2 />, onSelect: () => actions.bulk.disable(targets, false) } : { label: 'Disable', icon: <Ban />, onSelect: () => actions.bulk.disable(targets, true) },
      ...(tr.proxy || hasParkedProxy(s) ? [{ label: tr.proxy ? 'Switch proxy → mock response' : 'Switch mock → proxy', icon: <Shuffle />, onSelect: () => actions.bulk.toggleProxy(targets) }] : []),
      {
        label: 'Move to folder…',
        icon: <FolderInput />,
        onSelect: async () => {
          const f = await askFolder();
          if (f !== null) actions.bulk.move(targets, f);
        },
      },
      {
        label: 'Add tag…',
        icon: <Tag />,
        onSelect: async () => {
          const t = await promptDialog({ title: 'Add tag', placeholder: 'checkout' });
          if (t) actions.bulk.tag(targets, t.trim());
        },
      },
      { label: s.persistent ? 'Make non-persistent' : 'Make persistent', icon: <HardDrive />, onSelect: () => actions.bulk.persistent(targets, !s.persistent) },
      { separator: true },
      ...(!many
        ? [
            ...copyLinkItems(formatLink('stub', stubId(s)), displayName(s)),
            { label: 'Copy JSON', icon: <Braces />, onSelect: () => copyText(JSON.stringify(s, null, 2), 'Stub JSON copied') },
            { label: 'Copy as cURL', icon: <Code2 />, onSelect: () => copyText(curlForStub(client?.base ?? server?.url ?? '', s), 'cURL copied') },
            { label: 'Open in Requests', icon: <Send />, disabled: !client, onSelect: () => openInRequests(s, client?.base ?? '') },
          ]
        : []),
      { label: 'Export JSON', icon: <Download />, onSelect: () => downloadJson(many ? 'stubs.json' : `${(s.name || stubId(s)).replace(/[^\w.-]+/g, '_')}.json`, many ? { mappings: targets } : s) },
      ...(otherServers.length
        ? [
            { heading: 'Copy to server' },
            ...otherServers.slice(0, 8).map((t) => ({
              label: t.name,
              icon: <ArrowRightLeft />,
              onSelect: () => actions.copyTo(t, targets),
            })),
          ]
        : []),
      { separator: true },
      { label: many ? `Delete ${targets.length} stubs` : 'Delete', icon: <Trash2 />, danger: true, onSelect: () => actions.remove(targets) },
    ];
  };

  const templateItems = (): MenuItemDef[] => {
    const out: MenuItemDef[] = [{ label: 'Blank stub', icon: <Plus />, onSelect: () => openNewStub(structuredClone(BLANK_STUB)) }];
    let g = '';
    for (const t of TEMPLATES) {
      if (t.group !== g) out.push({ heading: (g = t.group) });
      out.push({
        label: t.label,
        hint: t.stubs.length > 1 ? `${t.stubs.length} stubs` : undefined,
        icon: <LayoutTemplate />,
        onSelect: () => (t.stubs.length === 1 ? openNewStub(structuredClone(t.stubs[0]), t.label) : actions.createMany(structuredClone(t.stubs))),
      });
    }
    return out;
  };

  return (
    <div className="col-sm h-full" style={{ gap: 0 }}>
      <div className="toolbar" style={{ flexWrap: 'nowrap' }}>
        <SearchInput value={q} onChange={setQ} placeholder="Filter  (method:post is:proxy tag:x)" className="grow" inputRef={(el) => el?.setAttribute('data-page-search', '')} />
        <Menu
          align="right"
          items={templateItems}
          trigger={(p) => (
            <button className="btn btn-sm btn-primary" {...p} title="New stub">
              <Plus /> New <ChevronDown />
            </button>
          )}
        />
      </div>
      {selecting ? (
        <div className="toolbar" style={{ minHeight: 32, paddingTop: 2, paddingBottom: 2, background: 'var(--accent-subtle)' }}>
          <Checkbox
            checked={selected.size > 0 && selected.size === stubRows.length}
            indeterminate={selected.size > 0 && selected.size < stubRows.length}
            onChange={(v) => setSelected(v ? new Set(stubRows.map((r) => stubId(r.stub))) : new Set())}
            title="Select all visible"
          />
          <span className="small strong grow nowrap">{selected.size ? `${selected.size} selected` : 'Select stubs'}</span>
          <button className="icon-btn sm" title="Enable" disabled={!selected.size} onClick={() => actions.bulk.disable(selectedStubs, false)}>
            <CheckCircle2 />
          </button>
          <button className="icon-btn sm" title="Disable" disabled={!selected.size} onClick={() => actions.bulk.disable(selectedStubs, true)}>
            <Ban />
          </button>
          <button
            className="icon-btn sm"
            title="Move to folder"
            disabled={!selected.size}
            onClick={async () => {
              const f = await askFolder();
              if (f !== null) actions.bulk.move(selectedStubs, f);
            }}
          >
            <FolderInput />
          </button>
          <button className="icon-btn sm" title="Export selected" disabled={!selected.size} onClick={() => downloadJson('stubs.json', { mappings: selectedStubs })}>
            <Download />
          </button>
          <Menu
            items={() => (selectedStubs.length ? stubMenu(selectedStubs[0]) : [])}
            align="right"
            trigger={(p) => (
              <button className="icon-btn sm" {...p} disabled={!selected.size} title="More actions">
                <MoreHorizontal />
              </button>
            )}
          />
          <button className="icon-btn sm danger" title="Delete selected" disabled={!selected.size} onClick={() => actions.remove(selectedStubs).then(exitSelection)}>
            <Trash2 />
          </button>
          <button className="btn btn-sm" onClick={exitSelection} title="Leave selection mode (Esc)">
            Done
          </button>
        </div>
      ) : (
        <div className="toolbar" style={{ minHeight: 32, paddingTop: 2, paddingBottom: 2 }}>
          <span className="subtle xs grow">{filtered.length === all.length ? `${all.length} stubs` : `${filtered.length} of ${all.length}`}</span>
          <select className="select sm" style={{ width: 'auto' }} value={sort} onChange={(e) => setSort(e.target.value as typeof sort)} title="Sort">
            {SORTS.map((s) => (
              <option key={s.v} value={s.v}>
                {s.l}
              </option>
            ))}
          </select>
          <button className={clsx('icon-btn sm', showFolders && 'active')} onClick={() => setPref('showFolders', !showFolders)} title={showFolders ? 'Folders on' : 'Flat list'}>
            {showFolders ? <ListTree /> : <List />}
          </button>
          <button className="icon-btn sm" onClick={() => mappings.refetch()} title="Refresh">
            {mappings.isFetching ? <Spinner /> : <RefreshCw />}
          </button>
          <button className="btn btn-sm btn-ghost" onClick={() => setSelecting(true)} disabled={!all.length} title="Select stubs for bulk actions (or Ctrl/Shift-click a row)">
            <SquareCheckBig /> Select
          </button>
        </div>
      )}
      <div ref={parentRef} className="list" style={{ position: 'relative' }}>
        {mappings.isPending && (
          <div className="empty">
            <Spinner />
          </div>
        )}
        {mappings.isSuccess && all.length === 0 && (
          <Empty title="No stubs yet" action={<button className="btn btn-primary btn-sm" onClick={() => openNewStub(structuredClone(BLANK_STUB))}><Plus /> Create a stub</button>}>
            Create one from scratch, pick a template from <b>New</b>, record real traffic, or import a file.
          </Empty>
        )}
        {mappings.isSuccess && all.length > 0 && rows.length === 0 && <div className="p-5 subtle small">No stubs match “{q}”.</div>}
        <div style={{ height: virt.getTotalSize(), position: 'relative' }}>
          {virt.getVirtualItems().map((vi) => {
            const row = rows[vi.index];
            const style: React.CSSProperties = { position: 'absolute', top: 0, left: 0, right: 0, height: vi.size, overflow: 'hidden', transform: `translateY(${vi.start}px)` };
            if (row.kind === 'folder') {
              const open = !collapsed.has(row.node.path);
              return (
                <div key={`f:${row.node.path}`} ref={virt.measureElement} data-index={vi.index} style={style}>
                  <div className="list-group-header" style={{ paddingLeft: 8 + row.depth * 14, position: 'static', height: '100%', boxSizing: 'border-box' }} onClick={() => toggleFolder(row.node.path)}>
                    {open ? <ChevronDown /> : <ChevronRight />}
                    {open ? <FolderOpen /> : <Folder />}
                    <span className="truncate grow">{row.node.name}</span>
                    <span className="subtle">{row.node.count}</span>
                  </div>
                </div>
              );
            }
            const s = row.stub;
            const id = stubId(s);
            const tr = traitsOf(s);
            return (
              <div key={id} ref={virt.measureElement} data-index={vi.index} style={style}>
                <div
                  className={clsx('list-item', id === activeStubId && 'on', selected.has(id) && 'checked', tr.disabled && 'disabled-stub')}
                  style={{ paddingLeft: 8 + row.depth * 14 + (showFolders && !q && row.depth > 0 ? 6 : 0), alignItems: 'flex-start', paddingTop: 5, paddingBottom: 5, height: '100%', boxSizing: 'border-box' }}
                  onClick={(e) => (selecting || e.metaKey || e.ctrlKey || e.shiftKey ? toggleSelect(id, e.shiftKey) : openStub(s, { preview: true }))}
                  onDoubleClick={() => !selecting && openStub(s)}
                  onContextMenu={(e) => ctx.open(e, stubMenu(s))}
                  draggable
                  onDragStart={(e) => e.dataTransfer.setData('text/plain', JSON.stringify(s, null, 2))}
                >
                  {selecting && <Checkbox checked={selected.has(id)} onChange={() => toggleSelect(id, false)} />}
                  <MethodBadge method={s.request.method} />
                  <div className="grow" style={{ minWidth: 0 }}>
                    <div className="row-sm">
                      <span className="truncate url" style={{ fontWeight: s.name ? 500 : 400 }} title={displayName(s)}>
                        {s.name || <span className="mono xs">{urlOf(s.request) || '(any URL)'}</span>}
                      </span>
                    </div>
                    <div className="row-sm meta" style={{ marginTop: 1 }}>
                      {tr.proxy ? <span className="badge info">PROXY</span> : tr.fault ? <span className="badge danger">FAULT</span> : <StatusCode status={s.response.status ?? 200} />}
                      {s.name && <span className="mono truncate">{urlOf(s.request)}</span>}
                      {!s.name && s.priority !== undefined && <span>p{s.priority}</span>}
                      <span className="spacer" />
                      {tr.delay && <Timer size={11} aria-label="delay" />}
                      {tr.templated && <Zap size={11} aria-label="templated" />}
                      {tr.webhook && <Webhook size={11} aria-label="webhook" />}
                      {tr.scenario && <GitBranch size={11} aria-label="scenario" />}
                      {tr.file && <FileText size={11} aria-label="body file" />}
                      {tr.persistent && <HardDrive size={11} aria-label="persistent" />}
                      {tagsOf(s)
                        .slice(0, 2)
                        .map((t) => (
                          <span key={t} className="tag" style={{ height: 16, fontSize: 10 }}>
                            {t}
                          </span>
                        ))}
                    </div>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>
      {ctx.node}
    </div>
  );
}

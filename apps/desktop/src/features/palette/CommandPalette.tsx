import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { create } from 'zustand';
import clsx from 'clsx';
import { ArrowRight, ClipboardPaste, Columns2, FileCheck2, FileDown, FileUp, GitBranch, Layers, Link2, PanelLeft, Plus, Rows2, Search, Server, Split, Sun, Binary, Sparkles, X } from 'lucide-react';
import { useImportExport } from '@/features/import';
import { useOpenApi } from '@/features/openapi';
import { openReposDialog, openSwitchBranch } from '@/features/git/dialogs';
import { useGitActions } from '@/features/git/gitActions';
import { MOCK_PALETTE_ACTIONS } from '@/features/mocks';
import { HitIcon } from '@/features/ai/SearchTab';
import { useProjectLinkOpener } from '@/features/notes/NotesPage';
import { aiApi, type SearchResult } from '@/lib/api/ai';
import { pluginOps, usePluginCatalog } from '@/lib/api/plugins';
import { copyText, MOD, MethodBadge } from '@/components/ui/misc';
import { pickLink } from '@/components/links/LinkPicker';
import { describe } from '@/lib/links/registry';
import { linkForRoute } from '@/lib/links/route';
import { formatLink, markdownLink } from '@/lib/links/syntax';
import { useTodos } from '@/store/todos';
import { promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { OpInfo, TreeNode } from '@/lib/types';
import { SECTIONS } from '@/lib/sections';
import { useNav } from '@/store/nav';
import { activeViewOf, useWorkbench } from '@/store/workbench';
import { viewsToClose } from '@/lib/workbench/model';
import { usePrefs } from '@/store/prefs';
import { useWorkspace } from '@/store/workspace';

export const usePalette = create<{ open: boolean; setOpen(v: boolean): void }>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
}));

interface Cmd {
  id: string;
  group: string;
  label: ReactNode;
  text: string;
  icon?: ReactNode;
  hint?: string;
  run(): void;
}

function flatten(n: TreeNode | null, out: Extract<TreeNode, { kind: 'request' }>[] = []) {
  if (!n) return out;
  if (n.kind === 'request') out.push(n);
  else n.children.forEach((c) => flatten(c, out));
  return out;
}

/** Copies the active view's `devall:` link (the open to-do on the to-dos page), plain or as Markdown. */
function copyCurrentLink(markdown: boolean) {
  const v = activeViewOf(useWorkbench.getState());
  if (!v) return;
  const openTodo = v.section === 'todos' ? useTodos.getState().openId : null;
  const link = openTodo ? formatLink('todo', openTodo) : linkForRoute(v.section, v.param);
  void copyText(markdown ? markdownLink(describe(link, v.label).title, link) : link, markdown ? 'Markdown link copied' : 'Link copied');
}

export function CommandPalette() {
  const { open, setOpen } = usePalette();
  if (!open) return null;
  return createPortal(<PaletteInner close={() => setOpen(false)} />, document.body);
}

let opsCache: OpInfo[] | null = null;

function PaletteInner({ close }: { close(): void }) {
  const [q, setQ] = useState('');
  // "Search project…" mode: meaning-based search over notes, requests, mocks, environments.
  const [semantic, setSemantic] = useState(false);
  const [found, setFound] = useState<SearchResult | null>(null);
  const openLink = useProjectLinkOpener();
  const [hl, setHl] = useState(0);
  const [builtinOps, setOps] = useState<OpInfo[]>(opsCache ?? []);
  const pluginCatalog = usePluginCatalog((s) => s.catalog);
  const ops = useMemo(() => [...builtinOps, ...pluginOps(pluginCatalog)], [builtinOps, pluginCatalog]);
  const go = useNav((s) => s.go);
  const tree = useWorkspace((s) => s.tree);
  const project = useWorkspace((s) => s.project);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!opsCache) api.utilList().then((o) => setOps((opsCache = o))).catch(() => {});
  }, []);

  const cmds = useMemo<Cmd[]>(() => {
    const ws = useWorkspace.getState();
    const out: Cmd[] = [];
    if (project) {
      out.push({ id: 'new', group: 'Actions', label: 'New request', text: 'new request create', icon: <Plus />, run: () => ws.newRequest('') });
      out.push({
        id: 'search-project',
        group: 'Actions',
        label: 'Search project…',
        text: 'search project semantic meaning notes requests mocks environments find',
        hint: '?',
        icon: <Sparkles />,
        run: () => {},
      });
      out.push({ id: 'import', group: 'Actions', label: 'Import collection or spec…', text: 'import postman insomnia bruno openapi swagger har http hurl', icon: <FileUp />, run: () => useImportExport.getState().openImport('') });
      out.push({ id: 'export', group: 'Actions', label: 'Export project…', text: 'export openapi postman http hurl', icon: <FileDown />, run: () => useImportExport.getState().openExport('') });
      out.push({ id: 'openapi-lint', group: 'Actions', label: 'OpenAPI: lint a spec…', text: 'openapi swagger lint validate spec rules', icon: <FileCheck2 />, run: () => useOpenApi.getState().open({ tab: 'lint' }) });
      out.push({ id: 'openapi-drift', group: 'Actions', label: 'OpenAPI: check drift (mocks / traffic vs spec)…', text: 'openapi drift contract mocks traffic schema validate', icon: <FileCheck2 />, run: () => useOpenApi.getState().open({ tab: 'drift' }) });
      if (!project.isScratch) {
        out.push({ id: 'repos', group: 'Actions', label: 'Repositories…', text: 'repositories repos multi-repo git add locate clone group frontend backend', icon: <GitBranch />, run: openReposDialog });
        out.push({ id: 'repos-switch', group: 'Actions', label: 'Git: switch branch across repositories…', text: 'git branch switch checkout create feature repositories repos all', icon: <Split />, run: () => openSwitchBranch() });
      }
      out.push({
        id: 'curl',
        group: 'Actions',
        label: 'Import cURL…',
        text: 'import curl paste',
        icon: <ClipboardPaste />,
        run: async () => {
          const cmd = await promptDialog({ title: 'Import cURL', label: 'Paste a cURL command' });
          if (!cmd) return;
          try {
            await ws.newRequest('', await api.importCurl(cmd));
          } catch (e) {
            toast.error('Could not parse cURL', errorMessage(e));
          }
        },
      });
      for (const e of project.environments)
        out.push({ id: `env-${e.id}`, group: 'Switch environment', label: e.file.name, text: `env environment switch ${e.file.name} ${e.id}`, icon: <Layers />, run: () => ws.setActiveEnv(e.id) });
    }
    out.push(
      { id: 'copy-link', group: 'Actions', label: 'Copy link to this page', text: 'copy link page item devall share reference', icon: <Link2 />, run: () => copyCurrentLink(false) },
      { id: 'copy-link-md', group: 'Actions', label: 'Copy Markdown link to this page', text: 'copy markdown link page item devall share reference', icon: <Link2 />, run: () => copyCurrentLink(true) },
      {
        id: 'link-to',
        group: 'Actions',
        label: 'Copy a Markdown link to…',
        text: 'copy link to find item request note todo environment devall markdown',
        icon: <Link2 />,
        run: async () => {
          const p = await pickLink({ placeholder: 'Copy a link to… requests, notes, to-dos, environments, pages' });
          if (p) void copyText(markdownLink(p.title, p.link), 'Markdown link copied');
        },
      },
    );
    out.push({
      id: 'theme',
      group: 'Actions',
      label: 'Toggle dark / light theme',
      text: 'theme dark light mode',
      icon: <Sun />,
      run: () => usePrefs.getState().set('theme', document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'),
    });
    const wb = useWorkbench.getState;
    out.push(
      { id: 'wb-split-right', group: 'View', label: 'Split right', text: 'view split editor right side by side group', icon: <Columns2 />, hint: `${MOD}+\\`, run: () => wb().split(activeViewOf(wb())?.id ?? null, 'right') },
      { id: 'wb-split-down', group: 'View', label: 'Split down', text: 'view split editor down below stacked group', icon: <Rows2 />, run: () => wb().split(activeViewOf(wb())?.id ?? null, 'down') },
      {
        id: 'wb-close-others',
        group: 'View',
        label: 'Close other tabs',
        text: 'view close other tabs',
        icon: <X />,
        run: () => {
          const v = activeViewOf(wb());
          if (v) void wb().closeMany(viewsToClose(wb(), v.id, 'others'));
        },
      },
      { id: 'wb-sidebar', group: 'View', label: 'Customize sidebar…', text: 'view sidebar customize hide reorder modules', icon: <PanelLeft />, run: () => go('settings', 'sidebar') },
    );
    for (const a of useGitActions.getState().actions) out.push({ id: `git-${a.id}`, group: 'Git', label: a.label, text: `git ${a.label} ${a.keywords}`, icon: <GitBranch />, hint: a.hint, run: a.run });
    for (const a of MOCK_PALETTE_ACTIONS) out.push({ id: `mock-${a.id}`, group: 'Mock server', label: a.label, text: `${a.label} ${a.keywords}`, icon: <Server />, run: a.run });
    for (const m of SECTIONS) out.push({ id: `go-${m.id}`, group: 'Go to', label: m.title ?? m.label, text: `go ${m.title ?? m.label} ${m.label} ${m.id}`, icon: <ArrowRight />, run: () => go(m.id) });
    for (const o of ops) out.push({ id: `op-${o.id}`, group: 'Utilities', label: o.name, text: `util ${o.name} ${o.id} ${o.category}`, icon: <Binary />, hint: o.category, run: () => go('utilities', o.id) });
    return out;
  }, [project, go, ops]);

  // Typing `?` first switches to project search.
  const query = semantic ? q : q.startsWith('?') ? q.slice(1) : q;
  const inSearch = semantic || q.startsWith('?');
  useEffect(() => {
    if (!inSearch || !project || !query.trim()) {
      setFound(null);
      return;
    }
    const t = setTimeout(() => {
      aiApi
        .search(project.root, query, 15)
        .then(setFound)
        .catch((e) => setFound({ mode: 'keyword', hits: [], note: errorMessage(e) }));
    }, 200);
    return () => clearTimeout(t);
  }, [inSearch, query, project]);

  const results = useMemo(() => {
    if (inSearch) {
      return (found?.hits ?? []).map<Cmd>((h, i) => ({
        id: `hit-${i}-${h.source}`,
        group: found?.mode === 'hybrid' ? 'Project · by meaning' : 'Project · by keyword',
        label: (
          <span className="row-sm truncate">
            <span className="truncate">{h.title}</span>
            {h.anchor && h.kind === 'note' && h.anchor !== h.title && <span className="subtle xs truncate">› {h.anchor}</span>}
            <span className="subtle xs truncate">{h.snippet}</span>
          </span>
        ),
        text: h.title,
        icon: <HitIcon kind={h.kind} />,
        hint: h.kind,
        run: () => openLink(h.source),
      }));
    }
    const needle = q.trim().toLowerCase();
    const reqs = flatten(tree)
      .filter((r) => !needle || `${r.name} ${r.method} ${r.path}`.toLowerCase().includes(needle))
      .slice(0, needle ? 30 : 6)
      .map<Cmd>((r) => ({
        id: `req-${r.path}`,
        group: 'Requests',
        label: (
          <span className="row-sm truncate">
            <MethodBadge method={r.method} short />
            <span className="truncate">{r.name}</span>
            <span className="subtle xs truncate mono">{r.path}</span>
          </span>
        ),
        text: r.name,
        run: () => void useWorkspace.getState().openRequest(r.path),
      }));
    const filtered = cmds.filter((c) => !needle || needle.split(/\s+/).every((t) => c.text.toLowerCase().includes(t)));
    const actions = filtered.filter((c) => c.group === 'Actions');
    const rest = filtered.filter((c) => c.group !== 'Actions');
    return needle ? [...reqs, ...actions, ...rest] : [...actions, ...reqs, ...rest.filter((c) => c.group !== 'Utilities')];
  }, [q, cmds, tree, inSearch, found, openLink]);

  useEffect(() => setHl(0), [q]);
  useEffect(() => {
    listRef.current?.querySelector('.item.hl')?.scrollIntoView({ block: 'nearest' });
  }, [hl]);

  const run = (c?: Cmd) => {
    if (!c) return;
    if (c.id === 'search-project') {
      setSemantic(true);
      setQ('');
      return;
    }
    close();
    c.run();
  };

  let lastGroup = '';
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal palette" role="dialog" aria-label="Command palette">
        <input
          autoFocus
          value={q}
          placeholder={inSearch ? 'Search notes, requests, mocks and environments by meaning…' : 'Search requests, utilities, actions…  (? searches the project)'}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              if (semantic) {
                setSemantic(false);
                setQ('');
              } else close();
            } else if (e.key === 'Backspace' && semantic && !q) setSemantic(false);
            else if (e.key === 'ArrowDown') {
              e.preventDefault();
              setHl((h) => Math.min(results.length - 1, h + 1));
            } else if (e.key === 'ArrowUp') {
              e.preventDefault();
              setHl((h) => Math.max(0, h - 1));
            } else if (e.key === 'Enter') run(results[hl]);
          }}
        />
        <div className="results" ref={listRef}>
          {inSearch && found?.note && <div className="group">{found.note}</div>}
          {results.length === 0 && (
            <div className="empty">
              <Search />
              <p>{inSearch && !query.trim() ? 'Describe what you are looking for' : 'No matches'}</p>
            </div>
          )}
          {results.map((c, i) => {
            const header = c.group !== lastGroup ? <div className="group">{(lastGroup = c.group)}</div> : null;
            return (
              <div key={c.id}>
                {header}
                <div className={clsx('item', i === hl && 'hl')} onMouseEnter={() => setHl(i)} onClick={() => run(c)}>
                  {c.icon}
                  <span className="grow truncate">{c.label}</span>
                  {c.hint && <span className="subtle xs">{c.hint}</span>}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

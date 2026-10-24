import { useCallback, useEffect, useMemo, useState, type ComponentType } from 'react';
import clsx from 'clsx';
import { ClipboardCheck, Globe, History, Wrench, X } from 'lucide-react';
import { Empty, SearchInput } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { detect } from '@/lib/detect';
import { utilsApi, type OpInfo, type UtilArgs } from '@/lib/api/utils';
import { pluginOps, usePluginCatalog } from '@/lib/api/plugins';
import { useNav, useViewLabel, useViewState } from '@/store/nav';
import { usePrefs } from '@/store/prefs';
import { useClipboardDetection } from './clipboard';
import { useUtilityLink } from './open';
import { CompareTool } from './CompareTool';
import { DiffTool } from './DiffTool';
import { GenericTool } from './GenericTool';
import { RecipeTool } from './RecipeTool';
import { RegexTool } from './RegexTool';
import { UtilitiesContext, type Seed, type ToolProps, type UtilitiesCtx } from './shared';
import { UtilityHistoryList } from './UtilityHistory';
import { restorableArgs } from '@/lib/api/history';
import './utilities.css';
import { SidePane } from '@/components/layout/SidePane';

const DEFAULT_OP = 'jwt.decode';

/** Specialized views by op id. Ops sharing a view share its state (`group`), so switching between
 * e.g. regex.test and regex.explain keeps the pattern. Everything else is registry-driven. */
const VIEWS: Record<string, { group: string; View: ComponentType<ToolProps> }> = {
  'regex.test': { group: 'regex', View: RegexTool },
  'regex.replace': { group: 'regex', View: RegexTool },
  'regex.split': { group: 'regex', View: RegexTool },
  'regex.explain': { group: 'regex', View: RegexTool },
  'regex.cases': { group: 'regex', View: RegexTool },
  'diff.text': { group: 'diff', View: DiffTool },
  'diff.json': { group: 'diff', View: DiffTool },
  'text.compare': { group: 'compare', View: CompareTool },
  'recipe.run': { group: 'recipe', View: RecipeTool },
};

/** Sidebar order; categories the backend adds later are appended. */
const CATEGORY_ORDER = ['Encoding', 'Data', 'Query', 'Regex', 'Diff', 'Codegen', 'JWT', 'Hashing', 'Generators', 'Time', 'Text', 'Network', 'Web & system', 'Recipes', 'Plugins'];

let opsCache: OpInfo[] | null = null;

export default function UtilitiesPage() {
  const param = useNav((s) => s.param);
  const go = useNav((s) => s.go);
  const [builtinOps, setOps] = useState<OpInfo[]>(opsCache ?? []);
  // Utility ops from installed / trusted project plugins (`<plugin>:<op>`).
  const catalog = usePluginCatalog((s) => s.catalog);
  const ops = useMemo(() => [...builtinOps, ...pluginOps(catalog)], [builtinOps, catalog]);
  const [q, setQ] = useState('');
  // The side pane shows past runs instead of the utility list (History button).
  const [showHistory, setShowHistory] = useViewState('history', false);
  const [hq, setHq] = useState('');
  // The selected utility is the view's route: the tab names it and keeps it across tab switches and restarts.
  const opId = param?.split('|')[0] || DEFAULT_OP;
  const [input, setInput] = useViewState('input', '');
  const [seed, setSeed] = useState<Seed | undefined>(undefined);

  useEffect(() => {
    if (opsCache) return;
    utilsApi
      .list()
      .then((o) => setOps((opsCache = o)))
      .catch((e) => toast.error('Could not load utilities', errorMessage(e)));
  }, []);

  const open = useCallback(
    (id: string, opts?: { input?: string; args?: UtilArgs }) => {
      if (opts?.input !== undefined) setInput(opts.input);
      setSeed(opts?.args ? { args: opts.args, nonce: Date.now() + Math.random() } : undefined);
      go('utilities', id);
    },
    [go, setInput],
  );

  // Deep link from other views: "op|value". The value becomes the input and only the op stays in
  // the route (the layout is saved, and the value may be a token).
  useEffect(() => {
    const bar = param?.indexOf('|') ?? -1;
    if (param && bar >= 0) open(param.slice(0, bar), { input: param.slice(bar + 1) });
  }, [param, open]);

  // "Open in Utilities" with arguments (e.g. a response body + its jq filter).
  const link = useUtilityLink((s) => s.link);
  useEffect(() => {
    if (!link) return;
    open(link.op, { input: link.input, args: link.args });
    useUtilityLink.setState({ link: undefined });
  }, [link, open]);

  const clipboardDetect = usePrefs((s) => s.clipboardDetect);
  const clip = useClipboardDetection(clipboardDetect, input);

  const pasteAndDetect = useCallback(async () => {
    try {
      const text = await navigator.clipboard.readText();
      const d = detect(text);
      if (d) {
        open(d.op, { input: d.value });
        toast.info(`Detected ${d.label}`);
      } else {
        setInput(text);
        toast.info('No known format detected', 'Try Magic detect (Recipes) or pick an operation.');
      }
    } catch {
      toast.error('Clipboard unavailable');
    }
  }, [open]);

  const ctx = useMemo<UtilitiesCtx>(() => ({ ops, open, pasteAndDetect }), [ops, open, pasteAndDetect]);

  const groups = useMemo(() => {
    const words = q.toLowerCase().split(/\s+/).filter(Boolean);
    const m = new Map<string, OpInfo[]>();
    for (const o of ops) {
      const hay = `${o.name} ${o.id} ${o.category} ${o.description}`.toLowerCase();
      if (words.every((w) => hay.includes(w))) m.set(o.category, [...(m.get(o.category) ?? []), o]);
    }
    const rank = (c: string) => (CATEGORY_ORDER.includes(c) ? CATEGORY_ORDER.indexOf(c) : CATEGORY_ORDER.length);
    return [...m.entries()].sort((a, b) => rank(a[0]) - rank(b[0]));
  }, [ops, q]);

  const op = ops.find((o) => o.id === opId);
  const view = op ? (VIEWS[op.id] ?? { group: op.id, View: GenericTool }) : null;
  useViewLabel(ops.length ? (op?.name ?? null) : undefined);

  return (
    <UtilitiesContext.Provider value={ctx}>
      <div className="split">
        <SidePane
          title={showHistory ? 'History' : 'Utilities'}
          label="Utilities"
          actions={
            <button
              className={clsx('icon-btn', showHistory && 'active')}
              onClick={() => setShowHistory(!showHistory)}
              title={showHistory ? 'Back to the utility list' : 'History: past runs (kept on this computer)'}
              aria-label="Utility history"
              aria-pressed={showHistory}
            >
              <History />
            </button>
          }
          toolbar={
            showHistory ? (
              <SearchInput value={hq} onChange={setHq} placeholder="Search past runs…" className="grow" />
            ) : (
              <SearchInput value={q} onChange={setQ} placeholder={`Find a utility (${ops.length})`} className="grow" />
            )
          }
        >
          {showHistory ? (
            <UtilityHistoryList opId={opId} opName={op?.name} query={hq} onPick={(r) => open(r.op, { input: r.inputMasked ? undefined : r.input, args: restorableArgs(r.args) })} />
          ) : (
            <div className="list">
              {groups.map(([cat, list]) => (
                <div key={cat}>
                  <div className="list-group-header">{cat}</div>
                  {list.map((o) => (
                    <div
                      key={o.id}
                      className={clsx('list-item', o.id === opId && 'on')}
                      onClick={() => open(o.id)}
                      title={o.description}
                      role="button"
                      tabIndex={0}
                      onKeyDown={(e) => e.key === 'Enter' && open(o.id)}
                    >
                      <span className="truncate grow">{o.name}</span>
                      {o.network && <Globe className="util-net-icon" aria-label="network" />}
                    </div>
                  ))}
                </div>
              ))}
              {ops.length > 0 && !groups.length && <div className="subtle xs p-4">No utility matches “{q}”.</div>}
            </div>
          )}
        </SidePane>
        <div className="pane fill">
          {clip.found && (
            <div className="util-clip" role="status">
              <ClipboardCheck size={14} />
              <span className="small">
                Detected on the clipboard: <span className="strong">{clip.found.label}</span>
              </span>
              <span className="mono xs subtle truncate grow">{clip.found.value.slice(0, 80)}</span>
              <button
                className="btn btn-sm btn-primary"
                onClick={() => {
                  const d = clip.found!;
                  clip.dismiss();
                  open(d.op, { input: d.value });
                }}
              >
                Decode
              </button>
              <button className="icon-btn sm" onClick={clip.dismiss} aria-label="Dismiss" title="Dismiss (turn off in Settings)">
                <X />
              </button>
            </div>
          )}
          {!op || !view ? (
            <Empty icon={<Wrench />} title="Pick a utility" />
          ) : (
            <view.View key={view.group} op={op} input={input} setInput={setInput} seed={seed} />
          )}
        </div>
      </div>
    </UtilitiesContext.Provider>
  );
}

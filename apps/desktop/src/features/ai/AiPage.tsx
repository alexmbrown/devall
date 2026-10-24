import { useEffect } from 'react';
import clsx from 'clsx';
import { Bot, Cpu, MessageSquare, Search } from 'lucide-react';
import { MANAGED_PROVIDER_ID } from '@/lib/api/ai';
import { useNav, useViewLabel } from '@/store/nav';
import { AgentsTab } from './AgentsTab';
import { AskTab } from './AskTab';
import { ModelsTab } from './ModelsTab';
import { SearchTab } from './SearchTab';
import { isLocalProvider, providerLabel, providerList, useAi } from './store';
import './ai.css';

type Tab = 'ask' | 'search' | 'models' | 'agents';
/** `tabLabel` names the workbench tab ("AI: Models"; Ask is the landing page, shown as "Local AI"). */
const TABS: { id: Tab; label: string; tabLabel: string | null; icon: React.ReactNode }[] = [
  { id: 'ask', label: 'Ask', tabLabel: null, icon: <MessageSquare size={13} /> },
  { id: 'search', label: 'Search', tabLabel: 'Search', icon: <Search size={13} /> },
  { id: 'models', label: 'Models', tabLabel: 'Models', icon: <Cpu size={13} /> },
  { id: 'agents', label: 'Agents (MCP)', tabLabel: 'Agents (MCP)', icon: <Bot size={13} /> },
];

/** Local AI: project Q&A, model/provider management and the MCP server for coding agents. */
export default function AiPage() {
  const param = useNav((s) => s.param);
  const go = useNav((s) => s.go);
  // The sub-tab is the view's route, so it survives tab switches and names the tab.
  const tab = TABS.find((t) => t.id === param) ?? TABS[0];
  const setTab = (t: Tab) => go('ai', t);
  const { view, load, selected, select } = useAi();
  useViewLabel(tab.tabLabel);

  useEffect(() => {
    load();
  }, [load]);

  const all = providerList(view);
  const defaultId = view?.config.defaultProvider;

  return (
    <div className="page">
      <div className="page-header">
        <h1>Local AI</h1>
        <span className="subtle xs">Bring your own model · runs on your machine · no account</span>
        <span className="spacer" />
        {all.length > 0 && (
          <select className="select" value={selected ?? ''} onChange={(e) => select(e.target.value || undefined)} title="Model used on this page and by AI buttons" style={{ maxWidth: 320 }}>
            <option value="">Default{defaultId ? ` (${all.find((p) => p.id === defaultId)?.name ?? defaultId})` : ''}</option>
            {all.map((p) => (
              <option key={p.id} value={p.id}>
                {providerLabel(p)} {p.id === MANAGED_PROVIDER_ID || isLocalProvider(p) ? '· local' : '· cloud'}
              </option>
            ))}
          </select>
        )}
      </div>
      <div className="subtabs" role="tablist">
        {TABS.map((t) => (
          <button key={t.id} role="tab" aria-selected={tab.id === t.id} className={clsx('subtab', tab.id === t.id && 'on')} onClick={() => setTab(t.id)}>
            <span className="row-sm">
              {t.icon}
              {t.label}
            </span>
          </button>
        ))}
      </div>
      {tab.id === 'ask' ? (
        <div className="ai-page-body" style={{ flex: 1 }}>
          <AskTab />
        </div>
      ) : (
        <div className="page-body">{tab.id === 'models' ? <ModelsTab /> : tab.id === 'search' ? <SearchTab /> : <AgentsTab />}</div>
      )}
    </div>
  );
}

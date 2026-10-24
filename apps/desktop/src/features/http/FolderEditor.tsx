import { FileCode, Heading, KeyRound, Save, Variable } from 'lucide-react';
import { ResponsiveTabs } from '@/components/ui/ResponsiveTabs';
import { useViewState } from '@/store/nav';
import { KeyValueEditor, COMMON_HEADERS } from './KeyValueEditor';
import { AuthEditor } from './AuthEditor';
import { useVariables } from '@/store/variables';
import { isDirty, useWorkspace, type FolderTab } from '@/store/workspace';
import type { KeyValue } from '@/lib/types';
import { ScriptsEditor } from '@/features/scripts';

type Sub = 'headers' | 'auth' | 'variables' | 'scripts';

const toRows = (v?: Record<string, string>): KeyValue[] => Object.entries(v ?? {}).map(([name, value]) => ({ name, value }));
const fromRows = (rows: KeyValue[]) => Object.fromEntries(rows.filter((r) => r.name && r.enabled !== false).map((r) => [r.name, r.value]));

/** Settings inherited by every request below a folder (`_folder.yaml`). */
export function FolderEditor({ tab }: { tab: FolderTab }) {
  const update = useWorkspace((s) => s.updateFolder);
  const save = useWorkspace((s) => s.save);
  const vars = useVariables(`${tab.path}/_`);
  const [sub, setSub] = useViewState<Sub>('folder.sub', 'headers');
  const d = tab.draft;
  return (
    <div className="request-editor">
      <div className="request-head">
        <h2 className="truncate">{d.name ?? (tab.path || 'Project root')}</h2>
        <span className="subtle xs mono">http/{tab.path ? `${tab.path}/` : ''}_folder.yaml</span>
        <span className="spacer" />
        <button className="btn btn-sm btn-primary" disabled={!isDirty(tab)} onClick={() => save(tab.id)}>
          <Save /> Save
        </button>
      </div>
      <p className="muted small px-5" style={{ marginTop: 8 }}>
        Everything here applies to every request inside this folder (and sub-folders). Requests override headers by name and auth by choosing anything but “Inherit”.
      </p>
      <ResponsiveTabs
        tabs={[
          { key: 'headers', label: 'Headers', icon: <Heading />, badge: (d.headers ?? []).filter((h) => h.enabled !== false).length || undefined },
          { key: 'auth', label: 'Auth', icon: <KeyRound />, dot: !!d.auth && d.auth.type !== 'inherit' && d.auth.type !== 'none' },
          { key: 'variables', label: 'Variables', icon: <Variable />, badge: Object.keys(d.variables ?? {}).length || undefined },
          { key: 'scripts', label: 'Scripts', icon: <FileCode /> },
        ]}
        value={sub}
        onChange={setSub}
        label="Folder settings"
      />
      <div className="request-sub" style={{ maxWidth: 900 }}>
        {sub === 'headers' && <KeyValueEditor rows={d.headers ?? []} onChange={(headers) => update(tab.id, (x) => ({ ...x, headers }))} vars={vars} namePlaceholder="Header" nameSuggestions={COMMON_HEADERS} />}
        {sub === 'auth' && <AuthEditor auth={d.auth ?? { type: 'inherit' }} onChange={(auth) => update(tab.id, (x) => ({ ...x, auth }))} vars={vars} isFolder />}
        {sub === 'variables' && (
          <>
            <KeyValueEditor rows={toRows(d.variables)} onChange={(rows) => update(tab.id, (x) => ({ ...x, variables: fromRows(rows) }))} vars={vars} namePlaceholder="Variable" />
            <p className="subtle xs mt-4">Environment variables take precedence over folder variables; folder variables over project variables. Never put secrets here — use environment secrets.</p>
          </>
        )}
        {sub === 'scripts' && <ScriptsEditor folder scripts={d.scripts} onChange={(scripts) => update(tab.id, (x) => ({ ...x, scripts }))} modelKey={tab.id} />}
      </div>
    </div>
  );
}

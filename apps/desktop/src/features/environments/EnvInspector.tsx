import { useMemo, useState } from 'react';
import clsx from 'clsx';
import { ChevronDown, ChevronRight, Columns3, Copy, FileCode, KeyRound, Pencil, Search as SearchIcon } from 'lucide-react';
import { EnvChip } from '@/components/EnvChip';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { SearchInput, Spinner } from '@/components/ui/misc';
import { promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { EnvironmentEntry, VarUsage } from '@/lib/types';
import { useNav } from '@/store/nav';
import { useVarsVersion } from '@/store/variables';
import { useWorkspace } from '@/store/workspace';
import { buildMatrix, usagesByName, usageTarget, type MatrixCell, type MatrixRow } from './matrix';
import { copySecretValue, declareSecret, reloadEnvData, setEnvVariable, useEnvData } from './store';

const KIND_LABEL: Record<VarUsage['kind'], string> = {
  request: 'Request',
  folder: 'Folder',
  realtime: 'Realtime',
  mock: 'Mock',
  note: 'Note',
  load: 'Load test',
  data: 'Fake data',
  connection: 'Connection',
  query: 'Query',
  webhook: 'Webhook',
  environment: 'Environment',
  project: 'Project',
  other: 'File',
};

/** Opens the file a usage points at (request, note, mock stub, load test…). */
export function openUsage(u: VarUsage, go: ReturnType<typeof useNav.getState>['go']) {
  const t = usageTarget(u);
  if (!t) {
    toast.info(u.path, `Line ${u.line}: ${u.excerpt}`);
    return;
  }
  go(t.section, t.param);
}

/** "Used in" list for one name. */
export function UsageList({ list, max = 40 }: { list: VarUsage[] | undefined; max?: number }) {
  const go = useNav((s) => s.go);
  const scanning = useEnvData((s) => s.scanning);
  if (!list?.length)
    return <div className="xs subtle env-usage-empty">{scanning ? <Spinner /> : 'Not referenced in any request, folder, mock, note or load test.'}</div>;
  return (
    <ul className="env-usages">
      {list.slice(0, max).map((u, i) => (
        <li key={`${u.path}:${u.line}:${i}`}>
          <button className="env-usage" onClick={() => openUsage(u, go)} title={`${u.path}:${u.line}\n${u.excerpt}`}>
            <span className="badge outline env-usage-kind">{KIND_LABEL[u.kind]}</span>
            <span className="mono xs truncate env-usage-path">
              {u.path}
              <span className="subtle">:{u.line}</span>
            </span>
            <span className="xs subtle truncate env-usage-excerpt">{u.excerpt}</span>
          </button>
        </li>
      ))}
      {list.length > max && <li className="xs subtle">+{list.length - max} more</li>}
    </ul>
  );
}

/** One environment's value (or secret status) for a row, with edit / copy actions. */
/** Edits of the environment open in the editor go to its draft instead of the file. */
export interface CurrentEnvHooks {
  current: string;
  setVariable(name: string, value: string): void;
  /** Adds a secret name to the draft and saves it (like "Add secret"). */
  declareSecret(name: string): Promise<boolean>;
}

export function CellActions({ row, cell, envs, hooks }: { row: MatrixRow; cell: MatrixCell; envs: EnvironmentEntry[]; hooks?: CurrentEnvHooks }) {
  const project = useWorkspace((s) => s.project!);
  const entry = envs.find((e) => e.id === cell.envId)!;
  const others = envs.filter((e) => e.id !== cell.envId);
  const isCurrent = (e: EnvironmentEntry) => !!hooks && e.id === hooks.current;
  const declare = (e: EnvironmentEntry, name: string) => (isCurrent(e) ? hooks!.declareSecret(name) : declareSecret(e, name));

  const edit = async () => {
    if (row.secret) {
      const value = await promptDialog({ title: `Set ${row.name} for ${entry.file.name}`, label: 'Stored in your OS keychain — never written to the project', placeholder: 'paste value' });
      if (value == null) return;
      try {
        if (!(await declare(entry, row.name))) return;
        await api.setSecret(project.root, entry.id, row.name, value);
        useVarsVersion.getState().bump();
        reloadEnvData();
        toast.success(`${row.name} stored in the keychain`);
      } catch (e) {
        toast.error('Keychain error', errorMessage(e));
      }
      return;
    }
    const value = await promptDialog({ title: `${row.name} in ${entry.file.name}`, label: cell.state === 'inherited' ? 'Overrides the project value from devall.yaml' : 'Value (committed with the environment file)', initial: cell.state === 'missing' ? '' : (cell.value ?? '') });
    if (value == null) return;
    if (isCurrent(entry)) hooks!.setVariable(row.name, value);
    else await setEnvVariable(entry, row.name, value);
  };

  const copyTo = (targets: EnvironmentEntry[]) => async () => {
    if (row.secret) return copySecretValue(entry.id, targets, row.name, declare);
    for (const t of targets) {
      if (isCurrent(t)) hooks!.setVariable(row.name, cell.value ?? '');
      else await setEnvVariable(t, row.name, cell.value ?? '');
    }
  };

  const canCopy = row.secret ? cell.state === 'set' : cell.state === 'value' || cell.state === 'empty';
  const items = (): MenuItemDef[] => [
    { heading: row.secret ? 'Copy keychain value to' : 'Copy value to' },
    ...others.map((o) => ({ label: <span className="row-sm"><EnvChip env={o.file} />{o.file.name}</span>, onSelect: copyTo([o]) })),
    ...(others.length > 1 ? [{ separator: true }, { label: 'All other environments', onSelect: copyTo(others) }] : []),
  ];

  return (
    <span className="env-cell-actions">
      <button className="icon-btn sm" onClick={edit} title={row.secret ? 'Set value' : 'Edit value'} aria-label={`Edit ${row.name} in ${entry.file.name}`}>
        {row.secret ? <KeyRound /> : <Pencil />}
      </button>
      {canCopy && others.length > 0 && (
        <Menu
          align="right"
          items={items}
          trigger={(p) => (
            <button className="icon-btn sm" {...p} title="Copy to another environment" aria-label="Copy to another environment">
              <Copy />
            </button>
          )}
        />
      )}
    </span>
  );
}

/** A matrix cell's value or status, as text. */
export function CellValue({ row, cell }: { row: MatrixRow; cell: MatrixCell }) {
  if (row.secret) {
    if (cell.state === 'absent') return <span className="env-cell missing">not declared</span>;
    if (cell.state === 'unset') return <span className="env-cell empty">not set{cell.linked ? ' · linked' : ''}</span>;
    return <span className="env-cell set">set · {cell.source}</span>;
  }
  if (cell.state === 'missing') return <span className="env-cell missing">missing</span>;
  if (cell.state === 'empty') return <span className="env-cell empty">empty</span>;
  return (
    <span className={clsx('env-cell mono', cell.state === 'inherited' && 'inherited')} title={cell.state === 'inherited' ? `From devall.yaml: ${cell.value}` : cell.value}>
      {cell.value}
    </span>
  );
}

function RowBadges({ row, current }: { row: MatrixRow; current?: string }) {
  const here = row.cells.find((c) => c.envId === current);
  const missingHere = here && (here.state === 'missing' || here.state === 'absent');
  return (
    <>
      {missingHere ? (
        <span className="badge danger">missing here</span>
      ) : row.missing > 0 ? (
        <span className="badge warning" title="Not defined in some environments">
          missing in {row.missing}
        </span>
      ) : null}
      {row.empty > 0 && <span className="badge outline">{row.secret ? `${row.empty} not set` : `${row.empty} empty`}</span>}
      {row.differs && <span className="badge accent">differs</span>}
    </>
  );
}

/**
 * Right-hand panel of the environment editor: every variable and secret of the project's
 * environments with its value in each environment, and where it is used.
 */
export function EnvInspector({ draft, hooks }: { draft: EnvironmentEntry; hooks: CurrentEnvHooks }) {
  const current = hooks.current;
  const project = useWorkspace((s) => s.project!);
  const status = useEnvData((s) => s.status);
  const usages = useEnvData((s) => s.usages);
  const go = useNav((s) => s.go);
  const [q, setQ] = useState('');
  const [open, setOpen] = useState<string | null>(null);
  // The environment being edited shows its unsaved draft.
  const envs = useMemo(() => project.environments.map((e) => (e.id === current ? draft : e)), [project.environments, current, draft]);
  const rows = useMemo(() => buildMatrix(envs, project.manifest.variables, status), [envs, project.manifest.variables, status]);
  const byName = useMemo(() => usagesByName(usages ?? []), [usages]);
  const needle = q.trim().toLowerCase();
  const visible = rows.filter((r) => !needle || r.key.toLowerCase().includes(needle));

  return (
    <aside className="env-aside" aria-label="Inspector">
      <div className="env-aside-head">
        <h3>Across environments</h3>
        <span className="spacer" />
        <button className="btn btn-sm" onClick={() => go('environments', '__compare')} title="Every variable × every environment">
          <Columns3 /> Compare all
        </button>
      </div>
      <SearchInput value={q} onChange={setQ} placeholder="Filter variables and secrets" />
      {rows.length === 0 ? (
        <div className="env-aside-empty xs subtle">
          <SearchIcon size={14} /> Add variables or secrets to compare them across environments and see where they are used.
        </div>
      ) : (
        <div className="env-aside-list">
          {visible.map((r) => {
            const isOpen = open === r.key;
            const uses = byName.get(r.key);
            return (
              <div key={r.key} className={clsx('env-insp-row', isOpen && 'open')}>
                <button className="env-insp-head" onClick={() => setOpen(isOpen ? null : r.key)} aria-expanded={isOpen}>
                  {isOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  {r.secret && <KeyRound size={12} className="subtle" />}
                  <span className="mono xs truncate grow">{r.secret ? `secret.${r.name}` : r.name}</span>
                  <RowBadges row={r} current={current} />
                  <span className={clsx('env-uses xs', !uses?.length && 'subtle')} title="References in project files">
                    <FileCode size={12} />
                    {usages ? (uses?.length ?? 0) : '…'}
                  </span>
                </button>
                {isOpen && (
                  <div className="env-insp-body">
                    <div className="env-insp-cells">
                      {r.cells.map((c) => {
                        const e = envs.find((x) => x.id === c.envId)!;
                        return (
                          <div key={c.envId} className={clsx('env-insp-cell', c.envId === current && 'current')}>
                            <span className="env-insp-env">
                              <EnvChip env={e.file} />
                              <span className="truncate small">{e.file.name}</span>
                            </span>
                            <CellValue row={r} cell={c} />
                            <CellActions row={r} cell={c} envs={envs} hooks={hooks} />
                          </div>
                        );
                      })}
                    </div>
                    <div className="label env-insp-label">Used in {uses?.length ? `(${uses.length})` : ''}</div>
                    <UsageList list={uses} />
                  </div>
                )}
              </div>
            );
          })}
          {!visible.length && <div className="xs subtle p-3">Nothing matches “{q}”.</div>}
        </div>
      )}
    </aside>
  );
}

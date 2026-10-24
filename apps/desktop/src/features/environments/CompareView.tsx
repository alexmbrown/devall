import { Fragment, useMemo, useState } from 'react';
import clsx from 'clsx';
import { ChevronDown, ChevronRight, Columns3, FileCode, KeyRound, RefreshCw } from 'lucide-react';
import { EnvChip } from '@/components/EnvChip';
import { Empty, SearchInput, Spinner, Switch } from '@/components/ui/misc';
import { useNav, useViewState } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { CellActions, CellValue, UsageList } from './EnvInspector';
import { buildMatrix, filterRows, hasIssue, usagesByName } from './matrix';
import { reloadEnvData, useEnvData } from './store';

/** Every variable and secret × every environment, with problems highlighted. */
export function CompareView() {
  const project = useWorkspace((s) => s.project!);
  const status = useEnvData((s) => s.status);
  const usages = useEnvData((s) => s.usages);
  const scanning = useEnvData((s) => s.scanning);
  const go = useNav((s) => s.go);
  const [q, setQ] = useViewState('env.compare.q', '');
  const [onlyIssues, setOnlyIssues] = useViewState('env.compare.issues', false);
  const [open, setOpen] = useState<string | null>(null);
  const envs = project.environments;
  const rows = useMemo(() => buildMatrix(envs, project.manifest.variables, status), [envs, project.manifest.variables, status]);
  const visible = filterRows(rows, q, onlyIssues);
  const byName = useMemo(() => usagesByName(usages ?? []), [usages]);
  const issues = rows.filter(hasIssue).length;

  return (
    <div className="page">
      <div className="page-header">
        <Columns3 size={16} className="subtle" />
        <h1>Compare environments</h1>
        <span className="subtle xs nowrap">
          {rows.filter((r) => !r.secret).length} variables · {rows.filter((r) => r.secret).length} secrets
          {issues > 0 && ` · ${issues} need attention`}
        </span>
        <span className="spacer" />
        <button className="icon-btn" onClick={reloadEnvData} title="Rescan usages and secret status" aria-label="Refresh">
          {scanning ? <Spinner /> : <RefreshCw />}
        </button>
      </div>
      {envs.length === 0 ? (
        <Empty icon={<Columns3 />} title="No environments yet">
          Create environments on the left, then compare their variables here.
        </Empty>
      ) : (
        <div className="page-body env-compare">
          <div className="row wrap env-compare-bar">
            <SearchInput value={q} onChange={setQ} placeholder="Filter by name or value" className="env-compare-search" />
            <Switch checked={onlyIssues} onChange={setOnlyIssues} label={<span className="small">Only missing, empty or different</span>} />
            <span className="spacer" />
            <span className="row-sm xs subtle env-legend">
              <span className="env-cell missing">missing</span>
              <span className="env-cell empty">empty / not set</span>
              <span className="env-cell inherited">from devall.yaml</span>
              <span className="badge accent">differs</span>
            </span>
          </div>
          <div className="env-matrix-wrap">
            <table className="table env-matrix">
              <thead>
                <tr>
                  <th className="env-matrix-name">Name</th>
                  {envs.map((e) => (
                    <th key={e.id}>
                      <button className="env-matrix-env" onClick={() => go('environments', e.id)} title={`Open ${e.file.name}`}>
                        <EnvChip env={e.file} />
                        <span className="truncate">{e.file.name}</span>
                      </button>
                    </th>
                  ))}
                  <th className="env-matrix-uses" title="References in project files">
                    Used
                  </th>
                </tr>
              </thead>
              <tbody>
                {visible.map((r) => {
                  const uses = byName.get(r.key);
                  const isOpen = open === r.key;
                  return (
                    <Fragment key={r.key}>
                      <tr className={clsx(r.differs && 'differs')}>
                        <td className="env-matrix-name">
                          <span className="row-sm">
                            {r.secret && <KeyRound size={12} className="subtle" />}
                            <span className="mono xs truncate">{r.secret ? `secret.${r.name}` : r.name}</span>
                            {r.differs && <span className="badge accent">differs</span>}
                          </span>
                        </td>
                        {r.cells.map((c) => (
                          <td key={c.envId} className={clsx('env-matrix-cell', `s-${c.state}`)}>
                            <span className="env-matrix-cell-inner">
                              <CellValue row={r} cell={c} />
                              <CellActions row={r} cell={c} envs={envs} />
                            </span>
                          </td>
                        ))}
                        <td className="env-matrix-uses">
                          <button className={clsx('env-uses-btn xs', !uses?.length && 'subtle')} onClick={() => setOpen(isOpen ? null : r.key)} aria-expanded={isOpen} title="Show where it is used">
                            {isOpen ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                            <FileCode size={12} /> {usages ? (uses?.length ?? 0) : '…'}
                          </button>
                        </td>
                      </tr>
                      {isOpen && (
                        <tr className="env-matrix-usages">
                          <td colSpan={envs.length + 2}>
                            <UsageList list={uses} max={100} />
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  );
                })}
              </tbody>
            </table>
            {!visible.length && <div className="p-4 subtle small">{rows.length ? 'Nothing matches the filter.' : 'No variables or secrets defined in any environment yet.'}</div>}
          </div>
          <p className="subtle xs">
            Secret values are never shown — only whether this machine has one and where it comes from. Copying a secret moves its keychain value between environments without
            displaying it.
          </p>
        </div>
      )}
    </div>
  );
}

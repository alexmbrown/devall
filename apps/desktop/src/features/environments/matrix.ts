import type { EnvironmentEntry, SecretStatus, VarUsage } from '@/lib/types';

// Pure model of the "compare environments" matrix (unit-tested in matrix.test.ts).

/** A variable cell: own value, empty string, inherited from devall.yaml, or not defined. */
export type VarCellState = 'value' | 'empty' | 'inherited' | 'missing';
/** A secret cell: has a value on this machine, declared without a value, or not declared. */
export type SecretCellState = 'set' | 'unset' | 'absent';

export interface MatrixCell {
  envId: string;
  state: VarCellState | SecretCellState;
  /** Variables only (never secret values). */
  value?: string;
  /** Secrets: where the value comes from (keychain, .env, a secret manager…). */
  source?: string;
  /** Secrets: secret-manager reference. */
  linked?: string;
}

export interface MatrixRow {
  /** `name` for variables, `secret.NAME` for secrets (what templates reference). */
  key: string;
  name: string;
  secret: boolean;
  cells: MatrixCell[];
  /** devall.yaml value (variables). */
  project?: string;
  missing: number;
  empty: number;
  /** Variables: environments resolve it to different values. */
  differs: boolean;
}

export function buildMatrix(envs: EnvironmentEntry[], projectVars: Record<string, string> | undefined, status: Record<string, SecretStatus[] | undefined>): MatrixRow[] {
  const proj = projectVars ?? {};
  const varNames = new Set<string>();
  const secretNames = new Set<string>();
  for (const e of envs) {
    Object.keys(e.file.variables ?? {}).forEach((n) => varNames.add(n));
    (e.file.secrets ?? []).forEach((n) => secretNames.add(n));
  }
  const rows: MatrixRow[] = [];
  for (const name of [...varNames].sort((a, b) => a.localeCompare(b))) {
    const cells: MatrixCell[] = envs.map((e) => {
      const vars = e.file.variables ?? {};
      if (Object.prototype.hasOwnProperty.call(vars, name)) return { envId: e.id, state: vars[name] === '' ? 'empty' : 'value', value: vars[name] };
      if (Object.prototype.hasOwnProperty.call(proj, name)) return { envId: e.id, state: 'inherited', value: proj[name] };
      return { envId: e.id, state: 'missing' };
    });
    const resolved = new Set(cells.filter((c) => c.state !== 'missing').map((c) => c.value));
    rows.push({
      key: name,
      name,
      secret: false,
      cells,
      project: proj[name],
      missing: cells.filter((c) => c.state === 'missing').length,
      empty: cells.filter((c) => c.state === 'empty').length,
      differs: resolved.size > 1,
    });
  }
  for (const name of [...secretNames].sort((a, b) => a.localeCompare(b))) {
    const cells: MatrixCell[] = envs.map((e) => {
      const linked = e.file.secretSources?.[name];
      if (!(e.file.secrets ?? []).includes(name)) return { envId: e.id, state: 'absent' };
      const src = status[e.id]?.find((s) => s.name === name)?.source ?? undefined;
      return { envId: e.id, state: src ? 'set' : 'unset', source: src, linked };
    });
    rows.push({
      key: `secret.${name}`,
      name,
      secret: true,
      cells,
      missing: cells.filter((c) => c.state === 'absent').length,
      empty: cells.filter((c) => c.state === 'unset').length,
      differs: false,
    });
  }
  return rows;
}

/** Rows that need attention: missing somewhere, empty/unset somewhere, or different values. */
export const hasIssue = (r: MatrixRow) => r.missing > 0 || r.empty > 0 || r.differs;

export function filterRows(rows: MatrixRow[], query: string, onlyIssues: boolean): MatrixRow[] {
  const q = query.trim().toLowerCase();
  return rows.filter((r) => (!onlyIssues || hasIssue(r)) && (!q || r.key.toLowerCase().includes(q) || r.cells.some((c) => c.value?.toLowerCase().includes(q))));
}

/** Usages grouped by referenced name. */
export function usagesByName(list: VarUsage[]): Map<string, VarUsage[]> {
  const m = new Map<string, VarUsage[]>();
  for (const u of list) m.set(u.name, [...(m.get(u.name) ?? []), u]);
  return m;
}

/** Every name a template can reference for these environments (variables + `secret.X`). */
export function referenceNames(envs: EnvironmentEntry[], projectVars?: Record<string, string>): string[] {
  const s = new Set<string>(Object.keys(projectVars ?? {}));
  for (const e of envs) {
    Object.keys(e.file.variables ?? {}).forEach((n) => s.add(n));
    (e.file.secrets ?? []).forEach((n) => s.add(`secret.${n}`));
  }
  return [...s].sort();
}

/** Where a usage opens (`go(section, param)`), or null. */
export function usageTarget(u: Pick<VarUsage, 'path' | 'kind'>): { section: 'http' | 'notes' | 'load' | 'mocks' | 'fakedata' | 'data' | 'webhooks' | 'environments'; param?: string } | null {
  const [top, ...rest] = u.path.split('/');
  const rel = rest.join('/');
  switch (top) {
    case 'http':
      return { section: 'http', param: rel };
    case 'notes':
      return { section: 'notes', param: rel };
    case 'load':
      return { section: 'load', param: rel };
    case 'mocks':
      return { section: 'mocks', param: rel.startsWith('mappings/') ? `stubs/file:${rel}` : undefined };
    case 'data':
      return { section: 'fakedata' };
    case 'connections':
    case 'queries':
      return { section: 'data' };
    case 'webhooks':
      return { section: 'webhooks' };
    case 'environments':
      return { section: 'environments', param: rel.replace(/\.env\.ya?ml$/, '') };
    default:
      return u.kind === 'project' ? { section: 'environments', param: '__project' } : null;
  }
}

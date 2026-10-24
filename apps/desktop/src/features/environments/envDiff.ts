/**
 * Per-variable differences between two versions of an environment file (`*.env.yaml`) or of the
 * project manifest (`devall.yaml` variables), for the environment history view. Secrets appear by
 * name only — files never hold their values anyway.
 */

export interface EnvLike {
  name?: string;
  color?: string;
  label?: string;
  protected?: boolean;
  description?: string;
  variables?: Record<string, unknown>;
  secrets?: string[];
  secretSources?: Record<string, unknown>;
}

export type EnvChange =
  | { kind: 'added'; target: 'variable'; name: string; value: string }
  | { kind: 'removed'; target: 'variable'; name: string; value: string }
  | { kind: 'changed'; target: 'variable'; name: string; from: string; to: string }
  | { kind: 'added' | 'removed'; target: 'secret'; name: string }
  | { kind: 'added' | 'removed'; target: 'secretSource'; name: string; value: string }
  | { kind: 'changed'; target: 'secretSource'; name: string; from: string; to: string }
  | { kind: 'changed'; target: 'name' | 'color' | 'label' | 'description'; from: string; to: string }
  | { kind: 'changed'; target: 'protected'; from: boolean; to: boolean };

const str = (v: unknown): string => (v === undefined || v === null ? '' : typeof v === 'string' ? v : JSON.stringify(v));

function asRecord(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
}

function asList(v: unknown): string[] {
  return Array.isArray(v) ? v.map(String) : [];
}

function diffMap(target: 'variable' | 'secretSource', before: Record<string, unknown>, after: Record<string, unknown>, out: EnvChange[]) {
  for (const [name, v] of Object.entries(after)) {
    if (!(name in before)) out.push({ kind: 'added', target, name, value: str(v) });
    else if (str(before[name]) !== str(v)) out.push({ kind: 'changed', target, name, from: str(before[name]), to: str(v) });
  }
  for (const [name, v] of Object.entries(before)) if (!(name in after)) out.push({ kind: 'removed', target, name, value: str(v) });
}

/**
 * What changed from `before` to `after` (either may be `null`: the file didn't exist). Order:
 * appearance (name, colour, badge, protected, description), variables, secrets, secret sources.
 */
export function diffEnv(before: EnvLike | null | undefined, after: EnvLike | null | undefined): EnvChange[] {
  const a = asRecord(before) as EnvLike;
  const b = asRecord(after) as EnvLike;
  const out: EnvChange[] = [];
  for (const target of ['name', 'color', 'label', 'description'] as const) {
    if (str(a[target]) !== str(b[target])) out.push({ kind: 'changed', target, from: str(a[target]), to: str(b[target]) });
  }
  if (!!a.protected !== !!b.protected) out.push({ kind: 'changed', target: 'protected', from: !!a.protected, to: !!b.protected });
  diffMap('variable', asRecord(a.variables), asRecord(b.variables), out);
  const sa = new Set(asList(a.secrets));
  const sb = new Set(asList(b.secrets));
  for (const name of sb) if (!sa.has(name)) out.push({ kind: 'added', target: 'secret', name });
  for (const name of sa) if (!sb.has(name)) out.push({ kind: 'removed', target: 'secret', name });
  diffMap('secretSource', asRecord(a.secretSources), asRecord(b.secretSources), out);
  return out;
}

/** One-line summary, e.g. "2 added · 1 changed · secret API_KEY removed". */
export function summarizeChanges(changes: EnvChange[]): string {
  if (!changes.length) return 'No changes';
  const vars = changes.filter((c) => c.target === 'variable');
  const parts: string[] = [];
  const n = (k: EnvChange['kind']) => vars.filter((c) => c.kind === k).length;
  if (n('added')) parts.push(`${n('added')} added`);
  if (n('changed')) parts.push(`${n('changed')} changed`);
  if (n('removed')) parts.push(`${n('removed')} removed`);
  for (const c of changes) {
    if (c.target === 'secret') parts.push(`secret ${c.name} ${c.kind}`);
    else if (c.target === 'protected') parts.push(c.to ? 'protected on' : 'protected off');
    else if (c.target === 'name') parts.push('renamed');
    else if (c.target === 'color' || c.target === 'label') parts.push('appearance');
    else if (c.target === 'description') parts.push('description');
    else if (c.target === 'secretSource') parts.push(`${c.name} link ${c.kind}`);
  }
  return [...new Set(parts)].join(' · ');
}

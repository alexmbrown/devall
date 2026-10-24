import type { PluginManifest, PluginStatus } from '@/lib/api/plugins';

/** Badge tone for a plugin status. */
export function statusTone(s: PluginStatus): 'success' | 'warning' | 'danger' | 'outline' {
  switch (s) {
    case 'ready':
      return 'success';
    case 'changed':
    case 'untrusted':
    case 'shadowed':
      return 'warning';
    case 'broken':
      return 'danger';
    default:
      return 'outline';
  }
}

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** "2 utilities · 1 hook · 1 auth provider" — what a plugin adds. */
export function contributionSummary(m: PluginManifest): string {
  const c = m.contributes;
  const parts = [
    c.utilities?.length ? plural(c.utilities.length, 'utility', 'utilities') : '',
    c.hooks?.length ? plural(c.hooks.length, 'request hook') : '',
    c.importers?.length ? plural(c.importers.length, 'import format') : '',
    c.codegen?.length ? plural(c.codegen.length, 'codegen target') : '',
    c.auth?.length ? plural(c.auth.length, 'auth provider') : '',
  ].filter(Boolean);
  return parts.length ? `Adds ${parts.join(' · ')}` : 'Adds nothing';
}

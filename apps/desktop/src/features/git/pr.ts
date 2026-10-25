import type { Comparison, ForgeKind, PrTemplate } from '@/lib/api/git';
import { markdownSummary } from './model';

/**
 * Prefilling the "Create pull/merge request" dialog from a comparison: title, description
 * (the repository's template, then the Markdown summary of commits and files), branch names on
 * the host. Pure, so it's testable.
 */

/** `feature/add-login_form` → `Add login form`. */
export function humanizeBranch(branch: string): string {
  const last = branch.split('/').pop() ?? branch;
  const words = last.replace(/^(\d+)[-_]/, '$1 ').replace(/[-_]+/g, ' ').trim();
  return words ? words[0].toUpperCase() + words.slice(1) : branch;
}

/** One commit: its subject; several: the branch name made readable. */
export function prTitle(c: Pick<Comparison, 'commits' | 'head'>, headBranch?: string | null): string {
  const own = c.commits.filter((k) => !k.equivalent);
  if (own.length === 1) return own[0].subject;
  return humanizeBranch(headBranch || c.head.name);
}

/** The template (if any), then the summary. */
export function prBody(c: Comparison, template: string | null, includeSummary = true): string {
  const parts: string[] = [];
  if (template?.trim()) parts.push(template.trimEnd());
  if (includeSummary) parts.push(markdownSummary(c).trimEnd());
  return parts.join('\n\n') + '\n';
}

/** `alice, @bob  carol` → `['alice', 'bob', 'carol']` (unique, `@` dropped). */
export function splitNames(s: string): string[] {
  const out: string[] = [];
  for (const raw of s.split(/[\s,;]+/)) {
    const n = raw.trim().replace(/^@/, '');
    if (n && !out.includes(n)) out.push(n);
  }
  return out;
}

/** `pull request` / `merge request` and the short names. */
export function nouns(kind: ForgeKind | null | undefined): { noun: string; short: string; host: string } {
  return kind === 'gitlab' ? { noun: 'merge request', short: 'MR', host: 'GitLab' } : kind === 'github' ? { noun: 'pull request', short: 'PR', host: 'GitHub' } : { noun: 'merge request', short: 'MR', host: 'the host' };
}

/** The default template: `pull_request_template.md` / `Default.md` first, else none. */
export function defaultTemplate(templates: PrTemplate[]): PrTemplate | null {
  return templates.find((t) => /^(pull_request_template|default)$/i.test(t.name)) ?? templates[0] ?? null;
}

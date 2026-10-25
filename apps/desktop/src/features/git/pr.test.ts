import { describe, expect, it } from 'vitest';
import type { Comparison } from '@/lib/api/git';
import { defaultTemplate, humanizeBranch, nouns, prBody, prTitle, splitNames } from './pr';

const ref = (name: string) => ({ name, sha: 'a'.repeat(40), short: 'aaaaaaa', kind: 'branch' as const, refname: `refs/heads/${name}` });
const commit = (subject: string, equivalent = false) => ({ hash: subject, short: subject.slice(0, 7), parents: [], author: 'A', email: '', date: '2026-10-01T00:00:00Z', subject, equivalent });

function cmp(commits: ReturnType<typeof commit>[]): Comparison {
  return {
    base: ref('main'),
    head: ref('feature/add-login_form'),
    mode: 'mergeBase',
    mergeBase: 'b'.repeat(40),
    mergeBases: 1,
    unrelated: false,
    from: 'b'.repeat(40),
    to: 'a'.repeat(40),
    headIsCurrent: true,
    uncommitted: false,
    ahead: commits.length,
    behind: 0,
    files: [{ path: 'src/login.ts', oldPath: null, status: 'A', similarity: null, added: 10, deleted: 0, binary: false, oldBlob: null, newBlob: 'c', generated: false, untracked: false }],
    commits,
    commitsTruncated: false,
    baseRemote: 'origin',
    fetchedSecsAgo: 10,
    remoteUrl: 'git@github.com:acme/web.git',
    baseRemoteBranch: 'main',
    headRemoteBranch: 'feature/add-login_form',
    headPushed: false,
  };
}

describe('pull request prefill', () => {
  it('titles from the only commit, else the branch', () => {
    expect(prTitle(cmp([commit('Add the login form')]))).toBe('Add the login form');
    expect(prTitle(cmp([commit('one'), commit('two')]))).toBe('Add login form');
    expect(prTitle(cmp([commit('one'), commit('already on base', true)]))).toBe('one');
    expect(humanizeBranch('JIRA-12_fix-thing')).toBe('JIRA 12 fix thing');
    expect(humanizeBranch('123-fix')).toBe('123 fix');
  });

  it('puts the template before the summary', () => {
    const body = prBody(cmp([commit('Add the login form')]), '## What\n\n## Why\n');
    expect(body.startsWith('## What\n\n## Why\n\n## feature/add-login_form → main')).toBe(true);
    expect(body).toContain('- A `src/login.ts` (+10 −0)');
    expect(prBody(cmp([]), null, false)).toBe('\n');
  });

  it('splits names and picks the default template', () => {
    expect(splitNames('alice, @bob  carol;alice')).toEqual(['alice', 'bob', 'carol']);
    expect(splitNames('  ')).toEqual([]);
    expect(nouns('gitlab').short).toBe('MR');
    expect(nouns('github').noun).toBe('pull request');
    expect(defaultTemplate([{ name: 'bug', path: 'x/bug.md', content: '' }, { name: 'Default', path: 'x/Default.md', content: 'd' }])?.content).toBe('d');
    expect(defaultTemplate([])).toBeNull();
  });
});

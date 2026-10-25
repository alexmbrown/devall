import { describe, expect, it } from 'vitest';
import type { ChangedFile, Comparison, Reviewed } from '@/lib/api/git';
import { applyMarks, displayOrder, fileTree, fileVersion, markChanges, nextUnreviewed, reviewProgress, reviewState, treeOrder, visibleFiles } from './review';
import { ageText, compareSpec, formatGitParam, hostCompareUrl, isStale, markdownSummary, parseGitParam, parseRemote } from './model';

const file = (path: string, over: Partial<ChangedFile> = {}): ChangedFile => ({
  path,
  oldPath: null,
  status: 'M',
  similarity: null,
  added: 1,
  deleted: 0,
  binary: false,
  oldBlob: `old-${path}`,
  newBlob: `new-${path}`,
  generated: false,
  untracked: false,
  ...over,
});

const mark = (blob: string): Reviewed => ({ blob, at: 1 });

describe('review state', () => {
  const files = [file('src/a.ts'), file('src/b.ts'), file('gone.ts', { status: 'D', newBlob: null }), file('package-lock.json', { generated: true })];

  it('keys marks to the file version and notices changes since review', () => {
    expect(fileVersion(files[0])).toBe('new-src/a.ts');
    expect(fileVersion(files[2])).toBe('-old-gone.ts');
    const marks = { 'src/a.ts': mark('new-src/a.ts'), 'src/b.ts': mark('an older blob'), 'gone.ts': mark('-old-gone.ts') };
    expect(reviewState(files[0], marks)).toBe('reviewed');
    expect(reviewState(files[1], marks)).toBe('changed');
    expect(reviewState(files[2], marks)).toBe('reviewed');
    expect(reviewState(files[3], marks)).toBe('unreviewed');
    expect(reviewProgress(files, marks)).toEqual({ reviewed: 2, changed: 1, total: 4 });
  });

  it('filters by text, hides reviewed files and shows only changed ones', () => {
    const marks = { 'src/a.ts': mark('new-src/a.ts'), 'src/b.ts': mark('stale') };
    const paths = (f: ChangedFile[]) => f.map((x) => x.path);
    expect(paths(visibleFiles(files, marks, { text: 'SRC .ts', hideReviewed: false, onlyChanged: false }))).toEqual(['src/a.ts', 'src/b.ts']);
    expect(paths(visibleFiles(files, marks, { text: '', hideReviewed: true, onlyChanged: false }))).toEqual(['src/b.ts', 'gone.ts', 'package-lock.json']);
    expect(paths(visibleFiles(files, marks, { text: '', hideReviewed: false, onlyChanged: true }))).toEqual(['src/b.ts']);
    // Renamed files match their old path too.
    expect(paths(visibleFiles([file('new/x.ts', { oldPath: 'legacy/x.ts', status: 'R' })], {}, { text: 'legacy', hideReviewed: false, onlyChanged: false }))).toEqual(['new/x.ts']);
  });

  it('finds the next file that still needs a look (wrapping, skipping reviewed ones)', () => {
    const order = [file('a'), file('b'), file('c'), file('d')];
    const marks = { b: mark('new-b'), c: mark('changed since') };
    expect(nextUnreviewed(order, marks, 'a')).toBe('c');
    expect(nextUnreviewed(order, marks, 'c')).toBe('d');
    expect(nextUnreviewed(order, marks, 'd')).toBe('a');
    expect(nextUnreviewed(order, marks, null)).toBe('a');
    const all = { a: mark('new-a'), b: mark('new-b'), c: mark('new-c'), d: mark('new-d') };
    expect(nextUnreviewed(order, all, 'a')).toBeNull();
  });

  it('marks and unmarks optimistically', () => {
    const changes = markChanges([files[0], files[2]], true);
    expect(changes).toEqual([
      { path: 'src/a.ts', blob: 'new-src/a.ts' },
      { path: 'gone.ts', blob: '-old-gone.ts' },
    ]);
    const marks = applyMarks({}, changes, 5);
    expect(Object.keys(marks)).toEqual(['src/a.ts', 'gone.ts']);
    expect(applyMarks(marks, markChanges([files[0]], false), 6)).toEqual({ 'gone.ts': { blob: '-old-gone.ts', at: 5 } });
  });

  it('builds a compact tree and orders files the way it shows them', () => {
    const tree = fileTree([file('src/lib/api/git.ts'), file('src/lib/api/http.ts'), file('src/app.tsx'), file('README.md'), file('docs/x.md')]);
    expect(tree.map((n) => n.name)).toEqual(['docs', 'src', 'README.md']);
    const src = tree[1];
    expect(src.children.map((n) => n.name)).toEqual(['lib/api', 'app.tsx']);
    expect(treeOrder(tree).map((f) => f.path)).toEqual(['docs/x.md', 'src/lib/api/git.ts', 'src/lib/api/http.ts', 'src/app.tsx', 'README.md']);
    const { main, generated } = displayOrder([file('z.ts'), file('yarn.lock', { generated: true }), file('a/b.ts')], false);
    expect(main.map((f) => f.path)).toEqual(['a/b.ts', 'z.ts']);
    expect(generated.map((f) => f.path)).toEqual(['yarn.lock']);
  });
});

describe('source control route', () => {
  it('round-trips repos, comparisons and commits', () => {
    expect(parseGitParam(undefined)).toEqual({ repo: null });
    expect(parseGitParam('web')).toEqual({ repo: 'web' });
    expect(parseGitParam('web:compare:origin/main...feature/x')).toEqual({ repo: 'web', compare: { base: 'origin/main', head: 'feature/x', mode: 'mergeBase' } });
    expect(parseGitParam(':compare:main..HEAD~2')).toEqual({ repo: null, compare: { base: 'main', head: 'HEAD~2', mode: 'direct' } });
    expect(parseGitParam('api:commit:abc1234')).toEqual({ repo: 'api', commit: 'abc1234' });
    expect(parseGitParam('api:compare:nonsense')).toEqual({ repo: 'api' });
    for (const p of ['web', 'web:compare:origin/main...feature/x', 'dev-all:compare:main..v1.2', 'api:commit:abc1234']) expect(formatGitParam(parseGitParam(p))).toBe(p);
    expect(formatGitParam({ repo: null })).toBeUndefined();
    expect(compareSpec('main', 'x', 'direct')).toBe('main..x');
  });
});

describe('compare helpers', () => {
  it('reads remotes and builds host compare URLs', () => {
    expect(parseRemote('git@github.com:acme/web.git')).toEqual({ host: 'github.com', path: 'acme/web' });
    expect(parseRemote('https://gitlab.example.com/group/sub/proj.git')).toEqual({ host: 'gitlab.example.com', path: 'group/sub/proj' });
    expect(parseRemote('ssh://git@gitlab.com:2222/group/proj.git')).toEqual({ host: 'gitlab.com', path: 'group/proj' });
    expect(parseRemote('')).toBeNull();
    expect(hostCompareUrl('git@github.com:acme/web.git', 'main', 'feature/x y')).toBe('https://github.com/acme/web/compare/main...feature/x%20y?expand=1');
    expect(hostCompareUrl('https://gitlab.com/acme/web', 'main', 'fix')).toBe('https://gitlab.com/acme/web/-/compare/main...fix');
    expect(hostCompareUrl('https://bitbucket.org/acme/web.git', 'main', 'fix')).toBeNull();
  });

  it('tells stale remote bases and formats ages', () => {
    expect(isStale({ baseRemote: 'origin', fetchedSecsAgo: null })).toBe(true);
    expect(isStale({ baseRemote: 'origin', fetchedSecsAgo: 60 })).toBe(false);
    expect(isStale({ baseRemote: 'origin', fetchedSecsAgo: 3 * 86400 })).toBe(true);
    expect(isStale({ baseRemote: null, fetchedSecsAgo: null })).toBe(false);
    expect(ageText(30)).toBe('30 s');
    expect(ageText(600)).toBe('10 min');
    expect(ageText(5 * 3600)).toBe('5 h');
    expect(ageText(3 * 86400)).toBe('3 days');
  });

  it('summarizes a comparison as Markdown', () => {
    const c = {
      base: { name: 'main', sha: 'b'.repeat(40), short: 'bbbbbbb', kind: 'branch', refname: 'refs/heads/main' },
      head: { name: 'feature/x', sha: 'h'.repeat(40), short: 'hhhhhhh', kind: 'branch', refname: 'refs/heads/feature/x' },
      mode: 'mergeBase',
      mergeBase: 'abcdef1234',
      commits: [{ hash: 'c1', short: 'c1c1c1c', parents: [], author: 'A', email: '', date: '', subject: 'Add login', equivalent: false }],
      commitsTruncated: false,
      uncommitted: false,
      files: [file('src/a.ts', { added: 3, deleted: 1 }), file('img.png', { binary: true, added: null, deleted: null, status: 'A' }), file('new.ts', { status: 'R', oldPath: 'old.ts' })],
    } as unknown as Comparison;
    const md = markdownSummary(c);
    expect(md).toContain('## feature/x → main');
    expect(md).toContain('1 commit · 3 files changed (+4 −1) · since abcdef1');
    expect(md).toContain('- c1c1c1c Add login');
    expect(md).toContain('- M `src/a.ts` (+3 −1)');
    expect(md).toContain('- A `img.png` (binary)');
    expect(md).toContain('- R `old.ts` → `new.ts`');
  });
});

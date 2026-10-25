import { useCallback, useEffect, useMemo, useRef } from 'react';
import clsx from 'clsx';
import {
  ArrowLeftRight,
  ArrowRight,
  CheckCheck,
  ChevronDown,
  ChevronUp,
  Copy,
  Eye,
  EyeOff,
  FoldVertical,
  GitBranch,
  GitCompare,
  GitMerge,
  GitPullRequestCreate,
  Link2,
  List,
  ListTree,
  RefreshCw,
  Space,
  UnfoldVertical,
  WrapText,
} from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import type { monaco } from '@/components/editor/monaco';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Callout, Checkbox, Empty, RelTime, SearchInput, Segmented, Spinner, copyText } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { formatLink } from '@/lib/links/syntax';
import { gitApi, type ChangedFile, type Repo } from '@/lib/api/git';
import { useActiveViewRef } from '@/store/nav';
import { ChangedFileList, Counts, StatusLetter } from './ChangedFileList';
import { diffSides, listedFiles, navOrder, useCompare, type CompareSpec } from './compareStore';
import { DiffPane } from './DiffPane';
import { reviewKey, step } from './keys';
import { ageText, compareSpec, compareTitle, hostCompareUrl, isStale, markdownSummary } from './model';
import { pickRef } from './RefPicker';
import { nextUnreviewed, reviewProgress, reviewState } from './review';

// ---------- side pane: files / commits ----------

/** The compare tab's side pane: Files (tree/flat, reviewed checkboxes, filter) and Commits (narrow the diff). */
export function CompareSide() {
  const s = useCompare();
  const data = s.data;
  if (!data) {
    if (s.error) return <div className="p-4"><Callout kind="danger">{s.error}</Callout></div>;
    return <Empty icon={<Spinner />} title="Comparing…" />;
  }
  const ranged = s.commitSel.length > 0;
  const order = navOrder(s);
  const files = listedFiles(s);
  const progress = reviewProgress(data.files, s.marks);
  return (
    <div className="git-compare-side">
      <div className="git-compare-tabs">
        <Segmented
          value={s.tab}
          onChange={s.setTab}
          options={[
            { value: 'files', label: `Files (${data.files.length})` },
            { value: 'commits', label: `Commits (${data.commits.length}${data.commitsTruncated ? '+' : ''})` },
          ]}
        />
      </div>
      {ranged && (
        <div className="git-range-banner xs">
          {s.rangeLoading ? <Spinner /> : <GitCompare size={12} />}
          <span className="grow">
            Showing {s.commitSel.length} of {data.commits.length} commits{ranged ? ` · ${files.length} files` : ''}
          </span>
          <button className="btn btn-sm btn-ghost" onClick={() => void s.selectCommits([])}>
            Show all
          </button>
        </div>
      )}
      {s.tab === 'files' ? (
        <>
          <div className="git-compare-filter">
            <SearchInput value={s.filter} onChange={s.setFilter} placeholder="Filter files…" />
            <button className={clsx('icon-btn sm', s.opts.tree && 'on')} title={s.opts.tree ? 'Show as a list' : 'Show as a tree'} onClick={() => s.setOpts({ tree: !s.opts.tree })}>
              {s.opts.tree ? <ListTree /> : <List />}
            </button>
            {!ranged && (
              <button className={clsx('icon-btn sm', s.opts.hideReviewed && 'on')} title={s.opts.hideReviewed ? 'Show reviewed files' : 'Hide reviewed files'} onClick={() => s.setOpts({ hideReviewed: !s.opts.hideReviewed })}>
                {s.opts.hideReviewed ? <EyeOff /> : <Eye />}
              </button>
            )}
          </div>
          {!ranged && progress.changed > 0 && (
            <div className="git-range-banner xs">
              <span className="grow">
                {progress.changed} file{progress.changed === 1 ? '' : 's'} changed since you reviewed {progress.changed === 1 ? 'it' : 'them'}
              </span>
              <button className={clsx('btn btn-sm btn-ghost', s.opts.onlyChanged && 'on')} onClick={() => s.setOpts({ onlyChanged: !s.opts.onlyChanged })}>
                {s.opts.onlyChanged ? 'Show all' : 'Show only those'}
              </button>
            </div>
          )}
          <ChangedFileList
            main={order.main}
            generated={order.generated}
            tree={s.opts.tree}
            selected={s.selected}
            onSelect={s.select}
            review={ranged ? undefined : { marks: s.marks, onToggle: (f, mark) => void s.setReviewed(f, mark).catch((e) => toast.error('Could not save the review mark', errorMessage(e))) }}
            showGenerated={s.opts.showGenerated}
            onToggleGenerated={(v) => s.setOpts({ showGenerated: v })}
            menu={(f) => fileMenu(f)}
          />
          {!files.length && <p className="muted small p-5">{s.commitSel.length && s.rangeLoading ? 'Reading the commits…' : data.files.length ? 'No files match.' : 'No differences.'}</p>}
        </>
      ) : (
        <CommitList />
      )}
    </div>
  );
}

function fileMenu(f: ChangedFile): MenuItemDef[] {
  const s = useCompare.getState();
  const state = s.commitSel.length ? null : reviewState(f, s.marks);
  return [
    ...(state ? [{ label: state === 'reviewed' ? 'Mark not reviewed' : 'Mark reviewed', icon: <CheckCheck />, onSelect: () => void s.setReviewed([f], state !== 'reviewed') }] : []),
    { label: 'Copy path', icon: <Copy />, onSelect: () => void copyText(f.path, 'Path copied') },
    ...(f.oldPath ? [{ label: 'Copy old path', icon: <Copy />, onSelect: () => void copyText(f.oldPath!, 'Path copied') }] : []),
  ];
}

function CommitList() {
  const data = useCompare((s) => s.data)!;
  const sel = useCompare((s) => s.commitSel);
  const selectCommits = useCompare((s) => s.selectCommits);
  const anchor = useRef<string | null>(null);
  const click = (e: React.MouseEvent, hash: string) => {
    if (e.shiftKey && anchor.current) {
      const a = data.commits.findIndex((c) => c.hash === anchor.current);
      const b = data.commits.findIndex((c) => c.hash === hash);
      const [lo, hi] = a < b ? [a, b] : [b, a];
      void selectCommits(data.commits.slice(lo, hi + 1).map((c) => c.hash));
    } else {
      anchor.current = hash;
      void selectCommits(sel.length === 1 && sel[0] === hash ? [] : [hash]);
    }
  };
  if (!data.commits.length) return <p className="muted small p-5">No commits on {data.head.name} that {data.base.name} doesn't have.</p>;
  return (
    <div className="list git-commit-list">
      <div className="xs subtle git-hint">Click a commit to see its changes; Shift+click selects a run of commits.</div>
      {data.commits.map((c) => (
        <div key={c.hash} className={clsx('list-item git-commit-row', sel.includes(c.hash) && 'on', c.equivalent && 'git-equivalent')} onClick={(e) => click(e, c.hash)} title={c.equivalent ? 'The same change is already on the base (cherry-picked or rebased)' : c.hash}>
          <span className="small truncate w-full">{c.subject}</span>
          <span className="xs subtle row-sm">
            <span className="mono">{c.short}</span> · <span className="truncate">{c.author}</span> · <RelTime ts={c.date} />
            {c.equivalent && <span className="badge outline">on {data.base.name}</span>}
          </span>
        </div>
      ))}
      {data.commitsTruncated && <p className="xs subtle p-4">Only the newest {data.commits.length} commits are listed.</p>}
    </div>
  );
}

/** Progress and the merge request button, pinned under the side pane. */
export function CompareFooter() {
  const data = useCompare((s) => s.data);
  const marks = useCompare((s) => s.marks);
  if (!data) return null;
  const p = reviewProgress(data.files, marks);
  const pct = p.total ? Math.round((p.reviewed / p.total) * 100) : 0;
  return (
    <div className="git-compare-footer">
      <div className="row-sm xs subtle">
        <span className="grow">
          {p.reviewed} / {p.total} reviewed
        </span>
        {p.reviewed > 0 && (
          <button className="btn btn-sm btn-ghost" title="Clear every reviewed mark of this comparison" onClick={() => void useCompare.getState().setReviewed(data.files.filter((f) => marks[f.path]), false)}>
            Reset
          </button>
        )}
      </div>
      <div className="git-progress-bar" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        <span style={{ width: `${pct}%` }} />
      </div>
      <button className="btn btn-primary w-full" onClick={() => void createMergeRequest()} title="Copies a Markdown summary (commits + files) and opens the host's compare page">
        <GitPullRequestCreate /> Create merge request…
      </button>
    </div>
  );
}

/**
 * For now: copy a Markdown summary of the comparison and open the host's compare page (GitHub /
 * GitLab). Creating the MR in the app (title, description, reviewers) is the next step; this is
 * where it plugs in.
 */
export async function createMergeRequest() {
  const s = useCompare.getState();
  const data = s.data;
  if (!data) return;
  await copyText(markdownSummary(data), 'Summary copied as Markdown — paste it into the merge request');
  const url = hostCompareUrl(data.remoteUrl, data.baseRemoteBranch ?? data.base.name, data.headRemoteBranch ?? data.head.name);
  if (!url) return toast.info('No compare page to open', "The remote isn't GitHub or GitLab — paste the summary where you open the merge request.");
  if (!data.headPushed) toast.info(`${data.head.name} isn't on the remote yet`, 'Push it first, then the compare page shows its changes.');
  await openUrl(url).catch((e) => toast.error('Could not open the browser', errorMessage(e)));
}

// ---------- main pane: header + diff ----------

/**
 * The compare tab's main pane: what is compared (head → base, mode, uncommitted), facts (merge
 * base, how far the base moved, conflicts, stale remote) and the selected file's diff with
 * keyboard review (j/k files, n/p changes, v reviewed + next).
 */
export function CompareMain({ repos, multi, onSpec, onPickRepo }: { repos: Repo[]; multi: boolean; onSpec(spec: CompareSpec): void; onPickRepo(name: string): void }) {
  const s = useCompare();
  const editor = useRef<monaco.editor.IStandaloneDiffEditor | null>(null);
  const onEditor = useCallback((ed: monaco.editor.IStandaloneDiffEditor | null) => (editor.current = ed), []);
  const active = useActiveViewRef();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!active.current) return;
      const k = reviewKey(e);
      if (!k) return;
      const st = useCompare.getState();
      if (!st.data) return;
      const order = navOrder(st);
      const flat = [...order.main, ...(st.opts.showGenerated ? order.generated : [])];
      e.preventDefault();
      if (k.type === 'file') {
        const j = step(flat.length, flat.findIndex((f) => f.path === st.selected), k.delta);
        if (j >= 0) st.select(flat[j].path);
      } else if (k.type === 'hunk') {
        editor.current?.goToDiff(k.delta > 0 ? 'next' : 'previous');
      } else if (!st.commitSel.length) {
        const f = listedFiles(st).find((x) => x.path === st.selected);
        if (!f) return;
        const mark = reviewState(f, st.marks) !== 'reviewed';
        void st.setReviewed([f], mark).catch((err) => toast.error('Could not save the review mark', errorMessage(err)));
        if (mark) {
          const next = nextUnreviewed(flat, st.marks, f.path);
          if (next) st.select(next);
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [active]);

  const file = useMemo(() => listedFiles(s).find((f) => f.path === s.selected) ?? null, [s]);
  const sides = file ? diffSides(s, file) : null;
  const spec = s.spec;
  if (!spec) return <Empty icon={<GitCompare />} title="Compare" />;
  const state = file && !s.commitSel.length ? reviewState(file, s.marks) : null;
  const toggle = (k: 'ignoreWs' | 'wrap' | 'full') => s.setOpts({ [k]: !s.opts[k] });

  return (
    <div className="col h-full min-h-0 git-compare" style={{ gap: 0 }}>
      <CompareHeader spec={spec} repos={repos} multi={multi} onSpec={onSpec} onPickRepo={onPickRepo} />
      {file && sides ? (
        <>
          <div className="toolbar git-diff-toolbar">
            <StatusLetter status={file.status} />
            <span className="mono xs truncate grow" title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}>
              {file.oldPath ? `${file.oldPath} → ` : ''}
              {file.path}
            </span>
            <Counts f={file} />
            <span className="badge outline shrink-0">{sides.label}</span>
            {state === 'changed' && (
              <button className={clsx('btn btn-sm', s.sinceReview && 'on')} title="Diff against the version you reviewed" onClick={() => s.setSinceReview(!s.sinceReview)}>
                {s.sinceReview ? 'Show full change' : 'Only what changed since my review'}
              </button>
            )}
            <Segmented
              value={s.opts.layout}
              onChange={(layout) => s.setOpts({ layout })}
              options={[
                { value: 'split', label: 'Split', title: 'Side by side' },
                { value: 'unified', label: 'Unified', title: 'One column' },
              ]}
            />
            <button className={clsx('icon-btn sm', s.opts.ignoreWs && 'on')} title="Ignore whitespace changes" onClick={() => toggle('ignoreWs')}>
              <Space />
            </button>
            <button className={clsx('icon-btn sm', s.opts.wrap && 'on')} title="Wrap long lines" onClick={() => toggle('wrap')}>
              <WrapText />
            </button>
            <button className={clsx('icon-btn sm', s.opts.full && 'on')} title={s.opts.full ? 'Show changed regions only' : 'Show whole files'} onClick={() => toggle('full')}>
              {s.opts.full ? <FoldVertical /> : <UnfoldVertical />}
            </button>
            <button className="icon-btn sm" title="Previous change (p)" onClick={() => editor.current?.goToDiff('previous')}>
              <ChevronUp />
            </button>
            <button className="icon-btn sm" title="Next change (n)" onClick={() => editor.current?.goToDiff('next')}>
              <ChevronDown />
            </button>
            {state && (
              <Checkbox
                checked={state === 'reviewed'}
                onChange={(v) => void s.setReviewed([file], v).catch((e) => toast.error('Could not save the review mark', errorMessage(e)))}
                label={<span className="xs">Reviewed</span>}
                title="Mark reviewed (v marks it and opens the next unreviewed file)"
              />
            )}
          </div>
          <div className="grow min-h-0 git-review-diff">
            <DiffPane root={s.root!} repo={s.repo} old={sides.old} next={sides.new} path={file.path} lines={(file.added ?? 0) + (file.deleted ?? 0)} opts={s.opts} onEditor={onEditor} />
          </div>
        </>
      ) : s.loading && !s.data ? (
        <Empty icon={<Spinner />} title="Comparing…" />
      ) : s.data ? (
        <Empty icon={<GitCompare />} title={s.data.files.length ? 'Select a file' : 'No differences'}>
          {s.data.files.length ? 'j / k move between files, n / p between changes, v marks a file reviewed and opens the next one.' : `${s.data.head.name} has no changes compared with ${s.data.base.name}.`}
        </Empty>
      ) : null}
    </div>
  );
}

function CompareHeader({ spec, repos, multi, onSpec, onPickRepo }: { spec: CompareSpec; repos: Repo[]; multi: boolean; onSpec(spec: CompareSpec): void; onPickRepo(name: string): void }) {
  const s = useCompare();
  const data = s.data;
  const root = s.root!;
  const pick = async (side: 'base' | 'head') => {
    const v = await pickRef({ root, repo: s.repo, title: side === 'base' ? 'Compare against (base)…' : 'Changes from (head)…', current: spec[side] });
    if (v && v !== spec[side]) onSpec({ ...spec, [side]: v, uncommitted: side === 'head' ? false : spec.uncommitted });
  };
  const fetch = async () => {
    try {
      await gitApi.sync(root, s.repo, 'fetch');
      toast.success('Fetched');
      await s.refresh();
    } catch (e) {
      toast.error('Fetch failed', errorMessage(e));
    }
  };
  const repoName = s.repo ?? data?.head.name ?? '';
  const link = formatLink('compare', `${repoName}/${compareSpec(spec.base, spec.head, spec.mode)}`);
  const found = repos.filter((r) => r.state === 'found');
  const added = data ? data.files.reduce((n, f) => n + (f.added ?? 0), 0) : 0;
  const deleted = data ? data.files.reduce((n, f) => n + (f.deleted ?? 0), 0) : 0;
  const c = s.conflicts;
  return (
    <div className="git-compare-head">
      <div className="git-compare-refs">
        {multi && (
          <Menu
            items={() => [{ heading: 'Repository' }, ...found.map((r) => ({ label: r.name, onSelect: () => onPickRepo(r.name) }))]}
            trigger={(p) => (
              <button className="switcher" {...p} title="Repository">
                <GitBranch />
                <span className="truncate strong">{s.repo}</span>
              </button>
            )}
          />
        )}
        <button className="btn btn-sm git-ref-btn" onClick={() => void pick('head')} title="The changes (head) — pick a branch, tag or commit">
          <span className="mono truncate">{spec.head}</span>
          {spec.uncommitted && <span className="badge accent">+ uncommitted</span>}
        </button>
        <ArrowRight size={14} className="subtle shrink-0" />
        <button className="btn btn-sm git-ref-btn" onClick={() => void pick('base')} title="Where they would merge (base) — pick a branch, tag or commit">
          <span className="mono truncate">{spec.base}</span>
        </button>
        <button className="icon-btn sm" title="Swap head and base" onClick={() => onSpec({ ...spec, base: spec.head, head: spec.base, uncommitted: false })}>
          <ArrowLeftRight />
        </button>
        <Segmented
          value={spec.mode}
          onChange={(mode) => onSpec({ ...spec, mode })}
          options={[
            { value: 'mergeBase', label: 'Since branched', title: 'Changes on the head since it branched off the base — what the merge request will show (git diff base...head)' },
            { value: 'direct', label: 'Tip to tip', title: 'Direct comparison of the two tips (git diff base head)' },
          ]}
        />
        <span title={data && !data.headIsCurrent ? 'Only when the head is the checked-out branch' : 'Include staged, unstaged and untracked changes of the working tree'}>
          <Checkbox checked={spec.uncommitted} onChange={(v) => (data?.headIsCurrent || !v ? onSpec({ ...spec, uncommitted: v }) : toast.info('Check out the head branch first', 'Uncommitted changes belong to the checked-out branch.'))} label={<span className="xs">Include uncommitted</span>} />
        </span>
        <span className="spacer" />
        <Menu
          align="right"
          items={() => [
            { label: 'Copy link to this comparison', icon: <Link2 />, onSelect: () => void copyText(link, 'Link copied') },
            { label: 'Copy summary as Markdown', icon: <Copy />, disabled: !data, onSelect: () => data && void copyText(markdownSummary(data), 'Summary copied') },
          ]}
          trigger={(p) => (
            <button className="icon-btn sm" title="More" {...p}>
              <Copy />
            </button>
          )}
        />
        <button className="icon-btn sm" title="Refresh" onClick={() => void s.refresh()}>
          {s.loading ? <Spinner /> : <RefreshCw />}
        </button>
      </div>
      {data && (
        <div className="git-compare-facts xs">
          {data.mode === 'mergeBase' && data.mergeBase && (
            <span title={`git merge-base ${data.base.name} ${data.head.name}`}>
              <GitMerge size={12} /> merge base <span className="mono">{data.mergeBase.slice(0, 7)}</span>
            </span>
          )}
          {data.behind > 0 && (
            <span className="git-warn-text" title={`Commits on ${data.base.name} that ${data.head.name} doesn't have`}>
              {data.base.name} has {data.behind} new commit{data.behind === 1 ? '' : 's'} since you branched
            </span>
          )}
          <span>
            {data.ahead} commit{data.ahead === 1 ? '' : 's'} · {data.files.length} file{data.files.length === 1 ? '' : 's'} <span className="git-add">+{added}</span> <span className="git-del">−{deleted}</span>
          </span>
          {s.conflictsLoading ? (
            <span className="subtle row-sm">
              <Spinner /> checking for conflicts…
            </span>
          ) : c && !c.supported ? (
            <span className="subtle" title={c.message ?? ''}>
              conflict check needs Git 2.38+ (you have {c.gitVersion})
            </span>
          ) : c && c.clean ? (
            <span className="git-ok">merges cleanly</span>
          ) : c && c.files.length ? (
            <span className="git-fail">would conflict in {c.files.length} file{c.files.length === 1 ? '' : 's'}</span>
          ) : null}
        </div>
      )}
      {data && (isStale(data) || data.unrelated || data.mergeBases > 1 || (c && !c.clean && c.files.length > 0) || (c?.supported && c.message)) && (
        <div className="callout-stack">
          {isStale(data) && (
            <Callout kind="warning">
              <div className="row-sm wrap">
                <span className="small grow">
                  {data.base.name} may be behind {data.baseRemote}: {data.fetchedSecsAgo === null ? 'this clone never fetched' : `last fetched ${ageText(data.fetchedSecsAgo)} ago`}.
                </span>
                <button className="btn btn-sm" onClick={() => void fetch()}>
                  <RefreshCw /> Fetch
                </button>
              </div>
            </Callout>
          )}
          {c && !c.clean && c.files.length > 0 && (
            <Callout kind="warning">
              <span className="small">
                Merging into {data.base.name} would conflict in: {c.files.slice(0, 8).join(', ')}
                {c.files.length > 8 ? ` and ${c.files.length - 8} more` : ''}. Rebase or merge {data.base.name} into {data.head.name} first.
              </span>
            </Callout>
          )}
          {c?.supported && c.message && <Callout kind="info"><span className="small">Conflict check: {c.message}</span></Callout>}
          {data.unrelated && <Callout kind="info"><span className="small">These histories have no common commit, so the tips are compared directly.</span></Callout>}
          {data.mergeBases > 1 && <Callout kind="info"><span className="small">There are {data.mergeBases} merge bases (criss-cross merges); the first one is used, as git does.</span></Callout>}
        </div>
      )}
      {s.error && data && (
        <div className="callout-stack">
          <Callout kind="danger">{s.error}</Callout>
        </div>
      )}
      {!data && s.error && (
        <div className="callout-stack">
          <Callout kind="danger">{s.error}</Callout>
        </div>
      )}
    </div>
  );
}

/** Tab label for a comparison: "web — feature/x → main" (multi-repo) or "feature/x → main". */
export function compareTabLabel(multi: boolean, repo: string | null, spec: { base: string; head: string }): string {
  const t = compareTitle(spec.base, spec.head);
  return multi && repo ? `${repo} — ${t}` : t;
}

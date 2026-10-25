import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type MergeMode, type OpResult, type RefEntry, type RemoteInfo, type ResetMode, type StashEntry, type Worktree } from '@/lib/api/git';
import { useRepos } from '@/store/repos';
import { formDialog } from './formDialog';

/**
 * Source control operations behind the branch panel, the graph's menus, the shortcuts and the
 * palette. Each asks first with the exact effect (destructive ones louder, the riskiest only
 * after typing a name), runs, reports, and calls `ctx.changed()`. Local operations are journaled
 * for undo (Ctrl/Cmd+Z); the confirmations say when something can't be undone (remote changes).
 */
export interface GitCtx {
  root: string;
  repo: string | null;
  /** The checked-out branch (`null` when detached). */
  current: string | null;
  changed(): void;
}

const UNDO_HINT = 'Undo (Ctrl/Cmd+Z) takes it back.';

export const validRefName = (v: string) =>
  /^[^\s~^:?*[\\]+$/.test(v.trim()) && !v.trim().startsWith('-') && !v.trim().startsWith('/') && !v.trim().endsWith('/') && !v.includes('..') && !v.includes('@{') && !v.trim().endsWith('.lock') ? undefined : 'Not a valid name';

async function run<T>(ctx: GitCtx, label: string, fn: () => Promise<T>, success?: (out: T) => string | null): Promise<T | undefined> {
  try {
    const out = await fn();
    const msg = success?.(out);
    if (msg) toast.success(msg, typeof out === 'string' && out.trim() ? out.trim().split('\n').slice(-3).join('\n') : undefined);
    return out;
  } catch (e) {
    toast.error(`${label} failed`, errorMessage(e));
    return undefined;
  } finally {
    ctx.changed();
  }
}

/** Reports a merge/rebase/cherry-pick/revert/stash result. */
export function reportOp(label: string, res: OpResult | undefined, done: string) {
  if (!res) return;
  if (res.conflicts) toast.error(`${label} stopped with conflicts`, 'Resolve them in Changes (the banner lists the files), then Continue — or Abort.');
  else if (res.paused) toast.info(`${label} paused`, 'Make your changes (amend the commit), then Continue in the banner.');
  else toast.success(done);
}

// ---------- branches ----------

export async function newBranch(ctx: GitCtx, at = 'HEAD', atLabel?: string) {
  const v = await formDialog({
    title: atLabel ? `New branch at ${atLabel}` : 'New branch',
    fields: [
      { kind: 'text', key: 'name', label: 'Branch name', mono: true, validate: validRefName },
      { kind: 'check', key: 'checkout', label: 'Check it out', initial: true },
    ],
    confirmText: (x) => (x.checkout ? 'Create & check out' : 'Create branch'),
  });
  if (!v) return;
  const name = String(v.name).trim();
  await run(ctx, 'Create branch', () => gitApi.branchCreate(ctx.root, ctx.repo, name, at, !!v.checkout), () => (v.checkout ? `Switched to new branch ${name}` : `Created ${name}`));
}

export async function renameBranch(ctx: GitCtx, name: string) {
  const next = await promptDialog({ title: `Rename ${name}`, label: 'New name', initial: name, confirmText: 'Rename', validate: (x) => validRefName(x) ?? (x.trim() === name ? 'Same name' : undefined) });
  if (!next) return;
  await run(ctx, 'Rename branch', () => gitApi.branchRename(ctx.root, ctx.repo, name, next.trim()), () => `Renamed ${name} to ${next.trim()}`);
}

export async function deleteBranch(ctx: GitCtx, b: RefEntry) {
  if (b.current) return toast.info(`${b.name} is checked out`, 'Check out another branch first.');
  if (b.worktree) return toast.info(`${b.name} is checked out in another worktree`, b.worktree);
  const ok = await confirmDialog({
    title: `Delete branch ${b.name}?`,
    message: `Removes the local branch ${b.name} (at ${b.sha.slice(0, 7)}). ${b.upstream ? `${b.upstream} on the remote stays. ` : ''}${UNDO_HINT}`,
    confirmText: 'Delete branch',
    danger: true,
  });
  if (!ok) return;
  try {
    await gitApi.branchDelete(ctx.root, ctx.repo, b.name, false);
    toast.success(`Deleted ${b.name}`, UNDO_HINT);
  } catch (e) {
    const msg = errorMessage(e);
    if (!/not fully merged/i.test(msg)) {
      toast.error('Delete branch failed', msg);
      ctx.changed();
      return;
    }
    const n = await gitApi.branchUnmerged(ctx.root, ctx.repo, b.name).catch(() => 0);
    const force = await confirmDialog({
      title: `${b.name} has unmerged work`,
      message: `${n || 'Some'} commit${n === 1 ? '' : 's'} on ${b.name} ${n === 1 ? 'is' : 'are'} on no other branch, tag or remote branch. Deleting the branch takes ${n === 1 ? 'it' : 'them'} off every ref (the undo journal keeps ${n === 1 ? 'it' : 'them'}: ${UNDO_HINT})`,
      confirmText: 'Delete anyway',
      danger: true,
    });
    if (force) await run(ctx, 'Delete branch', () => gitApi.branchDelete(ctx.root, ctx.repo, b.name, true), () => `Deleted ${b.name}`);
  }
  ctx.changed();
}

/** Deletes a branch on its remote (`origin/x`) — typed confirmation, can't be undone here. */
export async function deleteRemoteBranch(ctx: GitCtx, remoteBranch: string) {
  const [remote, ...rest] = remoteBranch.split('/');
  const branch = rest.join('/');
  const v = await formDialog({
    title: `Delete ${branch} on ${remote}?`,
    fields: [{ kind: 'note', content: <span>Runs <span className="mono xs">git push {remote} --delete {branch}</span>: the branch is deleted on the server for everyone. This can't be undone from here (re-push it if you still have the commits).</span> }],
    confirmText: 'Delete on remote',
    danger: true,
    typeToConfirm: () => remoteBranch,
  });
  if (!v) return;
  await run(ctx, 'Delete remote branch', () => gitApi.branchDeleteRemote(ctx.root, ctx.repo, remoteBranch), () => `Deleted ${remoteBranch}`);
}

export async function setUpstream(ctx: GitCtx, b: RefEntry, refs: RefEntry[]) {
  const remotes = refs.filter((r) => r.kind === 'remote').map((r) => r.name);
  if (!remotes.length) return toast.info('No remote branches', 'Push the branch with an upstream instead.');
  const guess = b.upstream ?? remotes.find((r) => r.endsWith(`/${b.name}`)) ?? remotes[0];
  const v = await formDialog({
    title: `Upstream of ${b.name}`,
    fields: [{ kind: 'select', key: 'up', label: 'Tracks', initial: guess, options: [{ value: '', label: '(none — stop tracking)' }, ...remotes.map((r) => ({ value: r, label: r }))] }],
    confirmText: 'Set upstream',
  });
  if (!v) return;
  const up = String(v.up) || null;
  await run(ctx, 'Set upstream', () => gitApi.branchUpstream(ctx.root, ctx.repo, b.name, up), () => (up ? `${b.name} tracks ${up}` : `${b.name} has no upstream now`));
}

export async function fastForward(ctx: GitCtx, b: RefEntry) {
  await run(ctx, 'Fast-forward', () => gitApi.branchFastForward(ctx.root, ctx.repo, b.name, null), () => `Fast-forwarded ${b.name} to ${b.upstream ?? 'its upstream'}`);
}

const MERGE_OPTIONS: { value: MergeMode; label: string; hint: string }[] = [
  { value: 'ff', label: 'Fast-forward if possible', hint: 'Moves the branch when it has no commits of its own, else makes a merge commit (git’s default).' },
  { value: 'noFf', label: 'Always create a merge commit', hint: 'Records the merge even when a fast-forward is possible (--no-ff).' },
  { value: 'ffOnly', label: 'Fast-forward only', hint: 'Refuses when the branches have diverged (--ff-only).' },
  { value: 'squash', label: 'Squash', hint: 'Stages all the changes as one commit for you to make; no merge is recorded (--squash).' },
];

export async function merge(ctx: GitCtx, rev: string) {
  const into = ctx.current ?? 'HEAD';
  if (rev === ctx.current) return;
  const v = await formDialog({
    title: `Merge ${rev} into ${into}`,
    fields: [{ kind: 'choice', key: 'mode', initial: 'ff', options: MERGE_OPTIONS }, { kind: 'note', content: <span className="subtle">If it conflicts, git stops so you can resolve or abort. {UNDO_HINT}</span> }],
    confirmText: (x) => (x.mode === 'squash' ? 'Squash' : 'Merge'),
  });
  if (!v) return;
  const mode = v.mode as MergeMode;
  const res = await run(ctx, 'Merge', () => gitApi.merge(ctx.root, ctx.repo, rev, mode));
  reportOp('Merge', res, mode === 'squash' ? `${rev} squashed into the staging area — commit it` : `Merged ${rev} into ${into}`);
}

export async function rebaseOnto(ctx: GitCtx, onto: string, ontoLabel = onto) {
  if (!ctx.current) return toast.info('HEAD is detached', 'Check out the branch to rebase first.');
  const ok = await confirmDialog({
    title: `Rebase ${ctx.current} onto ${ontoLabel}?`,
    message: `Replays the commits of ${ctx.current} that ${ontoLabel} doesn't have on top of it — they become new commits. If ${ctx.current} was pushed, you'll need to force-push (with lease). If a commit conflicts, git stops so you can resolve, skip it or abort. ${UNDO_HINT}`,
    confirmText: 'Rebase',
  });
  if (!ok) return;
  const res = await run(ctx, 'Rebase', () => gitApi.rebase(ctx.root, ctx.repo, onto));
  reportOp('Rebase', res, `Rebased ${ctx.current} onto ${ontoLabel}`);
}

const RESET_OPTIONS: { value: ResetMode; label: string; hint: string; danger?: boolean }[] = [
  { value: 'soft', label: 'Soft', hint: 'Moves the branch only: the undone commits’ changes stay staged.' },
  { value: 'mixed', label: 'Mixed', hint: 'Moves the branch and unstages: the changes stay in your files.' },
  { value: 'keep', label: 'Keep (safe)', hint: 'Moves the branch and updates files, but refuses if that would lose your uncommitted changes.' },
  { value: 'hard', label: 'Hard', hint: 'Moves the branch and throws away every uncommitted change to tracked files.', danger: true },
];

export async function resetTo(ctx: GitCtx, rev: string, revLabel: string) {
  const branch = ctx.current ?? 'HEAD';
  const v = await formDialog({
    title: `Reset ${branch} to ${revLabel}`,
    fields: [
      { kind: 'choice', key: 'mode', initial: 'keep', options: RESET_OPTIONS },
      { kind: 'note', content: (x) => (x.mode === 'hard' ? <span className="git-fail">Uncommitted changes to tracked files are discarded (untracked files stay). The undo journal snapshots them first — {UNDO_HINT}</span> : <span className="subtle">{UNDO_HINT}</span>) },
    ],
    confirmText: (x) => (x.mode === 'hard' ? 'Reset hard' : 'Reset'),
    danger: (x) => x.mode === 'hard',
    typeToConfirm: (x) => (x.mode === 'hard' ? branch : undefined),
  });
  if (!v) return;
  const mode = v.mode as ResetMode;
  await run(ctx, 'Reset', () => gitApi.reset(ctx.root, ctx.repo, rev, mode), () => `Reset ${branch} to ${revLabel} (${mode})`);
}

/** Points a branch that isn't checked out at a commit. */
export async function moveBranch(ctx: GitCtx, branch: string, rev: string, revLabel: string) {
  const ok = await confirmDialog({ title: `Move ${branch} to ${revLabel}?`, message: `Points the branch ${branch} at ${revLabel} (git branch -f). Commits only it had stay in the undo journal. ${UNDO_HINT}`, confirmText: 'Move branch' });
  if (ok) await run(ctx, 'Move branch', () => gitApi.branchMove(ctx.root, ctx.repo, branch, rev), () => `${branch} now points at ${revLabel}`);
}

// ---------- tags ----------

export async function createTag(ctx: GitCtx, at: string, atLabel: string) {
  const v = await formDialog({
    title: `New tag at ${atLabel}`,
    fields: [
      { kind: 'text', key: 'name', label: 'Tag name', mono: true, validate: validRefName },
      { kind: 'text', key: 'message', label: 'Message (makes an annotated tag)', multiline: true, placeholder: 'Leave empty for a lightweight tag' },
    ],
    confirmText: 'Create tag',
  });
  if (!v) return;
  const name = String(v.name).trim();
  await run(ctx, 'Create tag', () => gitApi.tagCreate(ctx.root, ctx.repo, name, at, String(v.message).trim() || null), () => `Tagged ${atLabel} as ${name}`);
}

export async function deleteTag(ctx: GitCtx, name: string) {
  const ok = await confirmDialog({ title: `Delete tag ${name}?`, message: `Removes the local tag. A copy on a remote stays (delete it there separately). ${UNDO_HINT}`, confirmText: 'Delete tag', danger: true });
  if (ok) await run(ctx, 'Delete tag', () => gitApi.tagDelete(ctx.root, ctx.repo, name), () => `Deleted tag ${name}`);
}

async function pickRemote(remotes: string[], title: string): Promise<string | null> {
  if (remotes.length <= 1) return remotes[0] ?? null;
  const v = await formDialog({ title, fields: [{ kind: 'select', key: 'remote', label: 'Remote', initial: remotes.includes('origin') ? 'origin' : remotes[0], options: remotes.map((r) => ({ value: r, label: r })) }], confirmText: 'Continue' });
  return v ? String(v.remote) : null;
}

export async function pushTag(ctx: GitCtx, name: string, remotes: string[]) {
  const remote = await pickRemote(remotes, `Push tag ${name}`);
  if (!remote) return remotes.length ? undefined : toast.info('No remote', 'Add a remote first.');
  await run(ctx, 'Push tag', () => gitApi.tagPush(ctx.root, ctx.repo, remote, name), () => `Pushed ${name} to ${remote}`);
}

export async function deleteRemoteTag(ctx: GitCtx, name: string, remotes: string[]) {
  const remote = await pickRemote(remotes, `Delete tag ${name} on a remote`);
  if (!remote) return;
  const v = await formDialog({
    title: `Delete tag ${name} on ${remote}?`,
    fields: [{ kind: 'note', content: <span>Runs <span className="mono xs">git push {remote} --delete refs/tags/{name}</span> — gone on the server for everyone; can't be undone from here.</span> }],
    confirmText: 'Delete on remote',
    danger: true,
    typeToConfirm: () => name,
  });
  if (v) await run(ctx, 'Delete remote tag', () => gitApi.tagDeleteRemote(ctx.root, ctx.repo, remote, name), () => `Deleted ${name} on ${remote}`);
}

// ---------- remotes ----------

export async function addRemote(ctx: GitCtx) {
  const v = await formDialog({
    title: 'Add remote',
    fields: [
      { kind: 'text', key: 'name', label: 'Name', initial: 'origin', mono: true, validate: validRefName },
      { kind: 'text', key: 'url', label: 'URL', mono: true, placeholder: 'git@github.com:owner/repo.git', validate: (x) => (x.trim() && !x.trim().startsWith('-') && !/\s/.test(x.trim()) ? undefined : 'Not a URL') },
      { kind: 'check', key: 'fetch', label: 'Fetch it now', initial: true },
    ],
    confirmText: 'Add remote',
  });
  if (!v) return;
  const name = String(v.name).trim();
  const ok = await run(ctx, 'Add remote', () => gitApi.remoteAdd(ctx.root, ctx.repo, name, String(v.url).trim()).then(() => true), () => `Added ${name}`);
  if (ok && v.fetch) await fetchRemote(ctx, name);
}

export async function renameRemote(ctx: GitCtx, r: RemoteInfo) {
  const next = await promptDialog({ title: `Rename remote ${r.name}`, label: 'New name', initial: r.name, confirmText: 'Rename', validate: validRefName });
  if (next && next.trim() !== r.name) await run(ctx, 'Rename remote', () => gitApi.remoteRename(ctx.root, ctx.repo, r.name, next.trim()), () => `Renamed ${r.name} to ${next.trim()}`);
}

export async function setRemoteUrl(ctx: GitCtx, r: RemoteInfo) {
  const next = await promptDialog({ title: `URL of ${r.name}`, label: 'URL', initial: r.fetchUrl ?? '', confirmText: 'Save', validate: (x) => (x.trim() && !/\s/.test(x.trim()) && !x.trim().startsWith('-') ? undefined : 'Not a URL') });
  if (next) await run(ctx, 'Change URL', () => gitApi.remoteSetUrl(ctx.root, ctx.repo, r.name, next.trim()), () => `${r.name} → ${next.trim()}`);
}

export async function removeRemote(ctx: GitCtx, r: RemoteInfo) {
  const ok = await confirmDialog({
    title: `Remove remote ${r.name}?`,
    message: `Forgets ${r.name} (${r.fetchUrl ?? 'no URL'}), its ${r.branches} remote-tracking branch${r.branches === 1 ? '' : 'es'} and the upstream settings that point at it. Your local branches and the repository on the server stay.`,
    confirmText: 'Remove remote',
    danger: true,
  });
  if (ok) await run(ctx, 'Remove remote', () => gitApi.remoteRemove(ctx.root, ctx.repo, r.name), () => `Removed ${r.name}`);
}

export async function pruneRemote(ctx: GitCtx, name: string) {
  await run(ctx, 'Prune', () => gitApi.remotePrune(ctx.root, ctx.repo, name), () => `Pruned ${name}`);
}

export async function fetchRemote(ctx: GitCtx, remote: string | null, prune = true) {
  await run(ctx, 'Fetch', () => gitApi.fetch(ctx.root, ctx.repo, remote, prune), () => (remote ? `Fetched ${remote}` : 'Fetched all remotes'));
}

// ---------- push / pull ----------

export async function pull(ctx: GitCtx) {
  await run(ctx, 'Pull', () => gitApi.sync(ctx.root, ctx.repo, 'pull'), () => 'Pulled');
}

/** Push: sets the upstream on the first push; offers force-with-lease (named) when rejected. */
export async function push(ctx: GitCtx) {
  let info;
  try {
    info = await gitApi.pushInfo(ctx.root, ctx.repo);
  } catch (e) {
    return toast.error('Push failed', errorMessage(e));
  }
  if (!info.branch) return toast.info('HEAD is detached', 'Check out a branch to push.');
  if (!info.remotes.length) return toast.info('No remote', 'Add a remote to push to (Branches → Remotes → +).');
  if (!info.upstream) {
    const remote = await pickRemote(info.remotes, `Push ${info.branch}`);
    if (!remote) return;
    const ok = await confirmDialog({ title: `Push ${info.branch} to ${remote}?`, message: `Creates ${remote}/${info.branch} and sets it as ${info.branch}'s upstream (git push -u ${remote} ${info.branch}).`, confirmText: 'Push' });
    if (ok) await run(ctx, 'Push', () => gitApi.push(ctx.root, ctx.repo, 'setUpstream', remote, info.branch), () => `Pushed ${info.branch} to ${remote}`);
    return;
  }
  try {
    const out = await gitApi.push(ctx.root, ctx.repo, 'normal');
    toast.success(`Pushed ${info.branch}`, out.trim().split('\n').slice(-2).join('\n') || undefined);
  } catch (e) {
    const msg = errorMessage(e);
    if (/non-fast-forward|rejected|fetch first|stale info/i.test(msg)) await forcePush(ctx, msg);
    else toast.error('Push failed', msg);
  }
  ctx.changed();
}

/** `--force-with-lease --force-if-includes`, after a confirmation naming the remote branch. */
export async function forcePush(ctx: GitCtx, why?: string) {
  const info = await gitApi.pushInfo(ctx.root, ctx.repo).catch(() => null);
  if (!info?.branch || !info.remote) return toast.error('Push rejected', why);
  const target = `${info.remote}/${info.remoteBranch ?? info.branch}`;
  const v = await formDialog({
    title: `Force-push ${info.branch} to ${target}?`,
    fields: [
      {
        kind: 'note',
        content: (
          <span className="col-sm">
            {why && <span className="subtle xs mono git-pre">{why.split('\n').slice(-3).join('\n')}</span>}
            <span>
              Overwrites <span className="mono">{target}</span> with your {info.branch} — with lease{info.forceIfIncludes ? ' and --force-if-includes' : ''}: only if nobody pushed to it since you last fetched. Commits only the remote has would be lost for everyone.
            </span>
          </span>
        ),
      },
    ],
    confirmText: 'Force-push with lease',
    danger: true,
    typeToConfirm: () => target,
  });
  if (v) await run(ctx, 'Force-push', () => gitApi.push(ctx.root, ctx.repo, 'forceWithLease', info.remote, info.remoteBranch ?? info.branch), () => `Force-pushed ${info.branch} to ${target}`);
}

// ---------- stash ----------

export async function saveStash(ctx: GitCtx) {
  const v = await formDialog({
    title: 'Stash changes',
    fields: [
      { kind: 'text', key: 'message', label: 'Message', placeholder: 'What you were doing (optional)' },
      { kind: 'check', key: 'untracked', label: 'Include untracked files', initial: true },
      { kind: 'check', key: 'staged', label: 'Only what is staged', hint: 'Git 2.35 or newer; leaves unstaged changes in place.' },
      { kind: 'check', key: 'keep', label: 'Keep staged changes in the working tree too (--keep-index)' },
    ],
    confirmText: 'Stash',
  });
  if (!v) return;
  const staged = !!v.staged;
  await run(
    ctx,
    'Stash',
    () => gitApi.stashSave(ctx.root, ctx.repo, { message: String(v.message).trim() || null, includeUntracked: !!v.untracked && !staged, stagedOnly: staged, keepIndex: !!v.keep }),
    (out) => (/No local changes/i.test(out) ? 'Nothing to stash' : 'Stashed'),
  );
}

export async function applyStash(ctx: GitCtx, e: StashEntry, pop: boolean) {
  const res = await run(ctx, pop ? 'Pop stash' : 'Apply stash', () => gitApi.stashApply(ctx.root, ctx.repo, e.index, e.oid, pop));
  reportOp(pop ? 'Pop' : 'Apply', res, pop ? `Popped “${e.title}”` : `Applied “${e.title}”`);
  if (res?.conflicts && pop) toast.info('The stash was kept', 'git keeps a stash when popping it conflicts; drop it once you resolved.');
}

export async function dropStash(ctx: GitCtx, e: StashEntry) {
  const ok = await confirmDialog({ title: `Drop stash “${e.title}”?`, message: `Removes ${e.name} from the stash list. ${UNDO_HINT}`, confirmText: 'Drop', danger: true });
  if (ok) await run(ctx, 'Drop stash', () => gitApi.stashDrop(ctx.root, ctx.repo, e.index, e.oid), () => `Dropped “${e.title}”`);
}

// ---------- worktrees ----------

export async function addWorktree(ctx: GitCtx, refs: RefEntry[], initialBranch?: string) {
  const locals = refs.filter((r) => r.kind === 'branch' && !r.current && !r.worktree).map((r) => r.name);
  const suggestFor = initialBranch ?? '';
  const initialPath = await gitApi.worktreeSuggest(ctx.root, ctx.repo, suggestFor || 'worktree').catch(() => '');
  const v = await formDialog({
    title: 'New worktree',
    size: 'lg',
    fields: [
      {
        kind: 'choice',
        key: 'mode',
        initial: initialBranch && locals.includes(initialBranch) ? 'existing' : 'new',
        options: [
          { value: 'new', label: 'A new branch' },
          { value: 'existing', label: 'An existing branch', hint: locals.length ? undefined : 'No branch is free (each is checked out somewhere)' },
          { value: 'detached', label: 'A commit, detached' },
        ],
      },
      { kind: 'text', key: 'name', label: 'New branch name', mono: true, validate: (x, all) => (all.mode === 'new' ? validRefName(x) : undefined), show: (x) => x.mode === 'new' },
      { kind: 'text', key: 'from', label: 'Starting at', mono: true, initial: ctx.current ?? 'HEAD', show: (x) => x.mode === 'new' || x.mode === 'detached' },
      { kind: 'select', key: 'branch', label: 'Branch', initial: initialBranch && locals.includes(initialBranch) ? initialBranch : (locals[0] ?? ''), options: locals.map((b) => ({ value: b, label: b })), show: (x) => x.mode === 'existing' },
      { kind: 'path', key: 'path', label: 'Folder (new or empty)', initial: initialPath, validate: (x) => (x.trim() ? undefined : 'Choose a folder') },
      { kind: 'check', key: 'open', label: 'Open it in Source control (this machine only, not added to devall.yaml)', initial: true },
    ],
    confirmText: 'Create worktree',
  });
  if (!v) return;
  const mode = String(v.mode);
  const checkout =
    mode === 'new' ? { type: 'newBranch' as const, name: String(v.name).trim(), from: String(v.from).trim() || 'HEAD' } : mode === 'existing' ? { type: 'branch' as const, name: String(v.branch) } : { type: 'detached' as const, rev: String(v.from).trim() || 'HEAD' };
  const w = await run(ctx, 'Create worktree', () => gitApi.worktreeAdd(ctx.root, ctx.repo, String(v.path).trim(), checkout), (x) => `Worktree at ${x.path}`);
  if (w && v.open) await openWorktree(ctx, w);
}

export async function openWorktree(ctx: GitCtx, w: Worktree): Promise<string | null> {
  try {
    const name = await gitApi.worktreeOpen(ctx.root, ctx.repo, w.path);
    await useRepos.getState().refreshOverview().catch(() => useRepos.getState().load());
    toast.success(`Opened ${name}`, 'In the repository list on this machine only.');
    return name;
  } catch (e) {
    toast.error('Could not open the worktree', errorMessage(e));
    return null;
  }
}

export async function lockWorktree(ctx: GitCtx, w: Worktree) {
  if (w.locked !== null) return void run(ctx, 'Unlock', () => gitApi.worktreeUnlock(ctx.root, ctx.repo, w.path), () => 'Unlocked');
  const reason = await promptDialog({ title: `Lock ${w.path}?`, label: 'Reason (optional)', confirmText: 'Lock' });
  if (reason !== null) await run(ctx, 'Lock', () => gitApi.worktreeLock(ctx.root, ctx.repo, w.path, reason || null), () => 'Locked — prune and remove leave it alone');
}

export async function removeWorktree(ctx: GitCtx, w: Worktree) {
  if (w.locked !== null) return toast.info('The worktree is locked', 'Unlock it first.');
  const ok = await confirmDialog({ title: `Remove the worktree at ${w.path}?`, message: `Removes that folder (it has nothing uncommitted — git refuses otherwise) and its worktree record. ${w.branch ? `The branch ${w.branch} stays.` : ''}`, confirmText: 'Remove worktree', danger: true });
  if (!ok) return;
  try {
    await gitApi.worktreeRemove(ctx.root, ctx.repo, w.path, false);
    toast.success('Worktree removed');
  } catch (e) {
    const msg = errorMessage(e);
    if (!/modified or untracked|contains modified|is dirty|uncommitted/i.test(msg)) {
      toast.error('Remove failed', msg);
    } else {
      const v = await formDialog({
        title: 'The worktree has uncommitted changes',
        fields: [{ kind: 'note', content: <span>Removing it anyway moves <span className="mono xs">{w.path}</span> — with its uncommitted and untracked files — to the OS trash, then git forgets the worktree. Restore the folder from the trash if you need them.</span> }],
        confirmText: 'Remove with changes',
        danger: true,
        typeToConfirm: () => w.branch ?? 'remove',
      });
      if (v) await run(ctx, 'Remove worktree', () => gitApi.worktreeRemove(ctx.root, ctx.repo, w.path, true), () => 'Worktree removed');
    }
  }
  await useRepos.getState().refreshOverview().catch(() => {});
  ctx.changed();
}

export async function pruneWorktrees(ctx: GitCtx) {
  await run(ctx, 'Prune worktrees', () => gitApi.worktreePrune(ctx.root, ctx.repo), (out) => (out.trim() ? 'Pruned' : 'Nothing to prune'));
}

# GitKraken Desktop parity research: Source control module

Researched 2026-10-24. Current GitKraken Desktop release: **12.6.1 (2026-10-20)**. GitKraken bundles Git 2.54.0 (since 12.3.1) and ships an Electron app (Electron 41 since 12.1.0).

Our starting point (apps/desktop/src/features/git/GitPage.tsx + src-tauri/src/modules/git.rs + crates/devall-core/src/repos/git.rs, which shell out to the user's `git`): changes list with stage/unstage/discard (whole files), file diff, commit/amend, flat history list (`git log --pretty=format:%H%x1f%h%x1f%an%x1f%aI%x1f%s%x1e`), branches + checkout, fetch/pull/push, secret-scan pre-commit hook, multi-repo bulk fetch/pull/switch (`git_bulk`, `git_switch_all`, `repos_*`), with another agent extending multi-repo projects right now.

Confidence labels used below: **[doc]** = stated in GitKraken's official help or release notes; **[pricing]** = from gitkraken.com/pricing; **[inferred]** = my reading of the docs' wording or screenshots, not stated outright; **[3p]** = third-party source.

---

## 0. TL;DR

- GitKraken's free **Community** tier only covers **local repos and public remotes** [pricing]. Any repo with a private remote needs a paid plan. On top of that, these are paid regardless of repo: all **AI** features, **multiple profiles**, **Launchpad** (Pro; Team view needs Advanced), Launchpad snooze, **Conflict Prevention** (Pro+), Cloud Workspace sharing, self-hosted integrations (Advanced), SSO (Advanced/Business), Insights (Business). Features marked "Public Repos Only" on Community include the merge-conflict output editor, interactive rebase, interactive cherry-pick, one-click undo/redo, worktrees, file history & blame, PR view/create, hiding/soloing, LFS, Gitflow and all Agent Session features [pricing].
- **Branch comparison is GitKraken's weak spot.** It has no dedicated "compare branch A to branch B" workflow [doc: the diff page covers comparing two selected commits, WIP vs a commit/branch, and combined consecutive commits]. Ctrl/Cmd-clicking two branch tips compares them "directly", which is endpoint-to-endpoint (two-dot) semantics [inferred]. It has no documented merge-base ("what the MR will show") mode and no per-file "reviewed" checkboxes in Desktop. In-app PR review was **removed in 12.3.0** in favour of a browser-based review on gitkraken.dev. Merge-base semantics only show up in the paid AI "Explain Branch Changes" and in "View changes in PR", which works only after a PR exists (12.4.1). GitKraken's own VS Code extension, GitLens, does have review-progress checkboxes in its Search & Compare view [doc]. **A free, local pre-MR review view with merge-base semantics, reviewed-file tracking and conflict prediction would beat GitKraken here.**
- GitKraken's 2026 direction is **AI coding agents running in worktrees** (Agent Sessions view, 12.0 to 12.6), embedded terminal tabs, stacked PRs on GitHub, and AI Commit Composer.

---

## 1. Pricing and tiers (what GitKraken charges for)

Tiers on the official page [pricing, fetched 2026-10-24; the dollar amounts did not render in the fetched page]:

| Tier | Seats | Weekly AI credits/user | Key adds |
|---|---|---|---|
| Community (free) | – | none listed for Desktop | Local repos and public remotes only. Includes a 14-day Advanced trial |
| Pro | 1–2 | 1,000,000 | Private repos, Launchpad, cloud issue trackers (Jira Cloud, Trello), Shared Cloud Workspaces |
| Advanced | ≤10 | 2,000,000 | Self-hosted integrations (GHES, GitLab Self-Managed, Bitbucket DC), single-domain SSO, Team Launchpad |
| Business | ≤100 | 3,000,000 | Multi-domain SSO, Insights, admin Conflict Prevention control, Cloud Patch storage options |
| Enterprise | 20+ | 4,000,000 | Custom contract, audit logs, SLA |

Prices: billed annually per seat; Pro is priced as "first seat, then per seat". Third-party figures disagree: Pro is about **$4–5/seat/mo annual** (Capterra $4.00; toolradar/gappsy/thesoftwarescout $4.95); Advanced is $8.95–12; Business about $14.95–18 [3p, unverified]. GitKraken runs frequent 40–60% first-year promotions. Students get Pro via the GitHub Student Pack [3p].

Community "Public Repos Only" Desktop rows [pricing]: Merge Conflict Output Editor, Code Editor, Gitflow, Git LFS, Git Worktrees, File history & blame, View & create pull requests, Hiding & soloing, Visual Interactive Rebase, Visual Interactive Cherry Pick, One-click undo & redo, Agent Sessions view/launch/terminal sessions/worktree status.
Also paid: Multiple Profiles [doc: profiles page says "paid subscription"]; every GitKraken AI feature [doc: AI page says "All AI features require a paid GitKraken subscription"]; Conflict Prevention needs Pro+ [doc]; Launchpad Team View needs Advanced+, snoozing needs a paid plan [doc]; multiple GitHub accounts need Pro via profiles [doc].

**Our stance (free mission):** everything in this document that runs locally ships free, private repos and self-hosted Git hosts included. Hosted-only items (patch sharing links, team dashboards) would be the only candidates for paid-at-cost.

---

## 2. Feature inventory by area

Format: **Feature**: what it does. *GK tier.* → **Our implementation** (git porcelain that backs it).

### 2.1 Commit graph
- **Lanes, colours, ref pills, WIP row**: rows are commits, newest first. Columns: Branch/Tag, Graph, Message by default; Author, Date/Time and SHA are optional; columns can be reordered and resized, saved per repo [doc: interface, May 2026]. Each worktree with changes, or with an active agent session, gets its own WIP row (12.3.0). *Free on public/local repos.* → Build the lane layout ourselves in Rust instead of parsing `git log --graph` (ASCII, slow, unstable to parse). Read `git log --all --topo-order -z --format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%D` (or `--date-order`) as a stream. Single pass: keep a vector of active lanes, each holding the hash it expects next. A commit takes the first lane expecting it, and other lanes expecting it merge into that one. The lane's expectation becomes the commit's first parent; extra parents get free or new lanes. Emit per row: column, colour index, edges (from lane, to lane). Cost is linear in commits × active lanes; 100k commits is fine in Rust. Refs come from `git for-each-ref --format='%(refname)%00%(objectname)%00%(upstream:short)%00%(upstream:track)%00%(HEAD)'`. Page rows to the UI (e.g., 500 per request) and draw a virtualized canvas, keeping only the parent graph in Rust memory.
- **Initial commit count / lazy load**: by default GitKraken shows up to 2000 commits ("Initial Commits in Graph", minimum 500), with "Lazy Load Commits" and "Show All Commits" options; the docs warn that show-all is slow on large repos [doc: preferences, Jan 2026]. → Stream with `--topo-order`. It stays incremental when a commit-graph file exists, so offer opt-in `git commit-graph write --reachable --changed-paths` or `git maintenance start`.
- **Branch highlighting / ghost branch**: hovering a branch highlights its commits and fades the rest; hovering a commit shows its nearest containing branch [doc]. → Highlighting is client-side from the lane data. Containing branch: `git branch -a --contains <sha>` (cache it), or `git name-rev --refs=... <sha>`.
- **Search** (Ctrl/Cmd+F) by message, SHA or author, with live highlighting; filter by author or team via the Author column [doc]. Marketing also claims search by file, code change and natural-language AI [3p/marketing]. → `git log --all -i --grep=<q>`, `--author=<q>`, SHA prefix via `git rev-parse --verify <q>^{commit}`, file via `-- <path>` (fast with changed-path Bloom filters), code via pickaxe `-S<str>` / `-G<regex>` (slow: make it cancellable and stream results).
- **Hide / Solo / pin / Smart Branch Visibility**: hide refs, solo a branch, bulk hide by section, pin a branch (12.3.1). Smart visibility (11.10) shows only HEAD, its target branch and their upstreams [doc + feedback thread]. *Hiding/soloing is Public Repos Only on free.* → Pass explicit refs to `git log` instead of `--all`; hide with `--exclude=<glob> --all` placed before `--all`.
- **Combined diff of consecutive commits** (12.6.0): selecting a range shows its combined changes, and you can make a patch from it [doc]. → `git diff <oldest>^ <newest>`. Use `--root` handling for the initial commit.
- **Signature badges**: shown in the commit panel, not the graph [doc]. → `git log --format=%G?%x1f%GS%x1f%GK`.

### 2.2 Left panel: branches, remotes, tags, stashes, PRs, issues, submodules, worktrees, teams
- Sections: Local, Remote, Pull Requests, Issues, Teams, Tags, Stashes, Submodules, plus worktrees; there's a `List | Agents` toggle [doc]. Sections can be filtered (Ctrl/Cmd+Alt+F), collapsed and resized [doc]. Branch ops: create (Ctrl+B), checkout by double-click, rename, delete with multi-select ("Deleting a branch is permanent"), and undo of delete [doc]. Left Panel search finds remote branches when you include the remote name (12.4.1). Prefetch and non-standard refs are hidden (11.8).
→ Branches: `git for-each-ref refs/heads refs/remotes` with `%(upstream:track)`, or `%(ahead-behind:<base>)` (Git ≥2.41) for ahead/behind against main. Rename: `git branch -m`. Delete: `git branch -d` (or `-D` after confirmation); remote delete: `git push <remote> --delete <b>`. Set upstream: `git branch -u`. Remotes: `git remote -v`, `add`, `rename`, `set-url`, `remove`, `git fetch --prune`. Tags: `git for-each-ref refs/tags --format=%(refname:short)%00%(objectname)%00%(*objectname)%00%(contents:subject)`.

### 2.3 Drag-and-drop
- Drag a branch onto another to get Merge, Rebase, Interactive Rebase, Fast-forward, Reset, or "Start a pull request"; drag a ref to a remote to push [doc: branching page and PR page]. The sidebar menu shows fast-forward/merge/rebase options [doc]. 12.6.0 stopped offering ff/merge for remote branches that are behind.
→ Treat drag-and-drop as a shortcut that opens a confirm popover showing the exact command. Commands: merge `git merge [--no-ff|--ff-only|--squash] <src>` (target must be checked out); fast-forward a branch that isn't checked out without switching `git fetch . <src>:<dst>` (refuses non-ff, which is safe); rebase `git rebase <onto> [<branch>]`; reset branch to a commit `git branch -f <b> <sha>`, or `git reset` when it is HEAD; push `git push <remote> <src>:<dst>`.

### 2.4 Interactive rebase
- Started by dragging a branch, right-clicking a branch, or right-clicking a parent commit. Actions: Pick/Reword/Squash/Drop (keys P/R/S/D) and reorder by drag, with a Reset button. Limits: no merge commits, no initial commit, a rebase started in GitKraken must be finished there [doc, Mar 2026]. "Rebase N commits onto <branch>" and "Rebase onto this commit" for commits without a branch (12.2.0) [doc]. *Public Repos Only on free.* GitKraken's docs don't mention fixup/edit.
→ `git rebase -i <base>` with `GIT_SEQUENCE_EDITOR` pointing at our own helper. The devall CLI can act as the editor: the app writes the todo file, and the helper copies it over `$1`. Reword: set `GIT_EDITOR` to a helper that writes the prepared message, or emit `pick` followed by `exec git commit --amend -F <msgfile>`. Support `fixup`, `fixup -C`, `edit` (show a paused state), `drop`, `--autosquash`, `--update-refs` (≥2.38; keeps stacked branches pointing at the rewritten commits) and `--rebase-merges`. Read progress from `.git/rebase-merge/{msgnum,end,done,git-rebase-todo}`. Continue with `GIT_EDITOR=true git rebase --continue`; abort/skip are the matching flags.

### 2.5 Cherry-pick / revert / reset
- Cherry-pick a single commit or multiple ("Visual Interactive Cherry Pick", Public Repos Only on free); Revert Commit; Reset soft/mixed/hard via the commit context menu or by dragging [doc].
→ `git cherry-pick -x <oldest>..<newest>` or an ordered list of SHAs, plus `--continue/--abort/--skip`. `git revert --no-edit <sha>`, with `-m 1` for merges. `git reset --soft|--mixed|--hard <sha>`; offer `--keep` as the safe "hard" option. Snapshot before any hard reset (see 2.6).

### 2.6 Undo / redo
- One-click undo/redo (Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z) covers checkout, commit, discard, delete branch, remove remote, reset branch to commit, interactive rebase, multi-commit cherry-pick, drop/reword commits, and AI Recompose. **Only the most recent action** can be undone [doc, Mar 2026]. 12.6.x fixed undo of reset/rebase on a detached HEAD. *Public Repos Only on free.*
→ Keep an operation journal in Rust. Before each mutating op, record HEAD, the values of the refs it touches, the index tree (`git write-tree`) and a worktree snapshot. Snapshot with `git stash create` (tracked files only, leaves the worktree alone), or include untracked files via a temp index: `GIT_INDEX_FILE=<tmp> git add -A && git write-tree && git commit-tree`. Anchor each snapshot under `refs/devall/undo/<n>` so gc won't prune it. Undo restores refs with `git update-ref` and the worktree/index with `git read-tree -u --reset` or `git checkout <snap> -- .`. This is multi-level, which beats GitKraken's single step. Use the reflog (`git reflog`, `ORIG_HEAD`) as a fallback.

### 2.7 Stash
- Toolbar Stash/Pop; apply keeps the entry, pop removes it; partial stash from staged files; applying a single file from a stash; AI stash message (paid) [doc].
→ `git stash push -m <msg> [--staged (≥2.35) | --keep-index | -u] [-- paths]`. List: `git stash list --format=%gd%x1f%H%x1f%gs%x1f%ct`. Show: `git stash show -p --include-untracked stash@{n}` (≥2.32). Apply/pop/drop (drop is destructive, so journal the SHA first). Branch: `git stash branch <b> stash@{n}`. Single file: `git restore --source=stash@{n} -- <path>`.

### 2.8 Staging (file / hunk / line)
- Stage or unstage a file (S/U keys), all (Ctrl+Shift+S/U), selected lines or hunks via right-click in the diff; discard all, selected files, hunks or lines ("Permanently removes uncommitted changes"); ignore by name/extension/dir (root .gitignore only); "Ignore and Stop Tracking" [doc, Mar 2026]. Commit panel: summary and description, amend, "Update Message", skip hooks, commit templates (`commit.template`), co-authors as trailers [doc].
→ Hunk/line staging: parse `git diff --no-color --no-ext-diff -U3 -- <path>`. For a line selection, build a patch: drop unselected `+` lines and turn unselected `-` lines into context. Apply with `git apply --cached --recount --whitespace=nowarn -` (stdin). Unstage applies the reverse of `git diff --cached` (`git apply --cached -R`). Discarding hunks/lines uses `git apply -R` on the worktree, after a snapshot. Stop tracking: `git rm --cached`. Intent-to-add for new files: `git add -N`. Skip hooks: `--no-verify`. Warn when it bypasses our secret-scan hook.

### 2.9 Diff views
- Hunk / Inline / Split modes; word diff; syntax highlighting; minimap; next/prev change; "Edit in working directory"; ignore-whitespace toggle (mentioned in a 12.3.0/12.3.1 fix); word wrap (11.10); external diff tools (Beyond Compare, Kaleidoscope, KDiff, Araxis, P4Merge, FileMerge, or git config default); per-hunk **Revert** button in hunk view; patch create/apply [doc, Sep 2026]. No documented `--ignore-space-change`; a feedback request is "under consideration".
→ `git diff` flags: `-w` / `--ignore-space-change` / `--ignore-blank-lines`, `-U<n>` or `--function-context` for context, `-M`/`-C` for renames, `--word-diff=porcelain` (or compute intra-line highlights client-side, which is better for split view). Image diff: `git show <rev>:<path>` → data URL. Binary: `--numstat` shows `-`. External tool: `git difftool -y --no-prompt <a> <b> -- <path>`. Patches: `git diff --binary > x.patch`, `git format-patch`, apply with `git apply --3way` or `git am -3`. UTF-16 files: check `working-tree-encoding` in .gitattributes, else treat as binary (GitKraken shipped a UTF-16 bug fix in 12.3).

### 2.10 Merge conflict editor
- Current branch on the left, target on the right, output at the bottom. Choose lines with checkboxes or "+", navigate with arrows, edit the output directly, search with Ctrl/Cmd+F, "Take current" / "Take incoming" per file [doc, Mar 2026]. **The output editor is paid** (Public Repos Only on free). **Auto-resolve with AI** (preview since 11.2, paid) gives per-hunk explanations and confidence. 12.6.1 made it faster with many added/deleted files. External merge tools are supported. The docs don't mention a base/ancestor pane [inferred: 2 inputs + output, not a full 3-way display].
→ Conflicted paths: `git status --porcelain=v2 -z` (`u` entries) or `git ls-files -u -z`. Versions: `git show :1:<p>` (base), `:2:` (ours), `:3:` (theirs). Chunking: `git merge-file -p --diff3 <ours> <base> <theirs>` on temp files, or `git merge-file --object-id` (≥2.43) with no temp files. Whole-file take: `git checkout --ours|--theirs -- <p>`. Re-create markers: `git checkout --conflict=zdiff3 -- <p>`. Mark resolved: `git add <p>`. Then `git merge|rebase|cherry-pick|revert --continue` with `GIT_EDITOR=true`, or `--abort`. Detect the operation in progress from `MERGE_HEAD`, `rebase-merge/`, `rebase-apply/`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`. Optionally turn on `rerere.enabled`. Show a real 3-way view (base pane toggle) for free.

### 2.11 Blame & file history
- History/Blame buttons in the file diff; blame colours lines by author and shows author/date annotations; "Search file to view history and blame" (Ctrl/Cmd+Shift+H) [doc]. *Public Repos Only on free.*
→ Blame: `git blame --porcelain -M -C -w [--ignore-revs-file=.git-blame-ignore-revs] <rev> -- <p>` (`--incremental` for streaming big files). History: `git log --follow --format=... --name-status -- <p>`. Line history: `git log -L <a>,<b>:<p>`.

### 2.12 Comparison / diff between refs (user priority; see section 3 for detail)
- GitKraken Desktop: select two commits to compare them directly; WIP vs any commit or branch (since 9.7.0); combined consecutive range (12.6.0); "View changes in PR" for existing PRs (12.4.1); AI "Explain Branch Changes" (base commit..HEAD, paid). No reviewed-file tracking in Desktop. In-app PR review was replaced by gitkraken.dev Code Review in 12.3.0.
→ `git diff --merge-base A B` (≥2.30) == `git diff A...B`; `git log A..B`; `git rev-list --left-right --count A...B`; `git merge-tree --write-tree --name-only A B` (≥2.38) for conflict prediction.

### 2.13 Worktrees
- Create from a branch, open in the current or a new tab, lock/unlock, remove, "Remove worktree and delete branch" (12.1); WIP rows per worktree in the graph; worktree tab groups (12.4.1); worktrees inherit hidden/soloed refs [doc, Sep 2026]. *Public Repos Only on free.*
→ `git worktree list --porcelain -z` (≥2.36 for `-z`), `git worktree add [-b <new>] <path> <base>`, `remove [--force]`, `lock --reason`, `unlock`, `prune`. Per-worktree status by running `git status` inside each path.

### 2.14 Submodules
- Add (URL + path) from the Left Panel, handle out-of-sync submodules (stash/commit/discard), automatic update setting global or per repo [doc, Jan 2026].
→ `git submodule status --recursive`, `git submodule update --init --recursive`, `git submodule add <url> <path>`, `git submodule sync`, `git diff --submodule=log` for readable pointer diffs.

### 2.15 Git LFS
- Initialize LFS, track patterns (Preferences or right-click), LFS badge in the commit panel, toolbar LFS pull/prune (prune flagged destructive), automatic LFS pull after clone [doc, Mar 2026]. No locks UI documented. *Public Repos Only on free.*
→ `git lfs install`, `git lfs track "<glob>"` (edits .gitattributes), `git lfs ls-files`, `git lfs pull`, `git lfs prune` (confirm), `git lfs locks` / `lock` / `unlock`. Detect pointer files in diffs (`version https://git-lfs.github.com/spec/v1`).

### 2.16 Tags / releases
- Create tag or annotated tag (Command Palette), Tags section, sign tags by default [doc]. No host "Release" creation is documented in Desktop.
→ `git tag -a <t> -m <msg> [<sha>]` (`-s` to sign), `git push <remote> refs/tags/<t>`, `git push <remote> --delete <t>`, `git tag -d`, `git ls-remote --tags`. Releases (beyond GitKraken): GitHub `POST /repos/{o}/{r}/releases` with `generate_release_notes:true`; GitLab `POST /projects/:id/releases`.

### 2.17 Commit signing (GPG / SSH)
- "Sign Commits/Tags by Default"; generate a GPG key in-app; SSH signing via gpg.format=ssh with allowed_signers (needs the Git Executable setting); signature badge plus details on hover in the commit panel [doc, Mar 2026]. Not paid.
→ We already use the user's git, so signing works once configured. Add a settings UI writing `commit.gpgsign`, `tag.gpgSign`, `gpg.format`, `user.signingkey`, `gpg.ssh.allowedSignersFile`. Verify with `%G?`. Risk: pinentry/passphrase prompts hanging a non-TTY child. Set `GIT_TERMINAL_PROMPT=0`, use timeouts, rely on the GUI pinentry of gpg-agent (Gpg4win on Windows).

### 2.18 Hooks
- pre-commit, prepare-commit-msg, commit-msg, post-commit, pre-rebase, post-checkout, post-merge, post-rewrite, pre-push; per-repo hooks folder setting; respects `core.hooksPath`; "Skip Git hooks" checkbox [doc, Mar 2026].
→ Already true for us. Add a skip-hooks toggle with a warning, a hook output panel (stream stderr), and the existing secret-scan hook installer.

### 2.19 AI features (all paid on GitKraken)
- AI commit message (GA); **Commit Composer** (preview; splits WIP or recomposes a commit range; new engine 12.5 keeps authors and adds co-authors; lockfiles excluded from the token budget); Explain commit(s); **Explain Branch Changes** (base commit → HEAD); PR title + description (uses PR template); Auto-resolve conflicts (preview); stash message; amend message; custom prompts per feature [doc, Sep 2026]. Default provider is Gemini hosted by GitKraken. BYO key: OpenAI, Azure, Anthropic, Gemini, or a Custom URL (Ollama/LM Studio/OpenRouter allowed but "not validated"). Weekly credits by tier (1M/2M/3M/4M) [pricing].
→ Route everything through our devall-ai crate with local models free and BYO cloud keys optional (per project policy: current models only). Inputs: `git diff --cached -- . ':(exclude)*.lock' ':(exclude)package-lock.json'` etc., truncated by token budget. MR description: `git log --format=%s%n%b <base>..<head>` + `git diff --stat --merge-base <base> <head>`. Composer: model proposes hunk groups → apply each with `git apply --cached` + commit on a temp branch, user confirms, undo journal covers it.

### 2.20 Integrations (GitHub / GitLab / Bitbucket / Azure DevOps)
- PRs/MRs: list in the Left Panel with CI/review status icons and filters; create by drag or right-click "Start a pull request" or + in the panel, with templates (GitHub/GitLab/Azure), draft (GitHub), reviewers/assignees/labels, and a conflict warning on create; **stacked PRs** (GitHub.com, 12.6.0); GitHub PR view: edit metadata, "Review Code and Suggest Changes" (Code Suggestions posted as comments), "Review on GitKraken.dev" (browser review with AI, approve/comment), merge (merge/squash/rebase), check out the PR branch by double-click; merged-PR pills (12.1.2); "View changes in PR" (12.4.1) [doc, Sep 2026]. Issues: GitHub, GitLab, Jira (Cloud view-only on some tiers, DC), Trello, Azure. GitHub Actions workflow create/manage page exists. *Free tier: public repos only; self-hosted needs Advanced.* Since 12.3.0, sign-in and integrations go through a gitkraken.dev account.
→ Provider detection from `git remote get-url <r>` (github.com / gitlab.* / bitbucket.org / dev.azure.com, plus user-configured self-hosted hosts, free). Create: GitHub `POST /repos/{o}/{r}/pulls` {title, head, base, body, draft}, then `POST .../pulls/{n}/requested_reviewers` and `POST .../issues/{n}/labels`; GitLab `POST /projects/:id/merge_requests` {source_branch, target_branch, title, description, draft/"Draft:" prefix, reviewer_ids, labels, remove_source_branch, squash}; Bitbucket Cloud `POST /2.0/repositories/{ws}/{slug}/pullrequests`; Azure DevOps `POST https://dev.azure.com/{org}/{proj}/_apis/git/repositories/{repo}/pullrequests?api-version=7.1`. Fallback to `gh pr create --base --head --title --body-file [--draft]` / `glab mr create --source-branch --target-branch --title --description [--draft]` when installed. Tokens go in the OS keychain (`keyring` crate: Windows Credential Manager, macOS Keychain, Secret Service), never in devall.yaml or logs; optionally import from `gh auth token`. Check out a PR: `git fetch <remote> pull/<n>/head:pr/<n>` (GitHub) or `merge-requests/<iid>/head` (GitLab), or into a worktree. Templates: `.github/pull_request_template.md`, `.github/PULL_REQUEST_TEMPLATE/*.md`, `docs/`, root; `.gitlab/merge_request_templates/*.md`; Azure `.azuredevops/pull_request_template.md`.

### 2.21 Launchpad
- A unified inbox of PRs (GitHub, GitLab, Bitbucket, Azure), issues (Jira, GitHub, Azure, Trello) and WIPs across a workspace. Groups such as ready to merge / no reviewers / merge conflicts appear in a status bar; filters; pin, snooze, open, check out (optionally into a worktree), start review (GitHub); Team view; start an agent session from an issue (12.4.0) [doc, Apr 2026]. *Personal: Pro. Team view: Advanced. Snooze: paid.*
→ Per project (all repos in it), use the GitHub search API `is:pr is:open review-requested:@me` / `author:@me` (or GraphQL), GitLab `GET /merge_requests?scope=...&state=opened`, Azure `pullrequests?searchCriteria...`. Keep pin/snooze state locally. WIP rows from our existing `git_overview`.

### 2.22 Workspaces (multi-repo; compare only, the other agent owns this)
- Local Workspaces (machine only; add a folder, a directory or a VS Code .code-workspace file; "Sync with local directory") vs Cloud Workspaces (stored in the GitKraken account, need a hosting integration, shareable with teams; Shared Cloud Workspaces is Pro). Bulk Clone/Fetch/Pull/Open/Locate/Remove; workspace Launchpad; Insights [doc, Mar 2026]. `gk ws` CLI: list/clone/fetch/pull/push/checkout/locate.
→ Our `repos:` in devall.yaml already gives a committed, shareable "workspace" for free through git itself. Gaps worth noting to the multi-repo agent: a per-repo table with branch / ahead-behind / PR / WIP columns, workspace-wide PR inbox, bulk clone of missing repos, a "locate" flow (`repos_locate` exists).

### 2.23 Cloud Patches
- Share staged changes or a commit as a hosted patch with a link; access by link / org / selected users; recipients apply it to a chosen branch; GitKraken storage or self-hosted S3 [doc, Mar 2026]. Preview; tier not stated; storage options are admin/Business. Conflict Prevention can suggest "share as Cloud Patch".
→ Local-first equivalent: `git diff --binary` / `git format-patch --stdout` / `git bundle create` to a file or clipboard; apply with `git apply --3way` or `git am -3`. An optional hosted link via our relay would be the only paid-at-cost candidate.

### 2.24 Terminal / CLI / agents
- Embedded terminal per repo/worktree with multiple tabs (12.6.0), drag-and-drop paths into it; `gitkraken .` / `gitkraken <path>` launcher (12.1.0); **Agent Sessions** (12.0+): worktree cards running Claude Code / Codex / Copilot CLI / Cursor CLI / Gemini CLI / OpenCode, live status, allow/deny permission prompts on the card, setup commands per repo [doc, Sep 2026] (Public Repos Only on free). `gk` CLI (free): `gk work start|commit [--ai]|push|pr create [--ai]|end`, `gk ws ...`, `gk graph`, git passthrough, plus an MCP server for agents [gitkraken.com/cli].
→ We already have devall-cli and devall-mcp. Later: add `devall git compare <base>..<head>` and MCP tools that expose the compare/review data (P2).

### 2.25 Profiles
- Separate git identity, integrations, preferences and tabs per profile; *creating or switching profiles is paid* [doc, Mar 2026].
→ Free: per-repo identity via `git config user.name/user.email` (local), a suggestion to add `includeIf "gitdir:<dir>/"` blocks to global config, multiple provider accounts in the keychain keyed by host + account.

### 2.26 Keyboard shortcuts / command palette
- Ctrl/Cmd+P palette (Repo, Settings, View, History, Core, File, Stash, Branch, Checkout, Patch, Logs categories) [doc, Apr 2026]. Shortcuts: Ctrl+/ list, Ctrl+B branch, Ctrl+L fetch all, S/U stage/unstage file, Ctrl+Shift+S/U all, Ctrl+Enter commit, Ctrl+Shift+Enter stage-all-and-commit, Ctrl+Shift+M focus message, J/K/H/L navigation, Shift+J/K within branch, Ctrl+F search commits, Ctrl+Shift+H file history/blame, Ctrl+D open diff/merge tool, Alt+T terminal, Ctrl+J/K toggle panels, Ctrl+Z/Y undo/redo, Ctrl+T/W tabs, Ctrl+1–9 [doc, Apr 2026].
→ Register git commands in our app palette and adopt the same bindings where they don't clash.

### 2.27 Other
- **Conflict Prevention** (Pro+): alerts on overlapping committed-but-unmerged edits vs the target branch and vs Org members' branches; suggests push/merge/rebase/copy summary/share Cloud Patch; per-repo base-branch globs [doc, Apr 2026]. → `git merge-tree --write-tree --name-only --no-messages <target> HEAD` (exit 1 = conflicts) and, for teammates, the intersection of `git diff --name-only <mb>..<their-branch>` with ours, run on remote branches updated in the last N days.
- **Gitflow** (Public Repos Only on free). → Thin wrappers over branch create/merge using naming conventions; P2.
- **Code editor** (edit/save files in-app; Public Repos Only on free). → Out of scope; open in the user's editor.
- **Repo maintenance** palette command → `git maintenance run` / `git gc`.
- **Shallow clone** options (11.10/12.0). → `git clone --depth N --filter=blob:none`.

---

## 3. Deep dive: comparing a branch to another before an MR

### 3.1 What GitKraken Desktop actually offers (as of 12.6.1)
1. **Select two commits / branch tips (Ctrl/Cmd+click)** → "compare them directly" [doc: diff page, Sep 2026]. This is a tree-to-tree diff, i.e. `git diff A B` semantics [inferred from "directly"; there is no merge-base option in the docs]. Comparing `main` and `feature` this way shows main's newer commits as reversed changes, which is noise for pre-MR review.
2. **Consecutive range** (Shift-click) → combined diff of the run of commits (12.6.0). Selecting a branch's own commits approximates the MR diff, but only when they are contiguous in graph order. More than two non-consecutive picks are refused with an explanation [doc].
3. **WIP vs any commit/branch** (right-click → compare against working directory; since 9.7.0) → `git diff <ref>` (working tree vs ref, two-dot style) [doc].
4. **View changes in PR** (12.4.1) → the full PR diff. Only exists once a PR exists on the host [doc: release notes].
5. **AI Explain Branch Changes** (paid) → summarizes "the diff between a branch's base commit and HEAD", i.e. merge-base semantics, but as prose rather than a review UI [doc: AI page].
6. **Conflict Prevention** (Pro+) → warns that your branch conflicts with its target [doc].
7. **Reviewing**: the in-app PR review mode was **removed in 12.3.0** in favour of browser-based GitKraken Code Review on gitkraken.dev (AI overview, chat, moved-code detection, approve/comment) [doc: release notes 12.3.0; 12.1.0 added the "Review on GitKraken.dev" button]. No "mark file as viewed/reviewed" exists in Desktop docs. GitLens Search & Compare does have "checkboxes next to files to track review progress", plus Ahead/Behind/files-changed sections [doc: GitLens side bar views, Sep 2026].

### 3.2 Git semantics we must get right
| Intent | Command | Notes |
|---|---|---|
| **What the MR will show** (default) | `git diff --merge-base <base> <head>` == `git diff <base>...<head>` == `git diff $(git merge-base <base> <head>) <head>` | Matches GitHub/GitLab/Bitbucket "three-dot" PR diffs (GitHub docs). Stable when base moves. `--merge-base` needs Git ≥2.30. |
| Direct tip-to-tip | `git diff <base> <head>` (== `<base>..<head>` for diff) | Shows base's newer changes reversed. Useful for "what will main look like after merge" only if already up to date. |
| Including uncommitted work | `git diff $(git merge-base <base> HEAD)` (worktree incl. staged + unstaged tracked) | Untracked files are not included: list via `git ls-files --others --exclude-standard -z`, or diff them against /dev/null (`git diff --no-index`). |
| Commits in the MR | `git log --format=... <base>..<head>` | For **log**, `A..B` = commits reachable from B not from A (dots mean something different than for diff). |
| Ahead / behind | `git rev-list --left-right --count <base>...<head>` | Or `%(ahead-behind:<base>)` in for-each-ref (≥2.41). |
| Hide already-upstreamed patches | `git log --cherry-pick --right-only <base>...<head>` / `--cherry-mark` | Marks commits whose patch is already in base (e.g., cherry-picked hotfix). |
| Will it conflict? | `git merge-tree --write-tree --name-only --no-messages <base> <head>` | Git ≥2.38; doesn't touch worktree/index; exit 0 clean, 1 conflicted; prints tree OID then conflicted paths. |
| File list with stats | `git diff --merge-base -z -M --name-status <b> <h>` + `--numstat` | `--raw --no-abbrev` gives old/new **blob SHAs** per file (key for reviewed-state). |
| Per-file diff | `git diff --merge-base -M [-w] [-U<n>|--function-context] <b> <h> -- <path>` | Lazy-load per selected file. |
| What changed since my last review | `git diff <reviewedBlob> <currentBlob>` per file, or `git range-diff <oldBase>..<oldHead> <newBase>..<newHead>` after a rebase | GitLab MR "versions" and GitHub "Viewed" reset are prior art. |
| Criss-cross merges | `git merge-base --all <b> <h>` | If >1 base, `A...B` picks one and warns; surface that. |
| Unrelated histories | `git merge-base` exits 1 | Fall back to direct diff with a banner. |
| Generated files | `git check-attr linguist-generated -- <paths>` + heuristics (lockfiles, dist/, *.min.js) | Collapse by default (GitHub does the same). |

### 3.3 Reviewed-file tracking design (beats GitKraken Desktop)
- Store locally (app data, not repo) a map keyed by `repoId + baseRef + headRef + path` → `{ blobSha, reviewedAt, note? }`.
- When the file's head-side blob SHA (from `git diff --raw`) differs from the stored one, show the file as **"changed since reviewed"** (auto-unchecked, like GitHub's Viewed). Offer "show only the changes since my review" = `git diff <oldBlob> <newBlob>`.
- Progress bar "9 / 23 files reviewed", "hide reviewed" filter, keyboard `v`/space marks the file reviewed and jumps to the next unreviewed one.
- Optional local notes per file/line → "Copy review checklist" / insert into MR description.

---

## 4. Prioritized parity plan

### P0 (must have)

**P0-1 Compare & Review ("Review before MR")**
- Entry points: a "Compare" button in the repo header ("feature/x → main ▾"); right-click any branch/tag/commit → "Compare with current branch" / "Compare with…"; Ctrl-click two commits in history; a "Review N commits before MR" call to action in Changes when HEAD is ahead of its base.
- Base default: the remote default branch (`git symbolic-ref --short refs/remotes/<remote>/HEAD`, else main/master/develop), remembered per repo; head default: current branch; swap button.
- Mode switch: **"Branch changes (since merge-base)"** (default, labelled "what the MR will show") | "Direct diff (tip to tip)"; toggle "Include uncommitted changes".
- Header facts: merge-base short SHA; "main has 12 new commits since you branched"; stale-remote warning from FETCH_HEAD age with a Fetch button; conflict prediction banner from `merge-tree` ("will conflict in 3 files" with a list).
- Left pane tabs: **Files (23)** as a tree or flat list with A/M/D/R status, +/− counts, reviewed checkbox, filter, hide-reviewed, generated files collapsed. **Commits (7)**: click one or Shift-select a range to narrow the diff; patch-equivalent commits greyed out.
- Right pane: diff with unified/split, hunk-only/full-file, ignore whitespace, intra-line highlight, wrap, image side-by-side, "open in editor", blame/history links. Keys: j/k file, n/p hunk, v reviewed.
- Footer: **"Create merge request…"** → P0-6, prefilled.
- Performance: list files first, then diff on demand; cap a huge file diff at ~5k lines with "load anyway"; cancel stale requests.

**P0-2 Commit graph** (replaces the flat history list): Rust lane layout + streamed paging + virtualized canvas; ref pills (local/remote/tag/HEAD), WIP row, columns; selection → commit details; two-select → P0-1; range-select → combined diff; context menu (checkout, branch/tag here, cherry-pick, revert, reset soft/mixed/hard, compare, copy SHA); basic search (message/author/SHA).

**P0-3 Hunk & line staging + diff options** in Changes: stage/unstage/discard per hunk and per selected lines (`git apply --cached --recount`), ignore-whitespace, split/unified, word-level highlights, ignore/stop-tracking.

**P0-4 Branch/tag/remote/stash panel + safe core ops + multi-level undo**: create/rename/delete (local + remote) branches, upstream, ahead/behind, fast-forward without checkout, merge (ff/no-ff/squash), plain rebase, cherry-pick, revert, reset, tags (create annotated/push/delete), stash (push with message/staged/untracked, list, show, apply, pop, drop). Every mutating op gets a journal entry under `refs/devall/undo/*` and Ctrl+Z/Ctrl+Y.

**P0-5 Merge conflict editor**: an in-progress banner (merge/rebase/cherry-pick/revert with continue/abort/skip); per file ours | base (toggle) | theirs + an editable output; take chunk/line/all; re-create markers; mark resolved. Free (GitKraken charges for the output editor).

**P0-6 Create PR/MR** (GitHub + GitLab first, including self-hosted): title/description (template picker), target/source, draft, reviewers, labels, "push first if needed", open in browser after; tokens in the OS keychain; `gh`/`glab` fallback.

### P1
- **Interactive rebase** (visual todo list: drag to reorder, pick/reword/squash/fixup/drop/edit, autosquash, `--update-refs`), driven by `GIT_SEQUENCE_EDITOR`.
- **PR/MR inbox** (Launchpad-lite per project): mine/review-requested/assigned; CI + review state; pin/snooze locally; check out a PR branch or open it in a worktree; "Review PR locally" opens P0-1 with base = PR target.
- **Blame, file history, line history** (`git blame --porcelain`, `log --follow`, `log -L`).
- **Worktrees panel** (list/add/open/lock/remove/prune; WIP per worktree in the graph).
- **Graph power features**: file/pickaxe search, hide/solo/pin, smart branch visibility, author filter, minimap of search hits.
- **Conflict prediction vs teammates' remote branches** (GitKraken Conflict Prevention, Pro+ there, free here).
- **Review extras**: per-file/line local notes, "changes since last review", range-diff after a rebase, export checklist into the MR.
- **Local-model AI**: commit message, explain commit / explain branch changes (merge-base), MR title/description, stash message.
- **Push safety**: `--force-with-lease --force-if-includes` UI after rewrites; pull modes (ff-only/rebase/merge, autostash); remote management; fetch prune.
- **Signing settings + verification badges**; skip-hooks toggle with a secret-scan warning.
- **Command palette entries + GitKraken-compatible shortcuts**; external diff/merge tool launch (`git difftool`/`mergetool`).
- **Bitbucket + Azure DevOps** PR create/list.

### P2
- Submodules panel; Git LFS (track/locks/pull/prune with confirmation); host releases from tags; Gitflow helpers.
- Identities/profiles (free): per-repo identity, `includeIf` helper, multiple accounts per host.
- AI Commit Composer and AI conflict auto-resolve (local models, applied on a temp branch, undoable).
- Patch sharing: format-patch/bundle to file or clipboard; optional hosted link via relay (only paid-at-cost candidate).
- Agent sessions in worktrees (if it fits the app's agent story); issue trackers (Jira/Linear) → branch from issue; CI/Actions status viewer; `devall git compare` CLI + MCP tools; team insights.

### Risks and mitigations
- **Destructive operations**: discard (file/hunk/line), `reset --hard`, `branch -D`, remote branch delete, force push, stash drop/clear, rebase drop, `git clean`, worktree `remove --force`, `lfs prune`. Use a confirm dialog naming the exact effect and count, take an automatic snapshot plus a journal entry first, prefer `reset --keep` and `--force-with-lease --force-if-includes`, never gc our undo refs automatically (prune them after N days or N entries), and move untracked deletions to the OS trash.
- **Big repos**: 100k+ commits and thousands of refs. Use a single-pass Rust lane layout, streaming and paging, virtualization, an option to exclude remotes/tags from `--all`, an opt-in commit-graph file with Bloom filters, cancellable pickaxe search, capped diffs, debounced watchers, `status --untracked-files=no` / `core.untrackedCache` / fsmonitor hints.
- **Concurrency**: serialize mutating git calls per repo (index.lock); detect stale locks; refresh after the external git changes that `git_watch` already emits.
- **Git version floor**: `--merge-base` 2.30, `--force-if-includes` 2.30, `stash --staged` 2.35, `worktree list -z` 2.36, `merge-tree --write-tree` / `rebase --update-refs` 2.38, for-each-ref `ahead-behind` 2.41, `merge-file --object-id` 2.43. Detect `git --version` and degrade (e.g., fall back to `git merge-base` + `git diff`, or a temp-worktree merge for conflict checks).
- **Windows/macOS specifics**: use `-z` everywhere and `core.quotePath=false`, CRLF/autocrlf when building line patches (generate and apply with the same git so normalization is consistent; test autocrlf=true), long paths, the GCM credential prompt, GPG pinentry hangs (`GIT_TERMINAL_PROMPT=0` + timeouts), hooks needing Git for Windows `sh`.
- **Semantics pitfalls**: multiple merge bases, unrelated histories, a stale remote base (offer fetch), rename limits (`diff.renameLimit`), binary/LFS pointers, UTF-16.
- **Security**: tokens only in the keychain; never in URLs, logs or devall.yaml; scrub credentials embedded in remote URLs before display; respect the user's credential helper for git transport.

---

## 5. Sources (accessed 2026-10-24)

Official GitKraken:
- Release notes, current (12.0.0 2026-04-14 … 12.6.1 2026-10-20): https://help.gitkraken.com/release-notes/current/ (redirect from support.gitkraken.com)
- Version 11 notes (11.8.0 2026-01-14, 11.10.0 2026-03-03): https://help.gitkraken.com/gitkraken-desktop/11x/
- Pricing (tiers, seats, AI credits, "Public Repos Only" rows; amounts not rendered): https://www.gitkraken.com/pricing
- Diff, file history, blame (updated Sep 2026): https://help.gitkraken.com/gitkraken-desktop/diff/
- Branch, merge, rebase (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/branching-and-merging/
- Interactive rebase (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/interactive-rebase/
- Committing changes (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/commits/
- Stage/unstage/ignore (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/staging/
- Undo/redo (Mar 2026): https://help.gitkraken.com/gitkraken-client/undo-and-redo
- Worktrees (Sep 2026): https://help.gitkraken.com/gitkraken-desktop/worktrees/
- Coding agents / Agent Sessions (Sep 2026): https://help.gitkraken.com/gitkraken-desktop/agents/
- GitKraken AI overview (Sep 2026): https://help.gitkraken.com/gitkraken-desktop/gkd-gitkraken-ai/
- Pull requests (Sep 2026): https://help.gitkraken.com/gitkraken-client/pull-requests/
- GitHub integration (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/github-gitkraken-desktop/
- Launchpad (Apr 2026): https://help.gitkraken.com/gitkraken-desktop/gitkraken-launchpad/
- Workspaces (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/workspaces/
- Experimental: Cloud Patches, Git Executable (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/experimental-features/
- Conflict Prevention (Apr 2026): https://help.gitkraken.com/gitkraken-desktop/conflict-prevention/
- Command palette (Apr 2026): https://help.gitkraken.com/gitkraken-desktop/command-palette/
- Keyboard shortcuts (Apr 2026): https://help.gitkraken.com/gitkraken-desktop/keyboard-shortcuts/
- Interface (May 2026): https://help.gitkraken.com/start-here/interface
- Hiding & soloing (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/hiding-and-soloing/
- Preferences (graph commit limits, Jan 2026): https://help.gitkraken.com/start-here/preferences/
- Profiles (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/profiles/
- Commit signing GPG/SSH (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/commit-signing-with-gpg/
- Git hooks (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/githooks/
- Git LFS (Mar 2026): https://help.gitkraken.com/gitkraken-desktop/git-lfs/
- Stashing: https://help.gitkraken.com/gitkraken-desktop/stashing ; Submodules (Jan 2026): https://support.gitkraken.com/repositories/submodules
- GitKraken CLI (free; work items, MCP): https://www.gitkraken.com/cli
- GitLens side bar views, Search & Compare with review checkboxes (Sep 2026): https://help.gitkraken.com/?p=1291
- Doc index: https://help.gitkraken.com/llms.txt
- Feedback: ignore space change request (under consideration): https://feedback.gitkraken.com/suggestions/289877/ignore-white-space-option-to-ignore-space-changes-in-middle-of-the-line
- Blog: Desktop 11.2 AI merge conflicts (2025): https://www.gitkraken.com/blog/gitkraken-desktop-11-2-merge-conflicts-meet-ai-and-more-dev-quality-of-life-wins

Semantics reference:
- GitHub Docs, "About comparing branches in pull requests" (two-dot vs three-dot): https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/proposing-changes-to-your-work-with-pull-requests/about-comparing-branches-in-pull-requests

Third-party (prices; treat as approximate):
- Capterra GitKraken Desktop 2026 (Pro $4, Advanced $12): https://www.capterra.com/p/10035742/GitKraken-Desktop/reviews/
- thesoftwarescout, "GitKraken Pricing 2026" (2026-05-24; uses outdated tier names, Pro $4.95): https://thesoftwarescout.com/gitkraken-pricing-2026-plans-costs-is-it-really-worth-it/
- toolradar GitKraken pricing: https://toolradar.com/tools/gitkraken/pricing
- ComponentSource release mirror (9.7.0 WIP compare; 12.3.0): https://www.componentsource.com/product/gitkraken/releases

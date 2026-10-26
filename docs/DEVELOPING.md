# Developing /dev/all — conventions

Read this before adding a module. It keeps the many modules consistent and lets several people (or
agents) work in parallel without stepping on each other.

## Ground rules

- **Free & local first.** Anything that runs on the user's machine is free with no account (Group 1 in
  `FEATURES.md`). Only hosted features (relay) are paid. Never paywall local features.
- **Platforms:** macOS and Windows are first-class; Linux best-effort. No Windows-only or macOS-only
  code paths without the other platform's equivalent. Use `std::path`, never hard-coded separators.
- **Secrets never land in project files.** Files reference `{{secret.NAME}}`; values come from the OS
  keychain (`devall_core::SecretResolver`). Redact secret values from anything persisted or exported
  (`Resolver::redact`, `vars::redact_values`).
- **Project files are the source of truth**; local state (history, caches, run summaries) goes in the
  project's data folder on the device (`devall_core::local::project_dir`, see "Local data & history")
  — never inside the repository. Large/binary data never goes into tracked files.
- **No arbitrary code from repository files.** Opening a project never runs its code. Code that
  ships in a repo (notebook cells, Markdown code blocks, project plugins) runs only sandboxed and
  after a per-machine trust decision pinned to a hash of what would run (`devall-notebook::trust`,
  `devall-plugins`); approvals live in the app data folder, never in the project.
- **No telemetry**, no network calls the user didn't trigger.
- **AI:** never recommend/ship Qwen2.5-family models; see the research report for the current lineup.

## Layout

```
crates/<name>/            one crate per module: pure Rust, no Tauri dependency, unit-tested
apps/desktop/src-tauri/src/modules/<name>.rs   thin Tauri commands + module state for that crate
apps/desktop/src/lib/api/<name>.ts             typed invoke() wrappers + TS mirror types
apps/desktop/src/features/<name>/              React UI for the module
docs/                     plan, features, this file, guides (MOCKS_IN_CI, LOAD_AGENTS, RELEASING)
schemas/                  JSON Schemas generated from devall-core's model
sdk/rust/devall-plugin/   plugin author SDK (not a workspace member; builds for wasm32)
examples/plugins/         example plugins (not workspace members)
```

Project folder layout (each module owns one top-level folder):

| Folder | Owner | Contents |
|---|---|---|
| `http/` | HTTP client + realtime | `*.req.yaml`, `*.ws.yaml`, `*.grpc.yaml`, `*.mqtt.yaml`, `*.sio.yaml`, `_folder.yaml` (may link the folder to an OpenAPI spec: `openapi: { source, autoSync }`); response snapshots in `__snapshots__/**/*.snap.json` |
| `environments/` | core | `*.env.yaml` |
| `mocks/` | mock server | WireMock layout: `mappings/*.json`, `__files/`; stub data sources in `data/` (`*.csv`, `*.json`, SQLite `*.db`/`*.sqlite`); `graphql/` schemas (`*.graphql` SDL, introspection `*.json`, `<name>/` multi-file, optional `config.json`); `websocket/*.json` message stubs; `grpc/` `*.proto` + stub `*.json`; `mqtt/*.json` broker behaviours |
| `plugins/` | plugins | project plugins `<id>/plugin.yaml` + `plugin.wasm` — never run until each user trusts them (pinned to the module's SHA-256); see `PLUGINS.md` |
| `openapi/` | OpenAPI tools (convention) | API specs (`*.yaml`/`*.json`); lint, drift checks and linked folders find specs anywhere outside the module folders |
| `load/` | load testing | `*.load.yaml` test definitions (run summaries are local data; older `runs/<test>/*.json` are still read) |
| `data/` | fake data | `*.data.yaml` generator schemas |
| `webhooks/` | webhooks | saved events/fixtures `*.webhook.yaml` |
| `connections/` | DB/infra | `*.conn.yaml` (Postgres, MySQL, SQLite, SQL Server, Redis, MongoDB; no secrets — passwords are `{{secret.X}}`) |
| `queries/` | DB clients | saved queries (`*.sql`, `*.redis`, `*.mongo.json`) |
| `notes/` | notes | shared Markdown notes (`**/*.md`), runnable notebooks (`*.ipynb`) and the images notes show (`notes_asset`) |
| `utilities/` | utilities | saved regexes with test cases (`regex/*.regex.yaml`) |
| `.devall/` | legacy | where older versions kept local state; moved to the project data folder on open (still git-ignored for old clones) |

These are the **content folders** (`devall_core::project::CONTENT_DIRS`). Project-wide walks (secret scan,
"where used", local file history, OpenAPI spec search, AI index) and the file watcher stay inside them (plus
`devall.yaml`) and never descend into nested git repositories — a multi-repo project's folder may hold several
repos' code, `node_modules` and build output.

### Multi-repo projects

A project can span several git repositories (frontends and backends sharing requests, environments, mocks and
notes). `devall.yaml` lists them — `repos: [{ name, path, remote?, group? }]`, `path` always relative to the project
folder (`web`, `frontend/admin`, `../api`), never absolute. `devall_core::repos` resolves them on this machine
(`Project::repos()` / `Project::repo(name)`): listed repos (found / missing / not a repo; a machine-specific location
from `<project data>/repos.json` wins), the repo containing the project folder (the single-repo case: nothing to list,
behaves as before) and unlisted nested repos (depth ≤ 3, "discovered"). `repos::git` runs the user's git per repo
(status/summary/branches/switch/clone, bounded parallelism) for the app, `devall repos` and MCP `list_repos`.

- **Link syntax:** `repo:<name>/<path>` — a file or folder in a repo, wherever it is on this machine. Used by note and
  to-do links (revealed in Explorer/Finder), OpenAPI spec sources and pickers, gRPC `.proto` files/import paths and
  notebook working directories. Store picked files with `repos_portable` (`repo:…` inside a listed repo, else
  project-relative, else absolute). Resolve with `repos::resolve_link` (can't climb out of the repo).
- **Security:** commands that act on a repository take its *name* (`repo`) and resolve the folder in Rust against the
  project's repo list — the renderer never passes a path to run git in. Clone, bulk pull and branch switches run
  only on explicit, confirmed user actions.
- **UI:** Source control (side pane of repos by group, "All repositories" overview, bulk fetch/pull/switch branch,
  per-repo changes with hunk/line staging, commit graph and Compare & review), Repositories dialog (project menu, Projects page, Source control, palette), the new/open
  project flow (offers nested repos), Docker (Compose files in repos), Logs (open files from a repo).
- **Local-only repos:** a worktree opened from Source control is listed in `<project data>/local-repos.json`
  (`[{ name, path }]`, absolute paths, never in `devall.yaml`) and resolves with `origin: local` (`repos::add_local_repo`,
  `remove_local_repo`; `Project::repos()` appends them).

### Source control operations (`devall_core::repos`)

| Module | What |
|---|---|
| `git` | the user's `git` (`GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, `core.quotePath=false`), `mutate` (one writer per repo, waits for `index.lock`), `run_env` / `run_input_env` (temporary index, editors), `git_path` |
| `ops` | refs (upstream ↑↓, gone, worktree, annotated), branches (create/rename/delete/unmerged count/remote delete/upstream/fast-forward without checkout/move), tags (+ push / remote delete), checkout, merge (ff/no-ff/ff-only/squash), rebase, reset (soft/mixed/keep/hard), cherry-pick, revert, stage/unstage/discard/commit/pull, push (normal / set-upstream / `--force-with-lease --force-if-includes`), paused operation (continue/skip/abort); `scrub` removes credentials from network output |
| `undo` | the journal (below) |
| `stash`, `remotes`, `worktree` | stash list/save/apply/pop/drop/files (entries addressed by index **and** oid); remotes list (URLs credential-free)/add/rename/set-url/remove/prune/fetch; worktrees list/add/lock/unlock/remove/prune (paths checked against `git worktree list`) |
| `conflicts` | conflicted files (`ls-files -u`), stages `:1:`/`:2:`/`:3:` + the working-tree file, labels per operation (ours/theirs swap in a rebase), save (+ `git add`), take a side, resolve as deleted, re-create markers (`--conflict=zdiff3`) |
| `rebase` | interactive rebase from a plan: `GIT_SEQUENCE_EDITOR="cp '<plan>'"` (git's own shell on every platform) and `exec git commit --amend -F <file>` lines for new messages (no editor ever opens); `--update-refs` lines computed; autosquash ordering; plans in `<app data>/git-rebase/<id>/` (cleaned after a week) |
| `blame` | `git blame --porcelain -M -C -w` (+ `.git-blame-ignore-revs`), `git log --follow` file history with each version's path and parent |

Every local mutation goes through `undo::journaled(dir, journal, Op { kind, label, scope }, f)` (the desktop passes the
repo's journal; the CLI and tests pass `None`). Network operations (push, fetch, prune, remote branch/tag delete) are
never journaled, run only on explicit confirmed actions, and their output is scrubbed of URL credentials.

**Undo journal.** `<project data>/git-undo.json` (`{ version, repos: { <repo name>: { undo: [...], redo: [...] } } }`,
≤ 50 entries per repo, ≤ 30 days). Each entry records the state before and after the operation, as far as its `Scope`
reaches: HEAD (symbolic or detached), the branch/tag refs that changed, branch upstreams, the index tree, a working-tree
snapshot (tracked + untracked, not ignored, untracked files > 50 MB skipped — written as a tree through a temporary copy
of the index, so the real index and files are untouched) and the stash list. Snapshot trees are anchored by
`refs/devall/undo/<journal key>/<entry>/s` (a tree ref: invisible to `git log --all`, excluded from the graph and its
signature) and commits that would otherwise become unreachable (an amended commit, a deleted branch's tip, a dropped stash,
a deleted annotated tag) by `…/<entry>/k-<oid>`; dropped entries delete their refs, orphans are cleaned when the journal
prunes. Undo restores only what the entry covers (`update-ref --stdin`, `symbolic-ref`, `update-index --index-info` for
the index delta, `checkout-index` from a temporary index for files, the OS trash for files the operation created, `stash
store` to rebuild the stash list); when the repository changed since it returns `Confirm` with the reasons (forcing makes
what it replaces the redo step); commits pushed after the operation are `Refused`. Operations run while a merge/rebase/
cherry-pick/revert is paused aren't journaled on their own: the operation is one pending entry, completed (or dropped on
abort) when it finishes, also when finished outside the app.

**Pull / merge requests** (`crates/devall-forge`): `detect(remote url, accounts)` → GitHub (github.com, `*github*`
hosts = Enterprise, API `/api/v3`) or GitLab (`*gitlab*` hosts, API `/api/v4`), other hosts once connected as one;
`create` (GitHub `POST /repos/{o}/{r}/pulls` + reviewers + labels; GitLab `POST /projects/:id/merge_requests` with
`reviewer_ids` looked up by name, `Draft:` prefix), `whoami` (checks a token), `templates`, `cli` (`gh pr create` /
`glab mr create`). Tokens live only in the OS keychain (`SecretScope("devall-forge", "*")`, entry = host); the list of
connected hosts (no tokens) is `<app data>/git-forges.json`; errors are cleaned of the token. Network only on Connect and
Create. Tests use a local mock HTTP server.

### Links between items (`devall:`)

Notes, to-dos (links, descriptions, comments), AI answers and agents link to anything in the app with one format,
`devall:<kind>/<id>`, readable as plain Markdown: `[Create order](devall:request/orders/create.req.yaml)`.

| Kind | Id | Example |
|---|---|---|
| `request` | path under `http/` (also `.ws/.grpc/.mqtt/.sio.yaml`) | `devall:request/orders/create.req.yaml` |
| `folder` | request folder under `http/` (opens its settings) | `devall:folder/orders` |
| `note` | path under `notes/` (`.md`, `.ipynb`) | `devall:note/design/auth.md` |
| `todo` | personal task id | `devall:todo/01JC8Z6Q3W7Y2V5T9R4N0M1K8H` |
| `env` / `variables` | environment id (file stem) / the project variables (no id) | `devall:env/staging`, `devall:variables` |
| `stub` | mock stub id (on the Mock server page's active server) | `devall:stub/7f3c…` |
| `load`, `fakedata`, `query`, `connection` | path under `load/`, `data/`, `queries/`, `connections/` | `devall:query/orders/top.sql` |
| `webhook` | inbox id (file stem under `webhooks/`) | `devall:webhook/stripe` |
| `utility` | op id | `devall:utility/jwt.decode` |
| `repo` | repository name (opens it in Source control; files stay `repo:<name>/<path>`) | `devall:repo/api` |
| `commit` | `<repo>/<sha>` (opens it in the commit graph) | `devall:commit/api/4ed566a` |
| `compare` | `<repo>/<base>...<head>` — a branch comparison (`...` since the head branched off, `..` tip to tip) | `devall:compare/web/origin/main...feature/x` |
| `page` | any page: `<section>[/<param>]` (the generic fallback) | `devall:page/settings/sidebar` |

- **Syntax:** each `/`-separated id segment is percent-encoded where Markdown needs it (space, `(`, `)`, `#`, `?`, `%`,
  non-ASCII); everything else stays readable. The scheme is case-insensitive, ids never contain `..`; a
  `?project=<devall.yaml id>` qualifier is read into `project` (Rust `Link.project`, TS `ParsedLink.project`), other
  `?…`/`#…` are ignored. A `?` inside an id is always encoded, so the first raw `?` starts the query. Rust:
  `devall_core::links` (`format`, `parse`, `parse_incoming`, `check`, `extract_targets`, `Project::note_links`);
  TypeScript mirror: `src/lib/links/syntax.ts`. The older forms keep working: project-relative paths (`http/…req.yaml`,
  `notes/…md`, relative note links), `repo:<name>/<path>`, URLs and absolute paths.
- **Registry** (`src/lib/links/`): `describe(link)` → `{ kind, title, icon, state, detail, note }` resolved live
  (renames show up; states `ok`/`missing`/`loading`/`unavailable`/`invalid`), `useLinkInfo`, `openLink(link, { toSide,
  go })`, `allLinkItems()` for the picker. Each feature registers a provider next to its code
  (`features/<name>/linkProvider.tsx`: items, resolve one id, open it — selecting the to-do, the stub, the saved
  query…), wired in `lib/links/providers.ts` (pages and repos: `lib/links/core.tsx`). Providers read their stores or a
  lazily loaded `listSource` (reloaded on file changes) and call `notifyLinks()` when that data changes. Adding a
  kind: the id rule in both `syntax` modules (+ tests), a provider, a case in `route.ts` if a page shows it.
- **UI:** `Markdown` renders `devall:` links as `LinkChip`s (kind icon — method badge, to-do circle, environment
  chip —, live title, struck through when gone, a flag such as "personal to-do — not on this machine"); click opens
  in the current group, Ctrl/Cmd+click or middle-click to the side. `pickLink()` is the "Link to…" picker (fuzzy,
  grouped, `todo:`/`note:`… narrows, `freeform` accepts URLs/paths). Markdown fields insert links with `[[`, a Link
  button and **Ctrl+Alt+K / ⌘⌥K** (Ctrl/Cmd+K is the palette): the WYSIWYG `MarkdownEditor` (as a chip at the caret,
  see "Markdown editing"), Monaco in the notes' Source view, `useTextareaLinks` for textareas. Menus offer
  `copyLinkItems(link, title)` ("Copy link", "Copy as Markdown link"); the palette has "Copy link to this page"
  (`route.ts`: `linkForRoute`). "Linked from": `BacklinksSection` (task detail, notes; collapsed to "Linked from N"
  by default, open/closed remembered in `prefs.backlinksOpen`) and `BacklinksButton` (request and environment
  editors), from `notes_links` (Rust scans every note's link targets) plus the to-do snapshot, both spellings counted
  (`lib/links/backlinks.ts`).
- **Safety:** a `devall:` link only navigates inside the app — never the webview, the browser, a shell or a file write.
  The Markdown sanitizer allows exactly one extra scheme for it.
- **Project-qualified links:** links inserted into notes and to-dos stay unqualified (relative to the project, portable
  across clones). Everything that copies a link — "Copy link", "Copy as Markdown link", the palette's "Copy (Markdown)
  link to this page", Source control's commit/comparison links — copies `…?project=<devall.yaml id>` for project items
  (`linkToShare` → `external.ts` `shareableLink`; to-dos, utilities and pages that need no project stay unqualified),
  since copied links mostly end up outside the app. Canonical forms drop the qualifier, so backlinks count both
  spellings; a link naming another project doesn't count, shows "in another project" and, clicked, opens that project
  first (same path as links from outside).
- **Links from outside the app** (a browser, Slack/Teams, mail, Markdown in VS Code/Obsidian, Win+R): the OS hands
  `devall:` URLs to the app (`tauri-plugin-deep-link`; `tauri-plugin-single-instance` makes a second launch focus the
  window — unminimized — and forward its URL, then exit). `src-tauri/src/modules/deep_link.rs` checks each URL
  (`links::parse_incoming`) and queues it until the UI takes it (`deep_link_take`, at start-up and on each `deep-link`
  event), so a cold start's link waits for React and the workspace. `lib/links/incoming.ts` checks it again
  (`checkIncomingLink`), resolves the project (`resolveLinkProject`, pure: unqualified → the open project, or ask when
  none is open; the open project's id → here; another id → the most recently opened recent project (or the scratch pad)
  with exactly that id, opened like the top bar's project switcher; else "isn't on this device's recent list" with
  "Open folder…", and the link follows once that project opens — a link never names a path) and follows it with
  `openLink`. An outside link is untrusted: ≤ 2048 characters, `devall:` only, known kinds and pages, no `.`/`..` part
  (between `/` or `:`), `|`, control characters, backslashes, drive or `~` paths, a well-formed qualifier; anything
  else is a toast. It only navigates: the routes it opens are held (`lib/links/hold.ts`), so the Databases page opens a
  saved query without connecting, utilities and recipes wait for Run and the mock server doesn't auto-start.
- **Registering the scheme:** `plugins.deep-link.desktop.schemes: ["devall"]` in tauri.conf.json gives installers
  (NSIS/WiX) the registry entries and the macOS bundle its `CFBundleURLTypes`. Windows: release builds also write
  `HKCU\Software\Classes\devall` on every start (`register_all`), so the local install (an exe copied without an
  installer) always points the scheme at the newest exe; `scripts/install-local.ps1` writes the same keys. macOS: links
  reach only a bundle LaunchServices knows — `install-local.sh` registers the installed app (`lsregister -f`) and
  unregisters the build output; `tauri dev` (no bundle) can't receive them. Linux (best effort): the bundles' `.desktop`.
- **Dev builds** (`debug_assertions`) load neither plugin: no single-instance lock (`tauri dev` would exit and focus the
  installed copy, which shares the identifier) and no scheme registration (the dev exe would take links over).
  `DEVALL_DEEP_LINK_DEV=1` turns both on for testing; run the dev exe again with a URL argument
  (`target/debug/devall-desktop.exe "devall:page/settings"`) to simulate a click. A dev exe started with a single
  `devall:` argument always takes the lock, so clicks reach the running dev app while it owns the scheme (until the
  installed copy starts and registers itself again). The `local` profile is release-like.
- **Personal to-dos** live on each machine: a `devall:todo/…` link in a shared note resolves only where the task
  exists and shows "personal to-do — not on this machine" elsewhere. Tasks completed long ago (outside the snapshot)
  resolve through `todos_get`.
- **Agents:** MCP `add_todo`/`update_todo` (`links`, `addLinks`) take `devall:` links (malformed ones are rejected with
  the list of kinds); `list_requests`, `list_environments`, `list_mocks`, `list_todos` and `search_notes` return each
  item's `link`; the server instructions explain the syntax. Notes and to-do links come back unchanged.

## Local data & history

Nothing history-like is written to the repository. Everything below lives in the **app data
directory** — `%APPDATA%\dev.devall.app` (Windows), `~/Library/Application Support/dev.devall.app`
(macOS), `$XDG_DATA_HOME/dev.devall.app` (Linux) — resolved by `devall_core::local::app_data_dir()`
(the desktop app sets Tauri's `app_data_dir()` with `local::set_app_data_dir`; the CLI and MCP server
compute the same path), so the app and `devall` share it.

Per project: `<app data>/projects/<folder slug>-<12 hex of SHA-256(canonical root)>/`
(`local::project_data_dir`; the root is canonicalized and lower-cased on Windows/macOS), with a
`project.json` (`{ root, name, lastOpened }`). Stores find it with `local::project_dir(root)`:

| Path in the project data folder | Owner | Contents |
|---|---|---|
| `history.sqlite` | `devall-history::History` | every response (masked request, headers, body — bodies optional) |
| `file-history.sqlite` | `devall-history::files::FileHistory` | local file history: versions of tracked project files, content-addressed (SHA-256, deflate) |
| `events.sqlite` | `devall-logs::events` | the project activity timeline |
| `webhooks.sqlite` | `devall-webhooks::EventStore` | captured webhook events |
| `load-runs/<test>/<timestamp>.json` | `devall-load::store` | load-run summaries (run id `load-runs/…`; `devall load run --out <file>` for CI artifacts) |
| `embeddings/` | `devall-ai::semantic` | semantic search index (a cache) |
| `captures/*.har` | `devall-proxy::session` | saved inspector captures |
| `relay-inbox/<date>.jsonl` | desktop `modules::relay` | hosted-inbox fallback sink |
| `repos.json` | `devall_core::repos` | this machine's repository locations `{ "<name>": "<absolute path>" }` (multi-repo projects; never committed) |
| `git-review.json` | `devall_core::repos::review` | reviewed files of branch comparisons: repo → `<base>...<head>` → path → blob id when marked (≤ 100 comparisons per repo) |
| `git-undo.json` | `devall_core::repos::undo` | Source control's undo/redo journal per repo (≤ 50 entries, 30 days); snapshots anchored under `refs/devall/undo/*` in each repo |
| `local-repos.json` | `devall_core::repos` | repositories (worktrees) shown in this project on this machine only `[{ name, path }]` |

Device-wide: `<app data>/history/utilities.sqlite` (utility runs: input ≤ 64 KB, secret-looking and
`secret`-kind arguments masked, output preview ≤ 2 KB, no input/output for TOTP/password/bcrypt ops;
the same op + input + args within 10 min only bumps the time; ≤ `utilitiesMax` entries) and
`<app data>/history/settings.json` (`local::HistorySettings`: response bodies on/off and per-request
retention, utilities on/off and cap, file versions on/off, versions per file (50), days (180), MB per
project (200)). The CLI honours the same settings. DB query history is per project in the webview's
`localStorage` (`devall.db.history.<root>`).

**Migration.** The first `local::project_dir(root)` call in a process moves `<project>/.devall/`
{`history,events,webhooks}.sqlite` (WAL checkpointed first; leftover `-wal`/`-shm` move or go),
`embeddings/`, `relay-inbox/` and `captures/` into the project data folder unless it already has
them, then removes `.devall/` when empty. Idempotent, never fatal, copy + delete across volumes.
Legacy `load/runs/` summaries in a repo are listed and read but never moved or deleted.

**Local file history.** Tracked: `devall.yaml`, `http/**`, `environments/**`, `notes/**` (`.md`,
`.ipynb`), `mocks/**` (`__files/` bodies only up to 64 KB), `load/**/*.load.yaml`, `data/**`,
`webhooks/**`, `queries/**`, `connections/**`, `utilities/**` — never hidden paths (`.env*`, `.git/`,
`.devall/`), files over 1 MB or binary files. Opening a project scans (first sight → "initial",
differs from the latest version → "changed outside", vanished → deletion marker) and prunes; the
file watcher records every change while the app runs. Attribution comes from a process-wide
registry of recent writes: `fsutil::atomic_write` marks its writes ("Saved in /dev/all"), the MCP
server calls `local::mark_write(path, WriteSource::Agent)`, restores mark `WriteSource::Restore`;
anything else is "Changed outside (editor, git…)". App saves within 10 s merge into one version.
Restore writes through `atomic_write` and records a new version.

UI: the **History** section (Activity, Requests, Files, Utilities, Queries), "Local history…" on
requests, notes, load tests, fake-data schemas and environments, the environment editor's History
(per-variable diff), the Utilities side pane's History button, and Settings → History (switches,
retention, sizes, Clear…, Show data folder).

Personal, user-specific data (to-dos) lives outside the project in the app data directory
(`<app data>/todos/todos.json`: every list — the Inbox, one per project keyed by project id, the
user's own lists — plus sections, labels, saved filters and reminder settings; schema `version: 2`),
so it is never committed and follows the user across clones. Task attachments are copies in
`<app data>/todos/attachments/<task id>/<name>` (`devall_core::todos::attachments`: sanitized names,
≤ 50 MB each, paths always derived in Rust); deleting a task keeps its files for undo, and files
nothing has referenced for 7 days are garbage-collected at startup (cleared completed tasks and
deleted lists/sections lose theirs at once). v1 files (`todos/<project id>.json`,
`todos/_global.json`) are migrated on the first save and kept in `todos/v1-backup/`. All to-do
edits go through `devall_core::todos::TodoOp` (one implementation for the app, the CLI and tests);
natural-language dates, quick add and the filter language live in `devall_core::todos::{dates,
quickadd, filter}` — the UI calls them over IPC (`todos_parse`, `todos_parse_date`,
`todos_filter`) rather than mirroring them in TypeScript. Reminders are desktop notifications
(`tauri-plugin-notification`, sent from Rust by `modules::notes::start_reminders`) and only fire
while the app is running.
So do installed plugins (`<app data>/plugins/<id>/`) and every plugin approval
(`<app data>/plugins.json`: trusted hashes, granted secrets, disabled hooks) — a repository can't
approve its own plugins.
Git hosting accounts for pull/merge requests are device-wide: `<app data>/git-forges.json` (`[{ host, kind, user }]`,
no tokens — those are keychain entries in `devall_forge::token_scope()`, one per host). Interactive-rebase plans live
in `<app data>/git-rebase/<id>/` while a rebase runs (removed after a week).
Bring-your-own tunnel settings (ngrok domain/region/flags, cloudflared mode, Funnel port) are per
user in `<app data>/tunnels.json`; their credentials (ngrok authtoken, basic-auth password,
Cloudflare tunnel token) are keychain entries in `devall_webhooks::tunnels::config::secret_scope()`
and reach the child process only through its environment. The CLI (`devall tunnel ngrok|cloudflared|tailscale`)
reads the same file and keychain.
Local AI and MCP read notes and to-dos through `devall_core::context`.

**Statuses (board columns).** Boards group by workflow status by default. The columns are global
(every list and view): `settings.columns` (`[{ id, name, color?, wipLimit?, done? }]`, default To do /
In progress / Done) — exactly one is the done column; the first other column is where new and
unstatused tasks land. A task's `status` is a column id (missing or unknown = the first column, which
is how files from before columns load) and `statusOrder` its manual position in the column (ties fall
back to list order). Completed tasks always show in the done column and keep `status` as the column
they reopen into. `TodoOp::SetStatus` moves tasks (into the done column = complete — recurring tasks
advance and return to the first column; out of it = reopen; `before`/`after` = position);
`TaskPatch.status` does the same without a position; `TodoOp::SaveColumns` replaces the column list
(tasks of removed columns go to `reassign[id]`, default the first column; `SaveSettings` never touches
columns). Filters take `status: <name>` (`!status: …` excludes); MCP `list_todos` returns each task's
`status` (column name) and `statuses`, `update_todo` takes `status` by name. Per-view layout, grouping,
sort, sub-tasks on boards (own cards — the default in All tasks — or inside the parent's card; a
sub-task in another column than its parent always gets a card) and board colour, and the done
column's window (1/3/7 days or all)/collapsed columns/label names on cards are per-device UI state in
`localStorage` (`devall.todos.viewOptions.v2`, `devall.todos.board`).

**Labels, covers, the snapshot.** Labels have a colour (`Label.color`: palette name or `#rrggbb`); new
labels get their name's default colour (`model::default_label_color`, FNV-1a → palette, mirrored by
`defaultLabelColor` in `features/todos/lib.ts` — keep them in sync), labels without one show it too
(`Label::color_or_default`, MCP `list_todos` → `labels: [{ name, color }]` for the labels its tasks use).
`TodoOp::SaveLabel` renames/recolours, `TodoOp::MergeLabel { name, into }` merges. A task's `cover`
(`{ attachmentId }` — one of its images — or `{ color }`) is set with `TaskPatch.cover`; `NewTask.atTop`
adds a task first in its slot and column (a board's "add a card at the top"). The snapshot carries open
tasks, the last week's completions and done sub-tasks of open tasks; `olderDone` counts the completed
tasks it leaves out per list, and `todos_snapshot`/`todos_apply`/attachment commands take `allDone` to
carry all of them (the UI turns it on, for the session, from "Show"/"Load"/"All completed").

**Filter bar.** Every to-do view but Completed has one (`FilterBar.tsx`, pure model `filterModel.ts`):
text (title, description, comments, labels) plus facets — labels (any/all), priority, due (overdue /
today / next 7 days / no date / has a deadline), status, list, "has" (attachments, links, sub-tasks,
comments, description), a saved filter — matched in the UI; non-matching rows/cards hide (board
columns stay as drop targets), "n of m" with Clear, `F` focuses it, Esc clears; per view for the session
(`sessionStorage` `devall.todos.filter.v1`, so a forgotten filter never outlives a restart). "Save as
filter" turns it into the filter language (`filterToQuery`; the Rust filter has `has: …` for that).
Under the bar, notices say what a view leaves out — completed tasks, older ones not loaded, sub-tasks
under collapsed tasks or inside cards, completions outside the done column's window — each with the
one click that shows them (`Hidden` in `filterModel.ts`, published by the views as `useTodoUi.stats`).

New file types: one entity per file, YAML (WireMock mappings stay JSON for compatibility), stable key
order, `version: 1`, no timestamps/UI state, written with `devall_core::fsutil::{write_yaml, atomic_write}`.
Resolve user paths with `devall_core::fsutil::safe_join` (rejects `..`).

## Rust conventions

- Edition 2024, MSRV 1.91. Use workspace dependencies from the root `Cargo.toml` where they exist;
  new third-party deps may be declared directly in the crate's `Cargo.toml` with an exact minor
  version (check the latest with `cargo search <crate>`; read the source under
  `~/.cargo/registry/src` when unsure of an API — versions move fast).
- Errors: `thiserror` enums in libraries; Tauri commands return `Result<T, String>`.
- Serde: `#[serde(rename_all = "camelCase")]` on everything crossing IPC; tagged enums
  `#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]`.
- Async: tokio. Long-running things (servers, load tests, tails) are tasks with a cancel handle and
  stream progress to the UI with `tauri::ipc::Channel<T>` (never JSON events for high-frequency data).
  Exception: background jobs any view (or none) may need to observe — model / llama.cpp downloads —
  are owned by a backend registry (`devall_ai::download::Registry`) and broadcast low-frequency state
  as an app event (`ai://download`, ≤ 5/s); the UI subscribes once at app start and re-hydrates from a
  snapshot command (`ai_downloads`), so a view that remounts mid-job shows live progress.
- Tests: every crate has unit tests; network tests use local servers bound to `127.0.0.1:0`.
  Run `cargo test -p <crate>` and `cargo clippy -p <crate> --all-targets -- -D warnings`.
- Build parallelism is capped in `.cargo/config.toml` (memory). Don't run `tauri dev` from scripts.

## Tauri command modules

`apps/desktop/src-tauri/src/modules/<name>.rs`:

```rust
pub struct MockState { /* per-module state, e.g. running servers */ }
#[tauri::command]
pub async fn mock_start(state: tauri::State<'_, MockState>, root: String) -> Result<MockInfo, String> { … }
```

- Prefix command names with the module (`mock_`, `load_`, `data_`, …) to avoid collisions.
- Every command that touches a project takes `root: String` explicitly. Commands that act on one of its git
  repositories take the repo's name (`repo: Option<String>`) and resolve the folder with `Project::repo` — never a path
  from the renderer.
- JS calls use camelCase argument names (`invoke('mock_start', { root, serverId })`); Tauri maps them
  to snake_case Rust parameters.
- Registration (in `lib.rs`: `mod`, `.manage(State)`, `generate_handler!`) is done by whoever
  integrates the module; list what's needed at the top of the module file in a doc comment.

## UI conventions

The design system is ported from `../wiremock-ui`: `src/styles/tokens.css` (all colours/sizes),
`app.css` (components), `devall.css` (app additions). Reuse before inventing:

- Layout: `.page > .page-header + .page-body`, `.split > SidePane + .pane.fill` (see "Side panes"),
  `.toolbar`, `.subtabs/.subtab`, `.panel`, `.card`, `.list/.list-item`, `.table`, `.kv`.
- Components (`src/components/ui`): `Menu`, `useContextMenu`, `Modal`, `confirmDialog`,
  `promptDialog`, `saveChangesDialog`, `toast`, `MethodBadge`, `StatusCode`, `Spinner`, `Empty`, `Callout`,
  `Switch`, `Checkbox`, `Segmented`, `Field`, `SearchInput`, `CopyButton`, `Resizer` (`axis="x"|"y"`),
  `RelTime`, `formatBytes`, `ResponsiveTabs` (sub-tab row that never scrolls: tighter padding → icons →
  "More" menu; give every tab an `icon`; long rows such as the mock server pass `prefer="labels"` to keep
  labels and overflow into "More" instead of collapsing everything to icons).
- Layout components (`src/components/layout`): `SidePane`, `ReqResSplit` + `LayoutToggle` (request/response
  side by side or stacked by the editor's own width), `SectionIcon`.
- Utilities: paired input/output panes are `.util-pane` with a `PaneHead` (fixed height, compact
  controls) so side-by-side editors share the same top edge and height whatever buttons they hold.
- Callouts are cards: never put one flush against a pane edge. Inside a padded body (`.p-5`, `.col`,
  `.page-body`, modals) they're fine; directly in a pane use `.m-3`, or several in a `.callout-stack`.
- Editors: `CodeEditor`, `CodeDiff`, `languageFor` (`src/components/editor`). Variable-aware input:
  `VarInput` + `useVariables(path)`. Name/value tables: `features/http/KeyValueEditor`. Markdown: `MarkdownEditor`
  / `MarkdownField` (`src/components/markdown`, see below) — never a textarea or an edit/preview switch.
- State: zustand stores; the open project is `useWorkspace((s) => s.project)` (`root`, `manifest`,
  `environments`); active environment `useWorkspace.getState().activeEnv()`; navigation `useNav` (below).
- Icons: `lucide-react`. Compact, keyboard-first, both themes, no hard-coded colours (use tokens).
- Destructive actions confirm; deletes go to the OS trash (`trash` crate), never permanent.

## Markdown editing (`components/markdown`)

Markdown is edited in place (WYSIWYG) everywhere: notes, notebook Markdown cells, to-do descriptions and comments,
the Docs tab of requests and connections. The source is one toggle away (notes: "Source" = Monaco; fields: the
`</>` button = a textarea; notebook cells: "Edit Markdown source" in the cell menu), never the default. Read-only
places (AI answers, posted comments, notebook outputs) keep `components/Markdown.tsx`; its task checkboxes are
inert unless `onToggleTask` is passed (then apply the click with `toggleTaskInSource(md, index)`).

- **Use:** `<MarkdownEditor value onChange variant="document" | "field" …/>` — `document` fills and scrolls its pane
  (notes), `field` grows with its content. `<MarkdownField>` is a form field: the editor in a box with a Link button
  and the Source toggle. Props: `onProjectLink` (relative and `repo:` links), `resolveImage` (relative image →
  `blob:` URL; notes use `noteImageUrl` → `notes_asset`, images inside `notes/` only — remote images aren't loaded),
  `codeBlockExtras` (UI under a code block: notes put the ▶ Run bar there, `useRunnableCodeExtras`), `onSubmit`
  (Ctrl/Cmd+Enter), `onEscape`, `onSave` (Ctrl/Cmd+S), `onBlur`, `handleRef` (`focus`, `insertLink`, `flush`).
  **`onChange` fires only when the user changed something** (never on load), with the whole document; a `value`
  other than the last one reported reloads the editor (disk changes, the Source view). The editor is a lazy chunk;
  until it has loaded the Markdown is shown read-only.
- **Engine:** ProseMirror (the `prosemirror-*` packages, pinned) with a schema that is exactly Markdown + GFM
  (`schema.ts`), parsed with `marked` — the parser of the read-only renderer, so both agree — (`parse.ts`) and
  written back by `serialize.ts`. Milkdown (remark), TipTap's Markdown extension, Lexical's transformers and
  BlockNote all re-print the whole document in their own style; files in git need the opposite (below), which
  needs control over the schema and both directions, so the editor uses ProseMirror directly rather than a
  framework around it.
- **Round trip** (`codec.test.ts`: docs/, README, example notes and a synthetic file with every construct, LF, CRLF and
  BOM):
  1. Opening and saving without edits is byte-identical, and nothing is written unless the user edited.
  2. Every block keeps its source (`src`, plus the whitespace after it, `gap`; top-level blocks also their original
     bytes, line endings included). A block equal to what was parsed is written from it; only edited blocks are
     re-written, and the separator between two blocks that are still neighbours stays as it was. Typing in one
     block changes only that block's lines.
  3. A re-written block keeps the file's style where it can be known: bullet character, ordered delimiter and
     numbering style (`1.` `1.` `1.` stays), `[X]`, `_`/`__`/`~` markers, ATX closing hashes / setext underline, fence
     character, length and info string, link style (inline / `<angle>` / bare / `[full][ref]` / `[collapsed][]` /
     `[shortcut]`, destination and title as written), backslash escapes (`escape` mark), entities (`entity` mark),
     hard-break style, the table delimiter row (while columns and alignments match), inline HTML.
  4. A re-written block is parsed again and compared with the editor's (`semantic.ts`); when it reads back
     differently it's written again with `*` markers, then with every special character escaped.
  5. Toggling a checkbox flips only that `[ ]`/`[x]`.
  6. HTML blocks, link reference definitions, YAML front matter and anything unknown are raw source blocks, written
     verbatim (editable as text).
  Known normalisations, only inside a block the user edited: lazy continuation lines get their `>` / indentation;
  edited table rows lose column padding (`| a | b |`); an ordered list numbered neither 1-2-3 nor all-the-same is
  renumbered; leading/trailing spaces of a paragraph go (Markdown ignores them); a list item moved into a list
  with another bullet takes that bullet. The fresh writer alone (no source) reproduces the corpus except blank-line
  runs and table padding.
- **Editing:** `/` opens the block menu (text, headings, lists, task list, quote, code block, table, divider, Link to…,
  web link); a format bar floats over a selection; a bubble under a link offers open / edit / remove; a bar above a
  table adds/removes rows and columns and aligns columns. Markdown shortcuts while typing: `# `, `- `/`* `/`+ `,
  `1. `, `[ ] `/`[x] `, `> `, ` ``` `/` ```lang` + space or Enter, `---`, `**b**`, `__b__`, `*i*`, `_i_`, `` `c` ``,
  `~~s~~`. Keys: Ctrl/Cmd+B / I / E (code) / Shift+X (strike), Ctrl/Cmd+Alt+0…4 (paragraph, headings), Ctrl/Cmd+
  Shift+7 / 8 / 9 (numbered, bulleted, task list), Ctrl/Cmd+Enter toggles the task item at the caret (else leaves a
  code block, else `onSubmit`), Tab / Shift+Tab indent list items and move between table cells, Shift+Enter is a
  line break, Ctrl/Cmd+Z / Y undo/redo. Paste: Markdown text (or text from a code editor) is parsed as Markdown,
  web HTML as rich text, a URL over a selection links it; copying puts Markdown on the clipboard as text.
- **Links:** `devall:` links are `LinkChip`s (node view) and stay plain `[Title](devall:…)` in the file; click opens,
  Ctrl/Cmd+click or middle-click to the side, Enter on a selected chip opens it, right-click: open, copy, change
  link, edit text, unlink, delete. `[[` and Ctrl/Cmd+Alt+K open the picker at the caret (a selection becomes the link
  text). Other links: Ctrl/Cmd+click follows; a plain click puts the caret in the link.
- **Code blocks:** language box (with suggestions) and Copy above the code; highlighting comes from Monaco's
  `colorize` (loaded on first use, Monaco's theme colours, cached per language + code).
- **Performance:** untouched blocks are recognised by node identity, so a keystroke costs the edited block only;
  `.md-document` has `contain: strict` (a 5k-line note types at plain-`contenteditable` speed); node-view portals
  render outside the editor's per-transaction re-render.
- **Adding a construct:** a node or mark in `schema.ts` (blocks get `src`/`gap` through `block()`), parsing in
  `buildBlock`/`buildInline`, writing in `Writer.fresh`/`Inline`, meaning in `sameMeaning`/`inlineKey`, and a line in
  the synthetic test document.

## Pages, views and navigation (the workbench)

The main area is a **workbench**: up to four editor groups in a split tree (split right/down,
resizable), each with its own tab strip. A tab is a **view** — one instance of a section's page:
`{ id, section, param?, label?, pinned, preview }` (title and icon are derived: `components/workbench/viewInfo.tsx`).

- Model: `src/lib/workbench/model.ts` (pure, unit-tested: open/preview/pin/close/split/move/persist).
  Store: `src/store/workbench.ts` (`useWorkbench`: wraps the model, close guards, per-project
  persistence in `localStorage['devall.workbench:<project root>']`). UI: `src/components/workbench/`.
- Sections are declared in `src/lib/sections.ts` (`label`, `needsProject`, `singleton`, `doc`), icons
  in `components/layout/sectionIcons.tsx`, page components in `components/workbench/ViewHost.tsx`.
- Only the active tab of each group is mounted (inactive tabs unmount like a page change). Keep what
  must survive in a store, or use `useViewState(key, initial)` (per-view, survives tab switches).
- **A page renders once per view.** Read your route with `useNav` exactly as before —
  `useNav((s) => s.param)`, `useNav((s) => s.go)` — `ViewContext` (provided by each tab) makes it the
  view's own `section`/`param`, and `go()` navigates *that view*:
  - same section → updates the view's `param` in place;
  - document sections (`doc: true`: `http` = request/proto file/`<folder>/_folder.yaml`, `notes` =
    note path) → a different `param` is a different tab: focus it if open anywhere, reuse this view
    when it is a preview or empty, else open a preview tab next to it;
  - another section → focus a tab of that section (or open a preview) in this view's group; the
    current view stays as it is.
  - `go(section, param, { preview: false })` opens/keeps a permanent tab, `{ newTab: true }` always
    opens another tab, `{ toSide: true }` uses the neighbouring group (splitting when needed).
- Outside any view (top bar, palette, global shortcuts, stores, `useNav.getState()`), `go()` acts on
  the active group: focus an existing tab of the section (setting its `param` when given), otherwise
  open a preview tab. `useNav.getState().section/param` are the active view's.
- **Tab titles name what the view shows.** Document tabs describe their document; every other page
  calls `useViewLabel(label)` with its current selection (the utility, environment, log source,
  sub-page…). The tab shows it after the section's dimmed short name (`SectionMeta.short`, e.g.
  "To-dos: Priority 1", "Env: Public API"), so the label shouldn't repeat the section. Pass `null` on the
  section's landing page (the tab shows the section name) and `undefined` while the data is still
  loading (keeps the label restored with the layout). Labels are saved with the layout. When the
  selection is the view's `param` (utilities, AI and tunnels sub-tabs), it also survives tab switches
  and restarts; strip one-off values from the route after reading them (the utilities deep link
  `op|value` keeps only `op`).
- **Preview tabs** (italic) are replaced by the next preview in the same group. They become permanent
  on double-click, pin, or interaction: call `useKeepView()()` on the first edit (request edits, sends,
  live connections and note edits already do).
- **Window-level key handlers** must ignore keys while their view is inactive (two views can be
  visible side by side): `const active = useActiveViewRef(); … if (!active.current) return;`.
- **Single-instance sections** (`singleton: true`): `mocks` (its sub-navigation store is global),
  `inspector` (global capture selection/filter), `logs` (global active source), `infra` (global UI
  store), `tunnels`, `todos`, `git`, `ai`, `history`, `projects`, `settings`, `plugins`. Opening them again focuses
  the existing tab; "split" and "open to the side" move it instead of duplicating it. Other sections
  can be open several times (e.g. two requests, two connections, two utilities side by side) — keep
  their selection in the view's `param` or component state, not in a global store.
- HTTP documents (`useWorkspace().tabs`) are keyed by path: a view loads its document with
  `ensureDoc(param)`; documents no view shows are released (send cancelled, live session closed).
  `openRequest(path, opts)` / `openFolder` / `openProto` open them as tabs from anywhere. Closing the
  last view of a document with unsaved changes asks Save / Don't save / Cancel
  (`registerCloseGuard(section, fn)` for other sections that need it).
- Keys: `Ctrl+Tab` / `Ctrl+Shift+Tab` cycle tabs, `Ctrl/Cmd+W` closes (not pinned tabs),
  `Ctrl/Cmd+\` splits right, `Ctrl/Cmd+1…4` focuses a group; middle-click closes a tab; tabs and
  sidebar items can be dragged onto a group's centre (move/open) or edges (split).

### Adding a section

1. Add the id to `Section` and an entry to `SECTIONS` in `src/lib/sections.ts` (decide `needsProject`,
   `singleton` — yes if its selection lives in a global store — and `doc`). New sections appear at the
   end of the first sidebar group for users with a customised sidebar (`normalizeSidebar`).
2. Add its icon to `SECTION_ICON` (`components/layout/sectionIcons.tsx`) and its page to `PAGES`
   (`components/workbench/ViewHost.tsx`, lazy import).
3. In the page, read `useNav((s) => s.param)` for sub-routes and use `SidePane` for a left list.

### Side panes

Every page with a left list/tree uses `components/layout/SidePane.tsx` as the first child of its
`.split`: one shared, persisted, resizable width (`prefs.sidePaneWidth`, 240–560 px, default 280,
double-click the handle to reset), the same header (`title` + icon-button `actions`), an optional
`toolbar` row (search) and `footer`. In narrow views (splits) it never takes more than 45% of the
view. Empty states inside it wrap and stack their buttons; don't give side panes their own widths.
The header has a collapse button (rail with an open button; `prefs.sidePaneCollapsed`, shared by all
pages), and a pane collapses on its own while its view is narrower than 760 px (split views) until the
user opens it in that view.

### Sidebar

Order, groups and hidden sections are user preferences (`prefs.sidebar`, helpers in `lib/sections.ts`):
drag to reorder in the sidebar or Settings → Sidebar, right-click → Hide; hidden sections are listed
under "More…" and stay reachable from the palette and links.

## Verifying UI

Dev builds expose `window.__devall` (`useWorkspace`, `useNav`, `usePrefs`, `useWorkbench`). On Windows, launch with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223` and drive the real window over
CDP (screenshots, clicks, `invoke`) — see `IMPLEMENTATION_PLAN.md` §5.

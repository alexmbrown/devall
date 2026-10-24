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
| `notes/` | notes | shared Markdown notes (`**/*.md`) |
| `utilities/` | utilities | saved regexes with test cases (`regex/*.regex.yaml`) |
| `.devall/` | legacy | where older versions kept local state; moved to the project data folder on open (still git-ignored for old clones) |

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
so it is never committed and follows the user across clones. v1 files (`todos/<project id>.json`,
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
Bring-your-own tunnel settings (ngrok domain/region/flags, cloudflared mode, Funnel port) are per
user in `<app data>/tunnels.json`; their credentials (ngrok authtoken, basic-auth password,
Cloudflare tunnel token) are keychain entries in `devall_webhooks::tunnels::config::secret_scope()`
and reach the child process only through its environment. The CLI (`devall tunnel ngrok|cloudflared|tailscale`)
reads the same file and keychain.
Local AI and MCP read notes and to-dos through `devall_core::context`.

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
- Every command that touches a project takes `root: String` explicitly.
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
  `VarInput` + `useVariables(path)`. Name/value tables: `features/http/KeyValueEditor`.
- State: zustand stores; the open project is `useWorkspace((s) => s.project)` (`root`, `manifest`,
  `environments`); active environment `useWorkspace.getState().activeEnv()`; navigation `useNav` (below).
- Icons: `lucide-react`. Compact, keyboard-first, both themes, no hard-coded colours (use tokens).
- Destructive actions confirm; deletes go to the OS trash (`trash` crate), never permanent.

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
  sub-page…), worded to read on its own ("Request journal", not "Requests"). Pass `null` on the
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

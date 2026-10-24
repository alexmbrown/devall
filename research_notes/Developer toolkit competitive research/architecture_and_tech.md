# Architecture and Technology Choices for /dev/all (Rust + Tauri, local-first, git-friendly developer toolkit), as of October 2026

Coverage note for the report writer: research ran on 2026-10-21. The shared WebSearch budget ran out partway through, after about 20 searches. The rest of the evidence comes from directly fetching primary sources: Tauri docs, the Cargo.toml manifests of Yaak, GitButler and gitoxide, Bruno and Yaak docs, and pricing pages. Some sub-topics could not be researched at all, including Hoppscotch, Spacedrive, Clash Verge, Pot, Cap, Screenpipe, xterm.js, Loro, Supabase, Cloudflare, PocketBase, tantivy, gitleaks, Postman's native Git format, Insomnia and WireMock. Each one is listed under Gaps rather than filled in from memory. Sources dated before 2025 are marked "(pre-2025, possibly outdated)".

## 1. Tauri 2.x in late 2026: versions, plugins, security model, IPC, platform pain points, and lessons from real apps

### Takeaway
Tauri 2 is mature and actively maintained. The current release is 2.12.3 (Oct 2026), and 2.12 was described as "the biggest update so far in the 2.x releases". The official plugin set covers almost everything /dev/all needs (fs, shell, updater, store, sql, stronghold, deep-link, single-instance, window-state, global-shortcut, notification, http, websocket, log, process, and more). Two weaknesses remain:
- **Linux.** WebKitGTK quality varies by distro, and NVIDIA/DMABUF problems are common. The official Chromium (CEF) runtime exists only in the Tauri 3.0 alpha.
- **High-volume IPC.** The JSON-based events and command returns are slow for large payloads. Use Channels and the raw-binary `tauri::ipc::Response`/`Request` instead.

### Cited Findings
**Versions and release cadence**
- Tauri 2.12 was announced **Sep 26, 2026** as "the biggest update so far in the 2.x releases" — [Tauri blog: Announcing Tauri 2.12](https://v2.tauri.app/blog/tauri-2.12/)
- 2.12 changes:
  - Drops Windows 7, because the latest WebView2 installer crashes on it.
  - Raises the MSRV to **Rust 1.90**, under a new "stable minus three releases" policy where the MSRV changes only in minor or major releases.
  - Android: Gradle 9 and Kotlin 2 templates, minimum Gradle 8.13, targetSdk 36→37, and `$VIDEO`/`video_dir()` moved to the app-specific Movies dir (breaking).
  - Adds an `on_permission_request` handler that allows or denies webview permission requests by `PermissionKind`.
  - Adds `await using` (Explicit Resource Management) support for `@tauri-apps/plugin-fs` resources.
  - Makes dev-server loading on mobile "much faster".
  - Source: [Tauri 2.12 blog](https://v2.tauri.app/blog/tauri-2.12/)
- Search-result summaries of the 2.12.0 release notes also mention an option to limit webview navigation to app-bound domains and an `appDirectoriesOverride` config for keeping app data in one location. These were not in the fetched blog text, so verify them in the changelog — [tauri@2.12.0 release page](https://v2.tauri.app/release/tauri/v2.12.0/)
- Latest patch versions: core `tauri` crate **2.12.3**, CLI **2.12.2** and JS API **2.12.3**, all released **Oct 10, 2026**. The 2.11 line ran from 2.11.0 (Apr 30, 2026) to 2.11.5 (Jul 1, 2026) — [Tauri release index](https://v2.tauri.app/release/) (via search summary)
- Tauri 3.0 is in **alpha** (`tauri = "3.0.0-alpha.0"`), with docs hosted at v3.tauri.app — [Tauri v3 docs: CEF](https://v3.tauri.app/develop/cef/)
- A third-party tutorial claims 3.0.0-alpha.1 and alpha.2 began appearing in September 2026. This is from a single source and was not verified — [tech-insider tutorial](https://tech-insider.org/tauri-tutorial-cross-platform-rust-app-2026/)

**Official plugins (30 listed)**
- The official plugins are: Autostart, Barcode Scanner, Biometric, Clipboard, CLI, Deep Linking, Dialog, File System, Geolocation, Global Shortcut, Haptics, HTTP Client, Localhost, Logging, NFC, Notifications, Opener, OS Information, Persisted Scope, Positioner, Process, Shell, Single Instance, SQL, Store, Stronghold, Updater, Upload, Websocket, Window State. None are marked deprecated. The support table lists Rust 1.90 for plugins — [Tauri plugin index](https://v2.tauri.app/plugin/)
- The Stronghold plugin page carries **no deprecation notice**, lists windows/linux/macos/android/ios, and requires Rust ≥1.90 — [Stronghold plugin](https://v2.tauri.app/plugin/stronghold/)

**Capability and permission model**
- Capabilities are JSON or TOML files in `src-tauri/capabilities`. Each one grants or denies permissions to specific windows or webviews (by **label**), optionally limited by `platforms` (linux/macOS/windows/iOS/android).
- A window that belongs to several capabilities gets the merged permissions of all of them.
- By default, all commands registered via `invoke_handler` can be called from all windows. `AppManifest::commands` in `build.rs` restricts this.
- Remote URLs get no API access unless granted through `remote.urls`.
- Source: [Tauri capabilities](https://v2.tauri.app/security/capabilities/)
- Documented caveats:
  - "Tauri is unable to distinguish between requests from an embedded `<iframe>` and the window itself" on Linux and Android.
  - Capabilities do **not** protect against malicious or insecure Rust code, overly lax scopes, incorrect scope checks in commands, WebView 0-days, or supply-chain attacks.
  - Security boundaries depend on window labels, not titles.
  - Source: [Tauri capabilities](https://v2.tauri.app/security/capabilities/)

**IPC: commands, events and channels**
- On events: "The event system is not designed for low latency or high throughput situations". Event payloads "are always JSON strings making them not suitable for bigger messages". By contrast, "Channels are designed to be fast and deliver ordered data". Tauri uses Channels internally for download progress, child-process output and WebSocket messages — [Tauri: Calling the frontend](https://v2.tauri.app/develop/calling-frontend/)
- On command returns:
  - Normal return values are JSON-serialized, which "can slow down your application" for large data.
  - Returning `tauri::ipc::Response` delivers an **ArrayBuffer** to JS.
  - A `tauri::ipc::Request` gives access to the raw body (`InvokeBody::Raw`) and headers. The frontend sends an ArrayBuffer or Uint8Array.
  - "The Tauri channel is the recommended mechanism for streaming data". The example uses `Channel<&[u8]>` with 4096-byte chunks.
  - Async commands are preferred for heavy work. Non-async commands run on the main thread unless marked `#[tauri::command(async)]`.
  - Source: [Tauri: Calling Rust](https://v2.tauri.app/develop/calling-rust/)
- Third-party benchmark (Rust-side round trip only, excluding the WebView bridge) — [tauri-conduit](https://github.com/userFRM/tauri-conduit):

  | Path at 64 KB | Round trip |
  |---|---|
  | Stock `invoke` | ~2.27 ms |
  | conduit JSON path | ~834 µs |
  | conduit binary path | ~202 µs |

- The author of connectrpc-tauri says Tauri IPC payloads are either raw bytes or JSON, and "JSON encodes each byte as a number". They therefore stream binary protobuf frames over a `Channel` — [connectrpc-tauri](https://libraries.io/cargo/connectrpc-tauri)
- No official Tauri benchmark of Channel throughput was found — [search result summary; see tauri-conduit](https://github.com/userFRM/tauri-conduit)

**WebView differences and Linux problems**
- Per-platform WebViews — [Tauri webview versions](https://v2.tauri.app/reference/webview-versions/):
  - **Windows** uses evergreen WebView2 (Chromium), so you get "a relatively recent chromium build on all Windows targets".
  - **macOS and iOS** use WKWebView, which updates only with the OS. "unsupported macOS versions do not receive WebKit updates".
  - **Linux** uses webkit2gtk, and the version depends on the distro ("very hard to compile accurate information"). The sample table shows webkitgtk 2.36 on Ubuntu 22.04 and 2.20 on Ubuntu 18.04.
  - **Android** uses the system WebView, which varies by provider.
- Tauri's Linux graphics guide says that "most often NVIDIA" setups fail because the WebKitGTK DMABUF renderer requests buffer formats the driver lacks. The result ranges from blank windows to subtle rendering problems. Fixes in the recommended order — [Tauri Linux graphics](https://v2.tauri.app/develop/debug/linux-graphics/):
  1. Set `nvidia_drm.modeset=1` (drivers older than 545).
  2. Set `__NV_DISABLE_EXPLICIT_SYNC=1` (fixes the Wayland Error 71 crash).
  3. Set `WEBKIT_DISABLE_DMABUF_RENDERER=1` (gives up the faster rendering path).
  4. As a last resort, set `WEBKIT_DISABLE_COMPOSITING_MODE=1`.

  The guide warns against shipping unconditional overrides.
- Yaak publishes a user-facing doc for the same NVIDIA blank or white screen and flicker problem. It suggests making the variable persistent in /etc/environment or in the desktop entry — [Yaak Linux graphics issues](https://yaak.app/docs/getting-started/linux-graphics-issues)
- A Mesa/AMD case where the DMABUF workaround did not help, but forcing Skia CPU rasterization did — [block/buzz issue #2643](https://github.com/block/buzz/issues/2643)
- In GitHub discussion #8524 ("Webkit is totally unstable…", started 2024), a maintainer said "if you need good linux support now/soon i can't 100% recommend tauri (for Linux)". A March 2026 commenter said the CEF branch is usable "but its very experimental" — [tauri-apps discussion #8524](https://github.com/orgs/tauri-apps/discussions/8524) (originated pre-2025; the 2026 comments are current)

**CEF (Chromium) runtime in Tauri 3 alpha**
- It is "new in the Tauri 3.0 alpha and is not available in Tauri 2.0", and its "APIs may still change before the stable release" — [Tauri v3 CEF docs](https://v3.tauri.app/develop/cef/)
- How it works: add the `tauri-runtime-cef = "3.0.0-alpha.0"` crate, call `.runtime(tauri_runtime_cef::Cef::default())`, and annotate `main` with `#[tauri_runtime_cef::cef_entry_point]`. It is **desktop only**; mobile keeps wry. Benefits are consistent Chromium rendering, access to the Chrome DevTools Protocol, and Chromium's sandbox and permission model — [Tauri v3 CEF docs](https://v3.tauri.app/develop/cef/)
- Costs and limitations — [Tauri v3 CEF docs](https://v3.tauri.app/develop/cef/):
  - Bundles are "hundreds of megabytes larger than with wry", and memory use is higher.
  - The first build downloads ~1 GB of CEF binaries.
  - CMake and Ninja are required on Windows and macOS.
  - Windows "currently runs unsandboxed regardless of the policy".
  - AppImages can lose the sandbox.
  - Only the en-US locale pack ships.
  - Observer callbacks run on CEF's UI thread.
- A third-party write-up (translated) says Linux CEF windows use X11/ANGLE with no native Wayland yet, and that it tracks Chromium 149.0.7827.201 — [gist](https://gist.github.com/MulverineX/4a26a5d6b20b91856c665655be7ce1eb). Lower confidence. A Hacker News commenter got a window under Wayland with cefsimple — [HN](https://news.ycombinator.com/item?id=48626999).
- Tauri 2.0's roadmap already listed bundling CEF for Linux as an alternative to WebKit2GTK — [Tauri 2.0 stable release blog](https://v2.tauri.app/blog/tauri-20/) (via search summary)

**Production experience with Tauri vs Electron**
- One developer shipped the same app in both frameworks — [alanregaya.dev](https://alanregaya.dev/blog/tauri-vs-electron-lessons):
  - WebKitGTK is "months behind" WKWebView, and a layout broke on Linux only.
  - The Linux inspector "feels like 2017".
  - Bundle size went from 138 MB (Electron) to 14 MB (Tauri), and idle RAM from ~210 MB to ~65 MB. This is a single anecdote.
- Comparison guides consistently warn that teams that don't test all three WebView backends find CSS inconsistencies in production. Their RAM and size numbers vary widely — [rustify.rs](https://rustify.rs/articles/rust-tauri-vs-electron-2026), [tech-insider](https://tech-insider.org/tauri-vs-electron-2026/) (SEO-style sources, low rigor)

**Auto-update and code signing**
- Updater — [Tauri updater plugin](https://v2.tauri.app/plugin/updater/):
  - Signatures are mandatory: "This cannot be disabled". The key pair comes from `tauri signer generate`, and the private key is passed through `TAURI_SIGNING_PRIVATE_KEY`. "if you lose this key you will NOT be able to publish new updates."
  - Update feeds can be static JSON (GitHub Releases, S3) or a dynamic server that returns 204 when there is no update.
  - Artifacts: AppImage (Linux), `.app.tar.gz` (macOS), MSI/NSIS (Windows). The page does not mention deb or rpm.
  - On Windows the app exits before installing.
  - There are **no built-in channels**. Implement them with runtime endpoints or a `?channel=` query.
- Windows signing — [Tauri Windows signing](https://v2.tauri.app/distribute/sign/windows/):
  - "Since 2024, EV certificates no longer grant instant SmartScreen reputation". OV and EV build reputation the same way.
  - Options are an OV certificate, Azure Key Vault with `relic`, Azure **Artifact Signing** (formerly Trusted Signing) via `artifact-signing-cli`, or a custom `signCommand`.
  - Cross-compiling Windows installers from Linux or macOS requires a custom sign command.
  - The page gives no prices.

**Real-world Tauri dev tools**
- **Yaak** is built with Tauri, Rust and React — [Yaak README](https://cdn.jsdelivr.net/gh/mountain-loop/yaak@main/README.md). Its root `Cargo.toml` — [Yaak Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/Cargo.toml):
  - Has 36 workspace members, split into `crates/` (core: yaak-core, yaak-models, yaak-http, yaak-grpc, yaak-ws, yaak-sse, yaak-templates, yaak-crypto, yaak-git, yaak-sync, yaak-plugins, yaak-wasm, yaak-mcp, yaak-proxy, yaak-tls…), `crates-tauri/` (app shell, window, license, fonts), `crates-cli/yaak-cli`, `crates-server/` (yaak-web, yaak-playground) and `crates-proxy/`.
  - Pins `tauri 2.11.1` with `default-features = false` and features `common-controls-v6, compression, dynamic-acl`, and patches tauri/tauri-build to a git revision.
  - Other dependencies: `ts-rs 11.1.0` (Rust→TS type generation), `schemars 0.8.22`, `reqwest 0.13.5` (native-tls, http2, system-proxy), `rustls 0.23.34` with `rustls-platform-verifier 0.7.1`, `keyring 3.6.3`.
- **GitButler** — [GitButler README](https://github.com/gitbutlerapp/gitbutler); [GitButler Cargo.toml](https://raw.githubusercontent.com/gitbutlerapp/gitbutler/master/Cargo.toml):
  - The desktop app is Tauri with a Svelte/TypeScript UI. The `but` CLI is "the same Rust backend engine with a Rust command line UI".
  - The workspace has ~49 members plus a `crates/gitbutler-*` glob.
  - Dependencies: `gix 0.89.0` (git rev), `git2 0.21.0` (patched fork), `rusqlite 0.39.0` (bundled), `notify 8.2.0`, `keyring 3.6.3`.

### Inferences
- **Target Tauri 2.12.x now. Do not start on 3.0 alpha.** Keep the frontend free of wry-specific assumptions so a CEF runtime can be adopted later for Linux, or for the Chrome DevTools Protocol. The CEF option costs hundreds of MB, so it only makes sense as an optional Linux build.
- **Use a three-tier IPC pattern for load-test metrics and log streaming** (based on the documented Channel, Response and Request guidance and the binary-vs-JSON benchmark):
  1. Do aggregation in Rust, for example per-interval histograms, and push compact binary or MessagePack frames over a `Channel` at a UI-friendly rate such as 4–10 Hz rather than one event per request.
  2. Keep large logs and response bodies in a Rust-side ring buffer or SQLite. The virtualized UI pulls windows of them through commands that return `ipc::Response` (ArrayBuffer).
  3. Use events only for low-rate notifications.
- **Treat Linux as a tier-2 platform at launch.** Ship Yaak-style troubleshooting docs for NVIDIA/DMABUF, include all three WebView engines in CI and visual tests, and avoid bleeding-edge CSS such as newer container-query or anchor features without checking WebKitGTK versions.
- **Follow Yaak's capability setup.** Its use of `dynamic-acl` and per-window crates suggests multi-window apps (for example a detached log viewer or load-test dashboard) need careful per-label capability design. Give low-privilege windows, such as anything rendering untrusted HTML responses, their own label with minimal permissions, and never iframe untrusted content into a privileged window. Linux and Android cannot tell iframes apart.
- **Budget for signing.** You need an Apple Developer account for notarization, Windows signing (Azure Artifact Signing is the CI-friendly path), and secure storage of the updater key. Losing the updater key strands users.

### Gaps
- No primary-source lessons were gathered for Hoppscotch desktop, Spacedrive, Clash Verge, Pot, Cap or Screenpipe, because the search budget ran out. Unverified prior knowledge worth checking:
  - Hoppscotch rebuilt its desktop app on Tauri.
  - Spacedrive used rspc and published Tauri lessons.
  - Cap and Screenpipe are Tauri apps.
- System tray, multi-window and mobile specifics were not fetched beyond the plugin list, the capability `platforms` field and the 2.12 mobile notes. Check v2.tauri.app/learn/system-tray and the window docs.
- There is no official Channel throughput benchmark. /dev/all should run its own, comparing JSON with ArrayBuffer at 1 KB, 64 KB and 1 MB.
- macOS notarization cost (Apple Developer Program fee) and Azure Artifact Signing pricing were not verified this session.
- The 3.0 alpha.1 and alpha.2 claims and Linux CEF Wayland support are unconfirmed.
- Whether Stronghold is still recommended was not verified. The docs show no deprecation notice, but upstream IOTA Stronghold maintenance status was not checked.

## 2. Frontend stack inside Tauri for a heavy dev tool: framework, code editor, data grids, terminal

### Takeaway
The most relevant Tauri dev tools split between React (Yaak) and Svelte (GitButler). Matching the author's existing wiremock-ui, if it is React, is a stronger reason than raw performance differences, for which no 2025–2026 benchmark was found. Choose the editor and grid by workload:
- **CodeMirror 6** is much lighter and more modular than Monaco. That is why Sourcegraph and Replit migrated away from Monaco, though those posts are pre-2025.
- **Monaco** brings VS Code-grade TypeScript IntelliSense, which helps for script editing.
- **Canvas grids** such as Glide Data Grid handle huge row counts best but cost accessibility.
- **DOM-virtualized grids** such as AG Grid and TanStack are more flexible.

### Cited Findings
- Yaak uses Tauri, Rust and React — [Yaak README](https://cdn.jsdelivr.net/gh/mountain-loop/yaak@main/README.md). GitButler uses Tauri, Rust and Svelte — [GitButler README](https://github.com/gitbutlerapp/gitbutler).
- Yaak generates TypeScript types from Rust models with `ts-rs` (11.1.0), used across yaak-models, yaak-git and yaak-plugins — [Yaak Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/Cargo.toml); [yaak-models Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-models/Cargo.toml)
- Sourcegraph replaced Monaco with CodeMirror and cut its JavaScript download from **6 MB to 3.4 MB (−43%)**. Monaco alone was still 2.4 MB after stripping unused features — [Sourcegraph: Migrating from Monaco to CodeMirror](https://sourcegraph.com/blog/migrating-monaco-codemirror) (pre-2025, possibly outdated)
- Replit moved off Monaco for mobile support and customization, and because "massive bundle size … led to degraded performance across the app" — [Replit: Betting on CodeMirror](https://replit.com/blog/codemirror) (pre-2025, possibly outdated)
- Reported sizes for the two editors disagree because configurations differ. Estimates: Monaco ~2.9 MB full or ~800 kB core; CodeMirror 6 from ~50 kB basic to ~200–300 kB with common features. Monaco remains stronger for VS Code-style TypeScript IntelliSense — [pkgpulse 2026 comparison](https://www.pkgpulse.com/guides/monaco-editor-vs-codemirror-6-vs-sandpack-in-browser-2026); [techinterview.org](https://www.techinterview.org/post/3233475355/build-code-editor-codemirror-monaco-tradeoffs/) (secondary sources)
- Grid architectures compared — [rv-grid.com comparison, reviewed Aug 13, 2026](https://rv-grid.com/compare/ag-grid-alternatives) (published by a competing grid vendor; possible bias):

  | Grid | Rendering | Cost areas |
  |---|---|---|
  | AG Grid Community | DOM virtualization | startup, memory, custom renderers |
  | Glide Data Grid | canvas, with lazy cell access and native scrolling | canvas accessibility, editor overlays, copying large ranges |
  | TanStack Table | headless row models; rendering and virtualization are assembled separately | not stated |

- A 50k-row benchmark of six React grids, including AG Grid Community and TanStack Table+Virtual but not Glide, found modest differences. Its conclusion was "the real winner was the tradeoff" — [itnext benchmark](https://itnext.io/i-benchmarked-6-react-data-grids-on-50-000-rows-the-real-winner-was-the-tradeoff-d447fbf5ecde)
- Canvas grids are fast, but "there's no way a blind user could use" a canvas spreadsheet without significant extra work — [search summary citing Deephaven grid discussion](https://deephaven.io/community/oss/grid/)

### Inferences
- **Use React** if wiremock-ui is React, for code and design reuse. Yaak is proof that a React + Tauri API client performs well. If the author is open to switching, GitButler shows Svelte works at scale too.
- **Editors: use CodeMirror 6 as the default** for the many small embedded editors (request body, headers, regex tester, JWT, diff, log filters). Bundle size matters less in a desktop app because loading is local, but memory and the startup cost of many editor instances do matter. Consider lazily loading Monaco only for the pre/post-request script editor, where TypeScript IntelliSense for a `pm.*`-style API adds real value, or use CodeMirror with a TS language-service worker to keep one editor stack.
- **Grids:** use TanStack Table + TanStack Virtual for general tables (headers, history, K8s/Docker lists). Use a canvas grid such as Glide for SQL/Mongo result sets with 100k+ rows, accepting the accessibility trade-off, or apply server-side windowing from Rust to keep DOM grids viable.
- **Logs:** keep data in Rust and virtualize in the UI (see section 1 IPC inferences).

### Gaps
- No 2025–2026 primary-source benchmark of React 19, Svelte 5 and Solid in Tauri was found. The search budget was exhausted before this was covered.
- xterm.js for exec into containers was not researched. Unverified prior knowledge: xterm.js is the de facto web terminal used by VS Code and Hyper, with WebGL and canvas renderers; the Rust side would need a PTY or the Docker/K8s exec stream. Verify the current version and WebKitGTK WebGL behavior.
- Current versions of TanStack Table/Virtual, Glide Data Grid and AG Grid were not verified.

## 3. Git-friendly project file format design

### Takeaway
The field has converged on **YAML, one file per entity, in a plain directory tree, with a published JSON Schema**:
- **Bruno** dropped its custom `.bru` DSL as the default in v3.1 (Feb 2026) in favor of OpenCollection YAML, citing parse speed, tooling and AI/ecosystem support.
- **Yaak** syncs one YAML file per resource.
- **Hurl** shows that a plain-text DSL works well for CLI-first test files.

The main design choices left for /dev/all are:
- Filename strategy: readable slugs (Bruno) or IDs (Yaak).
- Where ordering lives: a per-file `seq` (Bruno) or a parent index.
- Keeping volatile fields out of tracked files.
- Schema versioning with migrations.

### Cited Findings
- **Bruno v3.1 (announced Feb 17, 2026)** makes YAML the default for newly created or imported collections, and `.bru` remains supported — [Bruno blog](https://blog.usebruno.com/making-yaml-the-default-in-bruno-v3.1). Reasons given:
  - `.bru` parsing time "increases sharply as collection size grows", and YAML parsing was **4–5x faster** in Bruno's benchmarks.
  - `.bru` has no native linting, formatting or schema validation, while YAML works with yamllint, Prettier, JSON Schema and structural diffs.
  - The custom syntax is poorly understood by AI tools and CI.
  - Most users work in the GUI anyway.
  - TOML was considered but rejected for deeply nested structures and arrays.
  - Migration is export and re-import ("completely lossless"). A collection must use either YAML or `.bru` requests, not both.
- **OpenCollection layout in Bruno** — [Bruno docs: OpenCollection YAML](https://docs.usebruno.com/opencollection-yaml/overview):
  - The root config is `opencollection.yml`, versus `bruno.json` and `collection.bru` for the older format.
  - Environments live in `environments/development.yml`.
  - Each folder has a `folder.yml`, and each request is its own file, such as `create-user.yml`.
  - A request file has `info` (name, type, `seq`), `http` (method, url, body, auth: inherit), `runtime.scripts` (tests) and `settings`.
  - Validation is through standard YAML tools or the **OpenCollection JSON Schema** at schema.opencollection.com.
  - Spec: [spec.opencollection.com](https://spec.opencollection.com/).
- The Bruno CLI reportedly still works with `.bru` only. A community `bru-converter` converts OpenCollection YAML back to `.bru` for the CLI — [bru-converter README](https://cdn.jsdelivr.net/npm/bru-converter@0.1.0/README.md). This may already be outdated; verify current bruno-cli YAML support.
- **Yaak directory sync** — [Yaak docs: Local directory / Git sync](https://yaak.app/docs/using-yaak/local-directory-git-sync):
  - YAML "because it works well with Git".
  - **One file per resource, named `yaak.${MODEL_ID}.yaml`**, flat in the sync directory, which must be empty at setup (e.g. a `my-project/yaak` subdirectory).
  - Environments are **excluded by default** because they "usually contain sensitive values".
  - The workspace and directory are watched both ways. App-originated changes sync silently, and "Changes originating from the directory show a summary and confirmation prompt."
- The yaak-git crate depends on `serde_yaml = "0.9.34"`, and yaak-models uses `nanoid 0.4.0` for IDs and `schemars` for JSON Schema generation — [yaak-git Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-git/Cargo.toml); [yaak-models Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-models/Cargo.toml)
- **Hurl 8.0.0** is a Rust binary built on libcurl that runs a plain-text format: a request line, headers and body, then an `HTTP 200` expectation, `[Captures]`, `[Asserts]` (JSONPath, XPath, headers, duration, hashes) and `[Options]` (retries). It outputs text, JUnit, TAP and HTML reports and is "easy to integrate in CI/CD" — [hurl.dev](https://hurl.dev/)

### Inferences
Recommended format for /dev/all, following Bruno, OpenCollection and Yaak:
- **YAML, one file per entity**: request, mock route, environment, load-test scenario, tunnel config, saved SQL/Redis/Mongo query, snippet.
- **Layout:** a root `devall.yaml` holding `schemaVersion`, the project name and defaults; one directory per feature (`requests/`, `mocks/`, `envs/`, `loadtests/`, `db/`); and a gitignored `.devall/` directory for local state.
- **Filenames:** use human-readable slug filenames, as Bruno does, so PR diffs and code review make sense, and store a stable `id` (nanoid) inside each file, unlike Yaak's opaque `yaak.<id>.yaml`. Renames will show as delete plus add in git, but `git mv` detection handles content similarity. Yaak's ID-based names avoid that rename churn at the cost of readability.
- **Ordering:** store it in the parent `folder.yaml` as an ordered list of child IDs or slugs, rather than as `seq` in every file. Bruno's per-file `seq` means one drag-reorder can touch many files and create merge conflicts. The trade-off is that concurrent additions to the same folder conflict in one line-oriented list, which is easy to resolve.
- **Diff hygiene:**
  - Serialize from Rust structs in a fixed field order and omit defaults and empty fields.
  - Never write volatile data (timestamps, last response, UI state such as expanded folders or active tab) into tracked files. That belongs in SQLite or `.devall/`.
  - Use block scalars (`|-`) for bodies and scripts, as Bruno does, so they diff line by line.
  - Normalize line endings to LF and add a `.gitattributes` file.
- **Validation and versioning:** generate a JSON Schema per entity type with `schemars`, as Yaak does, publish it at a stable URL, and add a `# yaml-language-server: $schema=...` header so VS Code validates the files. Bump `schemaVersion` only on breaking changes and run forward migrations on load. Never silently rewrite every file on upgrade; ask the user first, because a mass rewrite creates a huge commit.
- **Interop:** import from and export to OpenCollection, Postman v2.1, OpenAPI, Hurl, `.http` and WireMock mappings, rather than inventing incompatible shapes. Model mocks so that WireMock mapping JSON round-trips, which also keeps the author's wiremock-ui consistent.
- **External edits:** watch the project directory, as Yaak does, apply external changes with a summary prompt, and treat the files as the source of truth.

### Gaps
- The following formats were not researched because the search budget ran out: Postman's newer native-Git and local-filesystem collection format (2025), Insomnia git sync format, Kreya `.krpr` files, `.http` (JetBrains/VS Code REST Client) conventions, WireMock mapping JSON details, the Mockoon environment JSON layout (its docs URL returned 404), and OpenAPI/AsyncAPI-as-source-of-truth approaches.
- Rust YAML crate choice was not verified. Unverified prior knowledge: `serde_yaml` was archived and marked unmaintained by its author in 2024 (0.9.34 is the final release, still used by Yaak). Evaluate maintained alternatives (serde_yml had quality concerns; serde_norway and serde-saphyr exist) and YAML 1.1/1.2 pitfalls such as the "Norway problem".
- How Yaak orders requests in its YAML files was not determined.

## 4. Secrets handling: keeping secrets out of project files

### Takeaway
The proven pattern is **variable references in tracked files, with values resolved at runtime from a local store**:
- **Bruno** keeps secret values on the local machine, encrypted with the OS or AES-256 as a fallback, and also supports `.env` files and secret-manager integrations. The integrations are a paid tier.
- **Yaak** adds optional per-workspace encryption: the key lives in the OS keychain, and values are encrypted inline with `secure(...)`, so environments can be committed safely if the key is shared out of band.

In Rust, the keyring ecosystem split in 2025–2026: `keyring-core` 1.0 is now the API, each platform store is its own crate, and `keyring` 4.x is only a sample/CLI crate. Both Yaak and GitButler still use keyring 3.6.3.

### Cited Findings
- Bruno: "Bruno stores secrets on your local machine". Values are protected with OS encryption where available, with **AES-256 fallback**. The three approaches are secret variables, a DotEnv file and secret-manager integration. The goal is that secrets are removed from a collection before it is shared via source control or export — [Bruno secrets overview](https://docs.usebruno.com/secrets-management/overview)
- Bruno's pricing table lists **secret manager integrations only in the Ultimate tier** ($11/user/month, billed annually) — [Bruno pricing](https://www.usebruno.com/pricing)
- Yaak encryption: it "must be enabled for each workspace, which will generate and store an encryption key using your host OS's keychain. Back this key up and share it with your team". The `secure(...)` template function encrypts any portion of text where variables are supported. With encryption, environments can be marked "sharable" for directory and Git sync — [Yaak feedback: secrets encryption](https://yaak.app/feedback/posts/secrets-encryption)
- Yaak users asked for per-secret OS-keychain lookup, KeePassXC and Vault backends. The maintainer pointed those requests toward the plugin system — [Yaak feedback: secrets from OS keychain](https://yaak.app/feedback/posts/secrets-from-os-keychain-keyring)
- Yaak's environment files are excluded from directory sync by default — [Yaak docs](https://yaak.app/docs/using-yaak/local-directory-git-sync)
- yaak-plugins depends on `keyring` with features `apple-native`, `windows-native` and `sync-secret-service`, and on yaak-crypto — [yaak-plugins Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-plugins/Cargo.toml). The Yaak and GitButler workspaces both pin `keyring 3.6.3` — [Yaak Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/Cargo.toml); [GitButler Cargo.toml](https://raw.githubusercontent.com/gitbutlerapp/gitbutler/master/Cargo.toml)
- **The keyring crate has been restructured** — [keyring 4.0.0 docs](https://docs.rs/crate/keyring/4.0.0); [keyring-core docs](https://docs.rs/keyring-core); [keyring 4.2.0](https://docs.rs/crate/keyring/4.2.0):
  - In 4.x, "the library/API parts [are] now part of the keyring-core crate and the credential stores all in separate crates of their own". v3 users are told to switch to `keyring-core` rather than upgrade to v4.
  - `keyring-core` is at 1.0.0. It requires stores to be Send and Sync, and its built-in mock and sample stores are "not warranted to be either secure or robust".
  - Native store crates exist for keychain, windows, secret-service and keyutils.
- A community store, `db-keystore`, implements the keyring-core traits as a file-backed (Turso/SQLite) store with optional encryption — [db-keystore](https://docs.rs/db-keystore)

### Inferences
Recommended secrets design for /dev/all:
1. **Tracked files contain only references**, for example `{{secret.STRIPE_KEY}}` or `{{env.API_URL}}`. Environments are split into a tracked file of non-secret defaults plus secret *names*, with the values in the local store.
2. **Local secret store:** use the OS keychain through `keyring-core` plus per-platform store crates, keyed by project ID, environment and name. Fall back to an encrypted local vault (for example a SQLite or `age`-encrypted file unlocked by a keychain-held key) on Linux machines without Secret Service. Do not adopt `keyring` 4.x as a library, because it is no longer the library crate.
3. **Optional shared-encrypted mode, as in Yaak:** a per-project key held in the keychain lets encrypted values be committed. Make the key-backup and key-sharing step explicit in the UI, since a lost key means lost secrets.
4. **`.env` support:** read a gitignored `.env` file in the project root, and have `devall init` write `.gitignore` entries for `.env` and `.devall/`.
5. **Provider integrations** (1Password `op`, Bitwarden, Vault, Doppler, Infisical, AWS/GCP secret managers) as plugins or template functions. Bruno shows this is a credible paid-tier feature.
6. **Guardrails:** scan for high-entropy or known-pattern strings before writing tracked files and warn in the UI. Optionally install a gitleaks-style pre-commit hook. Exclude environments from sync by default, as Yaak does.
7. **Scratch-pad project:** store it in the app data directory, not a repo, but in the same file format so items can be "moved into a project" with no conversion.

### Gaps
- 1Password CLI, Bitwarden, HashiCorp Vault, Doppler and Infisical integration patterns, and gitleaks or pre-commit scanning, were not researched because the search budget ran out.
- Bruno's exact secret-variable storage location and file format were not documented on the fetched page.
- The algorithm Yaak's encryption uses was not found.
- Current versions of the per-platform keyring store crates were not verified on crates.io.

## 5. Embed git (gitoxide/gix or git2/libgit2) or rely on the user's git?

### Takeaway
Both leading Tauri dev tools embed git libraries:
- **Yaak** ships git2 0.21 with vendored libgit2 and OpenSSL to provide branches, commits, push, pull, history and a selective-commit diff view.
- **GitButler** uses both gix 0.89 and a patched git2 0.21, with a bridging crate whose comment says it "Shouldn't be needed once `git2` is gone". In other words, it is migrating to gix.

gix still lacks push, rebase and reset, and its commit support lacks hooks. A pure-gix integration therefore cannot yet do a full commit, push and pull cycle.

### Cited Findings
- Yaak's git UI — [Yaak docs](https://yaak.app/docs/using-yaak/local-directory-git-sync):
  - Initialize git from the sync dropdown, or run `git init` manually.
  - Once a repo is detected, "Git actions are available for branches, commits, push, pull, and history".
  - A request diff view lets the user select which changes to commit.
- yaak-git depends on `git2 = { version = "0.21.0", features = ["vendored-libgit2", "vendored-openssl"] }` — [yaak-git Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-git/Cargo.toml)
- GitButler pins `gix 0.89.0` to a git revision and `git2 0.21.0` patched to `gitbutlerapp/git2-rs`. Its `crates/but-oxidize` is described as "Utilities to help translate between `git2` and `gix`. 👉Shouldn't be needed once `git2` is gone." — [GitButler Cargo.toml](https://raw.githubusercontent.com/gitbutlerapp/gitbutler/master/Cargo.toml)
- A secondary source says GitButler uses gix for hot paths and git2 for operations gix lacks. This is unconfirmed by GitButler — [openapps.pro](https://openapps.pro/apps/gitbutler)
- gitoxide feature status — [gitoxide README](https://raw.githubusercontent.com/GitoxideLabs/gitoxide/main/README.md):

  | Feature | Status |
  |---|---|
  | clone, fetch | done (some worktree sub-items unchecked) |
  | status | done |
  | blob and tree diff | done |
  | worktree checkout | done |
  | commit | done, but **hooks missing** |
  | merge | blobs and trees done, **commits not** |
  | blame | plumbing; `gix-blame` "very early" |
  | **push** | **missing** (only "create (thin) pack") |
  | **rebase** | **missing** |
  | **reset** | **missing** |

  Stability: only `gix-lock` and `gix-tempfile` are listed as production grade, and "usable" crates "may be missing some features".
- In a 2024 discussion about Radicle replacing git2 with gix, a commenter noted that the fetch-protocol approach was harder than the library swap — [gitoxide issue #1319](https://github.com/byron/gitoxide/issues/1319) (pre-2025, possibly outdated)

### Inferences
- **Use a hybrid approach.** Embed gix for fast read-only operations: repo detection, status of project files, per-file and per-entity diffs for a "changes" panel, log and history, and blob reads for "compare with HEAD". These are areas where gix is complete and pure Rust, so there is no OpenSSL or libgit2 to vendor.
- **For write and network operations** (commit with hooks, push, pull, rebase), either:
  - **(a)** shell out to the user's `git` when it is present. This respects credential helpers, SSH agents, signing configuration and hooks. It is an inference; the specific credential and hook behavior was not verified this session.
  - **(b)** use git2 with vendored libgit2, as Yaak does, at the cost of binary size, and with credentials and hooks reimplemented.
- **Keep the git panel minimal**: changes, diff, commit, push and pull for project files. Users with complex workflows will use their own git or IDE.
- **The format matters more than the git UI.** One file per entity, stable ordering and no volatile fields (section 3) are what make standard git workable.

### Gaps
- No published GitButler or Yaak write-up on why they chose these libraries was found.
- Whether Yaak shells out to `git` for push or pull credentials was not checked in the source.
- gix's push timeline was not found.
- Binary-size impact of vendored libgit2 plus OpenSSL was not measured.

## 6. Extensibility (plugins) and scripting runtime for pre/post-request scripts

### Takeaway
There are three viable plugin models:
- **(a) Node-runtime JS/TS plugins, as Yaak does.** Plugins get the full Node and npm ecosystem with no sandbox, and Yaak still labels them experimental.
- **(b) WASM component-model plugins via wasmtime and WIT, as Zed does.** These are sandboxed, with host-gated capabilities and versioned WIT interfaces, but cannot freely modify the UI.
- **(c) Extism**, a higher-level WASM plugin framework with many host SDKs.

For pre/post-request scripts, an embedded **QuickJS (rquickjs, QuickJS-NG)** is the size and startup leader at about 210 KiB of engine code. Boa is pure Rust but larger and slower, and deno_core/V8 gives maximum compatibility at tens of MB.

### Cited Findings
- **Yaak plugins** — [Yaak plugin quick start](https://yaak.app/docs/plugin-development/plugins-quick-start):
  - "written in TypeScript and executed within a NodeJS runtime, alongside Yaak"; plugins "have access to the full set of NodeJS APIs and NPM package ecosystem".
  - Published to the Yaak plugin registry as `@{namespace}/{name}` with `yaak plugin publish`.
  - "Plugins are currently experimental and the APIs may change suddenly."
- yaak-plugins depends on `tokio` (process, fs), `tokio-tungstenite 0.30.0` and `zip 7` — [yaak-plugins Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-plugins/Cargo.toml). This suggests, without confirming, that the host spawns a Node process and talks to it over WebSocket. Yaak also has a separate `yaak-wasm` crate — [Yaak Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/Cargo.toml).
- **Zed extensions** — [summary of Zed's "Life of a Zed Extension" blog](https://www.plushcap.com/content/zed/blog/life-of-a-zed-extension-rust-wit-wasm) (2024, possibly outdated):
  - Written in Rust, compiled to Wasm with the Component Model and executed via **Wasmtime**, using WIT and wit_bindgen.
  - Host capabilities such as http_client, process and settings are exposed as host modules.
  - Failures stay contained, and the module can be reloaded without restarting.
  - Limitation: extensions cannot modify the UI or perform arbitrary operations.
- The Zed extension API crate is generated from WIT definitions **versioned per Zed release**, so extensions keep working against older releases — [openapps.pro zed-extension-api](https://openapps.pro/packages/zed-extension-api) (secondary)
- Wasmtime's `bindgen!` "maps all WIT types into Rust automatically and generates traits for embedders to implement" — [wasmtime component docs](https://docs.wasmtime.dev/api/wasmtime/component/index.html)
- **Extism:**
  - Host SDKs exist for Go, Ruby, Python, Node, Rust, C, C++, OCaml, Haskell, PHP, Elixir/Erlang and more — [awesome.ecosyste.ms extism](https://awesome.ecosyste.ms/projects/github.com%2Fextism%2Fextism)
  - The Rust PDK's `#[plugin_fn]` macro handles JSON, MessagePack and Protobuf conversion — [extism-pdk](https://libraries.io/cargo/extism-pdk)
  - lib.rs lists crate 1.10.0 (Feb 10, 2025). The snapshot may be stale, and no 2026 release could be confirmed — [lib.rs extism](https://lib.rs/crates/extism)
- **rquickjs** is a high-level binding to **QuickJS-NG**, which is small and embeddable with "210 KiB of x86 code for a simple hello world" — [rquickjs 0.12.1 docs](https://docs.rs/crate/rquickjs/0.12.1)
- **Boa:** a maintainer said most of its size comes from 17 MB of internationalization data, and disabling default features brings the CLI to about 11 MB — [HN thread mirror](https://brianlovin.com/hn/45944337) (secondary mirror; verify)
- In a yt-dlp-ejs workload, QuickJS outperformed Boa and Deno — [dev.to: Improving yt-dlp-ejs with Rust](https://dev.to/ahaoboy/improving-yt-dlp-ejs-with-rust-smaller-and-faster-5cnl)
- **V8 vs QuickJS size:** Ryan Dahl reportedly said the denort binary is about 64 MiB with V8 versus about 35 MiB with QuickJS — [besthub.dev article](https://www.besthub.dev/articles/deno-s-big-shift-quickjs-replaces-v8-cutting-compile-size-by-almost-45-dcbacaae56b6) (aggregator; low confidence, verify against Deno primary sources)

### Inferences
- **Scripting:** embed rquickjs (QuickJS-NG) to run pre/post-request scripts and tests in-process, with a Postman-compatible `pm.*` shim (`pm.environment`, `pm.variables`, `pm.request`, `pm.response`, `pm.test`, `pm.expect` with chai-like assertions) plus a native `dv.*` API. Benefits:
  - Small binary, millisecond startup and per-request isolation by creating a fresh context.
  - The same engine runs in the CLI and Docker image, which gives parity (section 7).
  - Expose only explicitly bound host functions (HTTP via the Rust client, crypto, base64, a limited `require` for a curated module set). This gives a natural sandbox.
- **Escape hatch:** offer an opt-in "developer mode" that runs scripts in an external Node or Deno process for full npm compatibility.
- **Plugins:** use WASM components (wasmtime + WIT) for sandboxed, capability-gated extensions such as importers, template functions, auth providers, secret-provider integrations, encoders and fake-data generators. Version the WIT world per major release, as Zed does.
  - If JS plugins matter more for adoption, a QuickJS-hosted plugin API keeps one runtime.
  - Avoid Yaak's unsandboxed full-Node approach unless the app is willing to ship Node, since it is large and gives plugins full system access.
- **UI extensions:** let plugins contribute declarative UI (forms or schemas rendered by the host) rather than arbitrary webview code. Arbitrary webview code would bypass Tauri capabilities and make plugins a privileged-window attack surface.

### Gaps
- Bruno's scripting runtime (its "Safe Mode" and "Developer Mode" JS sandbox) could not be verified: the docs URL returned 404 and searching was exhausted. Unverified prior knowledge: Bruno's Safe Mode uses QuickJS and its Developer Mode uses Node vm with npm modules. Verify.
- The scripting and plugin models of Insomnia and Hoppscotch, and the DevToys .NET extension SDK, were not researched.
- No authoritative `pm.*` compatibility matrix for any QuickJS-based tool was found.
- Boa and deno_core current versions were not verified, and the wasmtime and Extism current versions were not confirmed.

## 7. Headless/CLI parity and shared Rust core architecture

### Takeaway
Both reference Tauri apps already put their logic in a shared Rust core consumed by a GUI shell and a CLI:
- **Yaak** has a `crates/` core, a `crates-tauri/` shell, `crates-cli/yaak-cli`, `crates-server/` and `crates-proxy/`.
- **GitButler** has the `but` CLI, "the same Rust backend engine", plus a `but-api` layer used by both CLI and GUI.

Hurl shows the CLI-first end state: a single Rust binary with JUnit, TAP and HTML reports for CI.

### Cited Findings
- Yaak's workspace has core crates (yaak-core, yaak-models, yaak-http, yaak-grpc, yaak-ws, yaak-sse, yaak-templates, yaak-sync, yaak-git, yaak-plugins, yaak-crypto, yaak-mcp, yaak-proxy, yaak-tls…) that are separate from `crates-tauri/*`, `crates-cli/yaak-cli`, `crates-server/yaak-web` and `crates-server/yaak-playground` — [Yaak Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/Cargo.toml)
- yaak-models has `cfg(not(target_arch = "wasm32"))` gating for its r2d2 pool. This implies the models crate also compiles to wasm32 for web or playground use — [yaak-models Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-models/Cargo.toml)
- GitButler: "The but CLI is the same Rust backend engine with a Rust command line UI" — [GitButler README](https://github.com/gitbutlerapp/gitbutler). DeepWiki describes `crates/but-api/` as a "unified API layer used by CLI, GUI, etc." with legacy `gitbutler-*` crates being migrated incrementally — [DeepWiki GitButler](https://deepwiki.com/gitbutlerapp/gitbutler) (secondary)
- Hurl is a single Rust and libcurl binary that "needs no runtime", outputs text, JUnit, TAP and HTML, and is designed for CI/CD — [hurl.dev](https://hurl.dev/)
- The Bruno CLI lagged the YAML format change: it still works with `.bru`, per a community converter's README — [bru-converter](https://cdn.jsdelivr.net/npm/bru-converter@0.1.0/README.md) (may be outdated)

### Inferences
- **Proposed workspace layout:**
  - `crates/devall-model` (file format, serde, schemars, migrations)
  - `devall-template` (variable resolution, secrets interface)
  - `devall-script` (rquickjs and the `pm` shim)
  - `devall-http`, `devall-mock` (hyper-based mock server that can load WireMock mappings)
  - `devall-webhook`, `devall-tunnel`, `devall-load` (load generator and HDR histograms)
  - `devall-db` (SQL/Redis/Mongo)
  - `devall-k8s` / `devall-docker`
  - `devall-git` (gix plus a git CLI wrapper)
  - `devall-store` (SQLite history and cache)
  - Then `apps/desktop` (a thin Tauri shell: commands and channels only), `apps/cli` (clap: `devall run`, `devall mock serve`, `devall load run`, `devall tunnel`) and a Docker image built from the CLI for mock servers in CI.
- **Lesson from Bruno's CLI lag:** run the CLI and GUI from the **same parser crate and the same test fixtures** from day one, so a format change never ships to one surface before the other.
- **Type sharing:** generate TypeScript types from Rust with ts-rs, as Yaak does, so frontend and core models cannot drift.
- **Secrets in CI:** the CLI needs a non-interactive path, for example environment variables such as `DEVALL_SECRET_<NAME>` or provider plugins, because OS keychains are unavailable in CI.

### Gaps
- bruno-cli current YAML support, `inso` (Insomnia CLI), `mockoon-cli` and k6 architecture details were not researched because the search budget ran out.
- Whether Yaak's CLI shares the full request engine with the app was not verified beyond the crate layout.

## 8. Local data: SQLite vs files, full-text search, file watching

### Takeaway
Yaak and GitButler both use **rusqlite with the bundled SQLite build**: Yaak with an r2d2 pool and sea-query, GitButler at rusqlite 0.39. GitButler uses **notify 8.2.0** for file watching, and Yaak watches its sync directory both ways, asking the user to confirm external changes. The established pattern is files as the source of truth for shareable definitions, and SQLite for history, response caches, indexes and UI state.

### Cited Findings
- yaak-models dependencies — [yaak-models Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-models/Cargo.toml):
  - `rusqlite = { version = "0.38", features = ["bundled", "chrono"] }`
  - `sea-query 1.0` and `sea-query-rusqlite 0.8.0`
  - `r2d2 0.8.10` and `r2d2_sqlite 0.32`
  - `include_dir 0.7` (likely embedded migrations; inference)
  - `nanoid 0.4.0`
  - There is also a separate `crates/common/yaak-database` crate.
- GitButler pins `rusqlite 0.39.0` (bundled) and `notify 8.2.0` — [GitButler Cargo.toml](https://raw.githubusercontent.com/gitbutlerapp/gitbutler/master/Cargo.toml)
- Yaak monitors "both the workspace and destination directory", syncing app changes silently and showing a summary and confirmation for directory-originated changes — [Yaak docs](https://yaak.app/docs/using-yaak/local-directory-git-sync)

### Inferences
- **Files vs SQLite:** project files are canonical. SQLite (rusqlite, bundled) holds:
  - request and response history, with large bodies in content-addressed blob files referenced from SQLite
  - load-test results
  - the log-viewer index
  - an index of project entities for fast lookup
  - UI and session state
- **SQLite settings:** use WAL mode and a small connection pool (r2d2 or deadpool).
- **File watching:** use notify (8.x) with debouncing, and suppress self-originated writes by matching content hashes the app just wrote. On external changes, re-parse, validate against the schema and show a non-blocking banner for conflicts with unsaved UI edits.
- **Search:** SQLite FTS5 is probably enough for searching requests and history. Consider tantivy only for very large log corpora.

### Gaps
- tantivy's current version and its fit for log search were not researched because the search budget ran out.
- No published lessons on notify edge cases (network drives, WSL, macOS FSEvents coalescing) were gathered.
- sqlx was not evaluated against rusqlite. Both reference apps use rusqlite.

## 9. Cloud pieces: Firebase fit and alternatives; CRDT collaboration vs git

### Takeaway
Firestore's free tier is modest (50k reads, 20k writes and 20k deletes per day, 1 GiB stored, 10 GiB/month egress) and documents are capped at 1 MiB. That suits accounts, licensing, small shared metadata and webhook or relay coordination, but not storing large request or response payloads. Git should stay the primary collaboration channel. CRDTs such as Automerge 3, which cut memory more than 10x in July 2025, are only worth adding for optional live co-editing sessions.

### Cited Findings
- Firestore free quota — [Firestore quotas](https://firebase.google.com/docs/firestore/quotas):
  - Resets daily: **1 GiB** stored, **50,000 reads/day**, **20,000 writes/day**, **20,000 deletes/day**, **10 GiB/month** outbound.
  - Max document size: **1 MiB**. Max API request: **10 MiB**.
  - Transactions are limited to 270 s with a 60 s idle expiry.
  - Composite indexes: 200 without billing, 1000 with.
  - Security Rules allow up to 10 or 20 `get()/exists()` calls per request.
- **Automerge 3.0 (July 2025)** — [Automerge 3 blog](https://automerge.org/blog/automerge-3/):
  - Memory use cut by "over 10x, sometimes dramatically more". Pasting Moby Dick used 700 MB in Automerge 2 versus 1.3 MB in Automerge 3.
  - Uses the compressed columnar format at runtime. One document that "had not loaded after 17 hours loaded in 9 seconds".
  - Same file format as Automerge 2, and `@automerge/automerge-repo` 2.1.0.

### Inferences
- **Firebase fit:**
  - Auth for optional accounts and licensing.
  - Firestore for team or org metadata, license and seat records, and shared "published mock" or webhook endpoint registrations.
  - Cloud Functions for webhook ingestion that fans out to clients via Firestore listeners. Payloads over ~1 MiB need Cloud Storage.
- **Reverse tunnel:** a tunnel needs long-lived TCP or WebSocket relays. Serverless functions are a poor fit, so plan a small dedicated relay service, for example on a container platform. This is an inference; Firebase connection limits were not verified.
- **Collaboration:** git plus the file format handles asynchronous team sharing. Real-time co-editing of a request is a nice-to-have. If added, use a CRDT doc per open entity, synced through a relay, and serialize it back to YAML on save so git remains the source of truth.

### Gaps
- The following were not researched because the search budget ran out: Firebase Auth inside a Tauri webview (OAuth popup or redirect behavior on `tauri://` or custom-protocol origins, the deep-link plugin flow), Cloud Functions limits and pricing, and alternatives (Supabase, Cloudflare Workers/D1/Durable Objects, PocketBase).
- Loro's 1.0 details could not be retrieved (its blog returned 403).
- Yjs/yrs status was not checked.

## 10. Distribution and licensing models

### Takeaway
Comparable tools monetize the prebuilt app while keeping source open or core free:
- **Yaak:** MIT source, free for personal use, with a commercial license required for work use: $79/yr individual, a $399 lifetime option, or $149/user/yr for business. After an annual license lapses, the last released version can still be used.
- **Bruno:** a free open-source tier, then Pro at $6 and Ultimate at $11 per user/month, billed annually. Private-repo git providers and secret-manager integrations are behind the paywall.
- **Kreya:** free, Pro and Enterprise tiers, with scripting and testing paid.
- **DevUtils:** offers perpetual and team licenses for a native macOS app with 47+ tools.

Signing and updates are mandatory engineering line items (section 1).

### Cited Findings
- **Yaak pricing** — [Yaak pricing](https://yaak.app/pricing):

  | Tier | Price |
  |---|---|
  | Personal | free |
  | Individual (commercial) | **$79/year** or **$399 lifetime** |
  | Business | **$149/user/year** |

  - Billing is annual only.
  - Up to 3 devices per license holder.
  - "Lifetime fallback" lets the user keep the last version released while the annual license was active after it lapses.
  - 30-day trial with no signup.
  - Purchasing-power-parity discounts are available.
  - Source is **MIT**, and "Licensing applies only to the prebuilt binaries."
- **Bruno pricing** — [Bruno pricing](https://www.usebruno.com/pricing):
  - Open Source: $0. Pro: **$6/user/month**. Ultimate: **$11/user/month**. Both paid tiers are billed annually.
  - Open Source is limited to 2 workspaces and public-repo git providers. Paid tiers add private repos.
  - Secret manager integrations, SSO, SCIM and audit logs are in Ultimate.
  - The page no longer mentions a "Golden Edition".
- **Kreya pricing** — [Kreya pricing](https://kreya.app/pricing/):
  - Free ("Free forever", history limited to the latest request), Pro and Enterprise (flat-rate, offline license).
  - Scripting, testing, snapshot tests, collections and history are excluded from the free tier.
  - A 10-day trial; "Cancel anytime" suggests a subscription.
  - Prices did not render in the fetched page.
- **DevUtils** — [devutils.com](https://devutils.com/):
  - Native macOS app with "47+" tools, latest version 1.17.0, requires macOS 10.13+.
  - Offers "Perpetual License" and "Team License".
  - Also on Setapp and the Mac App Store. Prices are not shown on the landing page.
- **Signing and updates:** EV no longer gives instant SmartScreen reputation (since 2024), Azure Artifact Signing is supported, and updater signatures are mandatory — [Tauri Windows signing](https://v2.tauri.app/distribute/sign/windows/); [Tauri updater](https://v2.tauri.app/plugin/updater/)
- **Telemetry:** Yaak markets "zero telemetry", with local-only data and secrets encryption — [Yaak search summary / yaak.app](https://yaak.app/)

### Inferences
- **Proposed model for /dev/all:** open-source core (MIT or Apache-2.0) covering the CLI and core crates, and a desktop app that is free for personal use with a commercial license for work. Yaak's structure, which pairs an annual subscription with a lifetime fallback and a lifetime option, is the best-documented developer-friendly precedent.
- **Paid-tier levers**, based on Bruno and Kreya: secret-manager integrations, a private-repo git UI, team or cloud features (Firebase-backed relay, hosted webhook URLs, the reverse tunnel), SSO, and advanced load-test reporting.
- **Telemetry:** off by default, opt-in, and documented. Yaak's "zero telemetry" stance is a competitive baseline in this category.
- **Tunnel economics:** a hosted reverse tunnel and webhook relay have ongoing server costs, which fits a subscription better than a one-time purchase.

### Gaps
- Kreya and DevUtils prices did not render on the fetched pages, and Bruno's "Golden Edition" history and price were not verified.
- Insomnia, Postman, Hoppscotch, RapidAPI and Proxyman pricing, as well as telemetry norms beyond Yaak, were not researched because the search budget ran out.
- Apple Developer Program and Azure Artifact Signing costs were not verified.

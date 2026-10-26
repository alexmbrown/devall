# Dev All (/dev/all)

A local-first developer toolkit in one desktop app: HTTP client, mock server, webhooks, tunnels,
load testing, databases, logs, Docker & Kubernetes, notes, to-dos, utilities and local AI —
connected to each other and organised around your projects.

- **Project-centric, in your repo.** Requests, environments, mocks, notes, load tests and data
  schemas are plain YAML/Markdown files next to your code, reviewed and shared with git. A project
  can span several repositories (e.g. frontends and backends) with shared requests and environments.
- **Secrets never touch the repo.** They live in the OS keychain; files only reference them.
- **History stays on your machine.** Responses, file versions, utility runs, activity and load runs
  are kept in the app data folder, never in the project.
- **Free and local.** Everything that runs on your machine is free, with no account. Optional
  hosted extras (public tunnels, an always-on webhook inbox, shares, monitors) run on a relay you can
  also self-host ([relay/](relay/)).
- **Windows and macOS** are first-class (Linux builds work too).

## What's in it

| Area | Highlights |
|---|---|
| **Requests** | HTTP, GraphQL, WebSocket, gRPC, MQTT, Socket.IO; environments and variables; auth (OAuth2 and more); tests and scripts; runner; history with diffs; import (Postman, Insomnia, Bruno, HAR, OpenAPI, cURL, `.http`/`.hurl`) and code generation |
| **Mock server** | WireMock-compatible engine with the full wiremock-ui admin: stubs, request journal, near misses, recording, scenarios, verification, OpenAPI drift; mock from a response in one click |
| **Webhooks & tunnels** | Local webhook receiver with inboxes and replay; expose any port through your own ngrok, cloudflared or Tailscale Funnel — or the relay |
| **Load testing** | Load tests from saved requests, live results, run history, thresholds for CI, distributed agents |
| **Data** | Postgres, MySQL, SQL Server, SQLite, Redis and MongoDB clients (SSH tunnels, TLS); fake-data generator with schemas |
| **Source control** | Multi-repo status and bulk actions, commit graph, compare a branch with its base and review every file before opening a merge request, hunk/line staging, secret scanning on commit |
| **Logs & infra** | Large log files with queries, patterns and SQL; Docker, Kubernetes and GCP emulator views; a traffic inspector |
| **Notes & to-dos** | Markdown notes and runnable notebooks in the repo; personal Todoist-style to-dos (lists, board, recurrence, reminders, attachments) kept on your machine; link anything to anything with `devall:` links |
| **Utilities** | 70+ encoders, decoders, formatters, hashes, JWT, regex, diff, string compare, time, network tools |
| **Local AI** | Bring your own model (or the managed local one): ask questions about the project, semantic search, AI-assisted features; an MCP server so coding agents can use your requests, notes and to-dos |

The full feature list, with priorities and what is free vs hosted, is in [docs/FEATURES.md](docs/FEATURES.md).

## Install

There are no published releases yet; build it from source.

Prerequisites: Rust 1.91+, Node 22 and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
for your OS (WebView2 is preinstalled on Windows 11; Xcode Command Line Tools on macOS).

```bash
cd apps/desktop && npm install
```

Build and install it for your user — a "Dev All" Start Menu entry on Windows, `~/Applications` on
macOS. Run it again after pulling or changing code to update the installed copy:

```bash
cd apps/desktop && npm run app:install
```

Signed release bundles: see [docs/RELEASING.md](docs/RELEASING.md).

## Getting started

1. Open the app and create a project in a folder (or open the bundled
   [examples/jsonplaceholder](examples/jsonplaceholder) demo). A folder with several git repos
   becomes a multi-repo project.
2. Add an environment and send a request; turn a response into a mock with one click.
3. Commit the project files with your code — teammates get the same requests, environments,
   mocks and notes.

## Links

Everything in the app has a `devall:` link (`devall:request/orders/create.req.yaml`,
`devall:note/design/auth.md`, `devall:page/settings`). Inside notes and to-dos they're relative to
the project; **Copy link** adds the project (`…?project=<id from devall.yaml>`), so the link also
works from a browser, Slack or Teams, an email, or Markdown in VS Code or Obsidian: Dev All comes to
the front, switches to that project if it's on your recent list, and shows the item. A link from
outside only navigates — it never sends, runs or connects anything.

- **Windows:** `npm run app:install` registers the `devall:` scheme for your user, and the installed
  app re-registers itself on every start.
- **macOS:** links open the installed app in `~/Applications` (`npm run app:install` registers it
  with LaunchServices); `npm run app` has no app bundle, so it can't receive them.
- **Dev builds** (`npm run app`) ignore OS links and don't register the scheme, so they never take
  links from the installed copy or exit because it is running. Set `DEVALL_DEEP_LINK_DEV=1` to try
  links with a dev build.

## Command line and agents

The `devall` CLI runs the same project files in a terminal or CI:

```bash
cargo run -p devall-cli -- run examples/jsonplaceholder --env public --filter users
```

It also serves mocks (`devall mock serve`), runs load tests (`devall load run`), checks OpenAPI
specs, scans for secrets (`devall scan --install-hook`), opens tunnels, manages repos and to-dos,
and serves the project to coding agents over MCP:

```bash
claude mcp add devall -- devall mcp --project <path>
```

See [docs/MOCKS_IN_CI.md](docs/MOCKS_IN_CI.md) and [docs/LOAD_AGENTS.md](docs/LOAD_AGENTS.md).

## Develop

```bash
cd apps/desktop && npm run app
```

```bash
cargo test --workspace
```

```bash
cd apps/desktop && npx vitest run
```

Regenerate the JSON Schemas after changing the model:

```bash
UPDATE_SCHEMAS=1 cargo test -p devall-core schemas_are_up_to_date
```

[docs/DEVELOPING.md](docs/DEVELOPING.md) covers the architecture, conventions (workbench, views,
links, local data and history) and how to add features. Plugins: [docs/PLUGINS.md](docs/PLUGINS.md).

## Repository layout

```
apps/desktop      Tauri 2 + React 19 desktop app (src-tauri = thin command layer)
crates/           Rust libraries shared by the app, the CLI and the MCP server
  devall-core       project format, variables, secrets, repos and git, notes, to-dos, links
  devall-http       HTTP engine          devall-proto    gRPC, WebSocket, MQTT, Socket.IO
  devall-mock       mock server          devall-webhooks webhooks and tunnels
  devall-load       load testing         devall-db       SQL, Redis, MongoDB
  devall-logs       log viewer engine    devall-infra    Docker, Kubernetes, GCP
  devall-utils      utility operations   devall-data     fake data
  devall-ai         local AI             devall-mcp      MCP server
  devall-history    local history        devall-import   importers and code generation
  devall-cli        the `devall` binary  (+ openapi, plugins, proxy, notebook, script, relay)
relay/            the hosted / self-hostable relay (Cloudflare Worker + Firebase)
sdk/rust/         SDK for WebAssembly plugins
schemas/          JSON Schemas for every project file (generated from the Rust model)
examples/         demo project and example plugins
scripts/          install, release and brand-asset scripts
docs/             developer docs, feature list, implementation plan
```

## Background

- [docs/FEATURES.md](docs/FEATURES.md) — features by priority, free vs hosted
- [docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md) — decisions, architecture, project file format, milestones
- [reports/Developer toolkit competitive research.md](reports/Developer%20toolkit%20competitive%20research.md) — competitor research behind the feature set (notes in `research_notes/`)

## License

The relay ([relay/](relay/)) is licensed under the AGPL-3.0. The rest of the repository does not
have a license file yet.

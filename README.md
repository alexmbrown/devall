# /dev/all

A developer swiss army knife toolbox. Http client, mock server, webhook tester, load tester. What other tools can I build in. The goal is to prevent users from having multiple subs for different dev tools. Build synergy between tools by having them in one connected app.

# Function Ideas
* Http Client
* Mock Server
* Webhook tester
* Ngrok like reverse proxy
* load tester
* jwt encoder/decoder
* encoder/decoder base64, etc
* formmaaters
* regex sandbox
* text diff
* light weight sql, redis, mongo client
* random data generator
* log file viewer
* wiremock viewer
* GCP/K8s viewer (use kubectl and other tools)
* Docker 

# Potential AI features
* smart mock data generation
* error & stack trace analysis
* regext/query builder/translator
* traffice to spec reverse engineer

# Technology
* Rust Tauri Desktop App
* Firebase functions/store when needed
* Locall LLM (Bring your own model). Should have a few options to download or use their own

# Goals
* Project based. All http calls, mocks, etc should be stored as files in a repo. So that users can use standard version control
* Should have scratch pad or default project where users can store personal htt requests, mocks, etc
* no secrets are stored in project files only locally and never commited
* History (responses, file versions, utility runs, activity, load runs) stays on the user's device in the app data folder — never in the repo (see [docs/DEVELOPING.md](docs/DEVELOPING.md#local-data--history))
* Should tie features together for synergy between features. For example I could take and http request and create a mock endpoint form it. then randomize data
* Should use local AI to enhance functionality
* Should try and setup everything locally thats possible. For some features like an ngrok reverse proxy, or webhook tester setup firebase

Style ine ../wiremock-ui. you can also bring in the wiremock functionality from this app

What other features can I inclue. ideate first and be as through as possible and research online. Look at other apps and the features that have. how can we improve on those app and create something that is truly unique by bringing eveyrhign together in a project cetneric space

---

# Status and docs

| Document | What's in it |
|---|---|
| [reports/Developer toolkit competitive research.md](reports/Developer%20toolkit%20competitive%20research.md) | Competitor research, pricing, gaps, synergy workflows, technical choices (notes in `research_notes/`) |
| [docs/FEATURES.md](docs/FEATURES.md) | Prioritized features in two groups: **Free (local, $0 to run)** and **Hosted (needs our cloud; free tier + Supporter tier)** |
| [docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md) | Decisions, architecture, project file format, milestones M0–M7 |

**Milestone M0 (foundations + first vertical slice) is implemented:** project files in git, scratch pad,
environments with keychain-backed secrets, an HTTP client, utilities (JWT, encoders, hashes, ids, time,
formats), a CLI for CI, and the desktop app with the wiremock-ui design system.

## Repository layout

```
crates/devall-core    project format (YAML, one entity per file), variables, secrets, cURL import
crates/devall-http    HTTP engine: resolve → send → timings; export as cURL
crates/devall-utils   utility operations shared by the app, the CLI and (later) MCP
crates/devall-cli     `devall` binary for terminals and CI
apps/desktop          Tauri 2 + React 19 app (src-tauri = thin command layer)
schemas/              JSON Schemas for every project file (generated from the Rust model)
examples/             a demo project you can open or run
```

## Develop

Prerequisites: Rust 1.91+, Node 22, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
for your OS (WebView2 is preinstalled on Windows 11; Xcode Command Line Tools on macOS).

```bash
cd apps/desktop && npm install
```

```bash
cd apps/desktop && npm run app
```

```bash
cargo test --workspace
```

```bash
cargo run -p devall-cli -- run examples/jsonplaceholder --env public --filter users
```

Regenerate the JSON Schemas after changing the model:

```bash
UPDATE_SCHEMAS=1 cargo test -p devall-core schemas_are_up_to_date
```
